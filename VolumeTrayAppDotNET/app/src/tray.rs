//! Pure tray logic: the volume glyph tiers and render input (`VolumeTrayIcon.cs`,
//! `GlyphCatalog.GetVolumeTier`), wheel steps (`TrayClickModifiers.Resolve` through
//! `Modifiers::resolve`), the volume adjustment of `App.AdjustTrackedDeviceVolume`, and the tray
//! menu entries (`VolumeTrayMenuEntries.cs`).

use tadn_common::theme::Argb;
use tadn_menu::MenuItem;
use tadn_shell::raster::GlyphLayer;
use tadn_shell::tray::Modifiers;
use vtadn_model::policy::ordering::flyout_device_order;
use vtadn_model::settings::{AppSettings, TrayMenuDeviceNameStyle, TrayWheelVolumeStep};
use vtadn_model::snapshot::{AudioSnapshot, DeviceId, DeviceSnapshot, RadioState};
use vtadn_model::text;

/// `VolumeTrayIcon.BackdropOpacityValue`.
pub const BACKDROP_OPACITY: f32 = 0.21;

/// `GlyphCatalog` playback volume glyphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VolumeGlyph {
    Mute,
    Silent,
    Low,
    Mid,
    High,
}

impl VolumeGlyph {
    pub const fn code_point(self) -> char {
        match self {
            Self::Mute => '\u{E74F}',
            Self::Silent => '\u{E992}',
            Self::Low => '\u{E993}',
            Self::Mid => '\u{E994}',
            Self::High => '\u{E995}',
        }
    }
}

/// `GlyphCatalog.GetVolumeTier`.
pub fn volume_tier(scalar: f32, muted: bool) -> VolumeGlyph {
    if muted {
        VolumeGlyph::Mute
    } else if scalar <= 0.001 {
        VolumeGlyph::Silent
    } else if scalar < 0.34 {
        VolumeGlyph::Low
    } else if scalar < 0.67 {
        VolumeGlyph::Mid
    } else {
        VolumeGlyph::High
    }
}

/// `TrayIconRenderInput` plus the icon size: equal inputs never re-render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrayRenderInput {
    pub foreground: VolumeGlyph,
    /// Faint full-volume glyph behind the tier (none when muted or already High).
    pub backdrop: bool,
    pub color: Argb,
    pub size_px: u32,
}

impl TrayRenderInput {
    /// No default device renders as muted (Volume 0, IsMuted true).
    pub fn new(device: Option<&DeviceSnapshot>, color: Argb, size_px: u32) -> Self {
        let (volume, muted) = device.map_or((0.0, true), |d| (d.volume.clamp(0.0, 1.0), d.muted));
        let foreground = volume_tier(volume, muted);
        let backdrop = !muted && foreground != VolumeGlyph::High;
        Self { foreground, backdrop, color, size_px }
    }

    pub fn layers(&self, font_family: &str) -> Vec<GlyphLayer> {
        let layer = |glyph: VolumeGlyph, opacity: f32| GlyphLayer {
            text: glyph.code_point().to_string(),
            font_family: font_family.to_owned(),
            color: self.color,
            opacity,
        };
        let mut layers = Vec::with_capacity(2);
        if self.backdrop {
            layers.push(layer(VolumeGlyph::High, BACKDROP_OPACITY));
        }
        layers.push(layer(self.foreground, 1.0));
        layers
    }
}

/// Wheel step in percent for the held modifiers; None when the resolved action is `Nothing`.
pub fn wheel_step_percent(settings: &AppSettings, modifiers: Modifiers) -> Option<i32> {
    let action = modifiers.resolve(
        settings.tray_ctrl_wheel_action,
        settings.tray_alt_wheel_action,
        settings.tray_wheel_action,
        TrayWheelVolumeStep::Nothing,
    );
    settings.wheel_step_percent(action)
}

/// `AdjustTrackedDeviceVolume` math: `clamp(cur·100 + steps·step, 0, 100)`, no snapping to the
/// step grid. Returns the new scalar, or None when nothing changes (`|cur − next| < 0.001`).
pub fn adjusted_volume(current: f32, steps: i32, step_percent: i32) -> Option<f32> {
    if steps == 0 {
        return None;
    }
    let current = f64::from(current) * 100.0;
    let next = (current + f64::from(steps) * f64::from(step_percent)).clamp(0.0, 100.0);
    ((current - next).abs() >= 0.001).then_some((next / 100.0) as f32)
}

