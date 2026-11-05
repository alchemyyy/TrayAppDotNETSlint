//! `AppDrawerVisibilityPolicy.Resolve` + the flyout's locally-visible group filter
//! (`VolumeFlyoutWindow.ResolveVisibleGroupsByDevice` / `IsLocallyVisibleGroup`).

use std::collections::{HashMap, HashSet};

use crate::settings::{AppSettings, CaptureActivityIndicator};
use crate::snapshot::{AppGroupSnapshot, AppId, AudioSnapshot, DeviceId, DeviceSnapshot, Flow, SessionState};

/// One locally eligible app group, for cross-device drawer ownership (`AppDrawerVisibilityCandidate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawerCandidate<'a> {
    pub flow: Flow,
    pub device_id: &'a str,
    /// The device is the active default of its flow (`DefaultDevice` / `DefaultCaptureDevice`).
    pub is_default_device: bool,
    pub app_id: &'a str,
    pub state: SessionState,
}

/// One visibility flag per candidate. Expired copies are never visible. When the same
/// `(flow, app)` sits on two or more devices: any Active copy wins (every Active copy stays); else
/// the copy on the default device wins; else every copy stays. No deduplication across flows.
pub fn resolve_drawer_visibility(candidates: &[DrawerCandidate<'_>]) -> Vec<bool> {
    let mut visible = vec![false; candidates.len()];
    let mut by_app: HashMap<(Flow, &str), Vec<usize>> = HashMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if candidate.state == SessionState::Expired {
            continue;
        }
        visible[index] = true;
        by_app.entry((candidate.flow, candidate.app_id)).or_default().push(index);
    }

    for indexes in by_app.values() {
        if indexes.len() < 2 {
            continue;
        }
        let devices: HashSet<&str> = indexes.iter().map(|i| candidates[*i].device_id).collect();
        if devices.len() < 2 {
            continue;
        }
        let has_active = indexes.iter().any(|i| candidates[*i].state == SessionState::Active);
        if has_active {
            for i in indexes {
                visible[*i] = candidates[*i].state == SessionState::Active;
            }
            continue;
        }
        if indexes.iter().any(|i| candidates[*i].is_default_device) {
            for i in indexes {
                visible[*i] = candidates[*i].is_default_device;
            }
        }
    }
    visible
}

/// `IsLocallyVisibleGroup`: drops expired groups, system sounds, this process's own sessions,
/// empty groups, and inactive capture groups under `CaptureActivityIndicator::HideInactive`.
pub fn is_locally_visible_group(
    device: &DeviceSnapshot,
    group: &AppGroupSnapshot,
    settings: &AppSettings,
    own_pid: u32,
) -> bool {
    if group.state == SessionState::Expired || group.is_system_sounds || group.session_count == 0 {
        return false;
    }
    // The own group is keyed by this executable's path, so its representative is this process.
    if own_pid != 0 && group.pid == own_pid {
        return false;
    }
    !(settings.capture_activity_indicator == CaptureActivityIndicator::HideInactive
        && device.is_capture()
        && group.state != SessionState::Active)
}

