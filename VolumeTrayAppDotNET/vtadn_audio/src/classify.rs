//! Rebuild-class vs in-place classification of snapshot changes (the C# flyout's
//! `QueueRebuild` triggers vs in-place property updates, AudioModelMap §4.6; see the
//! `vtadn_model::snapshot` module docs).

use vtadn_model::snapshot::{AppGroupSnapshot, AudioSnapshot, BtState, DeviceSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    /// Only in-place values (volume, device mute, capture sleeping, listen, exclusive, EAPO,
    /// BT countdown seconds, group volume/session count, EAPO availability).
    Values,
    /// Needs a flyout rebuild.
    Structure,
}

fn bt_structure_eq(a: &BtState, b: &BtState) -> bool {
    // `seconds_remaining` is the only in-place Bluetooth value.
    a.container_id == b.container_id
        && a.connected == b.connected
        && a.radio_off_pairing == b.radio_off_pairing
        && a.pending_deadline_ms == b.pending_deadline_ms
        && a.battery == b.battery
        && a.last_known_battery == b.last_known_battery
        && a.codec == b.codec
}

fn group_structure_eq(a: &AppGroupSnapshot, b: &AppGroupSnapshot) -> bool {
    // Volume (and the untracked session count) update in place.
    a.app_id == b.app_id
        && a.display_name == b.display_name
        && a.icon == b.icon
        && a.pid == b.pid
        && a.is_system_sounds == b.is_system_sounds
        && a.state == b.state
        && a.muted == b.muted
        && a.is_exclusive == b.is_exclusive
}

fn device_structure_eq(a: &DeviceSnapshot, b: &DeviceSnapshot) -> bool {
    a.id == b.id
        && a.flow == b.flow
        && a.order == b.order
        && a.state == b.state
        && a.name == b.name
        && a.description == b.description
        && a.interface_name == b.interface_name
        && a.enumerator == b.enumerator
        && a.is_default == b.is_default
        && a.is_default_comms == b.is_default_comms
        && a.format == b.format
        && match (&a.bt, &b.bt) {
            (Some(x), Some(y)) => bt_structure_eq(x, y),
            (None, None) => true,
            _ => false,
        }
        && a.groups.len() == b.groups.len()
        && a.groups.iter().zip(&b.groups).all(|(x, y)| group_structure_eq(x, y))
}

