//! `settings.xml` / `devices.xml` mapping for [`AppSettings`] and [`DeviceSettings`] (the
//! generated C# serializer's output for these types, element for element).
//!
//! `settings.xml`: the [`tadn_common::settings::CommonSettings`] elements first, then the Volume
//! elements in `AppSettings.cs` declaration order. Reading is order-agnostic, skips unknown
//! elements, keeps defaults for unparsable values, and applies the setter clamps afterwards.

use tadn_common::hotkey;
use tadn_common::theme::Argb;
use tadn_common::xml::{XmlDocument, XmlElement, XmlError, XmlWriter};

use crate::device_settings::{DEVICES_ROOT_ELEMENT, DeviceEntry, DeviceSettings};
use crate::settings::{
    AppSettings, METER_PEAK_COLOR_DEFAULT, METER_PEAK_STEREO_COLOR_DEFAULT, SliderThumbOption, SliderThumbShape,
};

/// `AppTheme.LegacyMeterPeakColorDefaultHex` (`VolumeAppTheme.LegacyMeterPeakColorDefault`).
const LEGACY_METER_PEAK_COLOR_DEFAULT: Argb = Argb(0xFFFF_FFFF);
/// `AppTheme.LegacyMeterPeakStereoColorDefaultHex`.
const LEGACY_METER_PEAK_STEREO_COLOR_DEFAULT: Argb = Argb(0x80FF_FFFF);

/// `MeterPeak*ColorHex` setter: blank, the current default or the legacy default (case-insensitive
/// `#AARRGGBB` text) means "use the default" (None); any other text is the user's color (an
/// unparsable one renders with the default, so it is None too).
fn read_meter_color(
    element: &XmlElement,
    default: Argb,
    legacy: Argb,
    target: &mut Option<Argb>,
) -> Result<(), XmlError> {
    let text = element.content()?;
    let is_default = text.trim().is_empty()
        || text.eq_ignore_ascii_case(&default.to_argb_hex())
        || text.eq_ignore_ascii_case(&legacy.to_argb_hex());
    *target = if is_default { None } else { Argb::parse_hex(text) };
    Ok(())
}

/// Written as the default `#AARRGGBB` when None, else the color picker's hex.
fn write_meter_color(writer: &mut XmlWriter, name: &str, value: Option<Argb>, default: Argb) {
    writer.element(name, &value.map_or_else(|| default.to_argb_hex(), Argb::to_hex));
}

/// A `<SliderThumb>` element read into a fresh `SliderThumbGlyphOption` (its initializers are the
/// "Circle" option), field by field.
fn read_slider_thumb(element: &XmlElement) -> Result<SliderThumbOption, XmlError> {
    let mut option = SliderThumbOption {
        name: "Circle".to_owned(),
        glyph: "\u{E91F}".to_owned(),
        font_family: "Segoe Fluent Icons".to_owned(),
        font_size: 18.0,
        width: 18.0,
        height: 18.0,
        x_scale: 1.0,
        shape: SliderThumbShape::Glyph,
    };
    for child in &element.children {
        match child.name.as_str() {
            "Name" => child.read_string(&mut option.name)?,
            "Glyph" => child.read_string(&mut option.glyph)?,
            "FontFamily" => child.read_string(&mut option.font_family)?,
            "FontSize" => child.read_f64(&mut option.font_size)?,
            "Width" => child.read_f64(&mut option.width)?,
            "Height" => child.read_f64(&mut option.height)?,
            "XScale" => child.read_f64(&mut option.x_scale)?,
            "Shape" => child.read_enum(&mut option.shape)?,
            _ => {}
        }
    }
    Ok(option)
}

/// `InitializeSliderThumbCatalog`: a loaded option with a non-empty name selects the built-in of
/// that name (its metrics win) or, for an unknown name, itself; otherwise Capsule stays selected.
fn resolve_slider_thumb(loaded: Option<SliderThumbOption>) -> SliderThumbOption {
    match loaded {
        Some(saved) if !saved.name.is_empty() => {
            SliderThumbOption::default_catalog().into_iter().find(|o| o.name == saved.name).unwrap_or(saved)
        }
        _ => SliderThumbOption::default(),
    }
}

fn write_slider_thumb(writer: &mut XmlWriter, option: &SliderThumbOption) {
    writer.start_element("SliderThumb");
    writer.element("Name", &option.name);
    writer.element("Glyph", &option.glyph);
    writer.element("FontFamily", &option.font_family);
    writer.element_f64("FontSize", option.font_size);
    writer.element_f64("Width", option.width);
    writer.element_f64("Height", option.height);
    writer.element_f64("XScale", option.x_scale);
    writer.element_enum("Shape", option.shape);
    writer.end_element();
}

