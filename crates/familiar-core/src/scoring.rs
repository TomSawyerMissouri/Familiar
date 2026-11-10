//! Signal-detection scoring and the adaptive exposure rule.

use serde::{Deserialize, Serialize};

/// Shortest allowed exposure of a face.
pub const MIN_EXPOSURE_MS: u32 = 250;
/// Longest allowed exposure of a face.
pub const MAX_EXPOSURE_MS: u32 = 3000;
/// Exposure of the very first session.
pub const DEFAULT_EXPOSURE_MS: u32 = 1500;
/// Exposure is re-evaluated after every block of this many trials.
pub const BLOCK_SIZE: usize = 10;
/// Block accuracy at or above this speeds the session up.
pub const SPEED_UP_ACCURACY: f64 = 0.85;
/// Block accuracy below this slows the session down.
pub const SLOW_DOWN_ACCURACY: f64 = 0.65;
/// Exposure multiplier after a good block.
pub const SPEED_UP_FACTOR: f64 = 0.88;
/// Exposure multiplier after a bad block.
pub const SLOW_DOWN_FACTOR: f64 = 1.12;

/// How one trial ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Pressed on a repeat.
    Hit,
    /// Did not press on a repeat.
    Miss,
    /// Pressed on a new face.
    FalseAlarm,
    /// Did not press on a new face.
    CorrectRejection,
}

impl Outcome {
    /// Whether this outcome is punished.
    #[must_use]
    pub fn is_mistake(self) -> bool {
        matches!(self, Self::Miss | Self::FalseAlarm)
    }
}

/// Tally of trial outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Counts {
    pub hits: u32,
    pub misses: u32,
    pub false_alarms: u32,
    pub correct_rejections: u32,
}

impl Counts {
    pub fn record(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Hit => self.hits += 1,
            Outcome::Miss => self.misses += 1,
            Outcome::FalseAlarm => self.false_alarms += 1,
            Outcome::CorrectRejection => self.correct_rejections += 1,
        }
    }

    /// Number of repeat trials.
    #[must_use]
    pub fn repeats(&self) -> u32 {
        self.hits + self.misses
    }

    /// Number of new-face trials.
    #[must_use]
    pub fn news(&self) -> u32 {
        self.false_alarms + self.correct_rejections
    }

    #[must_use]
    pub fn total(&self) -> u32 {
        self.repeats() + self.news()
    }

    /// Share of repeats that were caught; `None` without repeats.
    #[must_use]
    pub fn hit_rate(&self) -> Option<f64> {
        rate(self.hits, self.repeats())
    }

    /// Share of new faces that were correctly let through; `None` without new faces.
    #[must_use]
    pub fn correct_rejection_rate(&self) -> Option<f64> {
        rate(self.correct_rejections, self.news())
    }

    /// Mean of hit rate and correct rejection rate.
    ///
    /// When one kind of trial is absent the other rate is used alone; with no
    /// trials at all the accuracy is 0.
    #[must_use]
    pub fn balanced_accuracy(&self) -> f64 {
        match (self.hit_rate(), self.correct_rejection_rate()) {
            (Some(h), Some(c)) => (h + c) / 2.0,
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => 0.0,
        }
    }

    /// Sensitivity index d′ with the log-linear correction: 0.5 is added to
    /// hits and false alarms and 1 to both trial counts, so rates of exactly
    /// 0 or 1 still give a finite value.
    #[must_use]
    pub fn d_prime(&self) -> f64 {
        let hit_rate = (f64::from(self.hits) + 0.5) / (f64::from(self.repeats()) + 1.0);
        let fa_rate = (f64::from(self.false_alarms) + 0.5) / (f64::from(self.news()) + 1.0);
        inverse_normal_cdf(hit_rate) - inverse_normal_cdf(fa_rate)
    }
}

fn rate(part: u32, whole: u32) -> Option<f64> {
    (whole > 0).then(|| f64::from(part) / f64::from(whole))
}

/// Keeps an exposure inside the allowed range.
#[must_use]
pub fn clamp_exposure(exposure_ms: u32) -> u32 {
    exposure_ms.clamp(MIN_EXPOSURE_MS, MAX_EXPOSURE_MS)
}

/// Multiplies an exposure by `factor`, rounding and clamping the result.
#[must_use]
pub fn scale_exposure(exposure_ms: u32, factor: f64) -> u32 {
    let scaled = (f64::from(exposure_ms) * factor).round();
    // The clamp keeps the value far inside the u32 range before the cast.
    clamp_exposure(scaled.clamp(0.0, f64::from(MAX_EXPOSURE_MS)) as u32)
}

