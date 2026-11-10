//! Familiar core: platform-independent training logic.
//!
//! This crate has no UI and no platform dependencies, so the same logic can
//! later be shared with an iOS app.

/// Crate version, shown in the app's about screen.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_set() {
        assert!(!super::VERSION.is_empty());
    }
}
