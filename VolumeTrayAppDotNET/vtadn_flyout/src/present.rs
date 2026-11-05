//! Pure presentation rules of `VolumeFlyoutWindow`: glyph selection (with the `GlyphCatalog`
//! transforms), dimming, tooltips, the format line, and the click routing of the row and header
//! buttons. Strings are the English `Strings.resx` / `CommonStrings.resx` values.

use vtadn_model::settings::{
    AppSettings, BluetoothRadioButtonClickGesture, CaptureActivityIndicator, CommunicationsButtonVisibility,
};
use vtadn_model::snapshot::{AppGroupSnapshot, DeviceSnapshot, DuckingMode, EqApoState, RadioState, SessionState};

/// Opacity of inactive glyphs and dimmed rows (`Flyout.HeaderInactiveIconOpacity` and the 0.4
/// literals of the C# code).
pub const DIMMED_OPACITY: f32 = 0.4;

/// A `GlyphCatalog` entry: code point plus its render transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphSpec {
    pub text: &'static str,
    pub scale: f32,
    pub translate_x: f32,
    pub translate_y: f32,
    pub extra_bold: bool,
}

impl GlyphSpec {
    const fn plain(text: &'static str) -> Self {
        Self { text, scale: 1.0, translate_x: 0.0, translate_y: 0.0, extra_bold: false }
    }

    const fn shifted(text: &'static str, translate_y: f32) -> Self {
        Self { text, scale: 1.0, translate_x: 0.0, translate_y, extra_bold: false }
    }

    /// Microphone glyphs: ExtraBold, scaled 0.788, shifted one pixel left.
    const fn microphone(text: &'static str) -> Self {
        Self { text, scale: 0.788_461_54, translate_x: -1.0, translate_y: 0.0, extra_bold: true }
    }

    pub fn is_microphone(&self) -> bool {
        self.extra_bold
    }
}

/// Volume-specific `GlyphCatalog` entries (`Visuals/GlyphCatalog.axaml`) and the common ones the
/// flyout uses.
pub mod glyphs {
    use super::GlyphSpec;

    pub const PLAYBACK_VOLUME_MUTE: GlyphSpec = GlyphSpec::plain("\u{E74F}");
    pub const PLAYBACK_VOLUME_SILENT: GlyphSpec = GlyphSpec::plain("\u{E992}");
    pub const PLAYBACK_VOLUME_LOW: GlyphSpec = GlyphSpec::plain("\u{E993}");
    pub const PLAYBACK_VOLUME_MID: GlyphSpec = GlyphSpec::plain("\u{E994}");
    pub const PLAYBACK_VOLUME_HIGH: GlyphSpec = GlyphSpec::plain("\u{E995}");
    pub const MICROPHONE: GlyphSpec = GlyphSpec::microphone("\u{E720}");
    pub const MICROPHONE_OFF: GlyphSpec = GlyphSpec::microphone("\u{F781}");
    pub const MICROPHONE_SLEEP: GlyphSpec = GlyphSpec::microphone("\u{EC55}");
    pub const MICROPHONE_LISTENING: GlyphSpec = GlyphSpec::microphone("\u{F12E}");
    pub const LOCK: &str = "\u{E72E}";
    pub const UNLOCK: &str = "\u{E785}";
    pub const PLAYBACK_DEVICE_DEFAULT: GlyphSpec = GlyphSpec::shifted("\u{F137}", -1.0);
    pub const PLAYBACK_DEVICE_ENABLED: GlyphSpec = GlyphSpec::shifted("\u{F138}", -1.0);
    pub const PLAYBACK_DEVICE_DISABLED: GlyphSpec =
        GlyphSpec { text: "\u{F13D}", scale: 1.85, translate_x: 0.0, translate_y: -0.5, extra_bold: false };
    pub const PLAYBACK_DEVICE_DEFAULT_COMMS: GlyphSpec = GlyphSpec::plain("\u{E95B}");
    pub const CHEVRON_UP_SMALL: GlyphSpec = GlyphSpec::shifted("\u{E96D}", 1.0);
    pub const CHEVRON_DOWN_SMALL: GlyphSpec = GlyphSpec::shifted("\u{E96E}", -1.0);
    pub const BLUETOOTH: &str = "\u{E702}";
    pub const BLUETOOTH_AUDIO_WAITING: &str = "\u{EA91}";
    pub const CIRCLE: &str = "\u{E91F}";
    pub const VIEW: &str = "\u{E890}";
    pub const HIDE: &str = "\u{ED1A}";
    pub const UNDOCK: &str = "\u{E75B}";
    pub const REDOCK: &str = "\u{E75A}";
    /// `BT_BATTERY_0` .. `BT_BATTERY_10` (U+F5F2 + n).
    pub const BT_BATTERY: [&str; 11] = [
        "\u{F5F2}", "\u{F5F3}", "\u{F5F4}", "\u{F5F5}", "\u{F5F6}", "\u{F5F7}", "\u{F5F8}", "\u{F5F9}", "\u{F5FA}",
        "\u{F5FB}", "\u{F5FC}",
    ];
}

