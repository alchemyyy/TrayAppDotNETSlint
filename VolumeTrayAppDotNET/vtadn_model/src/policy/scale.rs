//! `AppVolumeScale`: the optional logarithmic app slider (6 dB per 10% of travel, linear fade
//! below the knee). Device sliders are always linear in the endpoint scalar.
//!
//! Windows applies a session level (`ISimpleAudioVolume`) as a plain amplitude multiplier, so a
//! slider wired straight to it spends most of its travel in the loudest few decibels. The
//! logarithmic scale spreads loudness evenly: every 10% of travel is 6 dB, and below the point
//! where a line from the origin touches that curve, the line itself fades to true silence at 0.

/// `PercentMax`.
pub const PERCENT_MAX: f64 = 100.0;

/// `DynamicRangeDecibels`: decibels across the whole travel of the constant-decibel curve.
pub const DYNAMIC_RANGE_DECIBELS: f64 = 60.0;

/// `PositionPercentDecimals`: decimal places kept on a logarithmic position read back from a level.
const POSITION_PERCENT_DECIMALS: i32 = 5;

/// `GrowthRate`: natural-log change in level per unit of travel, `level = e^(G * (position - 1))`.
fn growth_rate() -> f64 {
    DYNAMIC_RANGE_DECIBELS * std::f64::consts::LN_10 / 20.0
}

/// `KneePosition`: the line from the origin is tangent to the curve at `1 / G`.
pub fn knee_position() -> f64 {
    1.0 / growth_rate()
}

/// `KneeLevel` = `e^(1 - G)`.
fn knee_level() -> f64 {
    (1.0 - growth_rate()).exp()
}

/// `TangentSlope` = `KneeLevel / KneePosition`.
fn tangent_slope() -> f64 {
    knee_level() / knee_position()
}

fn level_from_position(position: f64) -> f64 {
    if position <= knee_position() { position * tangent_slope() } else { (growth_rate() * (position - 1.0)).exp() }
}

fn position_from_level(level: f64) -> f64 {
    if level <= knee_level() { level / tangent_slope() } else { 1.0 + level.ln() / growth_rate() }
}

/// C# `Math.Clamp(double, min, max)`: NaN passes through.
fn clamp_unit(value: f64) -> f64 {
    if value.is_nan() { value } else { value.clamp(0.0, 1.0) }
}

/// C# `Math.Round(value, digits)` (banker's rounding).
fn round_digits(value: f64, digits: i32) -> f64 {
    let power = 10f64.powi(digits);
    (value * power).round_ties_even() / power
}

/// Session level 0..1 for a slider percent 0..100.
pub fn level_from_slider_percent(percent: f64, logarithmic: bool) -> f32 {
    let position = clamp_unit(percent / PERCENT_MAX);
    (if logarithmic { level_from_position(position) } else { position }) as f32
}

/// Slider percent 0..100 for a session level (log: rounded to 5 decimals).
pub fn slider_percent_from_level(level: f32, logarithmic: bool) -> f64 {
    let clamped = clamp_unit(f64::from(level));
    if logarithmic {
        round_digits(position_from_level(clamped) * PERCENT_MAX, POSITION_PERCENT_DECIMALS)
    } else {
        clamped * PERCENT_MAX
    }
}

