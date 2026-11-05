//! `FlyoutDeviceOrdering.Build`.
//!
//! Sort modes:
//! - `StateGrouped`: bucket by (default, default-comms, enabled, disabled, disconnected), then
//!   reverse so the default render device lands at the bottom, closest to the tray.
//! - `WindowsEnumeration`: manager (enumeration) order, top to bottom.
//!
//! Disconnected Bluetooth endpoints get a flyout-only placement (hide, follow normal visibility,
//! dedicated section, or intermixed) and can collapse to one row per physical device.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::policy::visibility::is_device_visible;
use crate::settings::{AppSettings, FlyoutDeviceSort, FlyoutDisconnectedBluetoothDeviceVisibility};
use crate::snapshot::{DeviceSnapshot, Flow, RadioState};

/// Where a disconnected Bluetooth endpoint goes in the flyout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectedBluetoothPlacement {
    Hidden,
    Standard,
    DedicatedSection,
}

/// Devices to list, top to bottom, as indices into `devices` (manager order). With
/// `apply_flyout_bluetooth_policies` the radio gate, the disconnected-Bluetooth visibility modes and
/// the collapse of disconnected Bluetooth endpoints apply (flyout); without them only the parent
/// gate and `is_device_visible` apply (tray menu, which iterates the result in reverse).
pub fn flyout_device_order(
    devices: &[DeviceSnapshot],
    settings: &AppSettings,
    apply_flyout_bluetooth_policies: bool,
    radio: RadioState,
) -> Vec<usize> {
    let radio_on = radio == RadioState::On;
    let mut visible = Vec::with_capacity(devices.len());
    let mut dedicated = Vec::new();
    for (index, device) in devices.iter().enumerate() {
        if !passes_flyout_parent_gates(device, settings) {
            continue;
        }
        if apply_flyout_bluetooth_policies
            && !passes_bluetooth_radio_gate(
                device.is_bluetooth(),
                settings.show_bluetooth_devices_only_when_bluetooth_is_on,
                radio_on,
            )
        {
            continue;
        }

        let normally_visible = is_device_visible(device, settings);
        let mut placement = if normally_visible {
            DisconnectedBluetoothPlacement::Standard
        } else {
            DisconnectedBluetoothPlacement::Hidden
        };
        if apply_flyout_bluetooth_policies
            && device.is_bluetooth()
            && device.state.is_disconnected()
            && !device.bt_orphaned()
        {
            placement = classify_disconnected_bluetooth(
                settings.flyout_disconnected_bluetooth_device_visibility,
                normally_visible,
                device.bt_pending() || device.bt_audio_waiting(),
            );
        }

        match placement {
            DisconnectedBluetoothPlacement::Standard => visible.push(index),
            DisconnectedBluetoothPlacement::DedicatedSection => dedicated.push(index),
            DisconnectedBluetoothPlacement::Hidden => {}
        }
    }

    let flow_of = |index: &usize| devices[*index].flow;
    let mut ordered = sort(&visible, settings, flow_of, |index| classify_bucket(&devices[*index]));
    let mut dedicated_section = sort(&dedicated, settings, flow_of, |index| classify_bucket(&devices[*index]));
    if apply_flyout_bluetooth_policies && settings.collapse_disconnected_bluetooth_devices_in_flyout {
        let connected = containers_with_active_bluetooth_endpoint(devices);
        let container_of = |index: &usize| {
            let device = &devices[*index];
            resolve_collapsible_container(
                device.is_bluetooth(),
                device.state.is_disconnected(),
                device.bt_orphaned(),
                device.bt.as_ref().and_then(|bt| bt.container_id.as_ref()),
                &connected,
            )
        };
        ordered = collapse_disconnected_bluetooth(&ordered, settings.flyout_device_sort, container_of, flow_of);
        dedicated_section =
            collapse_disconnected_bluetooth(&dedicated_section, settings.flyout_device_sort, container_of, flow_of);
    }

    place_dedicated_section(&ordered, &dedicated_section, settings.flyout_device_sort)
}

