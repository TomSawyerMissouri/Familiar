//! The session screen: drives the engine from the frame clock and draws it.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, Align2, Color32, Key, Pos2, Rect, Stroke, StrokeKind, Vec2};
use familiar_core::engine::PUNISHMENT_PAUSE_MS;
use familiar_core::{Distortion, Engine, EngineState, Event, Mistake, SessionResult};

use crate::loader::Loader;
use crate::render::{PHOTO_ASPECT, paint_photo};
use crate::theme;

/// How many trials ahead photos are decoded.
const PRELOAD_AHEAD: usize = 6;
/// How long the green frame stays after a hit.
const HIT_FRAME_MS: u64 = 220;

/// How a session screen ended.
pub enum SessionExit {
    /// The user quit; nothing is saved.
    Aborted,
    Finished(Box<SessionResult>),
}

enum Photo {
    Ready(egui::TextureHandle),
    Failed,
}

pub struct SessionView {
    engine: Engine,
    faces_dir: PathBuf,
    loader: Loader,
    photos: HashMap<usize, Photo>,
    /// First trial whose photo has not been requested yet.
    next_request: usize,
    countdown: Option<u8>,
    /// Trial on screen and how to draw it.
    current: Option<(usize, Distortion)>,
    face_visible: bool,
    hit_until: u64,
    /// The mistake being flashed and when the flash started.
    flash: Option<(Mistake, u64)>,
}

impl SessionView {
    /// Starts a session at `now_ms`.
    pub fn new(mut engine: Engine, faces_dir: PathBuf, now_ms: u64) -> Self {
        let countdown = engine
            .start(now_ms)
            .into_iter()
            .find_map(|event| match event {
                Event::Countdown(n) => Some(n),
                _ => None,
            });
        let mut view = Self {
            engine,
            faces_dir,
            loader: Loader::new(),
            photos: HashMap::new(),
            next_request: 0,
            countdown,
            current: None,
            face_visible: false,
            hit_until: 0,
            flash: None,
        };
        view.request_photos(0);
        view
    }

    /// Runs one frame. Returns how the session ended, once it has.
    pub fn update(&mut self, ui: &mut egui::Ui, now_ms: u64) -> Option<SessionExit> {
        let ctx = ui.ctx().clone();
        ctx.request_repaint();

        let layout = Layout::new(ui.max_rect());
        let input = ctx.input(|i| Input {
            respond: i.key_pressed(Key::Space)
                || i.key_pressed(Key::J)
                || i.key_pressed(Key::Enter),
            escape: i.key_pressed(Key::Escape),
            click: i
                .pointer
                .primary_pressed()
                .then(|| i.pointer.interact_pos())
                .flatten(),
            focused: i.focused,
        });
        if input.escape {
            self.engine.abort();
            return Some(SessionExit::Aborted);
        }

        for loaded in self.loader.poll() {
            let photo = match loaded.image {
                Ok(image) => Photo::Ready(ctx.load_texture(
                    format!("face-{}", loaded.index),
                    image,
                    egui::TextureOptions::LINEAR,
                )),
                Err(error) => {
                    eprintln!("familiar: photo skipped: {error}");
                    Photo::Failed
                }
            };
            self.photos.insert(loaded.index, photo);
        }

        let clicked_target = input
            .click
            .is_some_and(|pos| layout.photo.contains(pos) || layout.button.contains(pos));
        let pressed = input.respond || clicked_target;

        let mut exit = None;
        if self.engine.state() == EngineState::Paused {
            if input.focused && (input.respond || input.click.is_some()) {
                self.engine.resume(now_ms);
            }
        } else if !input.focused {
            self.engine.pause(now_ms);
        } else {
            let events = if pressed {
                self.engine.press(now_ms)
            } else {
                self.engine.tick(now_ms)
            };
            for event in events {
                exit = self.apply(event, now_ms).or(exit);
            }
        }

        self.paint(ui, &layout, now_ms, pressed);
        exit
    }

