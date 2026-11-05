//! `MeterReader` weighting, `MeterLerp` interpolation and the endpoint meter pin rule.

use crate::snapshot::Flow;

/// `(min, max)` from per-channel peaks: 0 channels -> (0,0); 1 channel -> both the peak; else the
/// min and max of channels 0 and 1. Unified: both become `(lo*m + hi)/(m+1)` with
/// `m = max(0, low_channel_bias)` (0 = louder channel, 1 = mean, larger leans to the quieter one).
pub fn weigh_channel_peaks(channels: &[f32], unified: bool, low_channel_bias: i32) -> (f32, f32) {
    match channels {
        [] => (0.0, 0.0),
        [only] => (*only, *only),
        [a, b, ..] => {
            let lo = if a < b { *a } else { *b };
            let hi = if a > b { *a } else { *b };
            if unified {
                let combined = unified_weighting(lo, hi, low_channel_bias);
                (combined, combined)
            } else {
                (lo, hi)
            }
        }
    }
}

/// `ApplyUnifiedWeighting`: `(low * M + high) / (M + 1)`, M clamped to non-negative.
fn unified_weighting(low: f32, high: f32, bias: i32) -> f32 {
    let m = bias.max(0);
    (low * m as f32 + high) / (m + 1) as f32
}

/// `AudioDevice.ShouldPinEndpointMeterToSilence`: only an idle shared render endpoint is known to
/// have no meaningful peak. Capture endpoints are always polled.
pub fn should_pin_endpoint_meter_to_silence(flow: Flow, has_active_session: bool, exclusive_held: bool) -> bool {
    flow == Flow::Render && !exclusive_held && !has_active_session
}

/// Per-meter display interpolation driven by the UI render tick (`MeterLerp`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeterLerp {
    pub display: (f32, f32),
    prev: (f32, f32),
    target: (f32, f32),
    step: u32,
    steps: u32,
}

impl MeterLerp {
    /// A new raw sample arrived; interpolate toward it over `steps` render ticks
    /// (`max(1, fps / sample_rate)`).
    pub fn on_new_sample(&mut self, raw: (f32, f32), steps: u32) {
        self.prev = self.display;
        self.target = raw;
        self.step = 0;
        self.steps = steps.max(1);
    }

    /// One render tick: advance the lerp and move `display` at most `max_step` (0 freezes).
    /// Returns true when `display` changed.
    pub fn on_render_tick(&mut self, max_step: f32) -> bool {
        self.step = self.step.saturating_add(1);
        let lerped = if self.step >= self.steps {
            // Snap so a long render-only burst cannot drift past the most recent sample.
            self.target
        } else {
            let t = self.step as f32 / self.steps as f32;
            (self.prev.0 + (self.target.0 - self.prev.0) * t, self.prev.1 + (self.target.1 - self.prev.1) * t)
        };
        let next = (step_toward(self.display.0, lerped.0, max_step), step_toward(self.display.1, lerped.1, max_step));
        #[allow(clippy::float_cmp)]
        let changed = next.0 != self.display.0 || next.1 != self.display.1;
        self.display = next;
        changed
    }

