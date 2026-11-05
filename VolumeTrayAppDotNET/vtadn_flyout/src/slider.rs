//! `FlyoutSlider` math (port of the static helpers in `Controls/FlyoutSlider.cs`),
//! `WheelStepAccumulator`, and the percent label/editor text rules of `VolumeFlyoutWindow`
//! (`ScalarText`, `PercentText`, `TryParseSliderPercent`, `ApplyPercentText`).
//!
//! The Slint `FlyoutSlider` (`TrayAppDotNETCommon/ui/flyout_slider.slint`) evaluates the same
//! geometry formulas in its bindings; these functions are the tested reference.

use crate::metrics::{SLIDER_MAXIMUM, SLIDER_VALUE_EPSILON};

/// `WheelStepAccumulator`: turns vertical wheel deltas (1.0 per notch) into whole steps.
/// Precision touchpads report fractions, which accumulate until they add up to a step.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelStepAccumulator {
    pending: f64,
}

impl WheelStepAccumulator {
    /// Adds a vertical wheel delta and returns the whole steps it completes, positive for wheel up.
    pub fn add(&mut self, delta_y: f64) -> i32 {
        if delta_y == 0.0 || !delta_y.is_finite() {
            return 0;
        }
        // A reversed direction starts over instead of first paying back the opposite remainder.
        if self.pending != 0.0 && self.pending.signum() != delta_y.signum() {
            self.pending = 0.0;
        }
        self.pending += delta_y;
        let steps = self.pending.trunc();
        self.pending -= steps;
        steps as i32
    }

    /// Drops a partial step (pointer left the control).
    pub fn reset(&mut self) {
        self.pending = 0.0;
    }
}

/// Logical pixels winit reports per wheel notch (`LineDelta * 60` in the Slint winit backend).
pub const SLINT_PIXELS_PER_WHEEL_NOTCH: f64 = 60.0;

/// `FlyoutSlider.CalculateCenteredTop`.
pub fn calculate_centered_top(container_height: f64, item_height: f64) -> f64 {
    (container_height - item_height) / 2.0
}

/// `FlyoutSlider.CalculatePeakWidth`: the meter bar never passes the volume extent; the `radius`
/// compensates the rounded cap so a full-scale peak meets the progress end.
pub fn calculate_peak_width(peak_extent: f64, peak: f32, radius: f64) -> f64 {
    let peak = f64::from(peak.clamp(0.0, 1.0));
    peak_extent.min((peak_extent + radius) * peak).max(0.0)
}

/// Normalized slider position for `value` in `[0, maximum]`.
pub fn normalized(value: f64) -> f64 {
    (value / SLIDER_MAXIMUM.max(0.001)).clamp(0.0, 1.0)
}

/// Jump-to-point value for a pointer at `x` over a slider `width` wide with a `thumb_width` thumb.
pub fn value_from_point(x: f64, width: f64, thumb_width: f64) -> f64 {
    let fraction = ((x - thumb_width / 2.0) / (width - thumb_width).max(1.0)).clamp(0.0, 1.0);
    fraction * SLIDER_MAXIMUM
}

/// A slider `Value` set: clamped, and ignored when it moves less than the epsilon. Returns the new
/// value when it changed.
pub fn coerce_value(current: f64, requested: f64) -> Option<f64> {
    let clamped = requested.clamp(0.0, SLIDER_MAXIMUM);
    ((clamped - current).abs() >= SLIDER_VALUE_EPSILON).then_some(clamped)
}

/// `ScalarText`: endpoint scalar as a whole percent. C# multiplies in `float` before
/// `Math.Round`, so the product is rounded to `f32` first (0.355 shows 36, like the .NET app).
pub fn scalar_text(scalar: f32) -> String {
    (f64::from(scalar * 100.0).round_ties_even() as i64).to_string()
}