/// `GlyphCatalog.GetVolumeTier` for an unmuted level.
pub fn volume_tier_glyph(volume: f32) -> GlyphSpec {
    if volume <= 0.001 {
        glyphs::PLAYBACK_VOLUME_SILENT
    } else if volume < 0.34 {
        glyphs::PLAYBACK_VOLUME_LOW
    } else if volume < 0.67 {
        glyphs::PLAYBACK_VOLUME_MID
    } else {
        glyphs::PLAYBACK_VOLUME_HIGH
    }
}

/// `DeviceVolumeGlyph` for a given mute state (`muted = !device.muted` gives the hover preview).
pub fn device_volume_glyph(device: &DeviceSnapshot, muted: bool, settings: &AppSettings) -> GlyphSpec {
    if device.is_capture() {
        if muted {
            glyphs::MICROPHONE_OFF
        } else if device.listen_enabled {
            glyphs::MICROPHONE_LISTENING
        } else if device.capture_sleeping {
            glyphs::MICROPHONE_SLEEP
        } else {
            glyphs::MICROPHONE
        }
    } else if muted {
        glyphs::PLAYBACK_VOLUME_MUTE
    } else if settings.use_dynamic_playback_volume_glyph_in_flyout {
        volume_tier_glyph(device.volume)
    } else {
        glyphs::PLAYBACK_VOLUME_LOW
    }
}

/// `DeviceStateGlyph` (default-device button).
pub fn device_state_glyph(device: &DeviceSnapshot) -> GlyphSpec {
    if !device.state.is_active() {
        glyphs::PLAYBACK_DEVICE_DISABLED
    } else if device.is_default {
        glyphs::PLAYBACK_DEVICE_DEFAULT
    } else if device.is_default_comms {
        glyphs::PLAYBACK_DEVICE_DEFAULT_COMMS
    } else {
        glyphs::PLAYBACK_DEVICE_ENABLED
    }
}

/// `BatteryGlyph`: `clamp(round(level / 10), 0, 10)`.
pub fn battery_glyph(level: u8) -> &'static str {
    let index = (f64::from(level) / 10.0).round_ties_even().clamp(0.0, 10.0) as usize;
    glyphs::BT_BATTERY[index]
}

/// `ResolveBluetoothBatteryLevel`: the last known level while disconnected.
pub fn displayed_battery_level(is_disconnected: bool, current: Option<u8>, last_known: Option<u8>) -> Option<u8> {
    if is_disconnected { last_known } else { current }
}

/// The Bluetooth/battery button glyph.
pub fn bluetooth_button_glyph(device: &DeviceSnapshot) -> &'static str {
    let battery = bluetooth_displayed_battery(device);
    if device.bt_pending() {
        glyphs::BLUETOOTH
    } else if device.bt_audio_waiting() {
        glyphs::BLUETOOTH_AUDIO_WAITING
    } else if let Some(level) = battery {
        battery_glyph(level)
    } else {
        glyphs::BLUETOOTH
    }
}

pub fn bluetooth_displayed_battery(device: &DeviceSnapshot) -> Option<u8> {
    let bt = device.bt.as_ref()?;
    displayed_battery_level(device.state.is_disconnected(), bt.battery, bt.last_known_battery)
}