/// Containers of Bluetooth devices with at least one active endpoint, listed or not.
fn containers_with_active_bluetooth_endpoint(devices: &[DeviceSnapshot]) -> HashSet<Arc<str>> {
    devices
        .iter()
        .filter(|device| device.state.is_active())
        .filter_map(|device| device.bt.as_ref().and_then(|bt| bt.container_id.clone()))
        .collect()
}

/// Container a listed endpoint collapses into: a paired Bluetooth endpoint that is disconnected
/// while no endpoint of its device is active. None leaves the endpoint as its own row.
pub fn resolve_collapsible_container<'a>(
    is_bluetooth: bool,
    is_disconnected: bool,
    is_bluetooth_orphaned: bool,
    container_id: Option<&'a Arc<str>>,
    connected_containers: &HashSet<Arc<str>>,
) -> Option<&'a Arc<str>> {
    if !is_bluetooth || !is_disconnected || is_bluetooth_orphaned {
        return None;
    }
    let container = container_id?;
    if connected_containers.contains(container) { None } else { Some(container) }
}

/// Keeps one row per collapsible container, in the slot of its most prominent endpoint: the
/// playback endpoint that sorts with the highest priority, else the highest-priority recording
/// endpoint. StateGrouped lists are visually bottom-up (highest priority = last row);
/// WindowsEnumeration lists run top-down. Rows without a container keep their places.
pub fn collapse_disconnected_bluetooth<T: Clone, K: Eq + std::hash::Hash>(
    ordered: &[T],
    sort_order: FlyoutDeviceSort,
    container_of: impl Fn(&T) -> Option<K>,
    flow_of: impl Fn(&T) -> Flow,
) -> Vec<T> {
    let containers: Vec<Option<K>> = ordered.iter().map(&container_of).collect();
    let mut representative: HashMap<&K, usize> = HashMap::new();
    let bottom_up = sort_order == FlyoutDeviceSort::StateGrouped;
    for priority in 0..ordered.len() {
        let index = if bottom_up { ordered.len() - 1 - priority } else { priority };
        let Some(container) = containers[index].as_ref() else { continue };
        match representative.get(container).copied() {
            None => {
                representative.insert(container, index);
            }
            Some(current) => {
                // Playback outranks recording: the first playback endpoint replaces a recording
                // representative.
                if flow_of(&ordered[current]) != Flow::Render && flow_of(&ordered[index]) == Flow::Render {
                    representative.insert(container, index);
                }
            }
        }
    }

    ordered
        .iter()
        .enumerate()
        .filter(|(index, _)| containers[*index].as_ref().is_none_or(|container| representative[container] == *index))
        .map(|(_, item)| item.clone())
        .collect()
}

/// `PassesBluetoothRadioGate`.
pub fn passes_bluetooth_radio_gate(is_bluetooth: bool, show_only_when_on: bool, radio_on: bool) -> bool {
    !is_bluetooth || !show_only_when_on || radio_on
}

/// Parent gates that forced disconnected-Bluetooth modes must not bypass: recording endpoints stay
/// hidden when either the global recording master or the flyout recording switch is off.
fn passes_flyout_parent_gates(device: &DeviceSnapshot, settings: &AppSettings) -> bool {
    match device.flow {
        Flow::Render => true,
        Flow::Capture => settings.show_recording_devices && settings.show_recording_devices_in_flyout,
    }
}

/// `ClassifyDisconnectedBluetooth`.
pub fn classify_disconnected_bluetooth(
    visibility: FlyoutDisconnectedBluetoothDeviceVisibility,
    normally_visible: bool,
    has_connection_activity: bool,
) -> DisconnectedBluetoothPlacement {
    let should_show = normally_visible || has_connection_activity;
    match visibility {
        FlyoutDisconnectedBluetoothDeviceVisibility::NeverShow => DisconnectedBluetoothPlacement::Hidden,
        FlyoutDisconnectedBluetoothDeviceVisibility::Show => {
            if should_show {
                DisconnectedBluetoothPlacement::Standard
            } else {
                DisconnectedBluetoothPlacement::Hidden
            }
        }
        FlyoutDisconnectedBluetoothDeviceVisibility::AlwaysShow => DisconnectedBluetoothPlacement::DedicatedSection,
        FlyoutDisconnectedBluetoothDeviceVisibility::AlwaysShowIntermixed => DisconnectedBluetoothPlacement::Standard,
    }
}