/// Compares two snapshots ignoring `rev` / `structure_rev`.
pub fn classify(prev: &AudioSnapshot, next: &AudioSnapshot) -> Change {
    let structure_same = prev.ready == next.ready
        && prev.defaults == next.defaults
        && prev.radio == next.radio
        && prev.ducking == next.ducking
        && prev.is_elevated == next.is_elevated
        && prev.devices.len() == next.devices.len()
        && prev.devices.iter().zip(&next.devices).all(|(a, b)| device_structure_eq(a, b));
    if !structure_same {
        return Change::Structure;
    }
    let values_same = prev.eq_apo_available == next.eq_apo_available && prev.devices == next.devices;
    if values_same { Change::None } else { Change::Values }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use vtadn_model::snapshot::{
        Defaults, DeviceState, DuckingMode, EqApoState, Flow, Format, RadioState, SessionState,
    };

    use super::*;

    fn group(app: &str) -> AppGroupSnapshot {
        AppGroupSnapshot {
            app_id: Arc::from(app),
            display_name: Some(app.to_owned()),
            icon: None,
            pid: 42,
            is_system_sounds: false,
            state: SessionState::Active,
            volume: 0.5,
            muted: false,
            session_count: 1,
            is_exclusive: false,
        }
    }

    fn device(id: &str) -> DeviceSnapshot {
        DeviceSnapshot {
            id: Arc::from(id),
            flow: Flow::Render,
            order: 0,
            state: DeviceState(DeviceState::ACTIVE),
            name: "Speakers".into(),
            description: "Speakers".into(),
            interface_name: "Realtek".into(),
            enumerator: "HDAUDIO".into(),
            is_default: true,
            is_default_comms: false,
            volume: 0.3,
            muted: false,
            capture_sleeping: false,
            format: Some(Format { channels: 2, bits: 24, rate: 48000 }),
            listen_enabled: false,
            listen_target: None,
            exclusive_allowed: true,
            exclusive_held: false,
            eq_apo: EqApoState::NotAvailable,
            bt: Some(BtState::default()),
            groups: vec![group("a"), group("b")],
        }
    }

    fn snapshot() -> AudioSnapshot {
        AudioSnapshot {
            rev: 1,
            structure_rev: 1,
            devices: vec![device("x"), device("y")],
            defaults: Defaults { render: Some(Arc::from("x")), ..Defaults::default() },
            radio: RadioState::On,
            ducking: DuckingMode::Reduce80,
            eq_apo_available: false,
            is_elevated: false,
            ready: true,
        }
    }

    fn with(edit: impl FnOnce(&mut AudioSnapshot)) -> Change {
        let prev = snapshot();
        let mut next = snapshot();
        edit(&mut next);
        classify(&prev, &next)
    }

    #[test]
    fn identical_or_rev_only_is_no_change() {
        assert_eq!(with(|_| {}), Change::None);
        assert_eq!(with(|s| s.rev = 9), Change::None);
    }

    #[test]
    fn value_class_changes() {
        assert_eq!(with(|s| s.devices[0].volume = 0.9), Change::Values);
        assert_eq!(with(|s| s.devices[0].muted = true), Change::Values);
        assert_eq!(with(|s| s.devices[1].capture_sleeping = true), Change::Values);
        assert_eq!(with(|s| s.devices[1].listen_enabled = true), Change::Values);
        assert_eq!(with(|s| s.devices[1].listen_target = Some(Arc::from("t"))), Change::Values);
        assert_eq!(with(|s| s.devices[0].exclusive_allowed = false), Change::Values);
        assert_eq!(with(|s| s.devices[0].exclusive_held = true), Change::Values);
        assert_eq!(with(|s| s.devices[0].eq_apo = EqApoState::Running), Change::Values);
        assert_eq!(with(|s| s.devices[0].bt.as_mut().unwrap().seconds_remaining = 12), Change::Values);
        assert_eq!(with(|s| s.devices[0].groups[1].volume = 0.1), Change::Values);
        assert_eq!(with(|s| s.devices[0].groups[1].session_count = 3), Change::Values);
        assert_eq!(with(|s| s.eq_apo_available = true), Change::Values);
    }

    #[test]
    fn structure_class_changes() {
        assert_eq!(
            with(|s| {
                s.devices.pop();
            }),
            Change::Structure
        );
        assert_eq!(with(|s| s.devices.swap(0, 1)), Change::Structure);
        assert_eq!(with(|s| s.devices[0].state = DeviceState(DeviceState::DISABLED)), Change::Structure);
        assert_eq!(with(|s| s.devices[0].name = "Renamed".into()), Change::Structure);
        assert_eq!(with(|s| s.devices[0].is_default = false), Change::Structure);
        assert_eq!(with(|s| s.devices[0].is_default_comms = true), Change::Structure);
        assert_eq!(with(|s| s.devices[0].format = None), Change::Structure);
        assert_eq!(with(|s| s.devices[0].bt = None), Change::Structure);
        assert_eq!(with(|s| s.devices[0].bt.as_mut().unwrap().connected = true), Change::Structure);
        assert_eq!(with(|s| s.devices[0].bt.as_mut().unwrap().pending_deadline_ms = Some(5)), Change::Structure);
        assert_eq!(with(|s| s.devices[0].bt.as_mut().unwrap().battery = Some(80)), Change::Structure);
        assert_eq!(with(|s| s.devices[0].bt.as_mut().unwrap().codec = Some("AAC".into())), Change::Structure);
        assert_eq!(
            with(|s| {
                s.devices[0].groups.pop();
            }),
            Change::Structure
        );
        assert_eq!(with(|s| s.devices[0].groups[0].state = SessionState::Inactive), Change::Structure);
        assert_eq!(with(|s| s.devices[0].groups[0].muted = true), Change::Structure);
        assert_eq!(with(|s| s.devices[0].groups[0].display_name = None), Change::Structure);
        assert_eq!(with(|s| s.defaults.render = None), Change::Structure);
        assert_eq!(with(|s| s.radio = RadioState::Off), Change::Structure);
        assert_eq!(with(|s| s.ducking = DuckingMode::DoNothing), Change::Structure);
        assert_eq!(with(|s| s.ready = false), Change::Structure);
        // A structural change wins over simultaneous value changes.
        assert_eq!(
            with(|s| {
                s.devices[0].volume = 0.0;
                s.devices[1].order = 7;
            }),
            Change::Structure
        );
    }
}