/// `BluetoothButtonTooltip`.
pub fn bluetooth_button_tooltip(device: &DeviceSnapshot) -> String {
    let battery = bluetooth_displayed_battery(device);
    if device.bt_pending() {
        return "Bluetooth\nConnection pending\nClick to retry".to_owned();
    }
    if device.bt_audio_waiting() {
        return "Bluetooth connected\nAudio waiting\nClick to retry".to_owned();
    }
    match (device.state.is_disconnected(), battery) {
        (true, Some(level)) => format!("Battery: {level}% (last known)\nClick to connect"),
        (true, None) => "Bluetooth\nClick to connect".to_owned(),
        (false, Some(level)) => format!("Battery: {level}%\nCtrl + click to disconnect"),
        (false, None) => "Bluetooth\nCtrl + click to disconnect".to_owned(),
    }
}

/// Bluetooth row button outcome (`BluetoothButtonAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BluetoothButtonAction {
    None,
    Connect,
    Retry,
    Disconnect,
}

/// Click modifiers (Ctrl/Alt/Shift) of a pointer or keyboard activation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Modifiers {
    pub const NONE: Self = Self { control: false, alt: false, shift: false };
    pub const CONTROL: Self = Self { control: true, alt: false, shift: false };
    pub const ALT: Self = Self { control: false, alt: true, shift: false };
    pub const SHIFT: Self = Self { control: false, alt: false, shift: true };
}

/// `ResolveBluetoothButtonAction`.
pub fn bluetooth_button_action(is_disconnected: bool, is_pending: bool, modifiers: Modifiers) -> BluetoothButtonAction {
    if is_pending {
        BluetoothButtonAction::Retry
    } else if is_disconnected {
        BluetoothButtonAction::Connect
    } else if modifiers.control {
        BluetoothButtonAction::Disconnect
    } else {
        BluetoothButtonAction::None
    }
}

/// Header Bluetooth button outcome (`BluetoothRadioButtonAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BluetoothRadioButtonAction {
    None,
    ToggleRadio,
    OpenQuickSettings,
}

/// `BluetoothRadioButtonClickGestureModifiers`.
pub fn gesture_modifiers(gesture: BluetoothRadioButtonClickGesture) -> Modifiers {
    match gesture {
        BluetoothRadioButtonClickGesture::LeftClick => Modifiers::NONE,
        BluetoothRadioButtonClickGesture::ControlLeftClick => Modifiers::CONTROL,
        BluetoothRadioButtonClickGesture::AltLeftClick => Modifiers::ALT,
        BluetoothRadioButtonClickGesture::ShiftLeftClick => Modifiers::SHIFT,
    }
}

/// `ResolveBluetoothRadioButtonAction`: exactly the power-gesture modifiers toggle the radio when a
/// toggle can start (and do nothing otherwise); any other modifiers open quick settings.
pub fn bluetooth_radio_button_action(
    power_gesture: BluetoothRadioButtonClickGesture,
    modifiers: Modifiers,
    can_toggle_radio: bool,
) -> BluetoothRadioButtonAction {
    if modifiers != gesture_modifiers(power_gesture) {
        BluetoothRadioButtonAction::OpenQuickSettings
    } else if can_toggle_radio {
        BluetoothRadioButtonAction::ToggleRadio
    } else {
        BluetoothRadioButtonAction::None
    }
}

/// `BluetoothQuickSettingsClickGesture`.
pub fn quick_settings_gesture(power_gesture: BluetoothRadioButtonClickGesture) -> BluetoothRadioButtonClickGesture {
    if power_gesture == BluetoothRadioButtonClickGesture::LeftClick {
        BluetoothRadioButtonClickGesture::ControlLeftClick
    } else {
        BluetoothRadioButtonClickGesture::LeftClick
    }
}

fn gesture_text(gesture: BluetoothRadioButtonClickGesture) -> &'static str {
    match gesture {
        BluetoothRadioButtonClickGesture::LeftClick => "Left-click",
        BluetoothRadioButtonClickGesture::ControlLeftClick => "Ctrl + left-click",
        BluetoothRadioButtonClickGesture::AltLeftClick => "Alt + left-click",
        BluetoothRadioButtonClickGesture::ShiftLeftClick => "Shift + left-click",
    }
}