fn sort<T: Clone>(
    visible: &[T],
    settings: &AppSettings,
    flow_of: impl Fn(&T) -> Flow,
    bucket_of: impl Fn(&T) -> usize,
) -> Vec<T> {
    match settings.flyout_device_sort {
        FlyoutDeviceSort::WindowsEnumeration => {
            order_windows_enumeration(visible, settings.intermix_recording_with_playback_in_flyout, flow_of)
        }
        FlyoutDeviceSort::StateGrouped => {
            order_state_grouped(visible, settings.intermix_recording_with_playback_in_flyout, flow_of, bucket_of)
        }
    }
}

/// Places the dedicated disconnected-Bluetooth section after the whole normal block in the sort
/// direction: above it for StateGrouped (visually bottom-up), below it for WindowsEnumeration.
pub fn place_dedicated_section<T: Clone>(normal: &[T], dedicated: &[T], sort_order: FlyoutDeviceSort) -> Vec<T> {
    let mut combined = Vec::with_capacity(normal.len() + dedicated.len());
    if sort_order == FlyoutDeviceSort::StateGrouped {
        combined.extend_from_slice(dedicated);
        combined.extend_from_slice(normal);
    } else {
        combined.extend_from_slice(normal);
        combined.extend_from_slice(dedicated);
    }
    combined
}

const BUCKET_COUNT: usize = 5;

/// State-bucket ordering. Intermixed: state is the outer grouping. Separated: flow is the outer
/// grouping (render first). The result is reversed so the default render device sits at the
/// bottom and every capture device stays above every render device.
pub fn order_state_grouped<T: Clone>(
    visible: &[T],
    intermix: bool,
    flow_of: impl Fn(&T) -> Flow,
    bucket_of: impl Fn(&T) -> usize,
) -> Vec<T> {
    let mut buckets: [Vec<T>; BUCKET_COUNT] = Default::default();
    for item in visible {
        buckets[bucket_of(item).min(BUCKET_COUNT - 1)].push(item.clone());
    }

    let mut ordered = Vec::with_capacity(visible.len());
    if intermix {
        for bucket in &buckets {
            ordered.extend_from_slice(bucket);
        }
    } else {
        for flow in [Flow::Render, Flow::Capture] {
            for bucket in &buckets {
                ordered.extend(bucket.iter().filter(|item| flow_of(item) == flow).cloned());
            }
        }
    }
    ordered.reverse();
    ordered
}

/// Bucket classifier: disconnected 4, disabled 3, default 0, default comms 1, else 2.
fn classify_bucket(device: &DeviceSnapshot) -> usize {
    if device.state.is_disconnected() {
        4
    } else if device.state.is_disabled() {
        3
    } else if device.is_default {
        0
    } else if device.is_default_comms {
        1
    } else {
        2
    }
}

/// Windows enumeration order: render then capture (each in manager order) unless intermixed.
fn order_windows_enumeration<T: Clone>(visible: &[T], intermix: bool, flow_of: impl Fn(&T) -> Flow) -> Vec<T> {
    if intermix {
        return visible.to_vec();
    }
    let mut ordered = Vec::with_capacity(visible.len());
    for flow in [Flow::Render, Flow::Capture] {
        ordered.extend(visible.iter().filter(|item| flow_of(item) == flow).cloned());
    }
    ordered
}

#[cfg(test)]
mod tests {
    //! Port of `FlyoutDeviceOrderingTests.cs` plus end-to-end checks of `flyout_device_order`.
    use super::*;
    use crate::policy::visibility::tests::device;
    use crate::snapshot::{BtState, DeviceState};

    #[derive(Clone)]
    struct TestDevice {
        name: &'static str,
        flow: Flow,
        bucket: usize,
    }