impl AppSettings {
    /// Reads `<AppSettings>`; also reports whether a hotkey binding with an unknown action was
    /// dropped (a save is then due).
    fn read_xml_checked(root: &XmlElement) -> Result<(Self, bool), XmlError> {
        let mut s = Self::default();
        let mut dropped_hotkeys = false;
        let mut slider_thumb = None;
        for e in &root.children {
            if s.common.read_xml_element(e, &mut dropped_hotkeys)? {
                continue;
            }
            match e.name.as_str() {
                "SetDefaultCommsToDefault" => e.read_bool(&mut s.set_default_comms_to_default)?,
                "ShowDisabledPlaybackDevices" => e.read_bool(&mut s.show_disabled_playback_devices)?,
                "ShowDefaultPlaybackDeviceEvenIfDisabled" => {
                    e.read_bool(&mut s.show_default_playback_device_even_if_disabled)?
                }
                "ShowDefaultCommsPlaybackDeviceEvenIfDisabled" => {
                    e.read_bool(&mut s.show_default_comms_playback_device_even_if_disabled)?
                }
                "ShowDisconnectedPlaybackDevices" => e.read_bool(&mut s.show_disconnected_playback_devices)?,
                "ShowRecordingDevices" => e.read_bool(&mut s.show_recording_devices)?,
                "ShowDisabledRecordingDevices" => e.read_bool(&mut s.show_disabled_recording_devices)?,
                "ShowDefaultRecordingDeviceEvenIfDisabled" => {
                    e.read_bool(&mut s.show_default_recording_device_even_if_disabled)?
                }
                "ShowDefaultCommsRecordingDeviceEvenIfDisabled" => {
                    e.read_bool(&mut s.show_default_comms_recording_device_even_if_disabled)?
                }
                "ShowDisconnectedRecordingDevices" => e.read_bool(&mut s.show_disconnected_recording_devices)?,
                "DefaultAppDrawerExpanded" => e.read_bool(&mut s.default_app_drawer_expanded)?,
                "LastKnownDefaultPlaybackDeviceID" => {
                    e.read_opt_string(&mut s.last_known_default_playback_device_id)?
                }
                "LastKnownDefaultCommsPlaybackDeviceID" => {
                    e.read_opt_string(&mut s.last_known_default_comms_playback_device_id)?
                }
                "LastKnownDefaultRecordingDeviceID" => {
                    e.read_opt_string(&mut s.last_known_default_recording_device_id)?
                }
                "LastKnownDefaultCommsRecordingDeviceID" => {
                    e.read_opt_string(&mut s.last_known_default_comms_recording_device_id)?
                }
                "ShowNotPresentDevices" => e.read_bool(&mut s.show_not_present_devices)?,
                "ActivateRecordingDevicesForPeakMeters" => {
                    e.read_bool(&mut s.activate_recording_devices_for_peak_meters)?
                }
                "ShowTrayMenuRecordingLink" => e.read_bool(&mut s.show_tray_menu_recording_link)?,
                "ShowTrayMenuSoundsLink" => e.read_bool(&mut s.show_tray_menu_sounds_link)?,
                "ShowTrayMenuCommunicationsLink" => e.read_bool(&mut s.show_tray_menu_communications_link)?,
                "ShowTrayMenuDeviceLinks" => e.read_bool(&mut s.show_tray_menu_device_links)?,
                "UseLogarithmicVolumeScale" => e.read_bool(&mut s.use_logarithmic_volume_scale)?,
                "PlayDeviceVolumeChangeSound" => e.read_bool(&mut s.play_device_volume_change_sound)?,
                "SuppressDeviceVolumeChangeSoundWhenAudioPlaying" => {
                    e.read_bool(&mut s.suppress_device_volume_change_sound_when_audio_playing)?
                }
                "DingSuppressionPeakThresholdPercent" => e.read_i32(&mut s.ding_suppression_peak_threshold_percent)?,
                "PlayAppVolumeChangeSound" => e.read_bool(&mut s.play_app_volume_change_sound)?,
                "TrayMenuPlaybackDeviceNameStyle" => e.read_enum(&mut s.tray_menu_playback_device_name_style)?,
                "TrayMenuRecordingDeviceNameStyle" => e.read_enum(&mut s.tray_menu_recording_device_name_style)?,
                "TrayMenuDeviceNameMaxLength" => e.read_i32(&mut s.tray_menu_device_name_max_length)?,
                "TrayIconStyle" => e.read_enum(&mut s.tray_icon_style)?,
                "UnifiedPeakMeter" => e.read_bool(&mut s.unified_peak_meter)?,
                "UnifiedMeterLowChannelBiasMultiplier" => {
                    e.read_i32(&mut s.unified_meter_low_channel_bias_multiplier)?
                }
                "MeterPeakFps" => e.read_i32(&mut s.meter_peak_fps)?,
                "MeterPeakSampleRate" => e.read_i32(&mut s.meter_peak_sample_rate)?,
                "MeterPeakChangeCeiling" => e.read_i32(&mut s.meter_peak_change_ceiling)?,
                "IconRetryIntervalMs" => e.read_i32(&mut s.icon_retry_interval_ms)?,
                "IconLRULimit" => e.read_i32(&mut s.icon_lru_limit)?,
                "MeterPeakColorHex" => read_meter_color(
                    e,
                    METER_PEAK_COLOR_DEFAULT,
                    LEGACY_METER_PEAK_COLOR_DEFAULT,
                    &mut s.meter_peak_color,
                )?,
                "MeterPeakStereoColorHex" => read_meter_color(
                    e,
                    METER_PEAK_STEREO_COLOR_DEFAULT,
                    LEGACY_METER_PEAK_STEREO_COLOR_DEFAULT,
                    &mut s.meter_peak_stereo_color,
                )?,
                "SliderThumb" => slider_thumb = Some(read_slider_thumb(e)?),
                "TrayScrollEnabled" => e.read_bool(&mut s.tray_scroll_enabled)?,
                "PlayTrayScrollVolumeChangeSound" => e.read_bool(&mut s.play_tray_scroll_volume_change_sound)?,
                "WheelVolumeStepPercent" => e.read_i32(&mut s.wheel_volume_step_percent)?,
                "WheelVolumeStepFinePercent" => e.read_i32(&mut s.wheel_volume_step_fine_percent)?,
                "WheelVolumeStepCoarsePercent" => e.read_i32(&mut s.wheel_volume_step_coarse_percent)?,
                "TrayWheelAction" => e.read_enum(&mut s.tray_wheel_action)?,
                "TrayCtrlWheelAction" => e.read_enum(&mut s.tray_ctrl_wheel_action)?,
                "TrayAltWheelAction" => e.read_enum(&mut s.tray_alt_wheel_action)?,
                "PrecisionTouchpadScrollEnabled" => e.read_bool(&mut s.precision_touchpad_scroll_enabled)?,
                "PrecisionTouchpadUnitsPerScrollStep" => e.read_i32(&mut s.precision_touchpad_units_per_scroll_step)?,
                "TrayDoubleClickAction" => e.read_enum(&mut s.tray_double_click_action)?,
                "TrayCtrlLeftClickAction" => e.read_enum(&mut s.tray_ctrl_left_click_action)?,
                "TrayAltLeftClickAction" => e.read_enum(&mut s.tray_alt_left_click_action)?,
                "TrayCtrlRightClickAction" => e.read_enum(&mut s.tray_ctrl_right_click_action)?,
                "TrayAltRightClickAction" => e.read_enum(&mut s.tray_alt_right_click_action)?,
                "TrayCtrlDoubleLeftClickAction" => e.read_enum(&mut s.tray_ctrl_double_left_click_action)?,
                "TrayAltDoubleLeftClickAction" => e.read_enum(&mut s.tray_alt_double_left_click_action)?,
                "AllowFlyoutUndock" => e.read_bool(&mut s.allow_flyout_undock)?,
                "ClampUndockedFlyoutToScreen" => e.read_bool(&mut s.clamp_undocked_flyout_to_screen)?,
                "RestoreFlyoutUndockedOnStartup" => e.read_bool(&mut s.restore_flyout_undocked_on_startup)?,
                "FlyoutUndocked" => e.read_bool(&mut s.flyout_undocked)?,
                "FlyoutHasSavedPosition" => e.read_bool(&mut s.flyout_has_saved_position)?,
                "FlyoutLeft" => e.read_f64(&mut s.flyout_left)?,
                "FlyoutTop" => e.read_f64(&mut s.flyout_top)?,
                "FlyoutHeaderAtBottom" => e.read_bool(&mut s.flyout_header_at_bottom)?,
                "SoundSettingsTarget" => e.read_enum(&mut s.sound_settings_target)?,
                "FlyoutDeviceLayout" => e.read_enum(&mut s.flyout_device_layout)?,
                "FlyoutDeviceTitlePosition" => e.read_enum(&mut s.flyout_device_title_position)?,
                "FlyoutDeviceSort" => e.read_enum(&mut s.flyout_device_sort)?,
                "FlyoutDisconnectedBluetoothDeviceVisibility" => {
                    e.read_enum(&mut s.flyout_disconnected_bluetooth_device_visibility)?
                }
                "CollapseDisconnectedBluetoothDevicesInFlyout" => {
                    e.read_bool(&mut s.collapse_disconnected_bluetooth_devices_in_flyout)?
                }
                "ShowBluetoothDevicesOnlyWhenBluetoothIsOn" => {
                    e.read_bool(&mut s.show_bluetooth_devices_only_when_bluetooth_is_on)?
                }
                "ShowRecordingDevicesInFlyout" => e.read_bool(&mut s.show_recording_devices_in_flyout)?,
                "IntermixRecordingWithPlaybackInFlyout" => {
                    e.read_bool(&mut s.intermix_recording_with_playback_in_flyout)?
                }
                "UseDynamicPlaybackVolumeGlyphInFlyout" => {
                    e.read_bool(&mut s.use_dynamic_playback_volume_glyph_in_flyout)?
                }
                "ShowBluetoothRadioButtonInFlyoutHeader" => {
                    e.read_bool(&mut s.show_bluetooth_radio_button_in_flyout_header)?
                }
                "FlyoutBluetoothRadioButtonClickGesture" => {
                    e.read_enum(&mut s.flyout_bluetooth_radio_button_click_gesture)?
                }
                "FlyoutCommunicationsButtonVisibility" => {
                    e.read_enum(&mut s.flyout_communications_button_visibility)?
                }
                "ShowLockButtonForPlayback" => e.read_bool(&mut s.show_lock_button_for_playback)?,
                "ShowEqualizerAPOButtonForPlayback" => e.read_bool(&mut s.show_equalizer_apo_button_for_playback)?,
                "ShowDefaultDeviceButtonForPlayback" => e.read_bool(&mut s.show_default_device_button_for_playback)?,
                "ShowBatteryButtonForPlayback" => e.read_bool(&mut s.show_battery_button_for_playback)?,
                "ShowLockButtonForRecording" => e.read_bool(&mut s.show_lock_button_for_recording)?,
                "ShowEqualizerAPOButtonForRecording" => e.read_bool(&mut s.show_equalizer_apo_button_for_recording)?,
                "ShowListenButtonForRecording" => e.read_bool(&mut s.show_listen_button_for_recording)?,
                "ShowDefaultDeviceButtonForRecording" => {
                    e.read_bool(&mut s.show_default_device_button_for_recording)?
                }
                "ShowBatteryButtonForRecording" => e.read_bool(&mut s.show_battery_button_for_recording)?,
                "ShowDeviceFormatText" => e.read_bool(&mut s.show_device_format_text)?,
                "ShowDeviceCodecText" => e.read_bool(&mut s.show_device_codec_text)?,
                "CaptureActivityIndicator" => e.read_enum(&mut s.capture_activity_indicator)?,
                "RecordingAppDrawerDisplayType" => e.read_enum(&mut s.recording_app_drawer_display_type)?,
                "AppDrawerIconsCenterMode" => e.read_enum(&mut s.app_drawer_icons_center_mode)?,
                "AppDrawerIconsCenterSoftMax" => e.read_i32(&mut s.app_drawer_icons_center_soft_max)?,
                "AppDrawerIconScalePercent" => e.read_i32(&mut s.app_drawer_icon_scale_percent)?,
                "AppDrawerIconsPerRow" => e.read_i32(&mut s.app_drawer_icons_per_row)?,
                "AppDrawerStackDirection" => e.read_enum(&mut s.app_drawer_stack_direction)?,
                "PlaybackAppDrawerSlidersMaxApps" => e.read_i32(&mut s.playback_app_drawer_sliders_max_apps)?,
                "PlaybackAppDrawerIconsMaxRows" => e.read_i32(&mut s.playback_app_drawer_icons_max_rows)?,
                "RecordingAppDrawerSlidersMaxApps" => e.read_i32(&mut s.recording_app_drawer_sliders_max_apps)?,
                "RecordingAppDrawerIconsMaxRows" => e.read_i32(&mut s.recording_app_drawer_icons_max_rows)?,
                _ => {}
            }
        }
        s.slider_thumb = resolve_slider_thumb(slider_thumb);
        s.clamp_all();
        Ok((s, dropped_hotkeys))
    }
}

