//! The application: data, persistence and every screen but the session itself.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Align2, Color32, Key, RichText, Sense, StrokeKind, Vec2};
use familiar_core::storage::{self, SESSION_LENGTHS};
use familiar_core::{
    Catalog, Distortion, Engine, History, Level, Manifest, Outcome, PhotoSize, Progress,
    Punishment, SessionConfig, SessionPlan, SessionRecord, SessionResult, Settings, generate,
};

use crate::history_view;
use crate::loader;
use crate::session_view::{SessionExit, SessionView};
use crate::sound::Sound;
use crate::theme;

/// Attempts to plan a session around photos that are missing on disk.
const PLAN_ATTEMPTS: usize = 6;
/// At most this many faces are offered for exclusion after a session.
const MAX_MISTAKE_FACES: usize = 24;
/// The command that prepares the face set.
const PREPARE_COMMAND: &str = "python3 scripts/prepare_celeba.py --out ~/Documents/Familiar";

enum Screen {
    /// The face set is missing.
    Onboarding,
    Home,
    Session(Box<SessionView>),
    Results(Box<Summary>),
    History,
    Settings,
}

/// What a screen asks the app to do.
enum Action {
    Go(Screen),
    Start,
    ToggleExcluded(String),
    ClearExcluded,
    /// Pick the data folder in a system dialog.
    ChooseFolder,
    /// Go back to the default data folder.
    DefaultFolder,
    /// Look for the face set again.
    Reload,
}

/// A finished session and what it did to the progress.
struct Summary {
    result: SessionResult,
    before: Progress,
    after: Progress,
    /// People a mistake was made on, for "I know this person".
    mistakes: Vec<MistakeFace>,
}