    fn state_grouped_input() -> Vec<TestDevice> {
        vec![
            TestDevice { name: "Default microphone", flow: Flow::Capture, bucket: 0 },
            TestDevice { name: "Digital output playback", flow: Flow::Render, bucket: 3 },
            TestDevice { name: "Bluetooth headset microphone", flow: Flow::Capture, bucket: 2 },
            TestDevice { name: "Disconnected microphone", flow: Flow::Capture, bucket: 4 },
            TestDevice { name: "Bluetooth headset playback", flow: Flow::Render, bucket: 0 },
        ]
    }

    fn names<T>(items: &[T], name: impl Fn(&T) -> &'static str) -> Vec<&'static str> {
        items.iter().map(name).collect()
    }

    #[test]
    fn order_state_grouped_separates_capture_from_render_across_state_buckets() {
        let ordered = order_state_grouped(&state_grouped_input(), false, |d| d.flow, |d| d.bucket);
        assert_eq!(
            names(&ordered, |d| d.name),
            [
                "Disconnected microphone",
                "Bluetooth headset microphone",
                "Default microphone",
                "Digital output playback",
                "Bluetooth headset playback"
            ]
        );
    }

    #[test]
    fn order_state_grouped_preserves_state_first_ordering_when_intermixed() {
        let ordered = order_state_grouped(&state_grouped_input(), true, |d| d.flow, |d| d.bucket);
        assert_eq!(
            names(&ordered, |d| d.name),
            [
                "Disconnected microphone",
                "Digital output playback",
                "Bluetooth headset microphone",
                "Bluetooth headset playback",
                "Default microphone"
            ]
        );
    }

    #[test]
    fn classify_disconnected_bluetooth_implements_visibility_mode() {
        use DisconnectedBluetoothPlacement as P;
        use FlyoutDisconnectedBluetoothDeviceVisibility as V;
        for (visibility, normally_visible, expected) in [
            (V::NeverShow, false, P::Hidden),
            (V::NeverShow, true, P::Hidden),
            (V::Show, false, P::Hidden),
            (V::Show, true, P::Standard),
            (V::AlwaysShow, false, P::DedicatedSection),
            (V::AlwaysShow, true, P::DedicatedSection),
            (V::AlwaysShowIntermixed, false, P::Standard),
            (V::AlwaysShowIntermixed, true, P::Standard),
        ] {
            assert_eq!(expected, classify_disconnected_bluetooth(visibility, normally_visible, false));
        }
    }

    #[test]
    fn connection_activity_surfaces_bluetooth_rows_unless_explicitly_hidden() {
        use DisconnectedBluetoothPlacement as P;
        use FlyoutDisconnectedBluetoothDeviceVisibility as V;
        for (visibility, expected) in [
            (V::NeverShow, P::Hidden),
            (V::Show, P::Standard),
            (V::AlwaysShow, P::DedicatedSection),
            (V::AlwaysShowIntermixed, P::Standard),
        ] {
            assert_eq!(expected, classify_disconnected_bluetooth(visibility, false, true));
        }
    }

    #[test]
    fn only_unplugged_bluetooth_endpoints_are_reconnectable() {
        for (is_bluetooth, state, expected) in [
            (true, DeviceState::UNPLUGGED, true),
            (true, DeviceState::NOT_PRESENT, false),
            (true, DeviceState::ACTIVE, false),
            (true, DeviceState::DISABLED, false),
            (false, DeviceState::UNPLUGGED, false),
            (false, DeviceState::NOT_PRESENT, false),
        ] {
            let mut d = device("d", Flow::Render, state);
            d.bt = is_bluetooth.then(BtState::default);
            assert_eq!(expected, d.bt_unplugged());
        }
    }

    #[test]
    fn bluetooth_defaults() {
        let s = AppSettings::default();
        assert_eq!(
            FlyoutDisconnectedBluetoothDeviceVisibility::AlwaysShowIntermixed,
            s.flyout_disconnected_bluetooth_device_visibility
        );
        assert!(s.show_bluetooth_devices_only_when_bluetooth_is_on);
        assert!(s.collapse_disconnected_bluetooth_devices_in_flyout);
    }

    #[test]
    fn bluetooth_radio_gate_only_hides_bluetooth_devices_when_configured_and_off() {
        for (is_bt, only_when_on, radio_on, expected) in [
            (false, true, false, true),
            (true, false, false, true),
            (true, true, true, true),
            (true, true, false, false),
        ] {
            assert_eq!(expected, passes_bluetooth_radio_gate(is_bt, only_when_on, radio_on));
        }
    }

    #[test]
    fn always_show_section_follows_sort_direction_after_both_normal_flows() {
        let normal = ["Playback", "Recording"];
        let dedicated = ["Disconnected Bluetooth playback", "Disconnected Bluetooth recording"];
        assert_eq!(
            place_dedicated_section(&normal, &dedicated, FlyoutDeviceSort::StateGrouped),
            ["Disconnected Bluetooth playback", "Disconnected Bluetooth recording", "Playback", "Recording"]
        );
        assert_eq!(
            place_dedicated_section(&normal, &dedicated, FlyoutDeviceSort::WindowsEnumeration),
            ["Playback", "Recording", "Disconnected Bluetooth playback", "Disconnected Bluetooth recording"]
        );
    }

    const FIRST_HEADSET: &str = "{79CA349E-3234-5F32-9369-F304D62A4404}";
    const SECOND_HEADSET: &str = "{3BA371B9-A719-5D78-B530-7D17F546FBDF}";

    #[test]
    fn only_disconnected_bluetooth_endpoints_of_inactive_devices_collapse() {
        let first: Arc<str> = FIRST_HEADSET.into();
        for (is_bt, disconnected, orphaned, has_container, container_connected, expected) in [
            (true, true, false, true, false, true),
            (true, true, false, true, true, false),
            (false, true, false, true, false, false),
            (true, false, false, true, false, false),
            (true, true, true, true, false, false),
            (true, true, false, false, false, false),
        ] {
            let mut connected = HashSet::new();
            if container_connected {
                connected.insert(first.clone());
            }
            let container = has_container.then_some(&first);
            assert_eq!(
                expected,
                resolve_collapsible_container(is_bt, disconnected, orphaned, container, &connected).is_some()
            );
        }
    }

    #[derive(Clone)]
    struct CollapseDevice {
        name: &'static str,
        flow: Flow,
        container: Option<&'static str>,
    }

    fn c(name: &'static str, flow: Flow, container: Option<&'static str>) -> CollapseDevice {
        CollapseDevice { name, flow, container }
    }

    fn collapse(ordered: &[CollapseDevice], sort_order: FlyoutDeviceSort) -> Vec<&'static str> {
        let collapsed = collapse_disconnected_bluetooth(ordered, sort_order, |d| d.container, |d| d.flow);
        names(&collapsed, |d| d.name)
    }