impl XmlDocument for AppSettings {
    const ROOT: &'static str = tadn_common::settings::SETTINGS_ROOT_ELEMENT;

    fn read_xml(root: &XmlElement) -> Self {
        Self::try_read_xml(root).unwrap_or_else(|error| {
            log::warn!("settings.xml content rejected: {error}");
            Self::default()
        })
    }

    fn try_read_xml(root: &XmlElement) -> Result<Self, XmlError> {
        Self::read_xml_checked(root).map(|(settings, _)| settings)
    }

    /// `NormalizeLoadedSettings`: hotkey dedupe (Volume ships no default bindings).
    fn normalize(&mut self) -> bool {
        hotkey::normalize(&mut self.common.hotkeys)
    }

    fn read_normalized(root: &XmlElement) -> Result<(Self, bool), XmlError> {
        let (mut settings, dropped_hotkeys) = Self::read_xml_checked(root)?;
        let normalized = settings.normalize();
        Ok((settings, dropped_hotkeys || normalized))
    }

    fn write_xml(&self, w: &mut XmlWriter) {
        let s = self;
        s.common.write_xml(w);
        w.element_bool("SetDefaultCommsToDefault", s.set_default_comms_to_default);
        w.element_bool("ShowDisabledPlaybackDevices", s.show_disabled_playback_devices);
        w.element_bool("ShowDefaultPlaybackDeviceEvenIfDisabled", s.show_default_playback_device_even_if_disabled);
        w.element_bool(
            "ShowDefaultCommsPlaybackDeviceEvenIfDisabled",
            s.show_default_comms_playback_device_even_if_disabled,
        );
        w.element_bool("ShowDisconnectedPlaybackDevices", s.show_disconnected_playback_devices);
        w.element_bool("ShowRecordingDevices", s.show_recording_devices);
        w.element_bool("ShowDisabledRecordingDevices", s.show_disabled_recording_devices);
        w.element_bool("ShowDefaultRecordingDeviceEvenIfDisabled", s.show_default_recording_device_even_if_disabled);
        w.element_bool(
            "ShowDefaultCommsRecordingDeviceEvenIfDisabled",
            s.show_default_comms_recording_device_even_if_disabled,
        );
        w.element_bool("ShowDisconnectedRecordingDevices", s.show_disconnected_recording_devices);
        w.element_bool("DefaultAppDrawerExpanded", s.default_app_drawer_expanded);
        w.element_opt("LastKnownDefaultPlaybackDeviceID", s.last_known_default_playback_device_id.as_deref());
        w.element_opt(
            "LastKnownDefaultCommsPlaybackDeviceID",
            s.last_known_default_comms_playback_device_id.as_deref(),
        );
        w.element_opt("LastKnownDefaultRecordingDeviceID", s.last_known_default_recording_device_id.as_deref());
        w.element_opt(
            "LastKnownDefaultCommsRecordingDeviceID",
            s.last_known_default_comms_recording_device_id.as_deref(),
        );
        w.element_bool("ShowNotPresentDevices", s.show_not_present_devices);
        w.element_bool("ActivateRecordingDevicesForPeakMeters", s.activate_recording_devices_for_peak_meters);
        w.element_bool("ShowTrayMenuRecordingLink", s.show_tray_menu_recording_link);
        w.element_bool("ShowTrayMenuSoundsLink", s.show_tray_menu_sounds_link);
        w.element_bool("ShowTrayMenuCommunicationsLink", s.show_tray_menu_communications_link);
        w.element_bool("ShowTrayMenuDeviceLinks", s.show_tray_menu_device_links);
        w.element_bool("UseLogarithmicVolumeScale", s.use_logarithmic_volume_scale);
        w.element_bool("PlayDeviceVolumeChangeSound", s.play_device_volume_change_sound);
        w.element_bool(
            "SuppressDeviceVolumeChangeSoundWhenAudioPlaying",
            s.suppress_device_volume_change_sound_when_audio_playing,
        );
        w.element_i32("DingSuppressionPeakThresholdPercent", s.ding_suppression_peak_threshold_percent);
        w.element_bool("PlayAppVolumeChangeSound", s.play_app_volume_change_sound);
        w.element_enum("TrayMenuPlaybackDeviceNameStyle", s.tray_menu_playback_device_name_style);
        w.element_enum("TrayMenuRecordingDeviceNameStyle", s.tray_menu_recording_device_name_style);
        w.element_i32("TrayMenuDeviceNameMaxLength", s.tray_menu_device_name_max_length);
        w.element_enum("TrayIconStyle", s.tray_icon_style);
        w.element_bool("UnifiedPeakMeter", s.unified_peak_meter);
        w.element_i32("UnifiedMeterLowChannelBiasMultiplier", s.unified_meter_low_channel_bias_multiplier);
        w.element_i32("MeterPeakFps", s.meter_peak_fps);
        w.element_i32("MeterPeakSampleRate", s.meter_peak_sample_rate);
        w.element_i32("MeterPeakChangeCeiling", s.meter_peak_change_ceiling);
        w.element_i32("IconRetryIntervalMs", s.icon_retry_interval_ms);
        w.element_i32("IconLRULimit", s.icon_lru_limit);
        write_meter_color(w, "MeterPeakColorHex", s.meter_peak_color, METER_PEAK_COLOR_DEFAULT);
        write_meter_color(w, "MeterPeakStereoColorHex", s.meter_peak_stereo_color, METER_PEAK_STEREO_COLOR_DEFAULT);
        write_slider_thumb(w, &s.slider_thumb);
        w.element_bool("TrayScrollEnabled", s.tray_scroll_enabled);
        w.element_bool("PlayTrayScrollVolumeChangeSound", s.play_tray_scroll_volume_change_sound);
        w.element_i32("WheelVolumeStepPercent", s.wheel_volume_step_percent);
        w.element_i32("WheelVolumeStepFinePercent", s.wheel_volume_step_fine_percent);
        w.element_i32("WheelVolumeStepCoarsePercent", s.wheel_volume_step_coarse_percent);
        w.element_enum("TrayWheelAction", s.tray_wheel_action);
        w.element_enum("TrayCtrlWheelAction", s.tray_ctrl_wheel_action);
        w.element_enum("TrayAltWheelAction", s.tray_alt_wheel_action);
        w.element_bool("PrecisionTouchpadScrollEnabled", s.precision_touchpad_scroll_enabled);
        w.element_i32("PrecisionTouchpadUnitsPerScrollStep", s.precision_touchpad_units_per_scroll_step);
        w.element_enum("TrayDoubleClickAction", s.tray_double_click_action);
        w.element_enum("TrayCtrlLeftClickAction", s.tray_ctrl_left_click_action);
        w.element_enum("TrayAltLeftClickAction", s.tray_alt_left_click_action);
        w.element_enum("TrayCtrlRightClickAction", s.tray_ctrl_right_click_action);
        w.element_enum("TrayAltRightClickAction", s.tray_alt_right_click_action);
        w.element_enum("TrayCtrlDoubleLeftClickAction", s.tray_ctrl_double_left_click_action);
        w.element_enum("TrayAltDoubleLeftClickAction", s.tray_alt_double_left_click_action);
        w.element_bool("AllowFlyoutUndock", s.allow_flyout_undock);
        w.element_bool("ClampUndockedFlyoutToScreen", s.clamp_undocked_flyout_to_screen);
        w.element_bool("RestoreFlyoutUndockedOnStartup", s.restore_flyout_undocked_on_startup);
        w.element_bool("FlyoutUndocked", s.flyout_undocked);
        w.element_bool("FlyoutHasSavedPosition", s.flyout_has_saved_position);
        w.element_f64("FlyoutLeft", s.flyout_left);
        w.element_f64("FlyoutTop", s.flyout_top);
        w.element_bool("FlyoutHeaderAtBottom", s.flyout_header_at_bottom);
        w.element_enum("SoundSettingsTarget", s.sound_settings_target);
        w.element_enum("FlyoutDeviceLayout", s.flyout_device_layout);
        w.element_enum("FlyoutDeviceTitlePosition", s.flyout_device_title_position);
        w.element_enum("FlyoutDeviceSort", s.flyout_device_sort);
        w.element_enum(
            "FlyoutDisconnectedBluetoothDeviceVisibility",
            s.flyout_disconnected_bluetooth_device_visibility,
        );
        w.element_bool(
            "CollapseDisconnectedBluetoothDevicesInFlyout",
            s.collapse_disconnected_bluetooth_devices_in_flyout,
        );
        w.element_bool("ShowBluetoothDevicesOnlyWhenBluetoothIsOn", s.show_bluetooth_devices_only_when_bluetooth_is_on);
        w.element_bool("ShowRecordingDevicesInFlyout", s.show_recording_devices_in_flyout);
        w.element_bool("IntermixRecordingWithPlaybackInFlyout", s.intermix_recording_with_playback_in_flyout);
        w.element_bool("UseDynamicPlaybackVolumeGlyphInFlyout", s.use_dynamic_playback_volume_glyph_in_flyout);
        w.element_bool("ShowBluetoothRadioButtonInFlyoutHeader", s.show_bluetooth_radio_button_in_flyout_header);
        w.element_enum("FlyoutBluetoothRadioButtonClickGesture", s.flyout_bluetooth_radio_button_click_gesture);
        w.element_enum("FlyoutCommunicationsButtonVisibility", s.flyout_communications_button_visibility);
        w.element_bool("ShowLockButtonForPlayback", s.show_lock_button_for_playback);
        w.element_bool("ShowEqualizerAPOButtonForPlayback", s.show_equalizer_apo_button_for_playback);
        w.element_bool("ShowDefaultDeviceButtonForPlayback", s.show_default_device_button_for_playback);
        w.element_bool("ShowBatteryButtonForPlayback", s.show_battery_button_for_playback);
        w.element_bool("ShowLockButtonForRecording", s.show_lock_button_for_recording);
        w.element_bool("ShowEqualizerAPOButtonForRecording", s.show_equalizer_apo_button_for_recording);
        w.element_bool("ShowListenButtonForRecording", s.show_listen_button_for_recording);
        w.element_bool("ShowDefaultDeviceButtonForRecording", s.show_default_device_button_for_recording);
        w.element_bool("ShowBatteryButtonForRecording", s.show_battery_button_for_recording);
        w.element_bool("ShowDeviceFormatText", s.show_device_format_text);
        w.element_bool("ShowDeviceCodecText", s.show_device_codec_text);
        w.element_enum("CaptureActivityIndicator", s.capture_activity_indicator);
        w.element_enum("RecordingAppDrawerDisplayType", s.recording_app_drawer_display_type);
        w.element_enum("AppDrawerIconsCenterMode", s.app_drawer_icons_center_mode);
        w.element_i32("AppDrawerIconsCenterSoftMax", s.app_drawer_icons_center_soft_max);
        w.element_i32("AppDrawerIconScalePercent", s.app_drawer_icon_scale_percent);
        w.element_i32("AppDrawerIconsPerRow", s.app_drawer_icons_per_row);
        w.element_enum("AppDrawerStackDirection", s.app_drawer_stack_direction);
        w.element_i32("PlaybackAppDrawerSlidersMaxApps", s.playback_app_drawer_sliders_max_apps);
        w.element_i32("PlaybackAppDrawerIconsMaxRows", s.playback_app_drawer_icons_max_rows);
        w.element_i32("RecordingAppDrawerSlidersMaxApps", s.recording_app_drawer_sliders_max_apps);
        w.element_i32("RecordingAppDrawerIconsMaxRows", s.recording_app_drawer_icons_max_rows);
    }
}