struct MistakeFace {
    person_id: String,
    outcome: Outcome,
    /// `None` when the photo could not be read.
    texture: Option<egui::TextureHandle>,
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
    /// Why the manifest could not be loaded.
    manifest_error: Option<String>,
    catalog: Option<Catalog>,
    /// People dropped for this run because a photo of theirs is not on disk.
    unavailable: BTreeSet<String>,
    /// Something the user should know, shown on the home screen.
    notice: Option<String>,
    sound: Sound,
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

fn punishment_name(punishment: Punishment) -> (&'static str, &'static str) {
    match punishment {
        Punishment::Off => ("Off", "no flash, no sound"),
        Punishment::Soft => ("Soft", "red flash and a harsh sound"),
        Punishment::Hard => ("Hard", "flash, sound and screen shake"),
    }
}

/// Whether a photo is really on disk (not an empty or evicted placeholder).
fn is_readable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

/// The data folder meant by a folder the user picked: the folder that holds
/// `faces/`. Picking the `faces` folder itself is understood too.
fn data_dir_from_choice(picked: &Path) -> PathBuf {
    let holds_manifest = picked.join("manifest.json").is_file();
    let holds_faces = picked.join("faces").join("manifest.json").is_file();
    match picked.parent() {
        Some(parent) if holds_manifest && !holds_faces => parent.to_path_buf(),
        _ => picked.to_path_buf(),
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

        let mut app = Self {
            epoch: Instant::now(),
            state_dir,
            faces_dir: PathBuf::new(),
            settings,
            history,
            manifest: None,
            manifest_error: None,
            catalog: None,
            unavailable: BTreeSet::new(),
            notice: (!notices.is_empty()).then(|| notices.join("\n")),
            sound: Sound::new(),
            screen: Screen::Home,
        };
        app.reload_data();
        app
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn data_dir(&self) -> PathBuf {
        self.settings
            .data_dir
            .clone()
            .unwrap_or_else(|| self.state_dir.clone())
    }

    /// Loads the face set from the configured folder and picks the screen:
    /// home when it is there, onboarding when it is not.
    fn reload_data(&mut self) {
        self.faces_dir = self.data_dir().join("faces");
        self.unavailable.clear();
        match Manifest::load(&self.faces_dir.join("manifest.json")) {
            Ok(manifest) => {
                self.manifest = Some(manifest);
                self.manifest_error = None;
                self.screen = Screen::Home;
            }
            Err(error) => {
                eprintln!("familiar: face set not loaded: {error}");
                self.manifest = None;
                self.manifest_error = Some(error.to_string());
                self.screen = Screen::Onboarding;
            }
        }
        self.rebuild_catalog();
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
        self.notice = None;
        match self.plan_session() {
            Ok(plan) => {
                let engine = Engine::new(plan, self.settings.progress.exposure_ms);
                let view = SessionView::new(
                    engine,
                    self.faces_dir.clone(),
                    self.settings.punishment,
                    self.settings.photo_size,
                    self.now_ms(),
                );
                self.screen = Screen::Session(Box::new(view));
            }
            Err(message) => {
                self.notice = Some(message);
                self.screen = Screen::Home;
            }
        }
    }

    fn finish_session(&mut self, ctx: &egui::Context, result: SessionResult) {
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
        let mistakes = self.mistake_faces(ctx, &result);
        self.screen = Screen::Results(Box::new(Summary {
            result,
            before,
            after,
            mistakes,
        }));
    }

    /// One face per person a mistake was made on, in order of appearance.
    fn mistake_faces(&self, ctx: &egui::Context, result: &SessionResult) -> Vec<MistakeFace> {
        let mut seen = BTreeSet::new();
        result
            .records
            .iter()
            .filter(|record| record.outcome.is_mistake())
            .filter(|record| seen.insert(record.trial.person_id.clone()))
            .take(MAX_MISTAKE_FACES)
            .map(|record| {
                let path = self.faces_dir.join(&record.trial.photo_file);
                let texture = match loader::decode(&path, &Distortion::NONE) {
                    Ok(image) => Some(ctx.load_texture(
                        format!("mistake-{}", record.trial.person_id),
                        image,
                        egui::TextureOptions::LINEAR,
                    )),
                    Err(error) => {
                        eprintln!("familiar: photo skipped: {error}");
                        None
                    }
                };
                MistakeFace {
                    person_id: record.trial.person_id.clone(),
                    outcome: record.outcome,
                    texture,
                }
            })
            .collect()
    }

    fn perform(&mut self, action: Action) {
        match action {
            Action::Go(screen) => self.screen = screen,
            Action::Start => self.start_session(),
            Action::ToggleExcluded(person) => {
                if !self.settings.excluded.remove(&person) {
                    self.settings.excluded.insert(person);
                }
                self.save_settings();
                self.rebuild_catalog();
            }
            Action::ClearExcluded => {
                self.settings.excluded.clear();
                self.save_settings();
                self.rebuild_catalog();
            }
            Action::ChooseFolder => {
                let dialog = rfd::FileDialog::new()
                    .set_title("Choose the Familiar data folder")
                    .set_directory(self.data_dir());
                if let Some(picked) = dialog.pick_folder() {
                    let chosen = data_dir_from_choice(&picked);
                    self.settings.data_dir = (chosen != self.state_dir).then_some(chosen);
                    self.save_settings();
                    self.reload_data();
                }
            }
            Action::DefaultFolder => {
                self.settings.data_dir = None;
                self.save_settings();
                self.reload_data();
            }
            Action::Reload => self.reload_data(),
        }
    }

    fn onboarding(&self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = None;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.1);
            ui.label(
                RichText::new("F A M I L I A R")
                    .font(theme::mono(34.0))
                    .color(theme::ACCENT),
            );
            ui.add_space(18.0);
            ui.label(RichText::new("The face set was not found").size(22.0));
            ui.add_space(6.0);
            ui.allocate_ui(Vec2::new(640.0, 0.0), |ui| {
                ui.label(
                    RichText::new(format!(
                        "Familiar trains on a folder of face photos. It looked for\n{}",
                        self.faces_dir.join("manifest.json").display()
                    ))
                    .color(theme::DIM),
                );
                if let Some(error) = &self.manifest_error {
                    ui.label(RichText::new(error).size(12.0).color(theme::WARN));
                }
                ui.add_space(18.0);

                ui.label("1.  Prepare the set. In the repository folder run:");
                ui.label(
                    RichText::new(PREPARE_COMMAND)
                        .font(theme::mono(14.0))
                        .color(theme::ACCENT),
                );
                if ui.button("Copy command").clicked() {
                    ui.ctx().copy_text(PREPARE_COMMAND.to_owned());
                }
                ui.label(
                    RichText::new(
                        "It downloads CelebA (about 1.4 GB) and sorts the photos by person. \
                         CelebA is for non-commercial research use only: keep it to yourself.",
                    )
                    .size(12.0)
                    .color(theme::DIM),
                );
                ui.add_space(18.0);

                ui.label("2.  Or point to the folder where the set already is:");
                if ui.button("Choose folder…").clicked() {
                    action = Some(Action::ChooseFolder);
                }
                ui.add_space(18.0);
                if ui.button("Check again").clicked() {
                    action = Some(Action::Reload);
                }
            });
        });
        action
    }

    fn home(&self, ui: &mut egui::Ui) -> Option<Action> {
        let ready = self.catalog.is_some();
        let mut action = ui
            .input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space))
            .then_some(Action::Start);
        let progress = self.settings.progress;
        let last = self.history.sessions.last().map(|s| s.balanced_accuracy);

        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.1);
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
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{} trials per session · punishment: {}",
                    self.settings.trials_per_session,
                    punishment_name(self.settings.punishment).0.to_lowercase()
                ))
                .color(theme::DIM),
            );
            ui.add_space(22.0);

            if ui.add_enabled(ready, big_button("START", 300.0)).clicked() {
                action = Some(Action::Start);
            }
            ui.label(RichText::new("Enter or Space").size(12.0).color(theme::DIM));
            ui.add_space(14.0);
            if ui
                .add(egui::Button::new("History").min_size(Vec2::new(300.0, 34.0)))
                .clicked()
            {
                action = Some(Action::Go(Screen::History));
            }
            if ui
                .add(egui::Button::new("Settings").min_size(Vec2::new(300.0, 34.0)))
                .clicked()
            {
                action = Some(Action::Go(Screen::Settings));
            }

            if let Some(notice) = &self.notice {
                ui.add_space(18.0);
                ui.allocate_ui(Vec2::new(640.0, 0.0), |ui| {
                    ui.label(RichText::new(notice).color(theme::WARN));
                });
            }
        });
        action.filter(|action| ready || !matches!(action, Action::Start))
    }

    fn settings_screen(&mut self, ui: &mut egui::Ui) -> Option<Action> {
        let mut action = ui
            .input(|i| i.key_pressed(Key::Escape))
            .then_some(Action::Go(Screen::Home));
        let mut changed = false;
        let dim = |text: &str| RichText::new(text).color(theme::DIM);

        ui.vertical_centered(|ui| {
            ui.add_space(28.0);
            ui.label(
                RichText::new("SETTINGS")
                    .font(theme::mono(26.0))
                    .color(theme::ACCENT),
            );
            ui.add_space(18.0);

            ui.allocate_ui(Vec2::new(680.0, 0.0), |ui| {
                egui::Grid::new("settings")
                    .num_columns(2)
                    .spacing([24.0, 18.0])
                    .show(ui, |ui| {
                        ui.label(dim("Data folder"));
                        ui.vertical(|ui| {
                            let folder = self.data_dir();
                            ui.label(
                                RichText::new(folder.display().to_string()).font(theme::mono(13.0)),
                            );
                            ui.horizontal(|ui| {
                                if ui.button("Change…").clicked() {
                                    action = Some(Action::ChooseFolder);
                                }
                                let custom = self.settings.data_dir.is_some();
                                if ui
                                    .add_enabled(custom, egui::Button::new("Use default"))
                                    .clicked()
                                {
                                    action = Some(Action::DefaultFolder);
                                }
                            });
                            ui.label(
                                dim("The folder that contains faces/manifest.json.").size(12.0),
                            );
                        });
                        ui.end_row();

                        ui.label(dim("Punishment"));
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                for option in [Punishment::Off, Punishment::Soft, Punishment::Hard]
                                {
                                    let selected = self.settings.punishment == option;
                                    let (name, _) = punishment_name(option);
                                    if ui.selectable_label(selected, name).clicked() {
                                        self.settings.punishment = option;
                                        changed = true;
                                    }
                                }
                            });
                            ui.label(dim(punishment_name(self.settings.punishment).1).size(12.0));
                        });
                        ui.end_row();

                        ui.label(dim("Photo size"));
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                for (option, name) in [
                                    (PhotoSize::Small, "Small"),
                                    (PhotoSize::Medium, "Medium"),
                                    (PhotoSize::Large, "Large"),
                                ] {
                                    let selected = self.settings.photo_size == option;
                                    if ui.selectable_label(selected, name).clicked() {
                                        self.settings.photo_size = option;
                                        changed = true;
                                    }
                                }
                            });
                            ui.label(
                                dim("The photos are only 178 × 218 pixels: smaller looks sharper.")
                                    .size(12.0),
                            );
                        });
                        ui.end_row();

                        ui.label(dim("Session length"));
                        ui.horizontal(|ui| {
                            for length in SESSION_LENGTHS {
                                let selected = self.settings.trials_per_session == length;
                                if ui.selectable_label(selected, length.to_string()).clicked() {
                                    self.settings.trials_per_session = length;
                                    changed = true;
                                }
                            }
                            ui.label(dim("trials").size(12.0));
                        });
                        ui.end_row();

                        ui.label(dim("Level"));
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                for value in Level::MIN.get()..=Level::MAX.get() {
                                    let level = Level::new(value);
                                    let selected = self.settings.progress.level == level;
                                    if ui.selectable_label(selected, value.to_string()).clicked() {
                                        self.settings.progress.level = level;
                                        changed = true;
                                    }
                                }
                                ui.label(level_name(self.settings.progress.level));
                            });
                            let auto = ui.checkbox(
                                &mut self.settings.auto_promote,
                                "Raise and lower the level automatically",
                            );
                            changed |= auto.changed();
                        });
                        ui.end_row();

                        ui.label(dim("People you know"));
                        ui.horizontal(|ui| {
                            let count = self.settings.excluded.len();
                            ui.label(format!("{count} excluded from sessions"));
                            if ui
                                .add_enabled(count > 0, egui::Button::new("Clear the list"))
                                .clicked()
                            {
                                action = Some(Action::ClearExcluded);
                            }
                        });
                        ui.end_row();
                    });
            });

            ui.add_space(26.0);
            if ui.button("Back  (Esc)").clicked() {
                action = Some(Action::Go(Screen::Home));
            }
        });

        if changed {
            self.save_settings();
        }
        action
    }

    fn results(
        ui: &mut egui::Ui,
        summary: &Summary,
        excluded: &BTreeSet<String>,
    ) -> Option<Action> {
        let Summary {
            result,
            before,
            after,
            mistakes,
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

        let mut action = ui.input(|i| {
            if i.key_pressed(Key::Enter) || i.key_pressed(Key::Space) {
                Some(Action::Start)
            } else if i.key_pressed(Key::Escape) {
                Some(Action::Go(Screen::Home))
            } else {
                None
            }
        });

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                ui.label(
                    RichText::new("SESSION COMPLETE").font(theme::mono(26.0)).color(theme::ACCENT),
                );
                ui.label(RichText::new(verdict).size(18.0).color(color));
                ui.add_space(16.0);

                tiles(
                    ui,
                    &[
                        Tile::new("HITS", counts.hits.to_string(), theme::GOOD),
                        Tile::new("MISSES", counts.misses.to_string(), mistakes_color(counts.misses)),
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
                ui.add_space(10.0);
                ui.label(
                    RichText::new(format!(
                        "Level {} · {} trials · next session starts at {} ms",
                        result.config.level.get(),
                        result.config.trials,
                        after.exposure_ms
                    ))
                    .color(theme::DIM),
                );

                if !mistakes.is_empty() {
                    ui.add_space(16.0);
                    ui.label(RichText::new("FACES YOU GOT WRONG").font(theme::mono(14.0)));
                    ui.label(
                        RichText::new(
                            "Recognise a celebrity? Exclude them: knowing a face makes it too easy.",
                        )
                        .size(12.0)
                        .color(theme::DIM),
                    );
                    if let Some(toggled) = mistake_strip(ui, mistakes, excluded) {
                        action = Some(Action::ToggleExcluded(toggled));
                    }
                }

                ui.add_space(18.0);
                if ui.add(big_button("AGAIN", 300.0)).clicked() {
                    action = Some(Action::Start);
                }
                ui.label(RichText::new("Enter or Space").size(12.0).color(theme::DIM));
                ui.add_space(6.0);
                if ui.button("Home  (Esc)").clicked() {
                    action = Some(Action::Go(Screen::Home));
                }
                ui.add_space(16.0);
            });
        });
        action
    }
}