/// `BluetoothRadioTooltip`.
pub fn bluetooth_radio_tooltip(state: RadioState, power_gesture: BluetoothRadioButtonClickGesture) -> String {
    let power = gesture_text(power_gesture);
    let quick = gesture_text(quick_settings_gesture(power_gesture));
    match state {
        RadioState::On => format!("Bluetooth on\n{power} to turn off\n{quick} to open Bluetooth quick settings"),
        RadioState::Off => format!("Bluetooth off\n{power} to turn on\n{quick} to open Bluetooth quick settings"),
        RadioState::Unavailable => format!("Bluetooth unavailable\n{quick} to open Bluetooth quick settings"),
        RadioState::Unknown => format!("Bluetooth\nReading radio state\n{quick} to open Bluetooth quick settings"),
    }
}

/// `ToggleCommunicationsDucking`: Alt → reduce 50%, Ctrl → reduce 80%, else toggle mute-others.
pub fn communications_ducking_mode(modifiers: Modifiers, current: DuckingMode) -> DuckingMode {
    if modifiers.alt {
        DuckingMode::Reduce50
    } else if modifiers.control {
        DuckingMode::Reduce80
    } else if current.is_active() {
        DuckingMode::DoNothing
    } else {
        DuckingMode::MuteAll
    }
}

/// `ShouldShowCommunicationsButton`.
pub fn show_communications_button(settings: &AppSettings, ducking: DuckingMode) -> bool {
    match settings.flyout_communications_button_visibility {
        CommunicationsButtonVisibility::Hidden => false,
        CommunicationsButtonVisibility::WhenDuckingOn => ducking.is_active(),
        CommunicationsButtonVisibility::AlwaysShow => true,
    }
}

/// `ShowsDisabledDevices` (the eye button state).
pub fn shows_disabled_devices(settings: &AppSettings) -> bool {
    settings.show_disabled_playback_devices
        && (!settings.show_recording_devices
            || !settings.show_recording_devices_in_flyout
            || settings.show_disabled_recording_devices)
}

/// Default-device button action for a click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultButtonAction {
    SetDefaultComms,
    SetEnabled(bool),
    SetDefault,
}

pub fn default_button_action(modifiers: Modifiers, is_active: bool) -> DefaultButtonAction {
    if modifiers.shift {
        DefaultButtonAction::SetDefaultComms
    } else if modifiers.control {
        DefaultButtonAction::SetEnabled(!is_active)
    } else {
        DefaultButtonAction::SetDefault
    }
}

/// `BuildDeviceFormatLine`: non-empty segments joined with ", ".
pub fn build_format_line(format: &str, codec: &str, connection_status: &str) -> String {
    [format, codec, connection_status].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(", ")
}

/// `DeviceFormatLine`.
pub fn device_format_line(device: &DeviceSnapshot, settings: &AppSettings) -> String {
    let format = match (settings.show_device_format_text, device.format.as_ref()) {
        (true, Some(format)) => vtadn_model::text::format_text(format),
        _ => String::new(),
    };
    let codec = match (settings.show_device_codec_text, device.bt.as_ref()) {
        (true, Some(bt)) => bt.codec.clone().unwrap_or_default(),
        _ => String::new(),
    };
    let status = if device.bt_pending() {
        let seconds = device.bt.as_ref().map_or(0, |b| b.seconds_remaining);
        format!("Connection pending: {seconds}s")
    } else if device.bt_audio_waiting() {
        "Connected - audio waiting".to_owned()
    } else {
        String::new()
    };
    build_format_line(&format, &codec, &status)
}

/// Device slider, percent and mute button dimming (`IsMuted || !IsActive`).
pub fn device_dimmed(device: &DeviceSnapshot) -> bool {
    device.muted || !device.state.is_active()
}

/// `ResolveAppOpacity` as a dim flag.
pub fn app_dimmed(device: &DeviceSnapshot, group: &AppGroupSnapshot, settings: &AppSettings) -> bool {
    if group.muted || device.muted || !device.state.is_active() {
        return true;
    }
    device.is_capture()
        && settings.capture_activity_indicator == CaptureActivityIndicator::DimInactive
        && group.state != SessionState::Active
}