    #[test]
    fn collapsed_headset_takes_its_most_prominent_playback_slot() {
        let ordered = [
            c("Headset microphone", Flow::Capture, Some(FIRST_HEADSET)),
            c("Desk microphone", Flow::Capture, None),
            c("Headphones", Flow::Render, Some(FIRST_HEADSET)),
            c("Headset", Flow::Render, Some(FIRST_HEADSET)),
            c("Speakers", Flow::Render, None),
        ];
        assert_eq!(collapse(&ordered, FlyoutDeviceSort::StateGrouped), ["Desk microphone", "Headset", "Speakers"]);
    }

    #[test]
    fn collapsed_headset_takes_topmost_playback_slot_in_windows_order() {
        let ordered = [
            c("Headphones", Flow::Render, Some(FIRST_HEADSET)),
            c("Speakers", Flow::Render, None),
            c("Headset", Flow::Render, Some(FIRST_HEADSET)),
            c("Headset microphone", Flow::Capture, Some(FIRST_HEADSET)),
        ];
        assert_eq!(collapse(&ordered, FlyoutDeviceSort::WindowsEnumeration), ["Headphones", "Speakers"]);
    }

    #[test]
    fn collapsed_playback_outranks_recording_that_sorted_sooner() {
        let ordered = [
            c("Headphones", Flow::Render, Some(FIRST_HEADSET)),
            c("Headset microphone", Flow::Capture, Some(FIRST_HEADSET)),
        ];
        assert_eq!(collapse(&ordered, FlyoutDeviceSort::StateGrouped), ["Headphones"]);
    }