/// Fraction of the app slider's thumb extent a peak fills (log: on the same curve).
///
/// Linear: the session peak is the signal before the session level is applied. Logarithmic: the
/// meter ends where the thumb would sit for the level that reaches the device (peak times level),
/// so a full-scale signal still reaches the thumb.
pub fn meter_peak_fraction(peak: f32, level: f32, logarithmic: bool) -> f32 {
    if !logarithmic {
        return peak;
    }
    let clamped_peak = if peak.is_finite() { f64::from(peak.clamp(0.0, 1.0)) } else { 0.0 };
    let clamped_level = if level.is_finite() { f64::from(level.clamp(0.0, 1.0)) } else { 0.0 };
    let thumb_position = position_from_level(clamped_level);
    if clamped_peak <= 0.0 || thumb_position <= 0.0 {
        return 0.0;
    }
    let meter_position = position_from_level(clamped_peak * clamped_level);
    (meter_position / thumb_position).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    //! Port of `AppVolumeScaleTests.cs`.
    use super::*;

    /// FlyoutSlider ignores Value writes closer than this.
    const SLIDER_VALUE_RESOLUTION: f64 = 0.001;
    const PERCENT_STEPS: i32 = 10_000;
    /// `AudioSession.VolumeEqualityEpsilon`.
    const VOLUME_EQUALITY_EPSILON: f32 = 0.000_001;

    fn assert_precision(expected: f64, actual: f64, digits: i32) {
        // xUnit `Assert.Equal(double, double, precision)` rounds both to `digits` decimals.
        assert_eq!(round_digits(expected, digits), round_digits(actual, digits), "{expected} vs {actual}");
    }

    #[test]
    fn endpoints_are_silence_and_full_volume() {
        for log in [true, false] {
            assert_eq!(0.0, level_from_slider_percent(0.0, log));
            assert_eq!(1.0, level_from_slider_percent(100.0, log));
            assert_eq!(0.0, slider_percent_from_level(0.0, log));
            assert_eq!(100.0, slider_percent_from_level(1.0, log));
        }
    }

    #[test]
    fn out_of_range_percents_clamp_to_the_endpoints() {
        assert_eq!(0.0, level_from_slider_percent(-25.0, true));
        assert_eq!(1.0, level_from_slider_percent(250.0, true));
    }

    #[test]
    fn logarithmic_level_rises_strictly_with_slider_position() {
        let mut previous = level_from_slider_percent(0.0, true);
        for step in 1..=PERCENT_STEPS {
            let percent = f64::from(step) * 100.0 / f64::from(PERCENT_STEPS);
            let level = level_from_slider_percent(percent, true);
            assert!(level > previous, "Level stopped rising at {percent}%");
            previous = level;
        }
    }

    #[test]
    fn logarithmic_position_rises_strictly_with_level() {
        let mut previous = slider_percent_from_level(0.0, true);
        for step in 1..=PERCENT_STEPS {
            let level = step as f32 / PERCENT_STEPS as f32;
            let percent = slider_percent_from_level(level, true);
            assert!(percent > previous, "Position stopped rising at level {level}");
            previous = percent;
        }
    }

    #[test]
    fn windows_echo_of_a_set_level_leaves_the_slider_where_it_was() {
        for step in 0..=PERCENT_STEPS {
            let percent = f64::from(step) * 100.0 / f64::from(PERCENT_STEPS);
            let level = level_from_slider_percent(percent, true);
            let echoed = slider_percent_from_level(level, true);
            assert!((echoed - percent).abs() < SLIDER_VALUE_RESOLUTION / 2.0, "{percent}% came back as {echoed}%");
        }
    }

    #[test]
    fn repositioning_from_a_level_never_triggers_another_write() {
        for step in 0..=PERCENT_STEPS {
            let level = step as f32 / PERCENT_STEPS as f32;
            let percent = slider_percent_from_level(level, true);
            let written = level_from_slider_percent(percent, true);
            assert!((written - level).abs() < VOLUME_EQUALITY_EPSILON, "Level {level} drifted to {written}");
        }
    }

    #[test]
    fn equal_steps_above_the_fade_change_loudness_by_equal_decibels() {
        let mut percent = (knee_position() * 100.0).ceil();
        while percent + 10.0 <= 100.0 {
            let lower = level_from_slider_percent(percent, true);
            let upper = level_from_slider_percent(percent + 10.0, true);
            let decibels = 20.0 * (f64::from(upper) / f64::from(lower)).log10();
            assert_precision(DYNAMIC_RANGE_DECIBELS / 10.0, decibels, 3);
            percent += 1.0;
        }
    }

    #[test]
    fn fade_to_silence_meets_the_decibel_curve_without_a_jump() {
        let knee_percent = knee_position() * 100.0;
        let below = level_from_slider_percent(knee_percent - 0.001, true);
        let above = level_from_slider_percent(knee_percent + 0.001, true);
        assert!(below < above);
        assert!(above - below < 1e-6, "Jump of {} at the knee", above - below);
    }

    /// One wheel notch on a flyout app slider.
    fn step_through_windows(level: f32, step_percent: i32) -> f32 {
        let position = slider_percent_from_level(level, true);
        let next = (position + f64::from(step_percent)).clamp(0.0, 100.0);
        level_from_slider_percent(next, true)
    }

    #[test]
    fn wheel_steps_climb_out_of_silence_one_step_at_a_time() {
        for step_percent in [1, 2, 5] {
            let mut level = 0.0f32;
            let mut notch = 1;
            while notch * step_percent <= 100 {
                let next = step_through_windows(level, step_percent);
                assert!(next - level >= VOLUME_EQUALITY_EPSILON, "Notch {notch} was swallowed");
                assert_precision(f64::from(notch * step_percent), slider_percent_from_level(next, true), 3);
                level = next;
                notch += 1;
            }
        }
    }

    #[test]
    fn wheel_steps_descend_from_full_volume_to_silence() {
        for step_percent in [1, 2, 5] {
            let mut level = 1.0f32;
            let mut notch = 1;
            while notch * step_percent <= 100 {
                let next = step_through_windows(level, -step_percent);
                assert!(level - next >= VOLUME_EQUALITY_EPSILON, "Notch {notch} was swallowed");
                assert_precision(f64::from(100 - notch * step_percent), slider_percent_from_level(next, true), 3);
                level = next;
                notch += 1;
            }
            assert_eq!(0.0, level);
        }
    }

    #[test]
    fn setting_off_is_the_plain_percent_conversion() {
        for step in 0..=PERCENT_STEPS {
            let percent = f64::from(step) * 100.0 / f64::from(PERCENT_STEPS);
            let level = step as f32 / PERCENT_STEPS as f32;
            assert_eq!((percent / 100.0) as f32, level_from_slider_percent(percent, false));
            assert_eq!(f64::from(level) * 100.0, slider_percent_from_level(level, false));
        }
    }

    #[test]
    fn setting_off_passes_meter_peaks_through() {
        for (peak, level) in [(0.0f32, 0.5f32), (0.25, 0.5), (1.0, 0.003), (0.8, 0.0)] {
            assert_eq!(peak, meter_peak_fraction(peak, level, false));
        }
    }

    #[test]
    fn full_scale_peak_reaches_the_thumb() {
        for level in [1.0f32, 0.5, 0.02] {
            assert_precision(1.0, f64::from(meter_peak_fraction(1.0, level, true)), 5);
        }
    }

    #[test]
    fn silent_peak_or_silent_level_draws_no_meter() {
        for (peak, level) in [(0.0f32, 0.5f32), (0.5, 0.0), (f32::NAN, 0.5)] {
            assert_eq!(0.0, meter_peak_fraction(peak, level, true));
        }
    }

    #[test]
    fn meter_ends_where_the_thumb_would_sit_for_the_level_reaching_the_device() {
        let level = 0.5f32;
        let peak = 0.25f32;
        let thumb_percent = slider_percent_from_level(level, true);
        let expected_meter_percent = slider_percent_from_level(peak * level, true);
        let fraction = meter_peak_fraction(peak, level, true);
        assert_precision(expected_meter_percent, f64::from(fraction) * thumb_percent, 3);
    }

    #[test]
    fn meter_grows_strictly_with_peak() {
        let mut previous = 0.0f32;
        for step in 1..=1000 {
            let fraction = meter_peak_fraction(step as f32 / 1000.0, 0.3, true);
            assert!(fraction > previous, "Meter stopped growing at peak {}", step as f32 / 1000.0);
            previous = fraction;
        }
    }
}