/// Grid cell activity badge (`LOCK` when the device is held exclusively, else `CIRCLE`), or "".
pub fn app_activity_badge(device: &DeviceSnapshot, group: &AppGroupSnapshot, settings: &AppSettings) -> &'static str {
    if device.is_capture()
        && settings.capture_activity_indicator == CaptureActivityIndicator::ActiveGlyph
        && group.state == SessionState::Active
    {
        if device.exclusive_held { glyphs::LOCK } else { glyphs::CIRCLE }
    } else {
        ""
    }
}

/// `TooltipText`: "{DisplayName}\nPID: {pid}", or "Unknown" while unresolved.
pub fn app_tooltip(group: &AppGroupSnapshot) -> String {
    match &group.display_name {
        Some(name) => vtadn_model::text::app_tooltip(name, group.pid),
        None => vtadn_model::text::UNKNOWN_APP_NAME.to_owned(),
    }
}

pub fn exclusive_glyph(device: &DeviceSnapshot) -> &'static str {
    if device.exclusive_allowed && device.exclusive_held { glyphs::LOCK } else { glyphs::UNLOCK }
}

pub fn exclusive_tooltip(device: &DeviceSnapshot) -> &'static str {
    match (device.exclusive_allowed, device.exclusive_held) {
        (true, true) => {
            "An app is currently holding this device in exclusive mode. Click to disallow exclusive control."
        }
        (true, false) => {
            "Exclusive mode is allowed. Click to disallow apps from taking exclusive control of this device."
        }
        (false, _) => "Exclusive mode is disallowed. Click to allow apps to take exclusive control of this device.",
    }
}

pub fn equalizer_tooltip(state: EqApoState) -> &'static str {
    match state {
        EqApoState::Running => {
            "Equalizer APO is running on this device. Click to uninstall. Ctrl + click or right-click: open the Configuration Editor."
        }
        EqApoState::EnhancementsOff => {
            "Equalizer APO is installed but enhancements are disabled for this device. Click to enable. Ctrl + click or right-click: open the Configuration Editor."
        }
        EqApoState::NotInstalled => {
            "Equalizer APO is not installed on this device. Click to install. Ctrl + click or right-click: open the Configuration Editor."
        }
        EqApoState::NotAvailable => "Equalizer APO is not detected on this system. Click for installation options.",
    }
}

/// Equalizer row button outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EqualizerButtonAction {
    OpenEditor,
    ShowUnavailableDialog,
    Toggle,
}

pub fn equalizer_button_action(modifiers: Modifiers, state: EqApoState) -> EqualizerButtonAction {
    if modifiers.control {
        EqualizerButtonAction::OpenEditor
    } else if state == EqApoState::NotAvailable {
        EqualizerButtonAction::ShowUnavailableDialog
    } else {
        EqualizerButtonAction::Toggle
    }
}

pub const SETTINGS_TOOLTIP: &str = "Settings";
pub const SOUND_SETTINGS_TOOLTIP: &str = "Open Windows sound settings";
pub const DISABLED_DEVICES_TOOLTIP: &str = "Show/hide disabled devices";
pub const COMMUNICATIONS_TOOLTIP: &str = "Communications";
pub const UNDOCK_TOOLTIP: &str = "Undock";
pub const REDOCK_TOOLTIP: &str = "Redock";
pub const MUTE_TOOLTIP: &str = "Mute";
pub const UNMUTE_TOOLTIP: &str = "Unmute";
pub const DEVICE_STATE_TOOLTIP: &str =
    "Click: set as default. Ctrl + click: enable / disable. Shift + click: set as default communications.";
pub const LISTEN_TOOLTIP: &str = "Click: toggle Listen to this device. Ctrl + click: enable with Default Playback target. Right-click: pick target device.";
pub const LISTEN_MENU_DEFAULT_PLAYBACK: &str = "Default playback device";
pub const NO_AUDIO_DEVICES: &str = "No audio devices";
pub const EQ_UNAVAILABLE_TITLE: &str = "Equalizer APO not detected";
pub const EQ_UNAVAILABLE_BODY: &str = "Equalizer APO was not found on this system. Set its install location in the app settings, or download the latest x64 installer.";
pub const EQ_UNAVAILABLE_DOWNLOAD: &str = "Download installer";
pub const CANCEL: &str = "Cancel";