/// A row of the faces a mistake was made on. Returns the person whose
/// exclusion was toggled.
fn mistake_strip(
    ui: &mut egui::Ui,
    mistakes: &[MistakeFace],
    excluded: &BTreeSet<String>,
) -> Option<String> {
    const PHOTO: Vec2 = Vec2::new(89.0, 109.0);
    let mut toggled = None;
    let width = (ui.available_width() - 48.0).min(820.0);
    ui.allocate_ui(Vec2::new(width, PHOTO.y + 70.0), |ui| {
        egui::ScrollArea::horizontal()
            .id_salt("mistake-strip")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for face in mistakes {
                        let is_excluded = excluded.contains(&face.person_id);
                        ui.vertical(|ui| {
                            ui.set_width(PHOTO.x + 24.0);
                            match &face.texture {
                                Some(texture) => {
                                    let sized = egui::load::SizedTexture::new(texture.id(), PHOTO);
                                    let tint = if is_excluded {
                                        Color32::DARK_GRAY
                                    } else {
                                        Color32::WHITE
                                    };
                                    ui.add(egui::Image::from_texture(sized).tint(tint));
                                }
                                None => {
                                    let (rect, _) = ui.allocate_exact_size(PHOTO, Sense::hover());
                                    ui.painter().rect_filled(rect, 0.0, theme::PANEL);
                                }
                            }
                            let kind = match face.outcome {
                                Outcome::Miss => "miss",
                                _ => "false alarm",
                            };
                            ui.label(RichText::new(kind).size(11.0).color(theme::DIM));
                            let label = if is_excluded { "Undo" } else { "I know them" };
                            if ui.small_button(label).clicked() {
                                toggled = Some(face.person_id.clone());
                            }
                        });
                    }
                });
            });
    });
    toggled
}

