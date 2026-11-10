//! Settings and session history on disk.
//!
//! Both files are versioned JSON, written atomically (temporary file, then
//! rename). A file that cannot be understood is moved aside to `<name>.bak`
//! and replaced by defaults, so a damaged file never stops the app.

use std::collections::BTreeSet;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::engine::SessionResult;
use crate::error::{Error, Result};
use crate::progress::{Level, Progress};
use crate::scoring::Counts;

/// File name of the session history inside the data directory.
pub const HISTORY_FILE: &str = "history.json";
/// File name of the settings inside the data directory.
pub const SETTINGS_FILE: &str = "settings.json";
/// Schema version written to both files.
pub const SCHEMA_VERSION: u32 = 1;
/// Session lengths offered to the user.
pub const SESSION_LENGTHS: [usize; 4] = [40, 80, 120, 200];
/// Default session length.
pub const DEFAULT_SESSION_LENGTH: usize = 80;

/// How mistakes are punished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Punishment {
    /// No flash, no sound.
    Off,
    /// Red flash and harsh sound.
    #[default]
    Soft,
    /// Flash, sound and screen shake.
    Hard,
}

/// How large photos are drawn. The source photos are small, so a smaller
/// picture is enlarged less and looks sharper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhotoSize {
    Small,
    #[default]
    Medium,
    /// As large as the window allows.
    Large,
}

/// User settings and long-term progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    /// Folder holding `faces/`; `None` means the default location.
    pub data_dir: Option<PathBuf>,
    pub trials_per_session: usize,
    pub punishment: Punishment,
    pub photo_size: PhotoSize,
    /// Raise and lower the level automatically after each session.
    pub auto_promote: bool,
    pub progress: Progress,
    /// Ids of people the user knows; they never appear in sessions.
    pub excluded: BTreeSet<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            data_dir: None,
            trials_per_session: DEFAULT_SESSION_LENGTH,
            punishment: Punishment::default(),
            photo_size: PhotoSize::default(),
            auto_promote: true,
            progress: Progress::default(),
            excluded: BTreeSet::new(),
        }
    }
}

/// One finished session as stored in the history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// When the session ended, milliseconds since the Unix epoch.
    pub finished_at_unix_ms: u64,
    /// Seed the session was generated from, to reproduce it.
    pub seed: u64,
    pub level: Level,
    pub trials: usize,
    pub counts: Counts,
    pub balanced_accuracy: f64,
    pub d_prime: f64,
    pub mean_rt_ms: Option<f64>,
    pub exposure_start_ms: u32,
    pub exposure_end_ms: u32,
}

impl SessionRecord {
    /// Builds a history record. The core has no clock, so the caller supplies
    /// the finish time.
    #[must_use]
    pub fn from_result(result: &SessionResult, finished_at_unix_ms: u64) -> Self {
        Self {
            finished_at_unix_ms,
            seed: result.config.seed,
            level: result.config.level,
            trials: result.config.trials,
            counts: result.counts,
            balanced_accuracy: result.balanced_accuracy,
            d_prime: result.d_prime,
            mean_rt_ms: result.mean_rt_ms,
            exposure_start_ms: result.exposure_start_ms,
            exposure_end_ms: result.exposure_end_ms,
        }
    }
}

/// All finished sessions, oldest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub version: u32,
    pub sessions: Vec<SessionRecord>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            sessions: Vec::new(),
        }
    }
}

/// A loaded value, plus where the damaged original went if it had to be replaced.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded<T> {
    pub value: T,
    /// Path of the `.bak` copy when the file was unreadable and defaults were used.
    pub recovered_from: Option<PathBuf>,
}

trait Versioned {
    fn version(&self) -> u32;
}

impl Versioned for Settings {
    fn version(&self) -> u32 {
        self.version
    }
}

impl Versioned for History {
    fn version(&self) -> u32 {
        self.version
    }
}

/// Loads settings from `dir`. A missing file gives defaults.
pub fn load_settings(dir: &Path) -> Result<Loaded<Settings>> {
    load(&dir.join(SETTINGS_FILE))
}

/// Saves settings into `dir`, creating it if needed.
pub fn save_settings(dir: &Path, settings: &Settings) -> Result<()> {
    save(&dir.join(SETTINGS_FILE), settings)
}

/// Loads the history from `dir`. A missing file gives an empty history.
pub fn load_history(dir: &Path) -> Result<Loaded<History>> {
    load(&dir.join(HISTORY_FILE))
}

/// Saves the history into `dir`, creating it if needed.
pub fn save_history(dir: &Path, history: &History) -> Result<()> {
    save(&dir.join(HISTORY_FILE), history)
}

fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn load<T: DeserializeOwned + Default + Versioned>(path: &Path) -> Result<Loaded<T>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Ok(Loaded {
                value: T::default(),
                recovered_from: None,
            });
        }
        // Not valid UTF-8: the content is damaged, same as broken JSON.
        Err(e) if e.kind() == ErrorKind::InvalidData => String::new(),
        // Anything else (permissions, a file evicted to iCloud) may be
        // temporary, so the file is left alone and the error reported.
        Err(e) => return Err(io_error(path)(e)),
    };
    match serde_json::from_str::<T>(&text) {
        Ok(value) if value.version() == SCHEMA_VERSION => Ok(Loaded {
            value,
            recovered_from: None,
        }),
        _ => {
            let backup = with_suffix(path, ".bak");
            fs::rename(path, &backup).map_err(io_error(path))?;
            Ok(Loaded {
                value: T::default(),
                recovered_from: Some(backup),
            })
        }
    }
}

fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(io_error(dir))?;
    }
    let json = serde_json::to_vec_pretty(value).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })?;
    let tmp = with_suffix(path, ".tmp");
    let write = || -> std::io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&json)?;
        file.sync_all()
    };
    write().map_err(io_error(&tmp))?;
    fs::rename(&tmp, path).map_err(io_error(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_history() -> History {
        History {
            version: SCHEMA_VERSION,
            sessions: vec![SessionRecord {
                finished_at_unix_ms: 1_794_300_000_000,
                seed: 42,
                level: Level::new(3),
                trials: 80,
                counts: Counts {
                    hits: 20,
                    misses: 8,
                    false_alarms: 4,
                    correct_rejections: 48,
                },
                balanced_accuracy: 0.82,
                d_prime: 1.9,
                mean_rt_ms: Some(612.5),
                exposure_start_ms: 1500,
                exposure_end_ms: 1161,
            }],
        }
    }

    #[test]
    fn missing_files_give_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let settings = load_settings(dir.path()).unwrap();
        assert_eq!(settings.value, Settings::default());
        assert_eq!(settings.recovered_from, None);
        assert_eq!(load_history(dir.path()).unwrap().value, History::default());
    }

    #[test]
    fn settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            data_dir: Some(PathBuf::from("/Volumes/Faces")),
            trials_per_session: 200,
            punishment: Punishment::Hard,
            photo_size: PhotoSize::Small,
            auto_promote: false,
            progress: Progress {
                level: Level::new(4),
                exposure_ms: 640,
            },
            excluded: ["2880".to_string(), "17".to_string()].into(),
            ..Settings::default()
        };
        save_settings(dir.path(), &settings).unwrap();
        let loaded = load_settings(dir.path()).unwrap();
        assert_eq!(loaded.value, settings);
        assert_eq!(loaded.recovered_from, None);
    }

    #[test]
    fn history_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let history = sample_history();
        save_history(dir.path(), &history).unwrap();
        assert_eq!(load_history(dir.path()).unwrap().value, history);
        let text = fs::read_to_string(dir.path().join(HISTORY_FILE)).unwrap();
        assert!(text.contains("\"version\": 1"), "{text}");
    }

    #[test]
    fn save_creates_the_directory_and_leaves_no_temporary_file() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("nested").join("Familiar");
        save_history(&dir, &sample_history()).unwrap();
        save_history(&dir, &History::default()).unwrap();
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, [HISTORY_FILE]);
    }

    #[test]
    fn corrupted_files_are_moved_aside_and_replaced_by_defaults() {
        for garbage in [
            &b"{ \"version\": 1, \"sessions\": [ {"[..],
            b"",
            b"\xff\xfe\x00binary",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(HISTORY_FILE);
            fs::write(&path, garbage).unwrap();

            let loaded = load_history(dir.path()).unwrap();
            let backup = dir.path().join("history.json.bak");
            assert_eq!(loaded.value, History::default());
            assert_eq!(loaded.recovered_from.as_deref(), Some(backup.as_path()));
            assert_eq!(fs::read(&backup).unwrap(), garbage);
            assert!(!path.exists());

            // The app keeps working: the next save and load succeed.
            save_history(dir.path(), &sample_history()).unwrap();
            assert_eq!(load_history(dir.path()).unwrap().value, sample_history());
        }
    }

    #[test]
    fn corrupted_settings_recover_too() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), "not json at all").unwrap();
        let loaded = load_settings(dir.path()).unwrap();
        assert_eq!(loaded.value, Settings::default());
        assert!(loaded.recovered_from.is_some());
    }

    #[test]
    fn unknown_schema_version_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(HISTORY_FILE),
            r#"{"version": 2, "sessions": []}"#,
        )
        .unwrap();
        let loaded = load_history(dir.path()).unwrap();
        assert_eq!(loaded.value, History::default());
        assert!(dir.path().join("history.json.bak").exists());
    }

    #[test]
    fn settings_missing_fields_fall_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"version": 1, "trials_per_session": 40}"#,
        )
        .unwrap();
        let loaded = load_settings(dir.path()).unwrap();
        assert_eq!(
            loaded.value,
            Settings {
                trials_per_session: 40,
                ..Settings::default()
            }
        );
        assert_eq!(loaded.recovered_from, None);
    }
}