/// `BuildFallbackFormatMenu`: the current channel count and bit depth at the standard rates plus the
/// current format itself, sorted as (channels, bits, rate) tuples.
pub fn fallback_formats(current: vtadn_model::snapshot::Format) -> Vec<vtadn_model::snapshot::Format> {
    const RATES: [u32; 11] = [8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000];
    let mut formats: std::collections::BTreeSet<_> = RATES
        .iter()
        .map(|&rate| vtadn_model::snapshot::Format { channels: current.channels, bits: current.bits, rate })
        .collect();
    formats.insert(current);
    formats.into_iter().collect()
}

/// Countdown pie: remaining fraction of the reconnect window (`ResolveRemainingFraction`).
pub fn countdown_fraction(deadline_ms: u64, now_ms: u64, timeout_ms: u64) -> f64 {
    if timeout_ms == 0 {
        return 0.0;
    }
    let remaining = deadline_ms as f64 - now_ms as f64;
    (remaining / timeout_ms as f64).clamp(0.0, 1.0)
}

/// `ResolvePieStep`: whole degrees, any sliver left still draws.
pub fn pie_step(fraction: f64) -> i32 {
    (fraction.clamp(0.0, 1.0) * f64::from(crate::metrics::PIE_STEPS)).ceil() as i32
}

#[cfg(test)]
mod tests {
    use vtadn_model::snapshot::{BtState, DeviceState, Flow, Format};

    use super::*;

    fn device(flow: Flow) -> DeviceSnapshot {
        DeviceSnapshot {
            id: "dev".into(),
            flow,
            order: 0,
            state: DeviceState(DeviceState::ACTIVE),
            name: "Speakers".into(),
            description: String::new(),
            interface_name: String::new(),
            enumerator: String::new(),
            is_default: false,
            is_default_comms: false,
            volume: 0.5,
            muted: false,
            capture_sleeping: false,
            format: Some(Format { channels: 2, bits: 24, rate: 48000 }),
            listen_enabled: false,
            listen_target: None,
            exclusive_allowed: true,
            exclusive_held: false,
            eq_apo: EqApoState::NotAvailable,
            bt: None,
            groups: Vec::new(),
        }
    }

    #[test]
    fn battery_level_uses_current_only_while_connected() {
        assert_eq!(displayed_battery_level(false, Some(64), Some(23)), Some(64));
        assert_eq!(displayed_battery_level(false, None, Some(23)), None);
        assert_eq!(displayed_battery_level(true, None, Some(23)), Some(23));
        assert_eq!(displayed_battery_level(true, None, None), None);
        assert_eq!(displayed_battery_level(true, Some(64), Some(23)), Some(23));
    }

    #[test]
    fn battery_glyph_rounds_half_even() {
        assert_eq!(battery_glyph(0), "\u{F5F2}");
        assert_eq!(battery_glyph(25), "\u{F5F4}");
        assert_eq!(battery_glyph(35), "\u{F5F6}");
        assert_eq!(battery_glyph(100), "\u{F5FC}");
    }

    #[test]
    fn bluetooth_button_routes_pending_clicks_to_retry() {
        assert_eq!(bluetooth_button_action(true, true, Modifiers::NONE), BluetoothButtonAction::Retry);
        assert_eq!(bluetooth_button_action(true, false, Modifiers::NONE), BluetoothButtonAction::Connect);
        assert_eq!(bluetooth_button_action(false, false, Modifiers::CONTROL), BluetoothButtonAction::Disconnect);
        assert_eq!(bluetooth_button_action(false, false, Modifiers::NONE), BluetoothButtonAction::None);
    }

    #[test]
    fn radio_button_toggles_only_on_the_exact_gesture() {
        use BluetoothRadioButtonAction::*;
        use BluetoothRadioButtonClickGesture as G;
        let all = [Modifiers::NONE, Modifiers::CONTROL, Modifiers::ALT, Modifiers::SHIFT];
        for (gesture, matching) in
            [(G::LeftClick, 0), (G::ControlLeftClick, 1), (G::AltLeftClick, 2), (G::ShiftLeftClick, 3)]
        {
            for (index, modifiers) in all.iter().enumerate() {
                let expected = if index == matching { ToggleRadio } else { OpenQuickSettings };
                assert_eq!(bluetooth_radio_button_action(gesture, *modifiers, true), expected);
            }
            assert_eq!(bluetooth_radio_button_action(gesture, all[matching], false), None);
        }
        let ctrl_shift = Modifiers { control: true, shift: true, alt: false };
        assert_eq!(bluetooth_radio_button_action(G::ControlLeftClick, ctrl_shift, true), OpenQuickSettings);
    }

