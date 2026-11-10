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
pub mod error;
pub mod manifest;

pub use catalog::Catalog;
pub use error::{Error, Result};
pub use manifest::{Appearance, Manifest, Person, Photo};

/// Crate version, shown in the app's about screen.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