/// Warm-slot eviction check when the idle timer fires: never an open menu, never while
/// `KeepTrayContextMenuWarm` is on.
pub fn should_evict_tray_menu(keep_warm: bool, is_open: bool) -> bool {
    !keep_warm && !is_open
}

/// What a tray menu row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayMenuAction {
    DeviceProperties(DeviceId),
    SoundDevices,
    Recording,
    Sounds,
    Communications,
    Bluetooth,
    Settings,
    Exit,
}

pub const TRAY_SOUND_DEVICES: &str = "Sound devices";
pub const TRAY_RECORDING: &str = "Recording";
pub const TRAY_SOUNDS: &str = "Sounds";
pub const TRAY_COMMUNICATIONS: &str = "Communications";
pub const TRAY_BLUETOOTH: &str = "Bluetooth";
pub const TRAY_MENU_SETTINGS: &str = "Settings";
pub const TRAY_MENU_EXIT: &str = "Exit";

/// `FormatTrayMenuDeviceName`: style per flow, empty slice → the friendly name untruncated, else
/// cut to `TrayMenuDeviceNameMaxLength` UTF-16 units ending in "..".
pub fn tray_menu_device_name(device: &DeviceSnapshot, settings: &AppSettings) -> String {
    let style = if device.is_capture() {
        settings.tray_menu_recording_device_name_style
    } else {
        settings.tray_menu_playback_device_name_style
    };
    let raw = match style {
        TrayMenuDeviceNameStyle::Name => device.description.as_str(),
        TrayMenuDeviceNameStyle::Model => device.interface_name.as_str(),
        TrayMenuDeviceNameStyle::NameAndModel => device.name.as_str(),
    };
    if raw.is_empty() {
        return device.name.clone();
    }
    let max = usize::try_from(settings.tray_menu_device_name_max_length.clamp(3, 200)).unwrap_or(32);
    text::truncate_with_dots(raw, max)
}