/// `PercentText`: slider position as a whole percent (app rows show the position, not the level).
pub fn percent_text(position: f64) -> String {
    (position.round_ties_even() as i64).to_string()
}

/// `TryParseSliderPercent`: trims, strips one trailing `%`, parses with the user's decimal
/// separator first and the invariant one second, and rejects NaN and infinities.
pub fn try_parse_slider_percent(text: &str, culture_decimal_separator: &str) -> Option<f64> {
    let mut trimmed = text.trim();
    if let Some(stripped) = trimmed.strip_suffix('%') {
        trimmed = stripped.trim();
    }
    let parsed = parse_culture_float(trimmed, culture_decimal_separator).or_else(|| parse_invariant_float(trimmed));
    parsed.filter(|v| v.is_finite())
}

/// `ApplyPercentText`: the clamped value to set on the slider, and whether it differs enough from
/// `current` to write the model.
pub fn apply_percent_text(text: &str, current: f64, culture_decimal_separator: &str) -> Option<(f64, bool)> {
    let value = try_parse_slider_percent(text, culture_decimal_separator)?;
    let clamped = value.clamp(0.0, SLIDER_MAXIMUM);
    Some((clamped, (current - clamped).abs() > SLIDER_VALUE_EPSILON))
}

/// `double.TryParse(NumberStyles.Float, CurrentCulture)`: the culture's decimal separator only (no
/// group separators).
fn parse_culture_float(text: &str, decimal_separator: &str) -> Option<f64> {
    if decimal_separator.is_empty() || decimal_separator == "." {
        return parse_invariant_float(text);
    }
    if text.contains('.') {
        return None;
    }
    parse_invariant_float(&text.replacen(decimal_separator, ".", 1))
}

/// `double.TryParse(NumberStyles.Float, InvariantCulture)`: optional sign, digits with one `.`,
/// optional exponent. Rejects the textual forms Rust would also accept (`inf`, `NaN`).
fn parse_invariant_float(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || !bytes.iter().all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E')) {
        return None;
    }
    text.parse::<f64>().ok()
}