impl XmlDocument for DeviceSettings {
    const ROOT: &'static str = DEVICES_ROOT_ELEMENT;

    /// Inline `<Device Id=".." IsAppDrawerExpanded=".." CustomFriendlyName=".." />` rows; missing
    /// attributes keep the `DeviceSettingsEntry` initializers (empty id, expanded, no custom name).
    fn read_xml(root: &XmlElement) -> Self {
        let devices = root
            .children_named("Device")
            .map(|e| {
                let mut entry = DeviceEntry { is_app_drawer_expanded: true, ..DeviceEntry::default() };
                if let Some(id) = e.attr("Id") {
                    id.clone_into(&mut entry.id);
                }
                if let Some(expanded) = e.attr("IsAppDrawerExpanded").and_then(tadn_common::xml::parse_bool) {
                    entry.is_app_drawer_expanded = expanded;
                }
                if let Some(name) = e.attr("CustomFriendlyName") {
                    name.clone_into(&mut entry.custom_friendly_name);
                }
                entry
            })
            .collect();
        Self { devices }
    }

    fn write_xml(&self, writer: &mut XmlWriter) {
        for device in &self.devices {
            writer.start_element("Device");
            writer.attribute("Id", &device.id);
            writer.attribute("IsAppDrawerExpanded", tadn_common::xml::format_bool(device.is_app_drawer_expanded));
            writer.attribute("CustomFriendlyName", &device.custom_friendly_name);
            writer.end_element();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tadn_common::hotkey::{HotkeyAction, HotkeyBinding};
    use tadn_common::settings::ThemeMode;
    use tadn_common::theme::{Argb, NullableThemeColor};
    use tadn_common::xml::{LoadState, XmlFileStore, deserialize, serialize};

    use super::*;
    use crate::settings::{FlyoutDeviceSort, TrayClickAction};

    /// A settings.xml shaped exactly like one the .NET app writes (BOM, declaration, CRLF, 2-space
    /// indent, element order), with non-default values in every value kind. Device ids are fake.
    const FIXTURE_LINES: &[&str] = &[
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>",
        "<AppSettings>",
        "  <RunOnStartup>false</RunOnStartup>",
        "  <Autosave>true</Autosave>",
        "  <ThemeMode>Dark</ThemeMode>",
        "  <TextColor>",
        "    <LightHex>#102030</LightHex>",
        "    <DarkHex />",
        "  </TextColor>",
        "  <BackgroundColor>",
        "    <LightHex />",
        "    <DarkHex>#80405060</DarkHex>",
        "  </BackgroundColor>",
        "  <TrayIconColor>",
        "    <LightHex />",
        "    <DarkHex />",
        "  </TrayIconColor>",
        "  <EnableRoundedCorners>false</EnableRoundedCorners>",
        "  <UseWindows11SettingsNavigation>true</UseWindows11SettingsNavigation>",
        "  <SettingsSidebarWidth>262.75</SettingsSidebarWidth>",
        "  <AnimationMode>Disabled</AnimationMode>",
        "  <ToolTipShowDelayMs>500</ToolTipShowDelayMs>",
        "  <ContextMenuPosition>Classic</ContextMenuPosition>",
        "  <ContextMenuFontSize>17</ContextMenuFontSize>",
        "  <UseSystemSubmenuShowDelay>true</UseSystemSubmenuShowDelay>",
        "  <SubmenuShowDelayMs>300</SubmenuShowDelayMs>",
        "  <RenderingBackend>Software</RenderingBackend>",
        "  <Language>en-US</Language>",
        "  <CheckForUpdatesEnabled>false</CheckForUpdatesEnabled>",
        "  <ShowUpdateNotificationsEnabled>true</ShowUpdateNotificationsEnabled>",
        "  <ShowUpdateButtonInFlyout>false</ShowUpdateButtonInFlyout>",
        "  <UpdateCheckIntervalMs>7200000</UpdateCheckIntervalMs>",
        "  <SkippedUpdateVersion>12</SkippedUpdateVersion>",
        "  <KeepFlyoutWarm>false</KeepFlyoutWarm>",
        "  <KeepTrayContextMenuWarm>false</KeepTrayContextMenuWarm>",
        "  <Hotkeys>",
        "    <Binding Action=\"OpenFlyout\" Parameter=\"\" Modifiers=\"3\" VirtualKey=\"86\" Enabled=\"true\" BindingID=\"0\" RemovedByUser=\"false\" />",
        "    <Binding Action=\"OpenSettings\" Parameter=\"a&amp;b\" Modifiers=\"6\" VirtualKey=\"112\" Enabled=\"false\" BindingID=\"1\" RemovedByUser=\"true\" />",
        "  </Hotkeys>",
        "  <SetDefaultCommsToDefault>true</SetDefaultCommsToDefault>",
        "  <ShowDisabledPlaybackDevices>true</ShowDisabledPlaybackDevices>",
        "  <ShowDefaultPlaybackDeviceEvenIfDisabled>false</ShowDefaultPlaybackDeviceEvenIfDisabled>",
        "  <ShowDefaultCommsPlaybackDeviceEvenIfDisabled>false</ShowDefaultCommsPlaybackDeviceEvenIfDisabled>",
        "  <ShowDisconnectedPlaybackDevices>true</ShowDisconnectedPlaybackDevices>",
        "  <ShowRecordingDevices>false</ShowRecordingDevices>",
        "  <ShowDisabledRecordingDevices>true</ShowDisabledRecordingDevices>",
        "  <ShowDefaultRecordingDeviceEvenIfDisabled>false</ShowDefaultRecordingDeviceEvenIfDisabled>",
        "  <ShowDefaultCommsRecordingDeviceEvenIfDisabled>false</ShowDefaultCommsRecordingDeviceEvenIfDisabled>",
        "  <ShowDisconnectedRecordingDevices>true</ShowDisconnectedRecordingDevices>",
        "  <DefaultAppDrawerExpanded>false</DefaultAppDrawerExpanded>",
        "  <LastKnownDefaultPlaybackDeviceID>{0.0.0.00000000}.{11111111-2222-3333-4444-555555555555}</LastKnownDefaultPlaybackDeviceID>",
        "  <LastKnownDefaultCommsPlaybackDeviceID>{0.0.0.00000000}.{11111111-2222-3333-4444-555555555555}</LastKnownDefaultCommsPlaybackDeviceID>",
        "  <LastKnownDefaultRecordingDeviceID>{0.0.1.00000000}.{66666666-7777-8888-9999-000000000000}</LastKnownDefaultRecordingDeviceID>",
        "  <LastKnownDefaultCommsRecordingDeviceID />",
        "  <ShowNotPresentDevices>true</ShowNotPresentDevices>",
        "  <ActivateRecordingDevicesForPeakMeters>true</ActivateRecordingDevicesForPeakMeters>",
        "  <ShowTrayMenuRecordingLink>true</ShowTrayMenuRecordingLink>",
        "  <ShowTrayMenuSoundsLink>true</ShowTrayMenuSoundsLink>",
        "  <ShowTrayMenuCommunicationsLink>true</ShowTrayMenuCommunicationsLink>",
        "  <ShowTrayMenuDeviceLinks>true</ShowTrayMenuDeviceLinks>",
        "  <UseLogarithmicVolumeScale>true</UseLogarithmicVolumeScale>",
        "  <PlayDeviceVolumeChangeSound>false</PlayDeviceVolumeChangeSound>",
        "  <SuppressDeviceVolumeChangeSoundWhenAudioPlaying>false</SuppressDeviceVolumeChangeSoundWhenAudioPlaying>",
        "  <DingSuppressionPeakThresholdPercent>12</DingSuppressionPeakThresholdPercent>",
        "  <PlayAppVolumeChangeSound>false</PlayAppVolumeChangeSound>",
        "  <TrayMenuPlaybackDeviceNameStyle>Name</TrayMenuPlaybackDeviceNameStyle>",
        "  <TrayMenuRecordingDeviceNameStyle>Model</TrayMenuRecordingDeviceNameStyle>",
        "  <TrayMenuDeviceNameMaxLength>40</TrayMenuDeviceNameMaxLength>",
        "  <TrayIconStyle>Static</TrayIconStyle>",
        "  <UnifiedPeakMeter>false</UnifiedPeakMeter>",
        "  <UnifiedMeterLowChannelBiasMultiplier>4</UnifiedMeterLowChannelBiasMultiplier>",
        "  <MeterPeakFps>120</MeterPeakFps>",
        "  <MeterPeakSampleRate>60</MeterPeakSampleRate>",
        "  <MeterPeakChangeCeiling>10</MeterPeakChangeCeiling>",
        "  <IconRetryIntervalMs>300</IconRetryIntervalMs>",
        "  <IconLRULimit>20</IconLRULimit>",
        "  <MeterPeakColorHex>#102030</MeterPeakColorHex>",
        "  <MeterPeakStereoColorHex>#80FFFFFF</MeterPeakStereoColorHex>",
        "  <SliderThumb>",
        "    <Name>Custom</Name>",
        "    <Glyph>\u{E7E8}</Glyph>",
        "    <FontFamily>Segoe MDL2 Assets</FontFamily>",
        "    <FontSize>17.5</FontSize>",
        "    <Width>12</Width>",
        "    <Height>20</Height>",
        "    <XScale>0.85</XScale>",
        "    <Shape>Glyph</Shape>",
        "  </SliderThumb>",
        "  <TrayScrollEnabled>false</TrayScrollEnabled>",
        "  <PlayTrayScrollVolumeChangeSound>false</PlayTrayScrollVolumeChangeSound>",
        "  <WheelVolumeStepPercent>3</WheelVolumeStepPercent>",
        "  <WheelVolumeStepFinePercent>2</WheelVolumeStepFinePercent>",
        "  <WheelVolumeStepCoarsePercent>10</WheelVolumeStepCoarsePercent>",
        "  <TrayWheelAction>Fine</TrayWheelAction>",
        "  <TrayCtrlWheelAction>Coarse</TrayCtrlWheelAction>",
        "  <TrayAltWheelAction>Nothing</TrayAltWheelAction>",
        "  <PrecisionTouchpadScrollEnabled>false</PrecisionTouchpadScrollEnabled>",
        "  <PrecisionTouchpadUnitsPerScrollStep>120</PrecisionTouchpadUnitsPerScrollStep>",
        "  <TrayDoubleClickAction>OpenSettings</TrayDoubleClickAction>",
        "  <TrayCtrlLeftClickAction>OpenSettings</TrayCtrlLeftClickAction>",
        "  <TrayAltLeftClickAction>Nothing</TrayAltLeftClickAction>",
        "  <TrayCtrlRightClickAction>OpenSettings</TrayCtrlRightClickAction>",
        "  <TrayAltRightClickAction>Nothing</TrayAltRightClickAction>",
        "  <TrayCtrlDoubleLeftClickAction>OpenSettings</TrayCtrlDoubleLeftClickAction>",
        "  <TrayAltDoubleLeftClickAction>Nothing</TrayAltDoubleLeftClickAction>",
        "  <AllowFlyoutUndock>false</AllowFlyoutUndock>",
        "  <ClampUndockedFlyoutToScreen>false</ClampUndockedFlyoutToScreen>",
        "  <RestoreFlyoutUndockedOnStartup>false</RestoreFlyoutUndockedOnStartup>",
        "  <FlyoutUndocked>false</FlyoutUndocked>",
        "  <FlyoutHasSavedPosition>true</FlyoutHasSavedPosition>",
        "  <FlyoutLeft>-1234.5</FlyoutLeft>",
        "  <FlyoutTop>783</FlyoutTop>",
        "  <FlyoutHeaderAtBottom>true</FlyoutHeaderAtBottom>",
        "  <SoundSettingsTarget>WindowsSettingsApp</SoundSettingsTarget>",
        "  <FlyoutDeviceLayout>AppsBelowDevice</FlyoutDeviceLayout>",
        "  <FlyoutDeviceTitlePosition>AboveSlider</FlyoutDeviceTitlePosition>",
        "  <FlyoutDeviceSort>WindowsEnumeration</FlyoutDeviceSort>",
        "  <FlyoutDisconnectedBluetoothDeviceVisibility>Show</FlyoutDisconnectedBluetoothDeviceVisibility>",
        "  <CollapseDisconnectedBluetoothDevicesInFlyout>false</CollapseDisconnectedBluetoothDevicesInFlyout>",
        "  <ShowBluetoothDevicesOnlyWhenBluetoothIsOn>false</ShowBluetoothDevicesOnlyWhenBluetoothIsOn>",
        "  <ShowRecordingDevicesInFlyout>false</ShowRecordingDevicesInFlyout>",
        "  <IntermixRecordingWithPlaybackInFlyout>true</IntermixRecordingWithPlaybackInFlyout>",
        "  <UseDynamicPlaybackVolumeGlyphInFlyout>true</UseDynamicPlaybackVolumeGlyphInFlyout>",
        "  <ShowBluetoothRadioButtonInFlyoutHeader>false</ShowBluetoothRadioButtonInFlyoutHeader>",
        "  <FlyoutBluetoothRadioButtonClickGesture>ShiftLeftClick</FlyoutBluetoothRadioButtonClickGesture>",
        "  <FlyoutCommunicationsButtonVisibility>Hidden</FlyoutCommunicationsButtonVisibility>",
        "  <ShowLockButtonForPlayback>true</ShowLockButtonForPlayback>",
        "  <ShowEqualizerAPOButtonForPlayback>true</ShowEqualizerAPOButtonForPlayback>",
        "  <ShowDefaultDeviceButtonForPlayback>false</ShowDefaultDeviceButtonForPlayback>",
        "  <ShowBatteryButtonForPlayback>false</ShowBatteryButtonForPlayback>",
        "  <ShowLockButtonForRecording>true</ShowLockButtonForRecording>",
        "  <ShowEqualizerAPOButtonForRecording>true</ShowEqualizerAPOButtonForRecording>",
        "  <ShowListenButtonForRecording>true</ShowListenButtonForRecording>",
        "  <ShowDefaultDeviceButtonForRecording>false</ShowDefaultDeviceButtonForRecording>",
        "  <ShowBatteryButtonForRecording>false</ShowBatteryButtonForRecording>",
        "  <ShowDeviceFormatText>false</ShowDeviceFormatText>",
        "  <ShowDeviceCodecText>true</ShowDeviceCodecText>",
        "  <CaptureActivityIndicator>HideInactive</CaptureActivityIndicator>",
        "  <RecordingAppDrawerDisplayType>Sliders</RecordingAppDrawerDisplayType>",
        "  <AppDrawerIconsCenterMode>CenteredSoftMax</AppDrawerIconsCenterMode>",
        "  <AppDrawerIconsCenterSoftMax>5</AppDrawerIconsCenterSoftMax>",
        "  <AppDrawerIconScalePercent>125</AppDrawerIconScalePercent>",
        "  <AppDrawerIconsPerRow>8</AppDrawerIconsPerRow>",
        "  <AppDrawerStackDirection>LeftRight</AppDrawerStackDirection>",
        "  <PlaybackAppDrawerSlidersMaxApps>20</PlaybackAppDrawerSlidersMaxApps>",
        "  <PlaybackAppDrawerIconsMaxRows>6</PlaybackAppDrawerIconsMaxRows>",
        "  <RecordingAppDrawerSlidersMaxApps>18</RecordingAppDrawerSlidersMaxApps>",
        "  <RecordingAppDrawerIconsMaxRows>4</RecordingAppDrawerIconsMaxRows>",
        "</AppSettings>",
    ];

    fn fixture() -> Vec<u8> {
        format!("\u{FEFF}{}", FIXTURE_LINES.join("\r\n")).into_bytes()
    }

    fn read(xml: &str) -> AppSettings {
        deserialize(xml.as_bytes()).unwrap_or_else(|e| panic!("{e}"))
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vtadn-model-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_round_trip_is_byte_identical() {
        let bytes = fixture();
        let settings: AppSettings = deserialize(&bytes).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(serialize(&settings), bytes);

        assert!(!settings.common.run_on_startup);
        assert_eq!(settings.common.theme_mode, ThemeMode::Dark);
        assert_eq!(settings.common.text_color, NullableThemeColor { light: Some(Argb(0xFF10_2030)), dark: None });
        assert_eq!(settings.common.background_color.dark, Some(Argb(0x8040_5060)));
        assert_eq!(settings.common.settings_sidebar_width, 262.75);
        assert_eq!(settings.common.language, "en-US");
        assert_eq!(settings.common.hotkeys.len(), 2);
        assert_eq!(settings.common.hotkeys[1].parameter, "a&b");
        assert_eq!(settings.last_known_default_comms_recording_device_id, None);
        assert_eq!(settings.meter_peak_color, Some(Argb(0xFF10_2030)));
        assert_eq!(settings.meter_peak_stereo_color, None);
        assert_eq!(settings.slider_thumb.name, "Custom");
        assert_eq!(settings.slider_thumb.x_scale, 0.85);
        assert_eq!(settings.flyout_left, -1234.5);
        assert_eq!(settings.flyout_device_sort, FlyoutDeviceSort::WindowsEnumeration);
        assert_eq!(settings.tray_double_click_action, TrayClickAction::OpenSettings);
    }

    #[test]
    fn defaults_round_trip_and_match_the_dotnet_defaults_file_shape() {
        let defaults = AppSettings::default();
        let bytes = serialize(&defaults);
        let text = String::from_utf8(bytes.clone()).unwrap_or_default();
        assert!(text.starts_with("\u{FEFF}<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<AppSettings>\r\n  <RunOnStartup>true</RunOnStartup>\r\n"));
        assert!(text.contains("\r\n  <Hotkeys />\r\n  <SetDefaultCommsToDefault>false</SetDefaultCommsToDefault>\r\n"));
        assert!(text.contains("\r\n  <LastKnownDefaultPlaybackDeviceID />\r\n"));
        assert!(text.contains("<MeterPeakColorHex>#FFFFFFFF</MeterPeakColorHex>\r\n  <MeterPeakStereoColorHex>#80FFFFFF</MeterPeakStereoColorHex>"));
        assert!(text.contains("  <SliderThumb>\r\n    <Name>Capsule</Name>\r\n    <Glyph>\u{E91F}</Glyph>\r\n"));
        assert!(text.contains("<FlyoutUndocked>true</FlyoutUndocked>"));
        assert!(
            text.ends_with("<RecordingAppDrawerIconsMaxRows>10</RecordingAppDrawerIconsMaxRows>\r\n</AppSettings>")
        );
        assert_eq!(deserialize::<AppSettings>(&bytes).ok(), Some(defaults));
    }

    #[test]
    fn unknown_elements_and_invalid_values_keep_defaults_and_clamps_apply() {
        let settings = read(
            "<AppSettings><FutureSetting>1</FutureSetting><MeterPeakFps>fast</MeterPeakFps>\
             <ThemeMode>Purple</ThemeMode><ShowRecordingDevices>yes</ShowRecordingDevices>\
             <TrayMenuDeviceNameMaxLength>1</TrayMenuDeviceNameMaxLength><IconLRULimit>5000</IconLRULimit>\
             <ContextMenuFontSize>99</ContextMenuFontSize><UpdateCheckIntervalMs>-5</UpdateCheckIntervalMs>\
             <FlyoutLeft>1,5</FlyoutLeft><AppDrawerStackDirection>1</AppDrawerStackDirection>\
             <MeterPeakFps>60</MeterPeakFps></AppSettings>",
        );
        let defaults = AppSettings::default();
        assert_eq!(settings.common.theme_mode, defaults.common.theme_mode);
        assert_eq!(settings.show_recording_devices, defaults.show_recording_devices);
        assert_eq!(settings.flyout_left, 0.0);
        assert_eq!(settings.meter_peak_fps, 60, "the last of repeated elements wins");
        assert_eq!(settings.tray_menu_device_name_max_length, 3);
        assert_eq!(settings.icon_lru_limit, 1000);
        assert_eq!(settings.common.context_menu_font_size, 48);
        assert_eq!(settings.common.update_check_interval_ms, 3_600_000);
        assert_eq!(settings.app_drawer_stack_direction, crate::settings::AppDrawerStackDirection::BottomTop);
        assert!(!String::from_utf8(serialize(&settings)).unwrap_or_default().contains("FutureSetting"));
    }

    #[test]
    fn scalar_element_with_children_marks_the_file_corrupt() {
        assert!(deserialize::<AppSettings>(b"<AppSettings><Language><x /></Language></AppSettings>").is_err());
        assert!(deserialize::<AppSettings>(b"<Settings />").is_err());
        assert!(deserialize::<AppSettings>(b"<AppSettings />").is_ok());
    }

    #[test]
    fn meter_colors_follow_the_hex_setter_rules() {
        for (text, expected) in [
            ("", None),
            ("   ", None),
            ("#ffffffff", None),
            ("#FFFFFFFF", None),
            ("#102030", Some(Argb(0xFF10_2030))),
            ("#FFFFFF", Some(Argb(0xFFFF_FFFF))),
            ("not a color", None),
        ] {
            let settings = read(&format!("<AppSettings><MeterPeakColorHex>{text}</MeterPeakColorHex></AppSettings>"));
            assert_eq!(settings.meter_peak_color, expected, "{text:?}");
        }
        let stereo = read("<AppSettings><MeterPeakStereoColorHex>#80ffffff</MeterPeakStereoColorHex></AppSettings>");
        assert_eq!(stereo.meter_peak_stereo_color, None);
        assert_eq!(stereo.effective_meter_peak_stereo_color(), Argb(0x80FF_FFFF));
        let custom = read("<AppSettings><MeterPeakStereoColorHex>#80405060</MeterPeakStereoColorHex></AppSettings>");
        assert_eq!(custom.effective_meter_peak_stereo_color(), Argb(0x8040_5060));
        let written = String::from_utf8(serialize(&custom)).unwrap_or_default();
        assert!(written.contains("<MeterPeakStereoColorHex>#80405060</MeterPeakStereoColorHex>"));
    }

    #[test]
    fn slider_thumb_selection_follows_the_catalog_rules() {
        let builtin =
            read("<AppSettings><SliderThumb><Name>Star</Name><FontSize>99</FontSize></SliderThumb></AppSettings>");
        assert_eq!(
            builtin.slider_thumb,
            SliderThumbOption::default_catalog()[3],
            "a built-in name selects the built-in"
        );
        let unnamed = read("<AppSettings><SliderThumb><Width>5</Width></SliderThumb></AppSettings>");
        assert_eq!(unnamed.slider_thumb.name, "Circle", "a missing Name keeps the option initializer");
        let empty = read("<AppSettings><SliderThumb><Name /></SliderThumb></AppSettings>");
        assert_eq!(empty.slider_thumb, SliderThumbOption::default());
        let custom = read("<AppSettings><SliderThumb><Name>Mine</Name><Width>5</Width></SliderThumb></AppSettings>");
        assert_eq!(
            (custom.slider_thumb.name.as_str(), custom.slider_thumb.width, custom.slider_thumb.height),
            ("Mine", 5.0, 18.0)
        );
        assert_eq!(SliderThumbOption::catalog_with(&custom.slider_thumb).len(), 7);
    }

    #[test]
    fn hotkeys_with_an_unknown_action_and_duplicates_are_dropped_on_load_and_saved_once() {
        let dir = temp_dir("hotkeys");
        let path = dir.join("settings.xml");
        std::fs::write(
            &path,
            "<AppSettings>\r\n  <Hotkeys>\r\n\
             <Binding Action=\"OpenFlyout\" Parameter=\"\" Modifiers=\"3\" VirtualKey=\"70\" BindingID=\"0\" />\r\n\
             <Binding Action=\"OpenFlyout\" Parameter=\"\" Modifiers=\"6\" VirtualKey=\"71\" BindingID=\"0\" />\r\n\
             <Binding Action=\"ActionFromANewerBuild\" Parameter=\"\" Modifiers=\"3\" VirtualKey=\"72\" BindingID=\"0\" />\r\n\
             </Hotkeys>\r\n</AppSettings>",
        )
        .unwrap_or_default();
        let (store, settings, state) = XmlFileStore::<AppSettings>::open(path.clone());
        store.flush();
        drop(store);
        assert_eq!(state, LoadState::Loaded);
        assert_eq!(
            settings.common.hotkeys,
            vec![HotkeyBinding {
                action: HotkeyAction::OpenFlyout,
                modifiers: 3,
                virtual_key: 70,
                ..HotkeyBinding::default()
            }]
        );
        let saved: AppSettings = deserialize(&std::fs::read(&path).unwrap_or_default()).unwrap_or_default();
        assert_eq!(saved.common.hotkeys, settings.common.hotkeys);
        cleanup(&dir);
    }

    #[test]
    fn loading_schedules_no_save_and_edits_save_through_the_store() {
        let dir = temp_dir("save-request");
        let path = dir.join("settings.xml");
        let (store, mut settings, state) = XmlFileStore::<AppSettings>::open(path.clone());
        store.flush();
        assert_eq!(state, LoadState::Missing);
        let written = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        store.flush();
        assert_eq!(std::fs::metadata(&path).and_then(|m| m.modified()).ok(), written);

        settings.show_recording_devices_in_flyout = !settings.show_recording_devices_in_flyout;
        store.schedule_save(&settings);
        store.flush();
        let saved: AppSettings = deserialize(&std::fs::read(&path).unwrap_or_default()).unwrap_or_default();
        assert_eq!(saved.show_recording_devices_in_flyout, settings.show_recording_devices_in_flyout);
        drop(store);
        cleanup(&dir);
    }

    #[test]
    fn devices_round_trip_is_byte_identical() {
        let lines = [
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>",
            "<Devices>",
            "  <Device Id=\"{0.0.0.00000000}.{11111111-2222-3333-4444-555555555555}\" IsAppDrawerExpanded=\"false\" CustomFriendlyName=\"\" />",
            "  <Device Id=\"{0.0.1.00000000}.{66666666-7777-8888-9999-000000000000}\" IsAppDrawerExpanded=\"true\" CustomFriendlyName=\"My &quot;Buds&quot; &amp; Co\" />",
            "</Devices>",
        ];
        let bytes = format!("\u{FEFF}{}", lines.join("\r\n")).into_bytes();
        let devices: DeviceSettings = deserialize(&bytes).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(devices.devices.len(), 2);
        assert_eq!(devices.devices[1].custom_friendly_name, "My \"Buds\" & Co");
        assert_eq!(serialize(&devices), bytes);
        let empty = String::from_utf8(serialize(&DeviceSettings::default())).unwrap_or_default();
        assert!(empty.ends_with("\r\n<Devices />"));
        let partial: DeviceSettings =
            deserialize(b"<Devices><Device Id=\"x\" /><Other /></Devices>").unwrap_or_default();
        assert_eq!(
            partial.devices,
            vec![DeviceEntry { id: "x".into(), is_app_drawer_expanded: true, custom_friendly_name: String::new() }]
        );
    }

    #[test]
    fn custom_friendly_name_round_trips_through_devices_xml() {
        let dir = temp_dir("devices");
        let path = dir.join("devices.xml");
        let (store, mut devices, state) = XmlFileStore::<DeviceSettings>::open(path.clone());
        assert_eq!(state, LoadState::Missing);
        assert!(
            String::from_utf8(std::fs::read(&path).unwrap_or_default()).unwrap_or_default().ends_with("<Devices />")
        );
        devices.get_or_create("bluetooth-endpoint-id", true).custom_friendly_name =
            "My Bluetooth Headphones".to_owned();
        assert!(store.save_now(&devices).is_ok());
        drop(store);
        let (_store, loaded, _) = XmlFileStore::<DeviceSettings>::open(path.clone());
        assert_eq!(loaded.custom_name("bluetooth-endpoint-id"), Some("My Bluetooth Headphones"));
        drop(_store);
        cleanup(&dir);
    }

    #[test]
    fn write_does_not_reuse_a_locked_legacy_temporary_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp_dir("legacy-tmp");
        let path = dir.join("devices.xml");
        let legacy = dir.join("devices.xml.tmp");
        let holder =
            std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).share_mode(0).open(&legacy);
        let mut devices = DeviceSettings::default();
        devices.get_or_create("locked-temporary-file", true).custom_friendly_name = "Headphones".to_owned();
        assert!(tadn_common::xml::atomic_write(&path, &serialize(&devices)).is_ok());
        drop(holder);
        let loaded: DeviceSettings = deserialize(&std::fs::read(&path).unwrap_or_default()).unwrap_or_default();
        assert_eq!(loaded.custom_name("locked-temporary-file"), Some("Headphones"));
        cleanup(&dir);
    }

    #[test]
    fn concurrent_writes_produce_a_readable_file() {
        let dir = temp_dir("devices-concurrent");
        let path = dir.join("devices.xml");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(32));
        let writers: Vec<_> = (0..32)
            .map(|index| {
                let path = path.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut devices = DeviceSettings::default();
                    devices.get_or_create(&format!("device-{index}"), true).custom_friendly_name =
                        format!("Device {index}");
                    barrier.wait();
                    tadn_common::xml::atomic_write(&path, &serialize(&devices))
                })
            })
            .collect();
        for writer in writers {
            assert!(writer.join().is_ok_and(|r| r.is_ok()));
        }
        let loaded: DeviceSettings = deserialize(&std::fs::read(&path).unwrap_or_default()).unwrap_or_default();
        assert_eq!(loaded.devices.len(), 1);
        assert!(loaded.devices[0].custom_friendly_name.starts_with("Device "));
        let leftovers = std::fs::read_dir(&dir).map(|d| d.filter_map(Result::ok).count()).unwrap_or(0);
        assert_eq!(leftovers, 1);
        cleanup(&dir);
    }
}
