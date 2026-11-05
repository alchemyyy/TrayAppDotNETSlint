//! `DeviceVisibility.IsVisible`.

use crate::settings::AppSettings;
use crate::snapshot::{DeviceSnapshot, Flow};

/// Settings > Devices visibility switches for one device (precedence: Active, Disabled,
/// NotPresent, Unplugged).
///
/// - The recording parent toggle gates every recording-flow child setting.
/// - "Show disabled" is the broad gate; "show default even if disabled" matters only while it is off.
/// - NotPresent registry ghosts have their own cross-flow switch; orphaned Bluetooth endpoints are
///   never listed.
/// - Unplugged endpoints follow the per-flow "show disconnected" switch.
pub fn is_device_visible(device: &DeviceSnapshot, settings: &AppSettings) -> bool {
    let is_render = device.flow == Flow::Render;
    if !is_render && !settings.show_recording_devices {
        return false;
    }
    if device.state.is_active() {
        return true;
    }
    if device.state.is_disabled() {
        return is_disabled_visible(device, settings, is_render);
    }
    if device.state.is_not_present() {
        return !device.bt_orphaned() && settings.show_not_present_devices;
    }
    if is_render { settings.show_disconnected_playback_devices } else { settings.show_disconnected_recording_devices }
}

fn is_disabled_visible(device: &DeviceSnapshot, settings: &AppSettings, is_render: bool) -> bool {
    if is_render {
        settings.show_disabled_playback_devices
            || (device.is_default && settings.show_default_playback_device_even_if_disabled)
            || (device.is_default_comms && settings.show_default_comms_playback_device_even_if_disabled)
    } else {
        settings.show_disabled_recording_devices
            || (device.is_default && settings.show_default_recording_device_even_if_disabled)
            || (device.is_default_comms && settings.show_default_comms_recording_device_even_if_disabled)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::snapshot::{BtState, DeviceState, EqApoState};

    pub(crate) fn device(name: &str, flow: Flow, state: u32) -> DeviceSnapshot {
        DeviceSnapshot {
            id: name.into(),
            flow,
            order: 0,
            state: DeviceState(state),
            name: name.to_owned(),
            description: String::new(),
            interface_name: String::new(),
            enumerator: String::new(),
            is_default: false,
            is_default_comms: false,
            volume: 0.5,
            muted: false,
            capture_sleeping: false,
            format: None,
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
    fn active_devices_always_show_and_recording_parent_gates_capture() {
        let s = AppSettings::default();
        assert!(is_device_visible(&device("a", Flow::Render, DeviceState::ACTIVE), &s));
        assert!(is_device_visible(&device("b", Flow::Capture, DeviceState::ACTIVE), &s));
        let off = AppSettings { show_recording_devices: false, ..AppSettings::default() };
        assert!(!is_device_visible(&device("b", Flow::Capture, DeviceState::ACTIVE), &off));
    }

    #[test]
    fn disabled_default_shows_by_default_and_plain_disabled_does_not() {
        let s = AppSettings::default();
        let mut d = device("a", Flow::Render, DeviceState::DISABLED);
        assert!(!is_device_visible(&d, &s));
        d.is_default = true;
        assert!(is_device_visible(&d, &s));
        let mut c = device("c", Flow::Capture, DeviceState::DISABLED);
        c.is_default_comms = true;
        assert!(is_device_visible(&c, &s));
        let hide =
            AppSettings { show_default_comms_recording_device_even_if_disabled: false, ..AppSettings::default() };
        assert!(!is_device_visible(&c, &hide));
    }

    #[test]
    fn not_present_and_unplugged_follow_their_switches() {
        let s = AppSettings::default();
        let ghost = device("g", Flow::Render, DeviceState::NOT_PRESENT);
        let unplugged = device("u", Flow::Capture, DeviceState::UNPLUGGED);
        assert!(!is_device_visible(&ghost, &s));
        assert!(!is_device_visible(&unplugged, &s));
        let show = AppSettings {
            show_not_present_devices: true,
            show_disconnected_recording_devices: true,
            ..AppSettings::default()
        };
        assert!(is_device_visible(&ghost, &show));
        assert!(is_device_visible(&unplugged, &show));
        let mut orphan = ghost.clone();
        orphan.bt = Some(BtState::default());
        assert!(!is_device_visible(&orphan, &show));
    }
}