/// Exposure for the next block given the accuracy of the block just finished.
#[must_use]
pub fn adapt_exposure(exposure_ms: u32, block_accuracy: f64) -> u32 {
    if block_accuracy >= SPEED_UP_ACCURACY {
        scale_exposure(exposure_ms, SPEED_UP_FACTOR)
    } else if block_accuracy < SLOW_DOWN_ACCURACY {
        scale_exposure(exposure_ms, SLOW_DOWN_FACTOR)
    } else {
        clamp_exposure(exposure_ms)
    }
}

/// Blank interval after a face: `max(150, 0.25 · exposure)`.
#[must_use]
pub fn isi_ms(exposure_ms: u32) -> u32 {
    (exposure_ms / 4).max(150)
}

/// Quantile function of the standard normal distribution (Acklam's rational
/// approximation, relative error below 1.2e-9).
#[must_use]
pub fn inverse_normal_cdf(p: f64) -> f64 {
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    const P_LOW: f64 = 0.024_25;

    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    let tail = |q: f64| {
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    if p < P_LOW {
        tail((-2.0 * p.ln()).sqrt())
    } else if p > 1.0 - P_LOW {
        -tail((-2.0 * (1.0 - p).ln()).sqrt())
    } else {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(hits: u32, misses: u32, false_alarms: u32, correct_rejections: u32) -> Counts {
        Counts {
            hits,
            misses,
            false_alarms,
            correct_rejections,
        }
    }

    #[test]
    fn inverse_normal_cdf_matches_known_quantiles() {
        for (p, z) in [
            (0.5, 0.0),
            (0.975, 1.959_964),
            (0.841_344_746, 1.0),
            (0.001, -3.090_232),
        ] {
            assert!((inverse_normal_cdf(p) - z).abs() < 1e-5, "p={p}");
        }
    }

    #[test]
    fn balanced_accuracy_averages_both_rates() {
        assert!((counts(3, 1, 1, 5).balanced_accuracy() - (0.75 + 5.0 / 6.0) / 2.0).abs() < 1e-12);
        assert_eq!(counts(4, 0, 0, 6).balanced_accuracy(), 1.0);
        assert_eq!(counts(0, 4, 6, 0).balanced_accuracy(), 0.0);
    }

    #[test]
    fn balanced_accuracy_uses_the_available_rate_when_one_kind_is_absent() {
        assert_eq!(counts(0, 0, 2, 8).balanced_accuracy(), 0.8);
        assert_eq!(counts(1, 3, 0, 0).balanced_accuracy(), 0.25);
        assert_eq!(Counts::default().balanced_accuracy(), 0.0);
    }

    #[test]
    fn d_prime_is_finite_on_edge_rates() {
        let perfect = counts(28, 0, 0, 52).d_prime();
        let inverted = counts(0, 28, 52, 0).d_prime();
        let empty = Counts::default().d_prime();
        assert!(perfect.is_finite() && perfect > 4.0, "{perfect}");
        assert!(inverted.is_finite() && inverted < -4.0, "{inverted}");
        assert!((perfect + inverted).abs() < 1e-9);
        assert_eq!(empty, 0.0);
    }

    #[test]
    fn d_prime_is_zero_at_chance_and_grows_with_sensitivity() {
        assert!(counts(10, 10, 10, 10).d_prime().abs() < 1e-9);
        assert!(counts(18, 2, 2, 18).d_prime() > counts(14, 6, 6, 14).d_prime());
        // Hit rate 0.75, false alarm rate 0.25 after correction: 15.5/21 and 5.5/21.
        let expected = inverse_normal_cdf(15.5 / 21.0) - inverse_normal_cdf(5.5 / 21.0);
        assert!((counts(15, 5, 5, 15).d_prime() - expected).abs() < 1e-12);
    }

    #[test]
    fn exposure_speeds_up_holds_and_slows_down() {
        assert_eq!(adapt_exposure(1000, 0.85), 880);
        assert_eq!(adapt_exposure(1000, 1.0), 880);
        assert_eq!(adapt_exposure(1000, 0.84), 1000);
        assert_eq!(adapt_exposure(1000, 0.65), 1000);
        assert_eq!(adapt_exposure(1000, 0.64), 1120);
    }

    #[test]
    fn exposure_stays_within_bounds() {
        let mut fast = DEFAULT_EXPOSURE_MS;
        let mut slow = DEFAULT_EXPOSURE_MS;
        for _ in 0..100 {
            fast = adapt_exposure(fast, 1.0);
            slow = adapt_exposure(slow, 0.0);
        }
        assert_eq!(fast, MIN_EXPOSURE_MS);
        assert_eq!(slow, MAX_EXPOSURE_MS);
        assert_eq!(adapt_exposure(100, 0.75), MIN_EXPOSURE_MS);
    }

    #[test]
    fn isi_is_a_quarter_of_exposure_but_at_least_150() {
        assert_eq!(isi_ms(1500), 375);
        assert_eq!(isi_ms(600), 150);
        assert_eq!(isi_ms(250), 150);
    }
}
