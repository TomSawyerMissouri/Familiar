//! The application: data, persistence and the home and results screens.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Align2, Color32, Key, RichText, Sense, Stroke, StrokeKind, Vec2};
use familiar_core::storage::{self, SESSION_LENGTHS};
use familiar_core::{
    Catalog, Engine, History, Level, Manifest, Progress, SessionConfig, SessionPlan, SessionRecord,
    SessionResult, Settings, generate,
};

use crate::session_view::{SessionExit, SessionView};
use crate::theme;

/// Attempts to plan a session around photos that are missing on disk.
const PLAN_ATTEMPTS: usize = 6;

enum Screen {
    Home,
    Session(Box<SessionView>),
    Results(Box<Summary>),
}

/// A finished session and what it did to the progress.
struct Summary {
    result: SessionResult,
    before: Progress,
    after: Progress,
}

pub struct App {
    /// Origin of the millisecond clock fed to the engine.
    epoch: Instant,
    /// Holds `settings.json` and `history.json`.
    state_dir: PathBuf,
    faces_dir: PathBuf,
    settings: Settings,
    history: History,
    manifest: Option<Manifest>,
    catalog: Option<Catalog>,
    /// People dropped for this run because a photo of theirs is not on disk.
    unavailable: BTreeSet<String>,
    /// Something the user should know, shown on the home screen.
    notice: Option<String>,
    screen: Screen,
}

fn default_data_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join("Documents").join("Familiar")
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn level_name(level: Level) -> &'static str {
    match level.get() {
        1 => "Same look",
        2 => "Similar look",
        3 => "Different look",
        4 => "Strong change",
        _ => "Full disguise",
    }
}