/// The user's decimal separator (`LOCALE_SDECIMAL` of the user default locale), "." when unknown.
pub fn user_decimal_separator() -> String {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SDECIMAL};
    let mut buffer = [0u16; 8];
    // SAFETY: `buffer` is a live, writable UTF-16 buffer and its length is passed alongside it; a
    // null locale name selects the user default locale.
    let written = unsafe { GetLocaleInfoEx(windows_core::PCWSTR::null(), LOCALE_SDECIMAL, Some(&mut buffer)) };
    let len = usize::try_from(written).unwrap_or(0).saturating_sub(1).min(buffer.len());
    let separator = String::from_utf16_lossy(&buffer[..len]);
    if separator.is_empty() { ".".to_owned() } else { separator }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_steps_accumulate_fractions_and_ignore_horizontal_input() {
        let mut steps = WheelStepAccumulator::default();
        assert_eq!(steps.add(0.0), 0);
        assert_eq!(steps.add(0.4), 0);
        assert_eq!(steps.add(0.4), 0);
        assert_eq!(steps.add(0.4), 1);
        assert_eq!(steps.add(-0.6), 0);
        assert_eq!(steps.add(-0.6), -1);
        assert_eq!(steps.add(3.0), 3);
        assert_eq!(steps.add(f64::NAN), 0);
        steps.add(0.5);
        steps.reset();
        assert_eq!(steps.add(0.5), 0);
    }

    #[test]
    fn centered_top_aligns_track_and_thumb() {
        for (container, track, thumb) in [(32.0, 4.0, 22.0), (31.0, 4.0, 18.0), (31.5, 3.5, 17.25)] {
            let track_center = calculate_centered_top(container, track) + track / 2.0;
            let thumb_center = calculate_centered_top(container, thumb) + thumb / 2.0;
            assert!((track_center - container / 2.0).abs() < 1e-10);
            assert!((track_center - thumb_center).abs() < 1e-10);
        }
    }

    #[test]
    fn peak_width_stays_within_volume_extent() {
        assert_eq!(calculate_peak_width(0.0, 1.0, 2.0), 0.0);
        assert_eq!(calculate_peak_width(100.0, 0.0, 2.0), 0.0);
        assert_eq!(calculate_peak_width(100.0, 0.5, 2.0), 51.0);
        assert_eq!(calculate_peak_width(100.0, 1.0, 2.0), 100.0);
        assert_eq!(calculate_peak_width(100.0, 1.1, 2.0), 100.0);
    }

    #[test]
    fn jump_to_point_maps_thumb_centre_range() {
        assert_eq!(value_from_point(5.0, 210.0, 10.0), 0.0);
        assert_eq!(value_from_point(0.0, 210.0, 10.0), 0.0);
        assert_eq!(value_from_point(105.0, 210.0, 10.0), 50.0);
        assert_eq!(value_from_point(205.0, 210.0, 10.0), 100.0);
        assert_eq!(value_from_point(400.0, 210.0, 10.0), 100.0);
        // A slider narrower than its thumb still divides by at least one pixel.
        assert_eq!(value_from_point(6.0, 10.0, 10.0), 100.0);
    }

    #[test]
    fn value_sets_clamp_and_ignore_tiny_moves() {
        assert_eq!(coerce_value(50.0, 50.0005), None);
        assert_eq!(coerce_value(50.0, 120.0), Some(100.0));
        assert_eq!(coerce_value(50.0, -3.0), Some(0.0));
        assert_eq!(coerce_value(100.0, 120.0), None);
    }

    #[test]
    fn percent_labels_round_half_to_even() {
        assert_eq!(scalar_text(0.345), "34");
        assert_eq!(scalar_text(0.355), "36");
        assert_eq!(percent_text(12.5), "12");
        assert_eq!(percent_text(13.5), "14");
        assert_eq!(percent_text(99.6), "100");
    }

    #[test]
    fn percent_parsing_follows_try_parse_slider_percent() {
        assert_eq!(try_parse_slider_percent(" 42 ", "."), Some(42.0));
        assert_eq!(try_parse_slider_percent("42%", "."), Some(42.0));
        assert_eq!(try_parse_slider_percent(" 42 % ", "."), Some(42.0));
        assert_eq!(try_parse_slider_percent("12.5", "."), Some(12.5));
        assert_eq!(try_parse_slider_percent("-5", "."), Some(-5.0));
        assert_eq!(try_parse_slider_percent("1e2", "."), Some(100.0));
        assert_eq!(try_parse_slider_percent(".5", "."), Some(0.5));
        assert_eq!(try_parse_slider_percent("42%%", "."), None);
        assert_eq!(try_parse_slider_percent("abc", "."), None);
        assert_eq!(try_parse_slider_percent("", "."), None);
        assert_eq!(try_parse_slider_percent("NaN", "."), None);
        assert_eq!(try_parse_slider_percent("Infinity", "."), None);
        assert_eq!(try_parse_slider_percent("1e400", "."), None);
        // en-US rejects a comma (no group separators in NumberStyles.Float).
        assert_eq!(try_parse_slider_percent("12,5", "."), None);
        // A comma culture accepts its own separator and still falls back to the invariant one.
        assert_eq!(try_parse_slider_percent("12,5", ","), Some(12.5));
        assert_eq!(try_parse_slider_percent("12.5", ","), Some(12.5));
    }

    #[test]
    fn percent_apply_clamps_and_reports_change() {
        assert_eq!(apply_percent_text("150", 40.0, "."), Some((100.0, true)));
        assert_eq!(apply_percent_text("-1", 0.0, "."), Some((0.0, false)));
        assert_eq!(apply_percent_text("40", 40.0, "."), Some((40.0, false)));
        assert_eq!(apply_percent_text("x", 40.0, "."), None);
    }
}
