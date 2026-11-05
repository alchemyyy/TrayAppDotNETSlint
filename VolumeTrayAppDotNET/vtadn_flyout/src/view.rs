//! Content generation: which devices and app groups the flyout lists (`FlyoutDeviceOrdering` +
//! `ResolveVisibleGroupsByDevice`) and the Slint row data for each of them (`BuildCell` and the
//! per-widget `UpdateVisual` closures, evaluated from one snapshot).

use slint::{Image, ModelRc};
use vtadn_model::device_settings::DeviceSettings;
use vtadn_model::policy::{drawer, ordering, scale};
use vtadn_model::settings::AppSettings;
use vtadn_model::snapshot::{AppGroupSnapshot, AppId, AudioSnapshot, DeviceId, DeviceSnapshot, EqApoState};

use crate::layout;
use crate::present::{self, GlyphSpec};
use crate::slider;
use crate::ui::{AppRow, DeviceRow, GlyphData};

/// One listed device of a generation.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceStructure {
    pub id: DeviceId,
    /// Visible groups in the device's group order.
    pub apps: Vec<AppId>,
    pub expanded: bool,
    pub drawer_visible: bool,
    pub grid: bool,
    pub is_last: bool,
}

/// Devices to list, top to bottom, with their visible app groups.
pub fn build_structure(
    snapshot: &AudioSnapshot,
    settings: &AppSettings,
    devices_xml: &DeviceSettings,
    own_pid: u32,
) -> Vec<DeviceStructure> {
    let listed = ordering::flyout_device_order(&snapshot.devices, settings, true, snapshot.radio);
    let groups = drawer::visible_groups(snapshot, &listed, settings, own_pid);
    let count = listed.len();
    listed
        .iter()
        .enumerate()
        .map(|(position, &index)| {
            let device = &snapshot.devices[index];
            let apps = groups.iter().find(|(id, _)| *id == device.id).map(|(_, apps)| apps.clone()).unwrap_or_default();
            let expanded = devices_xml.is_drawer_expanded(&device.id, settings.default_app_drawer_expanded);
            DeviceStructure {
                id: device.id.clone(),
                drawer_visible: expanded && !apps.is_empty(),
                apps,
                expanded,
                grid: layout::uses_grid_drawer(settings, device.is_capture()),
                is_last: position + 1 == count,
            }
        })
        .collect()
}

/// `(device, app)` keys of the meters the flyout draws (`IsPeakMeterVisible`).
pub fn visible_group_meters(structure: &[DeviceStructure]) -> Vec<(DeviceId, AppId)> {
    structure
        .iter()
        .filter(|d| d.drawer_visible)
        .flat_map(|d| d.apps.iter().map(|app| (d.id.clone(), app.clone())))
        .collect()
}

pub fn glyph_data(glyph: GlyphSpec) -> GlyphData {
    GlyphData {
        text: glyph.text.into(),
        scale: glyph.scale,
        tx: glyph.translate_x,
        ty: glyph.translate_y,
        bold: glyph.extra_bold,
    }
}

/// Inputs of a device row that are not in the snapshot.
pub struct DeviceRowInputs {
    /// Slider value shown (endpoint scalar; may lag the model while the user adjusts).
    pub shown_volume: f32,
    pub meter: i32,
    pub apps: ModelRc<AppRow>,
    /// `GetTickCount64` now, for the countdown pie.
    pub now_ms: u64,
}