    fn apply(&mut self, event: Event, now_ms: u64) -> Option<SessionExit> {
        match event {
            Event::Countdown(n) => self.countdown = Some(n),
            Event::ShowFace { index, trial, .. } => {
                self.countdown = None;
                self.current = Some((index, trial.distortion));
                self.face_visible = true;
                self.photos.retain(|&i, _| i >= index);
                self.request_photos(index + 1);
            }
            Event::HideFace => self.face_visible = false,
            Event::Hit { .. } => self.hit_until = now_ms + HIT_FRAME_MS,
            Event::Punish(mistake) => self.flash = Some((mistake, now_ms)),
            Event::ExposureChanged { .. } => {}
            Event::Finished(result) => return Some(SessionExit::Finished(result)),
        }
        None
    }

    /// Queues photos up to `PRELOAD_AHEAD` trials from `from`.
    fn request_photos(&mut self, from: usize) {
        let trials = &self.engine.plan().trials;
        let end = (from + PRELOAD_AHEAD).min(trials.len());
        for (index, trial) in trials.iter().enumerate().take(end).skip(self.next_request) {
            self.loader.request(
                index,
                self.faces_dir.join(&trial.photo_file),
                trial.distortion,
            );
        }
        self.next_request = self.next_request.max(end);
    }

