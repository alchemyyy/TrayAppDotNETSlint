//! User-visible strings and number formatting shared by the flyout, tray and settings window
//! (from `Localization/Strings.resx`, `Audio/AudioLocalization.cs`, `App.RequestTrayRefresh`).
//!
//! .NET `Math.Round` rounds half to even; every percent shown to the user goes through
//! [`percent`] so the numbers match the C# app exactly.

use crate::snapshot::{DeviceSnapshot, Format};

pub const UNKNOWN_APP_NAME: &str = "Unknown";
pub const SYSTEM_SOUNDS_NAME: &str = "System Sounds";
pub const UNKNOWN_DEVICE_NAME: &str = "Unknown device";
pub const NO_AUDIO_DEVICE: &str = "No audio device";

/// `round_half_even(scalar * 100)` as an integer percent (device volume label, tray tooltip).
pub fn percent(scalar: f32) -> i32 {
    (f64::from(scalar) * 100.0).round_ties_even() as i32
}

/// "2 channel, 24 bit, 48000 Hz".
pub fn format_text(format: &Format) -> String {
    format!("{} channel, {} bit, {} Hz", format.channels, format.bits, format.rate)
}

/// Tray tooltip: "Speakers: 42%", "Speakers: muted", or "No audio device".
pub fn tray_tooltip(device: Option<&DeviceSnapshot>) -> String {
    match device {
        None => NO_AUDIO_DEVICE.to_owned(),
        Some(d) if d.muted => format!("{}: muted", d.name),
        Some(d) => format!("{}: {}%", d.name, percent(d.volume)),
    }
}

/// App icon tooltip: "{name}\nPID: {pid}".
pub fn app_tooltip(name: &str, pid: u32) -> String {
    format!("{name}\nPID: {pid}")
}

/// Battery level text, e.g. "80%".
pub fn battery_text(level: u8) -> String {
    format!("{level}%")
}

/// Truncates to `max_len` UTF-16 code units, ending in ".." when cut (tray menu device names).
pub fn truncate_with_dots(text: &str, max_len: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= max_len {
        return text.to_owned();
    }
    let keep = max_len.saturating_sub(2);
    let mut end = keep;
    // Do not split a surrogate pair.
    if end > 0 && (0xD800..0xDC00).contains(&units[end - 1]) {
        end -= 1;
    }
    let mut out = String::from_utf16_lossy(&units[..end]);
    out.push_str("..");
    out
}
