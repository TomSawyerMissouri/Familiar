//! The session screen: drives the engine from the frame clock and draws it.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, Align2, Color32, Key, Pos2, Rect, StrokeKind, Vec2};
use familiar_core::engine::PUNISHMENT_PAUSE_MS;
use familiar_core::{
    Distortion, Engine, EngineState, Event, Mistake, PhotoSize, Punishment, SessionResult,
};

use crate::loader::Loader;
use crate::render::{PHOTO_ASPECT, paint_photo};
use crate::sound::Sound;
use crate::theme;

/// How many trials ahead photos are decoded.
const PRELOAD_AHEAD: usize = 6;
/// How long the green frame stays after a hit.
const HIT_FRAME_MS: u64 = 220;
/// How far the picture is thrown about at the start of a hard punishment.
const SHAKE_POINTS: f32 = 22.0;

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
    punishment: Punishment,
    photo_size: PhotoSize,
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
    pub fn new(
        mut engine: Engine,
        faces_dir: PathBuf,
        punishment: Punishment,
        photo_size: PhotoSize,
        now_ms: u64,
    ) -> Self {
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
            punishment,
            photo_size,
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
    pub fn update(&mut self, ui: &mut egui::Ui, now_ms: u64, sound: &Sound) -> Option<SessionExit> {
        let ctx = ui.ctx().clone();
        ctx.request_repaint();

        let layout = Layout::new(ui.max_rect(), self.shake(now_ms), self.photo_size);
        let input = ctx.input(|i| Input {
            respond: i.key_pressed(Key::Space)
                || i.key_pressed(Key::J)
                || i.key_pressed(Key::Enter),
            escape: i.key_pressed(Key::Escape),
            pause: i.key_pressed(Key::P),
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
        let clicked_pause = input.click.is_some_and(|pos| layout.pause.contains(pos));

        let mut exit = None;
        if self.engine.state() == EngineState::Paused {
            if input.focused && (input.respond || input.pause || input.click.is_some()) {
                self.engine.resume(now_ms);
            }
        } else if !input.focused || input.pause || clicked_pause {
            self.engine.pause(now_ms);
        } else {
            let events = if pressed {
                self.engine.press(now_ms)
            } else {
                self.engine.tick(now_ms)
            };
            for event in events {
                exit = self.apply(event, now_ms, sound).or(exit);
            }
        }

        self.paint(ui, &layout, now_ms, pressed);
        exit
    }

    fn apply(&mut self, event: Event, now_ms: u64, sound: &Sound) -> Option<SessionExit> {
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
            Event::Hit { .. } => {
                self.hit_until = now_ms + HIT_FRAME_MS;
                sound.click();
            }
            Event::Punish(mistake) => {
                if self.punishment != Punishment::Off {
                    self.flash = Some((mistake, now_ms));
                    sound.punish();
                }
            }
            Event::ExposureChanged { .. } => {}
            Event::Finished(result) => return Some(SessionExit::Finished(result)),
        }
        None
    }

    /// Offset of the whole picture while a hard punishment shakes it.
    fn shake(&self, now_ms: u64) -> Vec2 {
        let Some((_, started)) = self.flash else {
            return Vec2::ZERO;
        };
        let age = now_ms.saturating_sub(started);
        if self.punishment != Punishment::Hard || age >= PUNISHMENT_PAUSE_MS {
            return Vec2::ZERO;
        }
        let t = age as f32;
        let strength = SHAKE_POINTS * (1.0 - t / PUNISHMENT_PAUSE_MS as f32);
        Vec2::new((t * 0.21).sin(), (t * 0.33).cos()) * strength
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
            let stroke = theme::stroke(2.0, theme::DIM);
            let c = frame.center();
            painter.line_segment([c - Vec2::new(arm, 0.0), c + Vec2::new(arm, 0.0)], stroke);
            painter.line_segment([c - Vec2::new(0.0, arm), c + Vec2::new(0.0, arm)], stroke);
        }
        let (outline, width) = if now_ms < self.hit_until {
            (theme::GOOD, 5.0)
        } else {
            (theme::OUTLINE, 1.0)
        };
        painter.rect_stroke(
            frame,
            0.0,
            theme::stroke(width, outline),
            StrokeKind::Outside,
        );

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
            theme::stroke(1.5, theme::ACCENT),
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
            "Space · J · Enter",
            theme::label(12.0),
            theme::DIM,
        );

        // The pause button, small and out of the way.
        let pause_fill = if ui.rect_contains_pointer(layout.pause) {
            Color32::from_rgb(32, 39, 46)
        } else {
            theme::PANEL
        };
        painter.rect_filled(layout.pause, 6.0, pause_fill);
        painter.rect_stroke(
            layout.pause,
            6.0,
            theme::stroke(1.0, theme::OUTLINE),
            StrokeKind::Inside,
        );
        painter.text(
            layout.pause.center() - Vec2::new(0.0, 8.0),
            Align2::CENTER_CENTER,
            "PAUSE",
            theme::mono(14.0),
            theme::TEXT,
        );
        painter.text(
            layout.pause.center() + Vec2::new(0.0, 11.0),
            Align2::CENTER_CENTER,
            "P  ·  Esc quits",
            theme::label(11.0),
            theme::DIM,
        );

        // Punishment: a red flash over everything.
        if let Some((mistake, started)) = self.flash {
            let age = now_ms.saturating_sub(started);
            if age < PUNISHMENT_PAUSE_MS {
                let fade = 1.0 - age as f32 / PUNISHMENT_PAUSE_MS as f32;
                painter.rect_filled(
                    layout.overlay,
                    0.0,
                    theme::BAD.gamma_multiply(0.45 + 0.5 * fade),
                );
                let text = match mistake {
                    Mistake::FalseAlarm => "FALSE ALARM",
                    Mistake::Miss => "MISS",
                };
                painter.text(
                    layout.overlay.center(),
                    Align2::CENTER_CENTER,
                    text,
                    theme::mono(72.0),
                    Color32::WHITE,
                );
            }
        }

        if paused {
            painter.rect_filled(layout.overlay, 0.0, Color32::from_black_alpha(200));
            painter.text(
                layout.overlay.center() - Vec2::new(0.0, 16.0),
                Align2::CENTER_CENTER,
                "PAUSED",
                theme::mono(44.0),
                theme::TEXT,
            );
            painter.text(
                layout.overlay.center() + Vec2::new(0.0, 30.0),
                Align2::CENTER_CENTER,
                "Space or P to continue · Esc to quit",
                theme::label(15.0),
                theme::DIM,
            );
        }
    }
}