    fn paint(&self, ui: &egui::Ui, layout: &Layout, now_ms: u64, pressed: bool) {
        let painter = ui.painter();
        let (done, total) = self.engine.progress();

        // Thin progress bar along the top edge.
        let fraction = if total == 0 {
            0.0
        } else {
            done as f32 / total as f32
        };
        painter.rect_filled(layout.progress, 0.0, theme::OUTLINE);
        let mut filled = layout.progress;
        filled.set_width(layout.progress.width() * fraction);
        painter.rect_filled(filled, 0.0, theme::ACCENT);

        let readout_y = layout.progress.bottom() + 24.0;
        let readout = |x: f32, anchor: Align2, text: String| {
            painter.text(
                Pos2::new(x, readout_y),
                anchor,
                text,
                theme::mono(14.0),
                theme::DIM,
            );
        };
        let shown = (done + 1).min(total);
        readout(
            layout.screen.left() + 24.0,
            Align2::LEFT_CENTER,
            format!("TRIAL {shown}/{total}"),
        );
        readout(
            layout.screen.center().x,
            Align2::CENTER_CENTER,
            format!("LEVEL {}", self.engine.plan().config.level.get()),
        );
        readout(
            layout.screen.right() - 24.0,
            Align2::RIGHT_CENTER,
            format!("EXPOSURE {} ms", self.engine.exposure_ms()),
        );

        // The photo frame.
        let frame = layout.photo;
        painter.rect_filled(frame, 0.0, theme::PANEL);
        let paused = self.engine.state() == EngineState::Paused;
        if let Some(n) = self.countdown {
            painter.text(
                frame.center(),
                Align2::CENTER_CENTER,
                n,
                theme::mono(frame.height() * 0.3),
                theme::ACCENT,
            );
            painter.text(
                frame.center_bottom() - Vec2::new(0.0, 28.0),
                Align2::CENTER_CENTER,
                "Press when the person was already shown",
                theme::label(15.0),
                theme::DIM,
            );
        } else if paused {
            // The face stays hidden so a pause gives no extra time to study it.
        } else if self.face_visible {
            match self
                .current
                .and_then(|(index, d)| self.photos.get(&index).map(|p| (p, d)))
            {
                Some((Photo::Ready(texture), distortion)) => {
                    paint_photo(painter, frame, texture.id(), &distortion);
                }
                Some((Photo::Failed, _)) => {
                    painter.text(
                        frame.center(),
                        Align2::CENTER_CENTER,
                        "photo unavailable",
                        theme::label(16.0),
                        theme::DIM,
                    );
                }
                // Still decoding: the frame stays empty for a moment.
                None => {}
            }
        } else {
            let arm = 14.0;
            let stroke = Stroke::new(2.0, theme::DIM);
            let c = frame.center();
            painter.line_segment([c - Vec2::new(arm, 0.0), c + Vec2::new(arm, 0.0)], stroke);
            painter.line_segment([c - Vec2::new(0.0, arm), c + Vec2::new(0.0, arm)], stroke);
        }
        let (outline, width) = if now_ms < self.hit_until {
            (theme::GOOD, 5.0)
        } else {
            (theme::OUTLINE, 1.0)
        };
        painter.rect_stroke(frame, 0.0, Stroke::new(width, outline), StrokeKind::Outside);

        // The response button.
        let hovered = ui.rect_contains_pointer(layout.button);
        let fill = if pressed {
            theme::ACCENT.gamma_multiply(0.5)
        } else if hovered {
            Color32::from_rgb(32, 39, 46)
        } else {
            theme::PANEL
        };
        painter.rect_filled(layout.button, 6.0, fill);
        painter.rect_stroke(
            layout.button,
            6.0,
            Stroke::new(1.5, theme::ACCENT),
            StrokeKind::Inside,
        );
        painter.text(
            layout.button.center() - Vec2::new(0.0, 8.0),
            Align2::CENTER_CENTER,
            "SEEN BEFORE",
            theme::mono(24.0),
            theme::TEXT,
        );
        painter.text(
            layout.button.center() + Vec2::new(0.0, 18.0),
            Align2::CENTER_CENTER,
            "Space · J · Enter        Esc to quit",
            theme::label(12.0),
            theme::DIM,
        );

        // Punishment: a red flash over everything.
        if let Some((mistake, started)) = self.flash {
            let age = now_ms.saturating_sub(started);
            if age < PUNISHMENT_PAUSE_MS {
                let fade = 1.0 - age as f32 / PUNISHMENT_PAUSE_MS as f32;
                painter.rect_filled(
                    layout.screen,
                    0.0,
                    theme::BAD.gamma_multiply(0.45 + 0.5 * fade),
                );
                let text = match mistake {
                    Mistake::FalseAlarm => "FALSE ALARM",
                    Mistake::Miss => "MISS",
                };
                painter.text(
                    layout.screen.center(),
                    Align2::CENTER_CENTER,
                    text,
                    theme::mono(72.0),
                    Color32::WHITE,
                );
            }
        }

        if paused {
            painter.rect_filled(layout.screen, 0.0, Color32::from_black_alpha(200));
            painter.text(
                layout.screen.center() - Vec2::new(0.0, 16.0),
                Align2::CENTER_CENTER,
                "PAUSED",
                theme::mono(44.0),
                theme::TEXT,
            );
            painter.text(
                layout.screen.center() + Vec2::new(0.0, 30.0),
                Align2::CENTER_CENTER,
                "Space to continue · Esc to quit",
                theme::label(15.0),
                theme::DIM,
            );
        }
    }
}

struct Input {
    respond: bool,
    escape: bool,
    click: Option<Pos2>,
    focused: bool,
}

/// Where things are on the session screen.
struct Layout {
    screen: Rect,
    progress: Rect,
    photo: Rect,
    button: Rect,
}

impl Layout {
    fn new(screen: Rect) -> Self {
        let progress = Rect::from_min_size(screen.min, Vec2::new(screen.width(), 4.0));
        let button_size = Vec2::new(380.0f32.min(screen.width() - 48.0), 72.0);
        let button = Rect::from_center_size(
            Pos2::new(
                screen.center().x,
                screen.bottom() - 28.0 - button_size.y / 2.0,
            ),
            button_size,
        );
        let area = Rect::from_min_max(
            Pos2::new(screen.left() + 24.0, progress.bottom() + 52.0),
            Pos2::new(screen.right() - 24.0, button.top() - 24.0),
        );
        let height = area.height().min(area.width() / PHOTO_ASPECT).max(1.0);
        let photo = Rect::from_center_size(area.center(), Vec2::new(height * PHOTO_ASPECT, height));
        Self {
            screen,
            progress,
            photo,
            button,
        }
    }
}