/// `VolumeTrayMenuEntries.Build` + the Settings/Exit rows of `TrayMenuWindow`. Row ids index the
/// returned actions; separators carry id -1.
pub fn build_tray_menu(snapshot: &AudioSnapshot, settings: &AppSettings) -> (Vec<MenuItem>, Vec<TrayMenuAction>) {
    let mut items = Vec::new();
    let mut actions = Vec::new();
    let mut add = |items: &mut Vec<MenuItem>, text: String, action: TrayMenuAction| {
        items.push(MenuItem::new(actions.len() as i32, text));
        actions.push(action);
    };

    if settings.show_tray_menu_device_links {
        let order = flyout_device_order(&snapshot.devices, settings, false, RadioState::On);
        if !order.is_empty() {
            for &index in order.iter().rev() {
                let device = &snapshot.devices[index];
                add(
                    &mut items,
                    tray_menu_device_name(device, settings),
                    TrayMenuAction::DeviceProperties(device.id.clone()),
                );
            }
            items.push(MenuItem::separator());
        }
    }
    add(&mut items, TRAY_SOUND_DEVICES.to_owned(), TrayMenuAction::SoundDevices);
    if settings.show_tray_menu_recording_link {
        add(&mut items, TRAY_RECORDING.to_owned(), TrayMenuAction::Recording);
    }
    if settings.show_tray_menu_sounds_link {
        add(&mut items, TRAY_SOUNDS.to_owned(), TrayMenuAction::Sounds);
    }
    if settings.show_tray_menu_communications_link {
        add(&mut items, TRAY_COMMUNICATIONS.to_owned(), TrayMenuAction::Communications);
    }
    add(&mut items, TRAY_BLUETOOTH.to_owned(), TrayMenuAction::Bluetooth);
    items.push(MenuItem::separator());
    add(&mut items, TRAY_MENU_SETTINGS.to_owned(), TrayMenuAction::Settings);
    items.push(MenuItem::separator());
    add(&mut items, TRAY_MENU_EXIT.to_owned(), TrayMenuAction::Exit);
    (items, actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use vtadn_model::snapshot::{DeviceState, Flow};

    fn device(name: &str, flow: Flow, volume: f32, muted: bool) -> DeviceSnapshot {
        DeviceSnapshot {
            id: Arc::from(format!("{{0.0.0.00000000}}.{name}")),
            flow,
            order: 0,
            state: DeviceState(DeviceState::ACTIVE),
            name: name.to_owned(),
            description: "Speakers".to_owned(),
            interface_name: "Realtek(R) Audio".to_owned(),
            enumerator: String::new(),
            is_default: true,
            is_default_comms: true,
            volume,
            muted,
            capture_sleeping: false,
            format: None,
            listen_enabled: false,
            listen_target: None,
            exclusive_allowed: true,
            exclusive_held: false,
            eq_apo: Default::default(),
            bt: None,
            groups: Vec::new(),
        }
    }

    #[test]
    fn glyph_tier_thresholds() {
        assert_eq!(volume_tier(0.5, true), VolumeGlyph::Mute);
        assert_eq!(volume_tier(0.0, false), VolumeGlyph::Silent);
        assert_eq!(volume_tier(0.001, false), VolumeGlyph::Silent);
        assert_eq!(volume_tier(0.0011, false), VolumeGlyph::Low);
        assert_eq!(volume_tier(0.3399, false), VolumeGlyph::Low);
        assert_eq!(volume_tier(0.34, false), VolumeGlyph::Mid);
        assert_eq!(volume_tier(0.6699, false), VolumeGlyph::Mid);
        assert_eq!(volume_tier(0.67, false), VolumeGlyph::High);
        assert_eq!(volume_tier(1.0, false), VolumeGlyph::High);
    }

    #[test]
    fn backdrop_only_below_high_and_unmuted() {
        let color = Argb(0xFFFF_FFFF);
        let mid = TrayRenderInput::new(Some(&device("A", Flow::Render, 0.5, false)), color, 16);
        assert!(mid.backdrop);
        assert_eq!(mid.layers("Segoe Fluent Icons").len(), 2);
        assert!((mid.layers("f")[0].opacity - 0.21).abs() < f32::EPSILON);
        let high = TrayRenderInput::new(Some(&device("A", Flow::Render, 0.9, false)), color, 16);
        assert!(!high.backdrop);
        let muted = TrayRenderInput::new(Some(&device("A", Flow::Render, 0.5, true)), color, 16);
        assert!(!muted.backdrop && muted.foreground == VolumeGlyph::Mute);
        let none = TrayRenderInput::new(None, color, 16);
        assert_eq!(none.foreground, VolumeGlyph::Mute);
        assert!(!none.backdrop);
        // Same tier, same input: no re-render.
        let a = TrayRenderInput::new(Some(&device("A", Flow::Render, 0.40, false)), color, 16);
        let b = TrayRenderInput::new(Some(&device("A", Flow::Render, 0.60, false)), color, 16);
        assert_eq!(a, b);
    }

    #[test]
    fn wheel_step_selection() {
        let mut settings = AppSettings::default();
        let ctrl = Modifiers { ctrl: true, ..Modifiers::default() };
        let alt = Modifiers { alt: true, ..Modifiers::default() };
        assert_eq!(wheel_step_percent(&settings, Modifiers::default()), Some(2));
        assert_eq!(wheel_step_percent(&settings, ctrl), Some(1));
        assert_eq!(wheel_step_percent(&settings, alt), Some(5));
        settings.tray_ctrl_wheel_action = TrayWheelVolumeStep::Nothing;
        assert_eq!(wheel_step_percent(&settings, ctrl), Some(2));
        settings.tray_wheel_action = TrayWheelVolumeStep::Nothing;
        assert_eq!(wheel_step_percent(&settings, Modifiers::default()), None);
    }

    #[test]
    fn volume_adjustment() {
        assert_eq!(adjusted_volume(0.5, 0, 2), None);
        assert!((adjusted_volume(0.5, 1, 2).unwrap() - 0.52).abs() < 1e-6);
        assert!((adjusted_volume(0.515, -1, 2).unwrap() - 0.495).abs() < 1e-6);
        assert_eq!(adjusted_volume(1.0, 3, 2), None);
        assert_eq!(adjusted_volume(0.0, -1, 2), None);
        assert_eq!(adjusted_volume(0.99, 5, 2), Some(1.0));
    }

    #[test]
    fn device_name_styles_and_truncation() {
        let mut settings = AppSettings::default();
        let mut d = device("Speakers (Realtek(R) Audio)", Flow::Render, 0.5, false);
        assert_eq!(tray_menu_device_name(&d, &settings), "Speakers (Realtek(R) Audio)");
        settings.tray_menu_device_name_max_length = 10;
        assert_eq!(tray_menu_device_name(&d, &settings), "Speakers..");
        settings.tray_menu_playback_device_name_style = TrayMenuDeviceNameStyle::Name;
        assert_eq!(tray_menu_device_name(&d, &settings), "Speakers");
        settings.tray_menu_playback_device_name_style = TrayMenuDeviceNameStyle::Model;
        assert_eq!(tray_menu_device_name(&d, &settings), "Realtek(..");
        d.interface_name.clear();
        // Empty slice: friendly name, untruncated.
        assert_eq!(tray_menu_device_name(&d, &settings), "Speakers (Realtek(R) Audio)");
        let mic = device("Mic", Flow::Capture, 0.5, false);
        settings.tray_menu_recording_device_name_style = TrayMenuDeviceNameStyle::Name;
        assert_eq!(tray_menu_device_name(&mic, &settings), "Speakers");
    }

    #[test]
    fn tray_menu_default_entries() {
        let settings = AppSettings::default();
        let (items, actions) = build_tray_menu(&AudioSnapshot::default(), &settings);
        let texts: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(texts, ["Sound devices", "Bluetooth", "", "Settings", "", "Exit"]);
        assert!(items[2].separator && items[4].separator);
        assert_eq!(
            actions,
            [TrayMenuAction::SoundDevices, TrayMenuAction::Bluetooth, TrayMenuAction::Settings, TrayMenuAction::Exit]
        );
        for item in items.iter().filter(|i| !i.separator) {
            assert!(item.enabled);
            assert!(actions.get(item.id as usize).is_some());
        }
    }

    #[test]
    fn tray_menu_optional_links_in_order() {
        let settings = AppSettings {
            show_tray_menu_recording_link: true,
            show_tray_menu_sounds_link: true,
            show_tray_menu_communications_link: true,
            ..AppSettings::default()
        };
        let (items, _) = build_tray_menu(&AudioSnapshot::default(), &settings);
        let texts: Vec<&str> = items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(
            texts,
            ["Sound devices", "Recording", "Sounds", "Communications", "Bluetooth", "", "Settings", "", "Exit"]
        );
    }

    #[test]
    fn tray_menu_device_links_reverse_flyout_order() {
        let settings = AppSettings { show_tray_menu_device_links: true, ..AppSettings::default() };
        let mut speakers = device("Speakers", Flow::Render, 0.5, false);
        speakers.order = 0;
        let mut headset = device("Headset", Flow::Render, 0.5, false);
        headset.order = 1;
        headset.is_default = false;
        headset.is_default_comms = false;
        let mut mic = device("Mic", Flow::Capture, 0.5, false);
        mic.order = 2;
        let snapshot = AudioSnapshot { devices: vec![speakers, headset, mic], ready: true, ..AudioSnapshot::default() };
        let order = flyout_device_order(&snapshot.devices, &settings, false, RadioState::On);
        assert_eq!(order.len(), 3);
        let (items, actions) = build_tray_menu(&snapshot, &settings);
        let expected: Vec<&str> = order.iter().rev().map(|&i| snapshot.devices[i].name.as_str()).collect();
        let texts: Vec<&str> = items.iter().take(3).map(|i| i.text.as_str()).collect();
        assert_eq!(texts, expected);
        assert!(items[3].separator);
        assert_eq!(items[4].text, "Sound devices");
        let first_id = snapshot.devices[*order.last().unwrap()].id.clone();
        assert_eq!(actions[0], TrayMenuAction::DeviceProperties(first_id));
    }

    #[test]
    fn tray_menu_eviction_rules() {
        assert!(should_evict_tray_menu(false, false));
        assert!(!should_evict_tray_menu(false, true));
        assert!(!should_evict_tray_menu(true, false));
        assert!(!should_evict_tray_menu(true, true));
    }
}