pub fn device_row(
    device: &DeviceSnapshot,
    structure: &DeviceStructure,
    settings: &AppSettings,
    inputs: DeviceRowInputs,
) -> DeviceRow {
    let capture = device.is_capture();
    let active = device.state.is_active();
    let mute_glyph = present::device_volume_glyph(device, device.muted, settings);
    let preview_glyph = present::device_volume_glyph(device, !device.muted, settings);
    let show_bt = device.is_bluetooth()
        && if capture { settings.show_battery_button_for_recording } else { settings.show_battery_button_for_playback };
    let show_lock =
        if capture { settings.show_lock_button_for_recording } else { settings.show_lock_button_for_playback };
    let show_eq = if capture {
        settings.show_equalizer_apo_button_for_recording
    } else {
        settings.show_equalizer_apo_button_for_playback
    };
    let show_default = if capture {
        settings.show_default_device_button_for_recording
    } else {
        settings.show_default_device_button_for_playback
    };
    let bt_pending = device.bt_pending();
    let pie_step = device
        .bt
        .as_ref()
        .and_then(|bt| bt.pending_deadline_ms)
        .map_or(0, |deadline| pie_step_at(deadline, inputs.now_ms));
    let drawer_max_height = if structure.grid {
        layout::grid_drawer_max_height(settings, capture)
    } else {
        layout::slider_drawer_max_height(settings, capture)
    };
    let grid_height = if structure.grid {
        layout::icon_grid_layout(
            structure.apps.len(),
            &layout::IconGridOptions::from_settings(settings, crate::metrics::ICON_GRID_AVAILABLE_WIDTH),
        )
        .height
    } else {
        0.0
    };
    DeviceRow {
        name: device.name.as_str().into(),
        format_line: present::device_format_line(device, settings).into(),
        capture,
        muted: device.muted,
        dimmed: present::device_dimmed(device),
        mute_glyph: glyph_data(mute_glyph),
        mute_preview_glyph: glyph_data(preview_glyph),
        mute_glyph_centered: mute_glyph.is_microphone(),
        mute_preview_centered: preview_glyph.is_microphone(),
        mute_tooltip: if device.muted { present::UNMUTE_TOOLTIP } else { present::MUTE_TOOLTIP }.into(),
        value: inputs.shown_volume * 100.0,
        percent: slider::scalar_text(inputs.shown_volume).into(),
        meter: inputs.meter,
        show_bt,
        bt_glyph: present::bluetooth_button_glyph(device).into(),
        bt_dimmed: !(active || device.bt_audio_waiting() || bt_pending),
        bt_tooltip: if show_bt { present::bluetooth_button_tooltip(device).into() } else { Default::default() },
        bt_pending,
        bt_pie_step: pie_step,
        show_lock,
        lock_glyph: present::exclusive_glyph(device).into(),
        lock_dimmed: !device.exclusive_allowed,
        lock_tooltip: present::exclusive_tooltip(device).into(),
        show_eq,
        eq_running: device.eq_apo == EqApoState::Running,
        eq_badge: device.eq_apo == EqApoState::NotAvailable,
        eq_tooltip: present::equalizer_tooltip(device.eq_apo).into(),
        show_listen: capture && settings.show_listen_button_for_recording,
        listen_active: device.listen_enabled,
        show_default,
        default_glyph: glyph_data(present::device_state_glyph(device)),
        default_dimmed: !active,
        drawer_expanded: structure.expanded,
        has_groups: !structure.apps.is_empty(),
        drawer_visible: structure.drawer_visible,
        grid: structure.grid,
        apps: inputs.apps,
        drawer_max_height: drawer_max_height as f32,
        grid_height: grid_height as f32,
        is_last: structure.is_last,
    }
}

/// Countdown pie step at `now_ms`.
pub fn pie_step_at(deadline_ms: u64, now_ms: u64) -> i32 {
    present::pie_step(present::countdown_fraction(
        deadline_ms,
        now_ms,
        crate::metrics::BLUETOOTH_CONNECTION_ATTEMPT_TIMEOUT_MS,
    ))
}

/// Inputs of an app row that are not in the snapshot.
pub struct AppRowInputs {
    /// Session level shown (may lag the model while the user adjusts).
    pub shown_level: f32,
    pub meter: i32,
    pub icon: Option<Image>,
    pub position: (f64, f64),
}

pub fn app_row(
    device: &DeviceSnapshot,
    group: &AppGroupSnapshot,
    settings: &AppSettings,
    inputs: AppRowInputs,
) -> AppRow {
    let position = scale::slider_percent_from_level(inputs.shown_level, settings.use_logarithmic_volume_scale);
    AppRow {
        has_icon: inputs.icon.is_some(),
        icon: inputs.icon.unwrap_or_default(),
        tooltip: present::app_tooltip(group).into(),
        muted: group.muted,
        dimmed: present::app_dimmed(device, group, settings),
        playback: !device.is_capture(),
        value: position as f32,
        percent: slider::percent_text(position).into(),
        meter: inputs.meter,
        badge: present::app_activity_badge(device, group, settings).into(),
        x: inputs.position.0 as f32,
        y: inputs.position.1 as f32,
    }
}

/// Icon-grid cell positions for a device's drawer (empty for slider drawers).
pub fn grid_positions(structure: &DeviceStructure, settings: &AppSettings) -> Vec<(f64, f64)> {
    if !structure.grid {
        return vec![(0.0, 0.0); structure.apps.len()];
    }
    layout::icon_grid_layout(
        structure.apps.len(),
        &layout::IconGridOptions::from_settings(settings, crate::metrics::ICON_GRID_AVAILABLE_WIDTH),
    )
    .positions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_keys_cover_only_visible_drawers() {
        let structure = vec![
            DeviceStructure {
                id: "a".into(),
                apps: vec!["x".into(), "y".into()],
                expanded: true,
                drawer_visible: true,
                grid: false,
                is_last: false,
            },
            DeviceStructure {
                id: "b".into(),
                apps: vec!["z".into()],
                expanded: false,
                drawer_visible: false,
                grid: true,
                is_last: true,
            },
        ];
        let keys = visible_group_meters(&structure);
        assert_eq!(keys.len(), 2);
        assert_eq!(&*keys[1].1, "y");
    }

    #[test]
    fn pie_steps_follow_the_deadline() {
        assert_eq!(pie_step_at(30_000, 0), 360);
        assert_eq!(pie_step_at(30_000, 15_000), 180);
        assert_eq!(pie_step_at(30_000, 31_000), 0);
    }
}
