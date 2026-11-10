//! Familiar core: platform-independent training logic.
//!
//! This crate has no UI and no platform dependencies, so the same logic can
//! later be shared with an iOS app.
//!
//! The flow is: load a [`Manifest`], build a [`Catalog`] without the excluded
//! people, [`generate`] a [`SessionPlan`] from a seed, and drive it with an
//! [`Engine`] by feeding it timestamps. The [`SessionResult`] then updates the
//! [`Progress`] and is appended to the [`History`].

pub mod catalog;
pub mod engine;
pub mod error;
pub mod manifest;
pub mod progress;
pub mod scoring;
pub mod session;
pub mod storage;

pub use catalog::Catalog;
pub use engine::{Engine, EngineState, Event, Mistake, SessionResult, TrialRecord};
pub use error::{Error, Result};
pub use manifest::{Appearance, Manifest, Person, Photo};
pub use progress::{Level, Progress};
pub use scoring::{Counts, Outcome};
pub use session::{Distortion, SessionConfig, SessionPlan, Trial, TrialKind, generate};
pub use storage::{History, PhotoSize, Punishment, SessionRecord, Settings};

/// Crate version, shown in the app's about screen.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