/// Visible groups per listed device, in each device's group order. `listed` are the device indices
/// from `flyout_device_order`. Drops expired groups, system sounds, this process's own session,
/// hidden-inactive capture groups (`CaptureActivityIndicator::HideInactive`), and resolves
/// cross-device duplicates of the same app (active copies win, else the copy on the active default
/// device, else all copies stay).
pub fn visible_groups(
    snapshot: &AudioSnapshot,
    listed: &[usize],
    settings: &AppSettings,
    own_pid: u32,
) -> Vec<(DeviceId, Vec<AppId>)> {
    let mut bindings: Vec<(usize, &AppGroupSnapshot)> = Vec::new();
    let mut candidates: Vec<DrawerCandidate<'_>> = Vec::new();
    let devices: Vec<&DeviceSnapshot> = listed.iter().filter_map(|i| snapshot.devices.get(*i)).collect();

    for (slot, device) in devices.iter().enumerate() {
        let default_id = match device.flow {
            Flow::Render => snapshot.defaults.render.as_deref(),
            Flow::Capture => snapshot.defaults.capture.as_deref(),
        };
        let is_default_device = default_id == Some(&*device.id);
        for group in &device.groups {
            if !is_locally_visible_group(device, group, settings, own_pid) {
                continue;
            }
            bindings.push((slot, group));
            candidates.push(DrawerCandidate {
                flow: device.flow,
                device_id: &device.id,
                is_default_device,
                app_id: &group.app_id,
                state: group.state,
            });
        }
    }

    let visible = resolve_drawer_visibility(&candidates);
    let mut result: Vec<(DeviceId, Vec<AppId>)> = devices.iter().map(|d| (d.id.clone(), Vec::new())).collect();
    for ((slot, group), is_visible) in bindings.into_iter().zip(visible) {
        if is_visible {
            result[slot].1.push(group.app_id.clone());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    //! Port of `AppDrawerVisibilityPolicyTests.cs` plus the local filter.
    use super::*;
    use crate::policy::visibility::tests::device;
    use crate::snapshot::{Defaults, DeviceState};

    fn candidate<'a>(device_id: &'a str, is_default_device: bool, state: SessionState) -> DrawerCandidate<'a> {
        DrawerCandidate { flow: Flow::Render, device_id, is_default_device, app_id: "test-app", state }
    }

    #[test]
    fn resolve_prefers_default_device_for_inactive_duplicates() {
        let c = [
            candidate("old-device", false, SessionState::Inactive),
            candidate("new-device", true, SessionState::Inactive),
        ];
        assert_eq!(resolve_drawer_visibility(&c), [false, true]);
    }

    #[test]
    fn resolve_prefers_active_non_default_copy_over_inactive_default_copy() {
        let c = [
            candidate("routed-device", false, SessionState::Active),
            candidate("default-device", true, SessionState::Inactive),
        ];
        assert_eq!(resolve_drawer_visibility(&c), [true, false]);
    }

    #[test]
    fn resolve_keeps_every_active_copy_for_simultaneous_output() {
        let c = [
            candidate("first-device", false, SessionState::Active),
            candidate("second-device", true, SessionState::Active),
        ];
        assert_eq!(resolve_drawer_visibility(&c), [true, true]);
    }

    #[test]
    fn resolve_keeps_inactive_copies_when_default_device_has_no_copy() {
        let c = [
            candidate("first-routed-device", false, SessionState::Inactive),
            candidate("second-routed-device", false, SessionState::Inactive),
        ];
        assert_eq!(resolve_drawer_visibility(&c), [true, true]);
    }

    #[test]
    fn resolve_keeps_unique_app_on_previous_default_device() {
        let c = [
            candidate("old-device", false, SessionState::Inactive),
            candidate("new-device", true, SessionState::Inactive),
            DrawerCandidate { app_id: "unique-app", ..candidate("old-device", false, SessionState::Inactive) },
        ];
        assert_eq!(resolve_drawer_visibility(&c), [false, true, true]);
    }

    #[test]
    fn resolve_does_not_deduplicate_across_data_flows() {
        let c = [
            candidate("render-device", true, SessionState::Inactive),
            DrawerCandidate { flow: Flow::Capture, ..candidate("capture-device", false, SessionState::Inactive) },
        ];
        assert_eq!(resolve_drawer_visibility(&c), [true, true]);
    }

    #[test]
    fn resolve_hides_expired_copies_without_suppressing_live_copy() {
        let c = [
            candidate("expired-default", true, SessionState::Expired),
            candidate("live-device", false, SessionState::Inactive),
        ];
        assert_eq!(resolve_drawer_visibility(&c), [false, true]);
    }

    #[test]
    fn resolve_transfers_inactive_ownership_when_default_copy_arrives() {
        let source_only = [candidate("old-device", false, SessionState::Inactive)];
        let both = [
            candidate("old-device", false, SessionState::Inactive),
            candidate("new-device", true, SessionState::Inactive),
        ];
        assert_eq!(resolve_drawer_visibility(&source_only), [true]);
        assert_eq!(resolve_drawer_visibility(&both), [false, true]);
    }

    fn group(app: &str, pid: u32, state: SessionState) -> AppGroupSnapshot {
        AppGroupSnapshot {
            app_id: app.into(),
            display_name: Some(app.to_owned()),
            icon: None,
            pid,
            is_system_sounds: false,
            state,
            volume: 1.0,
            muted: false,
            session_count: 1,
            is_exclusive: false,
        }
    }

    #[test]
    fn visible_groups_filters_locally_then_resolves_duplicates() {
        let mut old = device("old", Flow::Render, DeviceState::ACTIVE);
        old.groups = vec![
            group("c:\\app.exe", 10, SessionState::Inactive),
            group("c:\\own.exe", 99, SessionState::Active),
            AppGroupSnapshot { is_system_sounds: true, ..group("System.SystemSoundsSession", 0, SessionState::Active) },
            group("c:\\gone.exe", 11, SessionState::Expired),
        ];
        let mut new = device("new", Flow::Render, DeviceState::ACTIVE);
        new.groups = vec![group("c:\\app.exe", 10, SessionState::Inactive)];
        let mut mic = device("mic", Flow::Capture, DeviceState::ACTIVE);
        mic.groups = vec![group("c:\\rec.exe", 12, SessionState::Inactive)];
        let snapshot = AudioSnapshot {
            devices: vec![old, new, mic],
            defaults: Defaults { render: Some("new".into()), ..Defaults::default() },
            ..AudioSnapshot::default()
        };

        let s = AppSettings::default();
        let result = visible_groups(&snapshot, &[2, 0, 1], &s, 99);
        let ids: Vec<(&str, Vec<&str>)> =
            result.iter().map(|(d, g)| (&**d, g.iter().map(|a| &**a).collect())).collect();
        assert_eq!(ids, [("mic", vec!["c:\\rec.exe"]), ("old", vec![]), ("new", vec!["c:\\app.exe"])]);

        let hide = AppSettings { capture_activity_indicator: CaptureActivityIndicator::HideInactive, ..s };
        let result = visible_groups(&snapshot, &[2], &hide, 99);
        assert!(result[0].1.is_empty());
    }
}