impl App {
    pub fn new(ctx: &egui::Context) -> Self {
        theme::apply(ctx);
        let state_dir = default_data_dir();
        let mut notices = Vec::new();

        let settings = match storage::load_settings(&state_dir) {
            Ok(loaded) => {
                if let Some(backup) = loaded.recovered_from {
                    notices.push(format!(
                        "Settings were unreadable and have been reset. The old file is kept as {}.",
                        backup.display()
                    ));
                }
                loaded.value
            }
            Err(error) => {
                notices.push(format!("Settings could not be read: {error}"));
                Settings::default()
            }
        };
        let history = match storage::load_history(&state_dir) {
            Ok(loaded) => {
                if let Some(backup) = loaded.recovered_from {
                    notices.push(format!(
                        "History was unreadable and starts anew. The old file is kept as {}.",
                        backup.display()
                    ));
                }
                loaded.value
            }
            Err(error) => {
                notices.push(format!("History could not be read: {error}"));
                History::default()
            }
        };

        let faces_dir = settings
            .data_dir
            .clone()
            .unwrap_or_else(|| state_dir.clone())
            .join("faces");
        let manifest = match Manifest::load(&faces_dir.join("manifest.json")) {
            Ok(manifest) => Some(manifest),
            Err(error) => {
                notices.push(format!(
                    "No face set found: {error}. Prepare it with: \
                     python3 scripts/prepare_celeba.py --out ~/Documents/Familiar"
                ));
                None
            }
        };

        let mut app = Self {
            epoch: Instant::now(),
            state_dir,
            faces_dir,
            settings,
            history,
            manifest,
            catalog: None,
            unavailable: BTreeSet::new(),
            notice: (!notices.is_empty()).then(|| notices.join("\n")),
            screen: Screen::Home,
        };
        app.rebuild_catalog();
        app
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Rebuilds the catalog without the excluded and the unavailable people.
    fn rebuild_catalog(&mut self) {
        self.catalog = self.manifest.as_ref().map(|manifest| {
            let hidden: BTreeSet<String> = self
                .settings
                .excluded
                .union(&self.unavailable)
                .cloned()
                .collect();
            Catalog::new(manifest.clone(), &hidden)
        });
    }

    fn save_settings(&mut self) {
        if let Err(error) = storage::save_settings(&self.state_dir, &self.settings) {
            self.notice = Some(format!("Settings could not be saved: {error}"));
        }
    }

    /// Plans a session whose photos are all on disk.
    ///
    /// Files in `~/Documents` may have been evicted to iCloud. People with a
    /// missing photo are dropped for this run and the session is planned again.
    fn plan_session(&mut self) -> Result<SessionPlan, String> {
        let config = SessionConfig {
            trials: self.settings.trials_per_session,
            level: self.settings.progress.level,
            seed: unix_ms() ^ self.now_ms().rotate_left(32),
        };
        let mut dropped = 0usize;
        for _ in 0..PLAN_ATTEMPTS {
            let catalog = self.catalog.as_ref().ok_or("The face set is not loaded.")?;
            let plan = generate(catalog, &config).map_err(|error| error.to_string())?;
            let missing: BTreeSet<String> = plan
                .trials
                .iter()
                .filter(|trial| !is_readable(&self.faces_dir.join(&trial.photo_file)))
                .map(|trial| {
                    eprintln!(
                        "familiar: photo not on disk, person skipped: {}",
                        trial.photo_file
                    );
                    trial.person_id.clone()
                })
                .collect();
            if missing.is_empty() {
                if dropped > 0 {
                    self.notice = Some(format!(
                        "{dropped} people were skipped: their photos are not on this Mac \
                         (possibly moved to iCloud)."
                    ));
                }
                return Ok(plan);
            }
            dropped += missing.len();
            self.unavailable.extend(missing);
            self.rebuild_catalog();
        }
        Err(
            "Too many photos are missing on this Mac (possibly moved to iCloud). \
             Open the Familiar folder in Finder and download it."
                .into(),
        )
    }

    fn start_session(&mut self) {
        match self.plan_session() {
            Ok(plan) => {
                let engine = Engine::new(plan, self.settings.progress.exposure_ms);
                let view = SessionView::new(engine, self.faces_dir.clone(), self.now_ms());
                self.screen = Screen::Session(Box::new(view));
            }
            Err(message) => {
                self.notice = Some(message);
                self.screen = Screen::Home;
            }
        }
    }

    fn finish_session(&mut self, result: SessionResult) {
        let before = self.settings.progress;
        let after = before.after_session(
            result.balanced_accuracy,
            result.exposure_end_ms,
            self.settings.auto_promote,
        );
        self.settings.progress = after;
        self.save_settings();
        self.history
            .sessions
            .push(SessionRecord::from_result(&result, unix_ms()));
        if let Err(error) = storage::save_history(&self.state_dir, &self.history) {
            self.notice = Some(format!("History could not be saved: {error}"));
        }
        self.screen = Screen::Results(Box::new(Summary {
            result,
            before,
            after,
        }));
    }

    fn home(&mut self, ui: &mut egui::Ui) {
        let mut start = ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space));
        let mut changed = false;
        let progress = self.settings.progress;
        let last = self.history.sessions.last().map(|s| s.balanced_accuracy);

        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.08);
            ui.label(
                RichText::new("F A M I L I A R")
                    .font(theme::mono(34.0))
                    .color(theme::ACCENT),
            );
            ui.label(RichText::new("Face recognition trainer").color(theme::DIM));
            ui.add_space(26.0);

            tiles(
                ui,
                &[
                    Tile::new("LEVEL", progress.level.get().to_string(), theme::TEXT)
                        .note(level_name(progress.level)),
                    Tile::new("EXPOSURE", progress.exposure_ms.to_string(), theme::TEXT).note("ms"),
                    Tile::new(
                        "SESSIONS",
                        self.history.sessions.len().to_string(),
                        theme::TEXT,
                    ),
                    match last {
                        Some(accuracy) => Tile::new(
                            "LAST ACCURACY",
                            format!("{:.0}%", accuracy * 100.0),
                            theme::accuracy_color(accuracy),
                        ),
                        None => Tile::new("LAST ACCURACY", "—".into(), theme::DIM),
                    },
                ],
            );
            ui.add_space(22.0);

            ui.allocate_ui(Vec2::new(520.0, 0.0), |ui| {
                egui::Grid::new("home-settings")
                    .num_columns(2)
                    .spacing([18.0, 12.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("Session length").color(theme::DIM));
                        ui.horizontal(|ui| {
                            for length in SESSION_LENGTHS {
                                let selected = self.settings.trials_per_session == length;
                                if ui.selectable_label(selected, length.to_string()).clicked() {
                                    self.settings.trials_per_session = length;
                                    changed = true;
                                }
                            }
                        });
                        ui.end_row();

                        ui.label(RichText::new("Level").color(theme::DIM));
                        ui.horizontal(|ui| {
                            for value in Level::MIN.get()..=Level::MAX.get() {
                                let level = Level::new(value);
                                let selected = self.settings.progress.level == level;
                                let button = ui.selectable_label(selected, value.to_string());
                                if button.on_hover_text(level_name(level)).clicked() {
                                    self.settings.progress.level = level;
                                    changed = true;
                                }
                            }
                        });
                        ui.end_row();

                        ui.label(RichText::new("Level changes").color(theme::DIM));
                        let auto = ui.checkbox(&mut self.settings.auto_promote, "automatic");
                        changed |= auto.changed();
                        ui.end_row();
                    });
            });
            ui.add_space(26.0);

            let label = RichText::new("START")
                .font(theme::mono(24.0))
                .color(theme::BACKGROUND);
            let button = egui::Button::new(label)
                .fill(theme::ACCENT)
                .min_size(Vec2::new(300.0, 64.0));
            start |= ui.add_enabled(self.catalog.is_some(), button).clicked();
            ui.label(RichText::new("Enter or Space").size(12.0).color(theme::DIM));

            if let Some(notice) = &self.notice {
                ui.add_space(18.0);
                ui.allocate_ui(Vec2::new(640.0, 0.0), |ui| {
                    ui.label(RichText::new(notice).color(theme::WARN));
                });
            }
        });

        if changed {
            self.save_settings();
        }
        if start && self.catalog.is_some() {
            self.notice = None;
            self.start_session();
        }
    }

    /// Draws the results; returns the screen to switch to, if any.
    fn results(ui: &mut egui::Ui, summary: &Summary) -> Option<Next> {
        let Summary {
            result,
            before,
            after,
        } = summary;
        let counts = result.counts;
        let (verdict, color) = if after.level > before.level {
            (
                format!(
                    "Promoted to level {} — {}",
                    after.level.get(),
                    level_name(after.level)
                ),
                theme::GOOD,
            )
        } else if after.level < before.level {
            (
                format!(
                    "Back to level {} — {}",
                    after.level.get(),
                    level_name(after.level)
                ),
                theme::BAD,
            )
        } else if result.balanced_accuracy >= 0.85 {
            ("Clean run.".to_string(), theme::GOOD)
        } else if result.balanced_accuracy >= 0.65 {
            ("Solid. Keep going.".to_string(), theme::WARN)
        } else {
            (
                "Too many mistakes. Slow down and look closer.".to_string(),
                theme::BAD,
            )
        };

        let mut next = ui.input(|i| {
            if i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) {
                Some(Next::Again)
            } else if i.key_pressed(Key::Escape) {
                Some(Next::Home)
            } else {
                None
            }
        });

        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.08);
            ui.label(
                RichText::new("SESSION COMPLETE")
                    .font(theme::mono(26.0))
                    .color(theme::ACCENT),
            );
            ui.label(RichText::new(verdict).size(18.0).color(color));
            ui.add_space(22.0);

            tiles(
                ui,
                &[
                    Tile::new("HITS", counts.hits.to_string(), theme::GOOD),
                    Tile::new(
                        "MISSES",
                        counts.misses.to_string(),
                        mistakes_color(counts.misses),
                    ),
                    Tile::new(
                        "FALSE ALARMS",
                        counts.false_alarms.to_string(),
                        mistakes_color(counts.false_alarms),
                    ),
                    Tile::new(
                        "CORRECT REJECTIONS",
                        counts.correct_rejections.to_string(),
                        theme::GOOD,
                    ),
                ],
            );
            ui.add_space(12.0);
            tiles(
                ui,
                &[
                    Tile::new(
                        "ACCURACY",
                        format!("{:.0}%", result.balanced_accuracy * 100.0),
                        theme::accuracy_color(result.balanced_accuracy),
                    )
                    .note("balanced"),
                    Tile::new("D-PRIME", format!("{:.2}", result.d_prime), theme::TEXT),
                    match result.mean_rt_ms {
                        Some(rt) => {
                            Tile::new("REACTION", format!("{rt:.0}"), theme::TEXT).note("ms, hits")
                        }
                        None => Tile::new("REACTION", "—".into(), theme::DIM),
                    },
                    Tile::new(
                        "EXPOSURE",
                        format!("{} → {}", result.exposure_start_ms, result.exposure_end_ms),
                        theme::TEXT,
                    )
                    .note("ms"),
                ],
            );
            ui.add_space(14.0);
            ui.label(
                RichText::new(format!(
                    "Level {} · {} trials · next session starts at {} ms",
                    result.config.level.get(),
                    result.config.trials,
                    after.exposure_ms
                ))
                .color(theme::DIM),
            );
            ui.add_space(24.0);

            let again = RichText::new("AGAIN")
                .font(theme::mono(22.0))
                .color(theme::BACKGROUND);
            let button = egui::Button::new(again)
                .fill(theme::ACCENT)
                .min_size(Vec2::new(300.0, 60.0));
            if ui.add(button).clicked() {
                next = Some(Next::Again);
            }
            ui.label(RichText::new("Enter or Space").size(12.0).color(theme::DIM));
            ui.add_space(6.0);
            if ui.button("Home  (Esc)").clicked() {
                next = Some(Next::Home);
            }
        });
        next
    }
}