    #[test]
    fn radio_tooltips_name_both_gestures() {
        use BluetoothRadioButtonClickGesture as G;
        assert_eq!(
            bluetooth_radio_tooltip(RadioState::On, G::ControlLeftClick),
            "Bluetooth on\nCtrl + left-click to turn off\nLeft-click to open Bluetooth quick settings"
        );
        assert_eq!(
            bluetooth_radio_tooltip(RadioState::Off, G::LeftClick),
            "Bluetooth off\nLeft-click to turn on\nCtrl + left-click to open Bluetooth quick settings"
        );
        assert_eq!(
            bluetooth_radio_tooltip(RadioState::Unknown, G::AltLeftClick),
            "Bluetooth\nReading radio state\nLeft-click to open Bluetooth quick settings"
        );
    }

    #[test]
    fn ducking_modifiers() {
        assert_eq!(communications_ducking_mode(Modifiers::ALT, DuckingMode::DoNothing), DuckingMode::Reduce50);
        assert_eq!(communications_ducking_mode(Modifiers::CONTROL, DuckingMode::DoNothing), DuckingMode::Reduce80);
        let both = Modifiers { control: true, alt: true, shift: false };
        assert_eq!(communications_ducking_mode(both, DuckingMode::MuteAll), DuckingMode::Reduce50);
        assert_eq!(communications_ducking_mode(Modifiers::NONE, DuckingMode::Reduce80), DuckingMode::DoNothing);
        assert_eq!(communications_ducking_mode(Modifiers::NONE, DuckingMode::DoNothing), DuckingMode::MuteAll);
    }

    #[test]
    fn format_line_joins_non_empty_segments() {
        assert_eq!(
            build_format_line("2 channel, 24 bit, 96000 Hz", "LDAC", "Connection Pending: 18s"),
            "2 channel, 24 bit, 96000 Hz, LDAC, Connection Pending: 18s"
        );
        assert_eq!(
            build_format_line("2 channel, 24 bit, 96000 Hz", "", "Connected - Audio Waiting"),
            "2 channel, 24 bit, 96000 Hz, Connected - Audio Waiting"
        );
        assert_eq!(build_format_line("", "", "Connected - Audio Waiting"), "Connected - Audio Waiting");
        assert_eq!(build_format_line("", "", ""), "");

        let settings = AppSettings::default();
        let mut d = device(Flow::Render);
        assert_eq!(device_format_line(&d, &settings), "2 channel, 24 bit, 48000 Hz");
        d.bt = Some(BtState {
            pending_deadline_ms: Some(10),
            seconds_remaining: 18,
            codec: Some("LDAC".into()),
            ..BtState::default()
        });
        assert_eq!(device_format_line(&d, &settings), "2 channel, 24 bit, 48000 Hz, Connection pending: 18s");
        let mut with_codec = settings.clone();
        with_codec.show_device_codec_text = true;
        with_codec.show_device_format_text = false;
        assert_eq!(device_format_line(&d, &with_codec), "LDAC, Connection pending: 18s");
    }