    /// Snap to silence (`PinRawPeaksToSilence` + reset).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// `StepTowardTarget`: move by at most `max_step`, landing exactly on the target for the last
/// fractional step.
fn step_toward(current: f32, target: f32, max_step: f32) -> f32 {
    let delta = target - current;
    if delta > max_step {
        current + max_step
    } else if delta < -max_step {
        current - max_step
    } else {
        target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighting_handles_channel_counts() {
        assert_eq!((0.0, 0.0), weigh_channel_peaks(&[], false, 3));
        assert_eq!((0.4, 0.4), weigh_channel_peaks(&[0.4], true, 3));
        assert_eq!((0.2, 0.6), weigh_channel_peaks(&[0.6, 0.2], false, 3));
        // Only the first two channels count.
        assert_eq!((0.2, 0.6), weigh_channel_peaks(&[0.6, 0.2, 1.0, 0.0], false, 3));
    }

    #[test]
    fn unified_weighting_biases_toward_the_quieter_channel() {
        let (a, b) = weigh_channel_peaks(&[0.2, 0.6], true, 3);
        assert_eq!(a, b);
        assert!((a - (0.2 * 3.0 + 0.6) / 4.0).abs() < 1e-7);
        // M = 0 is the louder channel, M = 1 the mean, negative clamps to 0.
        assert_eq!((0.6, 0.6), weigh_channel_peaks(&[0.2, 0.6], true, 0));
        assert_eq!((0.6, 0.6), weigh_channel_peaks(&[0.2, 0.6], true, -5));
        let (mean, _) = weigh_channel_peaks(&[0.2, 0.6], true, 1);
        assert!((mean - 0.4).abs() < 1e-7);
    }

    /// Port of `AudioDeviceMeterPolicyTests.cs`.
    #[test]
    fn capture_endpoint_is_polled_regardless_of_session_visibility() {
        for (active, exclusive) in [(false, false), (false, true), (true, false), (true, true)] {
            assert!(!should_pin_endpoint_meter_to_silence(Flow::Capture, active, exclusive));
        }
    }

    #[test]
    fn render_endpoint_retains_session_and_exclusive_mode_gate() {
        for (active, exclusive, expected) in
            [(false, false, true), (true, false, false), (false, true, false), (true, true, false)]
        {
            assert_eq!(expected, should_pin_endpoint_meter_to_silence(Flow::Render, active, exclusive));
        }
    }

    #[test]
    fn lerp_interpolates_over_the_sample_steps() {
        let mut lerp = MeterLerp::default();
        lerp.on_new_sample((0.4, 0.8), 2);
        assert!(lerp.on_render_tick(1.0));
        assert_eq!((0.2, 0.4), lerp.display);
        assert!(lerp.on_render_tick(1.0));
        assert_eq!((0.4, 0.8), lerp.display);
        // Converged: further ticks hold the target and report no change.
        assert!(!lerp.on_render_tick(1.0));
        assert_eq!((0.4, 0.8), lerp.display);
    }

    #[test]
    fn lerp_rate_limits_by_max_step() {
        let mut lerp = MeterLerp::default();
        lerp.on_new_sample((1.0, 1.0), 1);
        assert!(lerp.on_render_tick(0.09));
        assert!((lerp.display.0 - 0.09).abs() < 1e-6);
        // The rate limit keeps advancing toward the target after the lerp converged.
        assert!(lerp.on_render_tick(0.09));
        assert!((lerp.display.0 - 0.18).abs() < 1e-6);
        // Falling is limited the same way.
        lerp.on_new_sample((0.0, 0.0), 1);
        assert!(lerp.on_render_tick(0.09));
        assert!((lerp.display.0 - 0.09).abs() < 1e-6);
    }

    #[test]
    fn zero_max_step_freezes_the_meter() {
        let mut lerp = MeterLerp::default();
        lerp.on_new_sample((0.7, 0.9), 1);
        assert!(!lerp.on_render_tick(0.0));
        assert_eq!((0.0, 0.0), lerp.display);
    }

    #[test]
    fn steps_below_one_snap_on_the_first_tick() {
        let mut lerp = MeterLerp::default();
        lerp.on_new_sample((0.3, 0.5), 0);
        assert!(lerp.on_render_tick(1.0));
        assert_eq!((0.3, 0.5), lerp.display);
    }

    #[test]
    fn lerp_starts_from_the_current_display() {
        let mut lerp = MeterLerp::default();
        lerp.on_new_sample((0.8, 0.8), 1);
        lerp.on_render_tick(1.0);
        lerp.on_new_sample((0.0, 0.0), 4);
        lerp.on_render_tick(1.0);
        assert!((lerp.display.0 - 0.6).abs() < 1e-6);
        lerp.reset();
        assert_eq!((0.0, 0.0), lerp.display);
    }
}