/// Where to go from the results screen.
enum Next {
    Again,
    Home,
}

fn mistakes_color(count: u32) -> Color32 {
    if count == 0 { theme::GOOD } else { theme::BAD }
}

/// Whether a photo is really on disk (not an empty or evicted placeholder).
fn is_readable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

/// A labelled readout.
struct Tile {
    label: &'static str,
    value: String,
    color: Color32,
    note: &'static str,
}

impl Tile {
    fn new(label: &'static str, value: String, color: Color32) -> Self {
        Self {
            label,
            value,
            color,
            note: "",
        }
    }

    fn note(mut self, note: &'static str) -> Self {
        self.note = note;
        self
    }
}

/// Draws a centred row of readouts.
fn tiles(ui: &mut egui::Ui, tiles: &[Tile]) {
    const GAP: f32 = 12.0;
    const HEIGHT: f32 = 104.0;
    let width = ((ui.available_width() - 48.0).min(820.0) - GAP * (tiles.len() as f32 - 1.0))
        / tiles.len() as f32;
    let total = Vec2::new(
        width * tiles.len() as f32 + GAP * (tiles.len() as f32 - 1.0),
        HEIGHT,
    );
    let (row, _) = ui.allocate_exact_size(total, Sense::hover());
    let painter = ui.painter();
    for (i, tile) in tiles.iter().enumerate() {
        let min = row.min + Vec2::new(i as f32 * (width + GAP), 0.0);
        let rect = egui::Rect::from_min_size(min, Vec2::new(width, HEIGHT));
        painter.rect_filled(rect, 4.0, theme::PANEL);
        painter.rect_stroke(
            rect,
            4.0,
            Stroke::new(1.0, theme::OUTLINE),
            StrokeKind::Inside,
        );
        painter.text(
            rect.center_top() + Vec2::new(0.0, 20.0),
            Align2::CENTER_CENTER,
            tile.label,
            theme::label(11.5),
            theme::DIM,
        );
        painter.text(
            rect.center() + Vec2::new(0.0, 4.0),
            Align2::CENTER_CENTER,
            &tile.value,
            theme::mono(30.0),
            tile.color,
        );
        painter.text(
            rect.center_bottom() - Vec2::new(0.0, 16.0),
            Align2::CENTER_CENTER,
            tile.note,
            theme::label(12.0),
            theme::DIM,
        );
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now_ms = self.now_ms();
        let panel = egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme::BACKGROUND));
        panel.show(ctx, |ui| match &mut self.screen {
            Screen::Home => self.home(ui),
            Screen::Session(view) => match view.update(ui, now_ms) {
                Some(SessionExit::Aborted) => self.screen = Screen::Home,
                Some(SessionExit::Finished(result)) => self.finish_session(*result),
                None => {}
            },
            Screen::Results(summary) => match Self::results(ui, summary) {
                Some(Next::Again) => self.start_session(),
                Some(Next::Home) => self.screen = Screen::Home,
                None => {}
            },
        });
    }
}
