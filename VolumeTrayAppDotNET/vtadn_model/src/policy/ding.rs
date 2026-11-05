//! `DingSuppressionPeak`: decaying recent maximum used to skip the volume-change ding while audio
//! is already playing. New peaks attack immediately; lower samples let the held maximum fall
//! exponentially without a discrete cutoff.

use std::time::Instant;

/// `TimeConstants.DingSuppressionPeakHalfLifeMs`.
pub const HALF_LIFE_MS: f64 = 250.0;

const THRESHOLD_PERCENT_MIN: i32 = 0;
const THRESHOLD_PERCENT_MAX: i32 = 100;
const PERCENT_TO_SCALAR: f32 = 0.01;

#[derive(Clone, Copy, Debug, Default)]
pub struct DingSuppressionPeak {
    value: f32,
    last: Option<Instant>,
}

impl DingSuppressionPeak {
    /// `value *= 0.5^(elapsed/250ms)`, then `value = max(value, sample)`. Returns the new value.
    /// Non-finite samples count as 0; samples are clamped to 0..1.
    pub fn observe(&mut self, sample: f32, now: Instant) -> f32 {
        let clamped = if sample.is_finite() { sample.clamp(0.0, 1.0) } else { 0.0 };
        self.advance(now);
        if clamped >= self.value {
            self.value = clamped;
        }
        self.value
    }

    /// The held maximum after applying the falloff through `now` (`Read`).
    pub fn read(&mut self, now: Instant) -> f32 {
        self.advance(now);
        self.value
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    fn advance(&mut self, now: Instant) {
        let Some(last) = self.last else {
            self.last = Some(now);
            return;
        };
        if now <= last {
            return;
        }
        let elapsed_ms = now.duration_since(last).as_secs_f64() * 1000.0;
        self.last = Some(now);
        self.value *= 0.5f32.powf((elapsed_ms / HALF_LIFE_MS) as f32);
    }
}

/// `ResolveThreshold`: the configured full-volume threshold scaled by the volume being set.
pub fn resolve_threshold(threshold_percent: i32, volume: f32) -> f32 {
    let percent = threshold_percent.clamp(THRESHOLD_PERCENT_MIN, THRESHOLD_PERCENT_MAX);
    let volume = if volume.is_finite() { volume.clamp(0.0, 1.0) } else { 0.0 };
    percent as f32 * PERCENT_TO_SCALAR * volume
}

/// Suppress when the peak is unavailable or above `threshold% * 0.01 * volume`
/// (`ShouldSuppressFeedback`; an unavailable peak fails closed: the endpoint is tearing down).
pub fn should_suppress(peak: Option<f32>, threshold_percent: i32, volume: f32) -> bool {
    let Some(peak) = peak else { return true };
    let clamped = if peak.is_finite() { peak.clamp(0.0, 1.0) } else { 0.0 };
    clamped > resolve_threshold(threshold_percent, volume)
}

#[cfg(test)]
mod tests {
    //! Port of `DingSuppressionPeakTests.cs`.
    use std::time::Duration;

    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn observe_takes_new_peaks_immediately_and_decays_held_peaks() {
        let start = Instant::now();
        let mut peak = DingSuppressionPeak::default();
        let initial = peak.observe(0.8, start);
        let decayed = peak.observe(0.1, start + ms(250));
        let replaced = peak.observe(0.6, start + ms(500));
        assert_eq!(0.8, initial);
        assert!((0.3999..=0.4001).contains(&decayed), "{decayed}");
        assert_eq!(0.6, replaced);
    }

    #[test]
    fn read_continues_decaying_without_a_discrete_expiration() {
        let start = Instant::now();
        let mut peak = DingSuppressionPeak::default();
        peak.observe(1.0, start);
        let after_two = peak.read(start + ms(2_000));
        let after_three = peak.read(start + ms(3_000));
        assert!((0.003_906_2..=0.003_906_3).contains(&after_two), "{after_two}");
        assert!((0.000_244_1..=0.000_244_2).contains(&after_three), "{after_three}");
    }

    #[test]
    fn observe_uses_the_current_peak_when_it_exceeds_decayed_history() {
        let start = Instant::now();
        let mut peak = DingSuppressionPeak::default();
        peak.observe(1.0, start);
        assert_eq!(0.12, peak.observe(0.12, start + ms(2_000)));
    }

    #[test]
    fn resolve_threshold_scales_with_volume() {
        for (percent, volume, expected) in
            [(5, 1.0f32, 0.05f32), (5, 0.5, 0.025), (5, 0.1, 0.005), (100, 0.75, 0.75), (0, 1.0, 0.0)]
        {
            let threshold = resolve_threshold(percent, volume);
            assert!((threshold - expected).abs() <= 0.000_001, "{percent} {volume} -> {threshold}");
        }
    }

    #[test]
    fn resolve_threshold_clamps_invalid_ranges() {
        for (percent, volume, expected) in [(-10, 0.5f32, 0.0f32), (150, 0.5, 0.5), (5, -1.0, 0.0), (5, 2.0, 0.05)] {
            let threshold = resolve_threshold(percent, volume);
            assert!((threshold - expected).abs() <= 0.000_001, "{percent} {volume} -> {threshold}");
        }
    }

    #[test]
    fn unavailable_peak_suppresses_feedback_during_endpoint_teardown() {
        assert!(should_suppress(None, 5, 1.0));
    }

    #[test]
    fn available_silent_peak_allows_rapid_feedback() {
        assert!(!should_suppress(Some(0.0), 5, 1.0));
    }
}