struct Input {
    respond: bool,
    escape: bool,
    pause: bool,
    click: Option<Pos2>,
    focused: bool,
}

/// Where things are on the session screen.
struct Layout {
    screen: Rect,
    /// The panel without the shake, for overlays that must cover everything.
    overlay: Rect,
    progress: Rect,
    photo: Rect,
    button: Rect,
    pause: Rect,
}

impl Layout {
    /// `screen` is the whole panel; everything but the full-screen overlays
    /// is moved by `shake`.
    fn new(screen: Rect, shake: Vec2, photo_size: PhotoSize) -> Self {
        let overlay = screen;
        let screen = screen.translate(shake);
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
        let pause_size = Vec2::new(120.0, 48.0);
        let pause = Rect::from_center_size(
            Pos2::new(
                screen.right() - 24.0 - pause_size.x / 2.0,
                button.center().y,
            ),
            pause_size,
        );
        let fitted = area.height().min(area.width() / PHOTO_ASPECT);
        let height = fitted.min(max_photo_height(photo_size)).max(1.0);
        let photo = Rect::from_center_size(area.center(), Vec2::new(height * PHOTO_ASPECT, height));
        Self {
            screen,
            overlay,
            progress,
            photo,
            button,
            pause,
        }
    }
}

/// Tallest a photo is drawn, in points. The source is 218 pixels tall.
fn max_photo_height(size: PhotoSize) -> f32 {
    match size {
        PhotoSize::Small => 290.0,
        PhotoSize::Medium => 440.0,
        PhotoSize::Large => f32::INFINITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(width: f32, height: f32, size: PhotoSize) -> Layout {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, height));
        Layout::new(screen, Vec2::ZERO, size)
    }

    #[test]
    fn photo_size_limits_how_large_the_photo_is_drawn() {
        let small = layout(1400.0, 1000.0, PhotoSize::Small).photo;
        let medium = layout(1400.0, 1000.0, PhotoSize::Medium).photo;
        let large = layout(1400.0, 1000.0, PhotoSize::Large).photo;
        assert_eq!(small.height(), 290.0);
        assert_eq!(medium.height(), 440.0);
        assert!(large.height() > 700.0);
        for photo in [small, medium, large] {
            assert!((photo.width() / photo.height() - PHOTO_ASPECT).abs() < 1e-4);
            assert_eq!(photo.center().x, 700.0, "centred horizontally");
        }
    }

    #[test]
    fn a_small_window_shrinks_every_photo_size_to_fit() {
        for size in [PhotoSize::Small, PhotoSize::Medium, PhotoSize::Large] {
            let l = layout(720.0, 400.0, size);
            assert!(l.photo.bottom() <= l.button.top(), "{size:?}");
            assert!(l.photo.top() >= l.progress.bottom(), "{size:?}");
        }
    }

    #[test]
    fn the_pause_button_does_not_overlap_the_response_targets() {
        for width in [720.0, 1000.0, 1600.0] {
            let l = layout(width, 780.0, PhotoSize::Large);
            assert!(!l.pause.intersects(l.button), "width {width}");
            assert!(!l.pause.intersects(l.photo), "width {width}");
            assert!(l.screen.contains_rect(l.pause));
        }
    }
}