    #[test]
    fn device_glyphs() {
        let mut settings = AppSettings::default();
        let mut d = device(Flow::Render);
        assert_eq!(device_volume_glyph(&d, false, &settings), glyphs::PLAYBACK_VOLUME_LOW);
        assert_eq!(device_volume_glyph(&d, true, &settings), glyphs::PLAYBACK_VOLUME_MUTE);
        settings.use_dynamic_playback_volume_glyph_in_flyout = true;
        assert_eq!(device_volume_glyph(&d, false, &settings), glyphs::PLAYBACK_VOLUME_MID);
        d.volume = 0.0;
        assert_eq!(device_volume_glyph(&d, false, &settings), glyphs::PLAYBACK_VOLUME_SILENT);
        d.volume = 0.9;
        assert_eq!(device_volume_glyph(&d, false, &settings), glyphs::PLAYBACK_VOLUME_HIGH);

        let mut mic = device(Flow::Capture);
        assert_eq!(device_volume_glyph(&mic, false, &settings), glyphs::MICROPHONE);
        mic.capture_sleeping = true;
        assert_eq!(device_volume_glyph(&mic, false, &settings), glyphs::MICROPHONE_SLEEP);
        mic.listen_enabled = true;
        assert_eq!(device_volume_glyph(&mic, false, &settings), glyphs::MICROPHONE_LISTENING);
        assert_eq!(device_volume_glyph(&mic, true, &settings), glyphs::MICROPHONE_OFF);
        assert!(glyphs::MICROPHONE.is_microphone());

        assert_eq!(device_state_glyph(&d), glyphs::PLAYBACK_DEVICE_ENABLED);
        d.is_default_comms = true;
        assert_eq!(device_state_glyph(&d), glyphs::PLAYBACK_DEVICE_DEFAULT_COMMS);
        d.is_default = true;
        assert_eq!(device_state_glyph(&d), glyphs::PLAYBACK_DEVICE_DEFAULT);
        d.state = DeviceState(DeviceState::DISABLED);
        assert_eq!(device_state_glyph(&d), glyphs::PLAYBACK_DEVICE_DISABLED);
    }

    #[test]
    fn disabled_devices_toggle_state() {
        let mut settings = AppSettings::default();
        assert!(!shows_disabled_devices(&settings));
        settings.show_disabled_playback_devices = true;
        assert!(!shows_disabled_devices(&settings));
        settings.show_disabled_recording_devices = true;
        assert!(shows_disabled_devices(&settings));
        settings.show_disabled_recording_devices = false;
        settings.show_recording_devices_in_flyout = false;
        assert!(shows_disabled_devices(&settings));
    }

    #[test]
    fn fallback_formats_add_standard_rates_and_keep_the_current_one() {
        let current = Format { channels: 2, bits: 24, rate: 44056 };
        let formats = fallback_formats(current);
        assert_eq!(formats.len(), 12);
        assert_eq!(formats[0], Format { channels: 2, bits: 24, rate: 8000 });
        assert!(formats.contains(&current));
        assert!(formats.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(fallback_formats(Format { channels: 1, bits: 16, rate: 48000 }).len(), 11);
    }

    #[test]
    fn countdown_fraction_and_pie_steps() {
        assert_eq!(countdown_fraction(30_000, 0, 30_000), 1.0);
        assert_eq!(countdown_fraction(30_000, 15_000, 30_000), 0.5);
        assert_eq!(countdown_fraction(30_000, 30_000, 30_000), 0.0);
        assert_eq!(countdown_fraction(30_000, 40_000, 30_000), 0.0);
        assert_eq!(countdown_fraction(40_000, 0, 30_000), 1.0);
        assert_eq!(pie_step(1.0), 360);
        assert_eq!(pie_step(0.5), 180);
        assert_eq!(pie_step(0.5001), 181);
        assert_eq!(pie_step(0.0001), 1);
        assert_eq!(pie_step(0.0), 0);
    }

    #[test]
    fn app_dimming_and_badges() {
        let settings = AppSettings::default();
        let mic = device(Flow::Capture);
        let mut group = AppGroupSnapshot {
            app_id: "a".into(),
            display_name: Some("App".into()),
            icon: None,
            pid: 42,
            is_system_sounds: false,
            state: SessionState::Active,
            volume: 1.0,
            muted: false,
            session_count: 1,
            is_exclusive: false,
        };
        assert!(!app_dimmed(&mic, &group, &settings));
        assert_eq!(app_activity_badge(&mic, &group, &settings), glyphs::CIRCLE);
        assert_eq!(app_tooltip(&group), "App\nPID: 42");
        group.state = SessionState::Inactive;
        assert_eq!(app_activity_badge(&mic, &group, &settings), "");
        let mut dim = settings.clone();
        dim.capture_activity_indicator = CaptureActivityIndicator::DimInactive;
        assert!(app_dimmed(&mic, &group, &dim));
        group.display_name = None;
        assert_eq!(app_tooltip(&group), "Unknown");
    }
}
