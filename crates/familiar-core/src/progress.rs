//! Long-term progress: disguise level and starting exposure across sessions.

use serde::{Deserialize, Serialize};

use crate::scoring::{DEFAULT_EXPOSURE_MS, clamp_exposure, scale_exposure};

/// Session accuracy at or above this promotes to the next level.
pub const PROMOTE_ACCURACY: f64 = 0.85;
/// Session accuracy below this demotes to the previous level.
pub const DEMOTE_ACCURACY: f64 = 0.60;
/// Exposure multiplier applied on promotion, to soften the harder level.
pub const PROMOTION_EXPOSURE_FACTOR: f64 = 1.15;

/// Disguise level, 1 (same photo) to 5 (full disguise with lookalikes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(from = "u8", into = "u8")]
pub struct Level(u8);

impl Level {
    pub const MIN: Self = Self(1);
    pub const MAX: Self = Self(5);

    /// Creates a level, clamping the value into `1..=5`.
    #[must_use]
    pub fn new(value: u8) -> Self {
        Self(value.clamp(Self::MIN.0, Self::MAX.0))
    }

    #[must_use]
    pub fn get(self) -> u8 {
        self.0
    }

    #[must_use]
    pub fn up(self) -> Self {
        Self::new(self.0 + 1)
    }

    #[must_use]
    pub fn down(self) -> Self {
        Self::new(self.0.saturating_sub(1))
    }
}

impl Default for Level {
    fn default() -> Self {
        Self::MIN
    }
}

impl From<u8> for Level {
    fn from(value: u8) -> Self {
        Self::new(value)
    }
}

impl From<Level> for u8 {
    fn from(level: Level) -> Self {
        level.0
    }
}

/// Where the next session starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub level: Level,
    pub exposure_ms: u32,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            level: Level::MIN,
            exposure_ms: DEFAULT_EXPOSURE_MS,
        }
    }
}

impl Progress {
    /// Progress after a finished session.
    ///
    /// The exposure the session ended on becomes the next starting exposure.
    /// With `auto_promote`, an accuracy of at least [`PROMOTE_ACCURACY`] raises
    /// the level and, only if the level actually rose, relaxes the exposure by
    /// [`PROMOTION_EXPOSURE_FACTOR`]; an accuracy below [`DEMOTE_ACCURACY`]
    /// lowers the level.
    #[must_use]
    pub fn after_session(self, accuracy: f64, end_exposure_ms: u32, auto_promote: bool) -> Self {
        let mut next = Self {
            level: self.level,
            exposure_ms: clamp_exposure(end_exposure_ms),
        };
        if !auto_promote {
            return next;
        }
        if accuracy >= PROMOTE_ACCURACY {
            next.level = self.level.up();
            if next.level != self.level {
                next.exposure_ms = scale_exposure(next.exposure_ms, PROMOTION_EXPOSURE_FACTOR);
            }
        } else if accuracy < DEMOTE_ACCURACY {
            next.level = self.level.down();
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::{MAX_EXPOSURE_MS, MIN_EXPOSURE_MS};

    fn at(level: u8, exposure_ms: u32) -> Progress {
        Progress {
            level: Level::new(level),
            exposure_ms,
        }
    }

    #[test]
    fn level_is_clamped() {
        assert_eq!(Level::new(0), Level::MIN);
        assert_eq!(Level::new(9), Level::MAX);
        assert_eq!(Level::MAX.up(), Level::MAX);
        assert_eq!(Level::MIN.down(), Level::MIN);
        assert_eq!(serde_json::from_str::<Level>("42").unwrap(), Level::MAX);
    }

    #[test]
    fn good_session_promotes_and_relaxes_exposure() {
        assert_eq!(at(2, 1500).after_session(0.85, 1000, true), at(3, 1150));
        assert_eq!(at(2, 1500).after_session(0.95, 1000, true), at(3, 1150));
    }

    #[test]
    fn top_level_keeps_exposure_on_a_good_session() {
        assert_eq!(at(5, 1500).after_session(0.99, 400, true), at(5, 400));
    }

    #[test]
    fn bad_session_demotes() {
        assert_eq!(at(3, 1500).after_session(0.59, 2000, true), at(2, 2000));
        assert_eq!(at(1, 1500).after_session(0.10, 2000, true), at(1, 2000));
    }

    #[test]
    fn middling_session_keeps_the_level() {
        assert_eq!(at(3, 1500).after_session(0.60, 900, true), at(3, 900));
        assert_eq!(at(3, 1500).after_session(0.849, 900, true), at(3, 900));
    }

    #[test]
    fn manual_mode_only_carries_the_exposure_over() {
        assert_eq!(at(3, 1500).after_session(1.0, 700, false), at(3, 700));
        assert_eq!(at(3, 1500).after_session(0.0, 700, false), at(3, 700));
    }

    #[test]
    fn exposure_stays_within_bounds() {
        assert_eq!(
            at(1, 1500).after_session(0.9, 2900, true).exposure_ms,
            MAX_EXPOSURE_MS
        );
        assert_eq!(
            at(1, 1500).after_session(0.7, 10, true).exposure_ms,
            MIN_EXPOSURE_MS
        );
    }
}