    #[test]
    fn collapsed_row_falls_back_to_recording_without_listed_playback() {
        let ordered = [c("Headset microphone", Flow::Capture, Some(FIRST_HEADSET)), c("Speakers", Flow::Render, None)];
        assert_eq!(collapse(&ordered, FlyoutDeviceSort::StateGrouped), ["Headset microphone", "Speakers"]);
    }

    #[test]
    fn collapse_keeps_one_row_per_device() {
        let ordered = [
            c("Headphones (first)", Flow::Render, Some(FIRST_HEADSET)),
            c("Headphones (second)", Flow::Render, Some(SECOND_HEADSET)),
            c("Headset (first)", Flow::Render, Some(FIRST_HEADSET)),
            c("Headset (second)", Flow::Render, Some(SECOND_HEADSET)),
        ];
        assert_eq!(collapse(&ordered, FlyoutDeviceSort::StateGrouped), ["Headset (first)", "Headset (second)"]);
    }

    fn bt(mut d: DeviceSnapshot, container: &str) -> DeviceSnapshot {
        d.bt = Some(BtState { container_id: Some(container.into()), radio_off_pairing: true, ..BtState::default() });
        d
    }

    #[test]
    fn build_puts_the_default_render_device_at_the_bottom() {
        let mut speakers = device("Speakers", Flow::Render, DeviceState::ACTIVE);
        speakers.is_default = true;
        let monitor = device("Monitor", Flow::Render, DeviceState::ACTIVE);
        let mut mic = device("Mic", Flow::Capture, DeviceState::ACTIVE);
        mic.is_default = true;
        let disabled = device("Disabled", Flow::Render, DeviceState::DISABLED);
        let devices = [speakers, monitor, mic, disabled];
        let s = AppSettings::default();
        let order = flyout_device_order(&devices, &s, true, RadioState::On);
        let listed: Vec<&str> = order.iter().map(|i| devices[*i].name.as_str()).collect();
        assert_eq!(listed, ["Mic", "Monitor", "Speakers"]);

        let windows =
            AppSettings { flyout_device_sort: FlyoutDeviceSort::WindowsEnumeration, ..AppSettings::default() };
        let order = flyout_device_order(&devices, &windows, true, RadioState::On);
        assert_eq!(order, [0, 1, 2]);

        let no_flyout_capture = AppSettings { show_recording_devices_in_flyout: false, ..AppSettings::default() };
        let order = flyout_device_order(&devices, &no_flyout_capture, false, RadioState::Unknown);
        assert_eq!(order, [1, 0]);
    }

    #[test]
    fn build_applies_radio_gate_and_collapse_only_for_the_flyout() {
        let headphones = bt(device("Headphones", Flow::Render, DeviceState::UNPLUGGED), FIRST_HEADSET);
        let headset = bt(device("Headset", Flow::Render, DeviceState::UNPLUGGED), FIRST_HEADSET);
        let speakers = device("Speakers", Flow::Render, DeviceState::ACTIVE);
        let devices = [headphones, headset, speakers];
        let s = AppSettings::default();

        // Radio off: every Bluetooth row hidden in the flyout.
        let order = flyout_device_order(&devices, &s, true, RadioState::Off);
        assert_eq!(order, [2]);
        // Radio on: AlwaysShowIntermixed lists them, then the collapse keeps one row.
        let order = flyout_device_order(&devices, &s, true, RadioState::On);
        assert_eq!(order.len(), 2);
        assert_eq!(*order.last().unwrap_or(&usize::MAX), 2);
        // Tray menu: normal visibility only (unplugged hidden by default), no collapse.
        let order = flyout_device_order(&devices, &s, false, RadioState::Off);
        assert_eq!(order, [2]);
    }
}