fn mistakes_color(count: u32) -> Color32 {
    if count == 0 { theme::GOOD } else { theme::BAD }
}

/// The main button of a screen.
fn big_button(text: &str, width: f32) -> egui::Button<'_> {
    let label = RichText::new(text)
        .font(theme::mono(24.0))
        .color(theme::BACKGROUND);
    egui::Button::new(label)
        .fill(theme::ACCENT)
        .min_size(Vec2::new(width, 62.0))
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
    let count = tiles.len() as f32;
    let width = ((ui.available_width() - 48.0).min(820.0) - GAP * (count - 1.0)) / count;
    let total = Vec2::new(width * count + GAP * (count - 1.0), HEIGHT);
    let (row, _) = ui.allocate_exact_size(total, Sense::hover());
    let painter = ui.painter();
    for (i, tile) in tiles.iter().enumerate() {
        let min = row.min + Vec2::new(i as f32 * (width + GAP), 0.0);
        let rect = egui::Rect::from_min_size(min, Vec2::new(width, HEIGHT));
        painter.rect_filled(rect, 4.0, theme::PANEL);
        painter.rect_stroke(
            rect,
            4.0,
            theme::stroke(1.0, theme::OUTLINE),
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
        panel.show(ctx, |ui| {
            let action = match &mut self.screen {
                Screen::Onboarding => self.onboarding(ui),
                Screen::Home => self.home(ui),
                Screen::Session(view) => match view.update(ui, now_ms, &self.sound) {
                    Some(SessionExit::Aborted) => Some(Action::Go(Screen::Home)),
                    Some(SessionExit::Finished(result)) => {
                        self.finish_session(ctx, *result);
                        None
                    }
                    None => None,
                },
                Screen::Results(summary) => Self::results(ui, summary, &self.settings.excluded),
                Screen::History => {
                    history_view::show(ui, &self.history).then_some(Action::Go(Screen::Home))
                }
                Screen::Settings => self.settings_screen(ui),
            };
            if let Some(action) = action {
                self.perform(action);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picked_folder_is_resolved_to_the_one_holding_faces() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("Familiar");
        let faces = data.join("faces");
        std::fs::create_dir_all(&faces).unwrap();
        std::fs::write(faces.join("manifest.json"), "{}").unwrap();

        assert_eq!(data_dir_from_choice(&data), data, "the data folder itself");
        assert_eq!(
            data_dir_from_choice(&faces),
            data,
            "the faces folder inside it"
        );
        // A folder without a face set is kept as picked; onboarding explains.
        assert_eq!(data_dir_from_choice(root.path()), root.path());
    }

    #[test]
    fn empty_and_missing_photos_are_not_readable() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("1.jpg");
        assert!(!is_readable(&photo));
        std::fs::write(&photo, b"").unwrap();
        assert!(!is_readable(&photo), "an empty placeholder");
        std::fs::write(&photo, b"jpeg").unwrap();
        assert!(is_readable(&photo));
        assert!(!is_readable(dir.path()), "a directory");
    }
}
