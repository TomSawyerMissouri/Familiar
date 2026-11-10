//! Typed errors of the core crate.

use std::path::PathBuf;

/// Everything that can go wrong inside the core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file could not be read, written or renamed.
    #[error("I/O error at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A file was read but is not valid JSON of the expected shape.
    #[error("invalid JSON in {}: {source}", path.display())]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// The manifest was parsed but breaks one of its invariants.
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),

    /// The catalog is too small for the requested session.
    #[error("not enough people for the session: need {needed}, have {available}")]
    NotEnoughPeople { needed: usize, available: usize },

    /// The session configuration makes no sense.
    #[error("invalid session config: {0}")]
    InvalidConfig(String),
}

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;
