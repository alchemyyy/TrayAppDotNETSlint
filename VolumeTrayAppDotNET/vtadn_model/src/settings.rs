//! VolumeTrayAppDotNET settings (port of `Models/AppSettings.cs` + `Models/AppSettingsEnums.cs`).
//!
//! `settings.xml` layout: root `AppSettings`, the [`CommonSettings`] elements first, then the
//! fields below in declaration order. XML element names are the C# property names (noted on each
//! field when not the plain PascalCase of the Rust name). The (de)serializer lives in
//! `settings_xml.rs`.

use tadn_common::settings::{CommonSettings, XmlEnum};
use tadn_common::theme::Argb;

macro_rules! xml_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident),+ $(,)? } default $default:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($(#[$vmeta])* $variant),+ }

        impl Default for $name {
            fn default() -> Self { Self::$default }
        }

        impl XmlEnum for $name {
            const VARIANTS: &'static [(Self, &'static str)] = &[$((Self::$variant, stringify!($variant))),+];
        }
    };
}

xml_enum!(
    /// `TrayClickAction`: what a modified tray click does. `Nothing` falls back to the plain action.
    TrayClickAction { Nothing, OpenSettings } default Nothing
);
xml_enum!(
    /// `TrayWheelVolumeStep`: which wheel step a (modified) wheel uses.
    TrayWheelVolumeStep { Nothing, Default, Fine, Coarse } default Default
);
xml_enum!(
    /// `SoundSettingsTarget`: the flyout's sound settings button target.
    SoundSettingsTarget { LegacySoundPanel, WindowsSettingsApp } default LegacySoundPanel
);
xml_enum!(
    /// `TrayMenuDeviceNameStyle`: NameAndModel = FriendlyName, Name = DeviceDesc, Model = interface name.
    TrayMenuDeviceNameStyle { NameAndModel, Name, Model } default NameAndModel
);
xml_enum!(
    /// `TrayAppDotNETTrayIconStyle` (`TrayIconStyle`). Persisted only; Volume does not read it.
    TrayIconStyle { Dynamic, Static } default Dynamic
);
xml_enum!(
    /// `FlyoutDeviceLayoutStyle` (`FlyoutDeviceLayout`).
    FlyoutDeviceLayout { AppsAboveDevice, AppsBelowDevice } default AppsAboveDevice
);
xml_enum!(
    /// `FlyoutDeviceTitlePosition`.
    FlyoutDeviceTitlePosition { BelowSlider, AboveSlider } default BelowSlider
);
xml_enum!(
    /// `FlyoutDeviceSortOrder` (`FlyoutDeviceSort`).
    FlyoutDeviceSort { StateGrouped, WindowsEnumeration } default StateGrouped
);
xml_enum!(
    /// `FlyoutDisconnectedBluetoothDeviceVisibility`.
    FlyoutDisconnectedBluetoothDeviceVisibility { NeverShow, Show, AlwaysShow, AlwaysShowIntermixed } default AlwaysShowIntermixed
);
xml_enum!(
    /// `BluetoothRadioButtonClickGesture` (`FlyoutBluetoothRadioButtonClickGesture`).
    BluetoothRadioButtonClickGesture { LeftClick, ControlLeftClick, AltLeftClick, ShiftLeftClick } default ControlLeftClick
);
xml_enum!(
    /// `CommunicationsButtonVisibility` (`FlyoutCommunicationsButtonVisibility`).
    CommunicationsButtonVisibility { AlwaysShow, WhenDuckingOn, Hidden } default WhenDuckingOn
);
xml_enum!(
    /// `CaptureActivityIndicator`.
    CaptureActivityIndicator { DimInactive, ActiveGlyph, HideInactive, None } default ActiveGlyph
);
xml_enum!(
    /// `AppDrawerDisplayType` (`RecordingAppDrawerDisplayType`).
    AppDrawerDisplayType { Sliders, Icons } default Icons
);
xml_enum!(
    /// `AppDrawerStackDirection`.
    AppDrawerStackDirection { TopBottom, BottomTop, LeftRight, RightLeft, Auto } default Auto
);
xml_enum!(
    /// `AppDrawerIconsCenterMode`.
    AppDrawerIconsCenterMode { Off, Centered, CenteredSoftMax } default Off
);
xml_enum!(
    /// `SliderThumbGlyphOption.Shape`.
    SliderThumbShape { Glyph, Capsule } default Glyph
);

/// `SliderThumbGlyphOption` (`<SliderThumb>`: `Name`, `Glyph`, `FontFamily`, `FontSize`, `Width`,
/// `Height`, `XScale`, `Shape`).
#[derive(Clone, Debug, PartialEq)]
pub struct SliderThumbOption {
    pub name: String,
    pub glyph: String,
    pub font_family: String,
    pub font_size: f64,
    pub width: f64,
    pub height: f64,
    pub x_scale: f64,
    pub shape: SliderThumbShape,
}

impl SliderThumbOption {
    fn glyph(name: &str, glyph: char, font_size: f64) -> Self {
        Self {
            name: name.to_owned(),
            glyph: glyph.to_string(),
            font_family: "Segoe Fluent Icons".to_owned(),
            font_size,
            width: 18.0,
            height: 18.0,
            x_scale: 1.0,
            shape: SliderThumbShape::Glyph,
        }
    }

    /// `SliderThumbGlyphOption.CreateDefaults()`; the first entry (Capsule) is the default.
    pub fn default_catalog() -> Vec<Self> {
        vec![
            Self {
                name: "Capsule".to_owned(),
                glyph: "\u{E91F}".to_owned(),
                font_family: "Segoe Fluent Icons".to_owned(),
                font_size: 18.0,
                width: 10.0,
                height: 22.0,
                x_scale: 1.0,
                shape: SliderThumbShape::Capsule,
            },
            Self::glyph("Circle", '\u{E91F}', 18.0),
            Self::glyph("Diamond", '\u{EA3B}', 16.0),
            Self::glyph("Star", '\u{E734}', 18.0),
            Self::glyph("Square", '\u{E73B}', 16.0),
            Self::glyph("Heart", '\u{EB51}', 16.0),
        ]
    }

    /// Catalog shown in the settings window: the defaults, plus `persisted` appended when its name
    /// is not one of them.
    pub fn catalog_with(persisted: &SliderThumbOption) -> Vec<Self> {
        let mut catalog = Self::default_catalog();
        if !catalog.iter().any(|o| o.name == persisted.name) {
            catalog.push(persisted.clone());
        }
        catalog
    }
}

impl Default for SliderThumbOption {
    fn default() -> Self {
        Self::default_catalog().remove(0)
    }
}

/// Default solid peak meter color (`#FFFFFFFF`).
pub const METER_PEAK_COLOR_DEFAULT: Argb = Argb(0xFFFF_FFFF);
/// Default stereo overlay color (`#80FFFFFF`).
pub const METER_PEAK_STEREO_COLOR_DEFAULT: Argb = Argb(0x80FF_FFFF);
/// `IconRetryAttempts`: icon loads retry 4 times, waiting 1x, 2x, 3x the interval.
pub const ICON_RETRY_ATTEMPTS: u32 = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct AppSettings {
    pub common: CommonSettings,

    pub set_default_comms_to_default: bool,
    pub show_disabled_playback_devices: bool,
    pub show_default_playback_device_even_if_disabled: bool,
    pub show_default_comms_playback_device_even_if_disabled: bool,
    pub show_disconnected_playback_devices: bool,
    pub show_recording_devices: bool,
    pub show_disabled_recording_devices: bool,
    pub show_default_recording_device_even_if_disabled: bool,
    pub show_default_comms_recording_device_even_if_disabled: bool,
    pub show_disconnected_recording_devices: bool,
    pub default_app_drawer_expanded: bool,
    /// `LastKnownDefaultPlaybackDeviceID`. Runtime state (persist without change notification).
    pub last_known_default_playback_device_id: Option<String>,
    /// `LastKnownDefaultCommsPlaybackDeviceID`. Runtime state.
    pub last_known_default_comms_playback_device_id: Option<String>,
    /// `LastKnownDefaultRecordingDeviceID`. Runtime state.
    pub last_known_default_recording_device_id: Option<String>,
    /// `LastKnownDefaultCommsRecordingDeviceID`. Runtime state.
    pub last_known_default_comms_recording_device_id: Option<String>,
    /// `ShowNotPresentDevices` ("Show registry ghost devices").
    pub show_not_present_devices: bool,
    pub activate_recording_devices_for_peak_meters: bool,
    pub show_tray_menu_recording_link: bool,
    pub show_tray_menu_sounds_link: bool,
    pub show_tray_menu_communications_link: bool,
    pub show_tray_menu_device_links: bool,
    pub use_logarithmic_volume_scale: bool,
    pub play_device_volume_change_sound: bool,
    pub suppress_device_volume_change_sound_when_audio_playing: bool,
    /// 0..100.
    pub ding_suppression_peak_threshold_percent: i32,
    pub play_app_volume_change_sound: bool,
    pub tray_menu_playback_device_name_style: TrayMenuDeviceNameStyle,
    pub tray_menu_recording_device_name_style: TrayMenuDeviceNameStyle,
    /// 3..200.
    pub tray_menu_device_name_max_length: i32,
    pub tray_icon_style: TrayIconStyle,
    pub unified_peak_meter: bool,
    /// 0..100.
    pub unified_meter_low_channel_bias_multiplier: i32,
    /// `MeterPeakFps`, 1..1000.
    pub meter_peak_fps: i32,
    /// 1..1000.
    pub meter_peak_sample_rate: i32,
    /// 0..100.
    pub meter_peak_change_ceiling: i32,
    /// 50..5000.
    pub icon_retry_interval_ms: i32,
    /// `IconLRULimit`, 0..1000.
    pub icon_lru_limit: i32,
    /// `MeterPeakColorHex`; None = default (written as `#FFFFFFFF`).
    pub meter_peak_color: Option<Argb>,
    /// `MeterPeakStereoColorHex`; None = default (written as `#80FFFFFF`).
    pub meter_peak_stereo_color: Option<Argb>,
    /// `<SliderThumb>`.
    pub slider_thumb: SliderThumbOption,
    pub tray_scroll_enabled: bool,
    pub play_tray_scroll_volume_change_sound: bool,
    /// 1..100.
    pub wheel_volume_step_percent: i32,
    /// 1..100.
    pub wheel_volume_step_fine_percent: i32,
    /// 1..100.
    pub wheel_volume_step_coarse_percent: i32,
    pub tray_wheel_action: TrayWheelVolumeStep,
    pub tray_ctrl_wheel_action: TrayWheelVolumeStep,
    pub tray_alt_wheel_action: TrayWheelVolumeStep,
    pub precision_touchpad_scroll_enabled: bool,
    /// 1..1000.
    pub precision_touchpad_units_per_scroll_step: i32,
    pub tray_double_click_action: TrayClickAction,
    pub tray_ctrl_left_click_action: TrayClickAction,
    pub tray_alt_left_click_action: TrayClickAction,
    pub tray_ctrl_right_click_action: TrayClickAction,
    pub tray_alt_right_click_action: TrayClickAction,
    pub tray_ctrl_double_left_click_action: TrayClickAction,
    pub tray_alt_double_left_click_action: TrayClickAction,
    pub allow_flyout_undock: bool,
    pub clamp_undocked_flyout_to_screen: bool,
    pub restore_flyout_undocked_on_startup: bool,
    /// Runtime state.
    pub flyout_undocked: bool,
    /// Runtime state.
    pub flyout_has_saved_position: bool,
    /// Runtime state, physical px.
    pub flyout_left: f64,
    /// Runtime state, physical px.
    pub flyout_top: f64,
    pub flyout_header_at_bottom: bool,
    pub sound_settings_target: SoundSettingsTarget,
    pub flyout_device_layout: FlyoutDeviceLayout,
    pub flyout_device_title_position: FlyoutDeviceTitlePosition,
    pub flyout_device_sort: FlyoutDeviceSort,
    pub flyout_disconnected_bluetooth_device_visibility: FlyoutDisconnectedBluetoothDeviceVisibility,
    pub collapse_disconnected_bluetooth_devices_in_flyout: bool,
    pub show_bluetooth_devices_only_when_bluetooth_is_on: bool,
    pub show_recording_devices_in_flyout: bool,
    pub intermix_recording_with_playback_in_flyout: bool,
    pub use_dynamic_playback_volume_glyph_in_flyout: bool,
    pub show_bluetooth_radio_button_in_flyout_header: bool,
    pub flyout_bluetooth_radio_button_click_gesture: BluetoothRadioButtonClickGesture,
    pub flyout_communications_button_visibility: CommunicationsButtonVisibility,
    pub show_lock_button_for_playback: bool,
    /// `ShowEqualizerAPOButtonForPlayback`.
    pub show_equalizer_apo_button_for_playback: bool,
    pub show_default_device_button_for_playback: bool,
    pub show_battery_button_for_playback: bool,
    pub show_lock_button_for_recording: bool,
    /// `ShowEqualizerAPOButtonForRecording`.
    pub show_equalizer_apo_button_for_recording: bool,
    pub show_listen_button_for_recording: bool,
    pub show_default_device_button_for_recording: bool,
    pub show_battery_button_for_recording: bool,
    pub show_device_format_text: bool,
    pub show_device_codec_text: bool,
    pub capture_activity_indicator: CaptureActivityIndicator,
    pub recording_app_drawer_display_type: AppDrawerDisplayType,
    pub app_drawer_icons_center_mode: AppDrawerIconsCenterMode,
    /// 1..16.
    pub app_drawer_icons_center_soft_max: i32,
    /// 50..200.
    pub app_drawer_icon_scale_percent: i32,
    /// 1..16.
    pub app_drawer_icons_per_row: i32,
    pub app_drawer_stack_direction: AppDrawerStackDirection,
    /// 1..200.
    pub playback_app_drawer_sliders_max_apps: i32,
    /// 1..200.
    pub playback_app_drawer_icons_max_rows: i32,
    /// 1..200.
    pub recording_app_drawer_sliders_max_apps: i32,
    /// 1..200.
    pub recording_app_drawer_icons_max_rows: i32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            common: CommonSettings::default(),
            set_default_comms_to_default: false,
            show_disabled_playback_devices: false,
            show_default_playback_device_even_if_disabled: true,
            show_default_comms_playback_device_even_if_disabled: true,
            show_disconnected_playback_devices: false,
            show_recording_devices: true,
            show_disabled_recording_devices: false,
            show_default_recording_device_even_if_disabled: true,
            show_default_comms_recording_device_even_if_disabled: true,
            show_disconnected_recording_devices: false,
            default_app_drawer_expanded: true,
            last_known_default_playback_device_id: None,
            last_known_default_comms_playback_device_id: None,
            last_known_default_recording_device_id: None,
            last_known_default_comms_recording_device_id: None,
            show_not_present_devices: false,
            activate_recording_devices_for_peak_meters: false,
            show_tray_menu_recording_link: false,
            show_tray_menu_sounds_link: false,
            show_tray_menu_communications_link: false,
            show_tray_menu_device_links: false,
            use_logarithmic_volume_scale: false,
            play_device_volume_change_sound: true,
            suppress_device_volume_change_sound_when_audio_playing: true,
            ding_suppression_peak_threshold_percent: 5,
            play_app_volume_change_sound: true,
            tray_menu_playback_device_name_style: TrayMenuDeviceNameStyle::NameAndModel,
            tray_menu_recording_device_name_style: TrayMenuDeviceNameStyle::NameAndModel,
            tray_menu_device_name_max_length: 32,
            tray_icon_style: TrayIconStyle::Dynamic,
            unified_peak_meter: true,
            unified_meter_low_channel_bias_multiplier: 3,
            meter_peak_fps: 180,
            meter_peak_sample_rate: 90,
            meter_peak_change_ceiling: 9,
            icon_retry_interval_ms: 250,
            icon_lru_limit: 10,
            meter_peak_color: None,
            meter_peak_stereo_color: None,
            slider_thumb: SliderThumbOption::default(),
            tray_scroll_enabled: true,
            play_tray_scroll_volume_change_sound: true,
            wheel_volume_step_percent: 2,
            wheel_volume_step_fine_percent: 1,
            wheel_volume_step_coarse_percent: 5,
            tray_wheel_action: TrayWheelVolumeStep::Default,
            tray_ctrl_wheel_action: TrayWheelVolumeStep::Fine,
            tray_alt_wheel_action: TrayWheelVolumeStep::Coarse,
            precision_touchpad_scroll_enabled: true,
            precision_touchpad_units_per_scroll_step: 90,
            tray_double_click_action: TrayClickAction::Nothing,
            tray_ctrl_left_click_action: TrayClickAction::Nothing,
            tray_alt_left_click_action: TrayClickAction::Nothing,
            tray_ctrl_right_click_action: TrayClickAction::Nothing,
            tray_alt_right_click_action: TrayClickAction::Nothing,
            tray_ctrl_double_left_click_action: TrayClickAction::Nothing,
            tray_alt_double_left_click_action: TrayClickAction::Nothing,
            allow_flyout_undock: true,
            clamp_undocked_flyout_to_screen: true,
            restore_flyout_undocked_on_startup: true,
            flyout_undocked: true,
            flyout_has_saved_position: false,
            flyout_left: 0.0,
            flyout_top: 0.0,
            flyout_header_at_bottom: false,
            sound_settings_target: SoundSettingsTarget::LegacySoundPanel,
            flyout_device_layout: FlyoutDeviceLayout::AppsAboveDevice,
            flyout_device_title_position: FlyoutDeviceTitlePosition::BelowSlider,
            flyout_device_sort: FlyoutDeviceSort::StateGrouped,
            flyout_disconnected_bluetooth_device_visibility:
                FlyoutDisconnectedBluetoothDeviceVisibility::AlwaysShowIntermixed,
            collapse_disconnected_bluetooth_devices_in_flyout: true,
            show_bluetooth_devices_only_when_bluetooth_is_on: true,
            show_recording_devices_in_flyout: true,
            intermix_recording_with_playback_in_flyout: false,
            use_dynamic_playback_volume_glyph_in_flyout: false,
            show_bluetooth_radio_button_in_flyout_header: true,
            flyout_bluetooth_radio_button_click_gesture: BluetoothRadioButtonClickGesture::ControlLeftClick,
            flyout_communications_button_visibility: CommunicationsButtonVisibility::WhenDuckingOn,
            show_lock_button_for_playback: false,
            show_equalizer_apo_button_for_playback: false,
            show_default_device_button_for_playback: true,
            show_battery_button_for_playback: true,
            show_lock_button_for_recording: false,
            show_equalizer_apo_button_for_recording: false,
            show_listen_button_for_recording: false,
            show_default_device_button_for_recording: true,
            show_battery_button_for_recording: true,
            show_device_format_text: true,
            show_device_codec_text: false,
            capture_activity_indicator: CaptureActivityIndicator::ActiveGlyph,
            recording_app_drawer_display_type: AppDrawerDisplayType::Icons,
            app_drawer_icons_center_mode: AppDrawerIconsCenterMode::Off,
            app_drawer_icons_center_soft_max: 7,
            app_drawer_icon_scale_percent: 110,
            app_drawer_icons_per_row: 9,
            app_drawer_stack_direction: AppDrawerStackDirection::Auto,
            playback_app_drawer_sliders_max_apps: 24,
            playback_app_drawer_icons_max_rows: 10,
            recording_app_drawer_sliders_max_apps: 24,
            recording_app_drawer_icons_max_rows: 10,
        }
    }
}

impl AppSettings {
    /// Applies every C# setter clamp (common and volume). Called after load and after every edit.
    pub fn clamp_all(&mut self) {
        self.common.clamp_all();
        let c = |v: &mut i32, lo: i32, hi: i32| *v = (*v).clamp(lo, hi);
        c(&mut self.ding_suppression_peak_threshold_percent, 0, 100);
        c(&mut self.tray_menu_device_name_max_length, 3, 200);
        c(&mut self.unified_meter_low_channel_bias_multiplier, 0, 100);
        c(&mut self.meter_peak_fps, 1, 1000);
        c(&mut self.meter_peak_sample_rate, 1, 1000);
        c(&mut self.meter_peak_change_ceiling, 0, 100);
        c(&mut self.icon_retry_interval_ms, 50, 5000);
        c(&mut self.icon_lru_limit, 0, 1000);
        c(&mut self.wheel_volume_step_percent, 1, 100);
        c(&mut self.wheel_volume_step_fine_percent, 1, 100);
        c(&mut self.wheel_volume_step_coarse_percent, 1, 100);
        c(&mut self.precision_touchpad_units_per_scroll_step, 1, 1000);
        c(&mut self.app_drawer_icons_center_soft_max, 1, 16);
        c(&mut self.app_drawer_icon_scale_percent, 50, 200);
        c(&mut self.app_drawer_icons_per_row, 1, 16);
        c(&mut self.playback_app_drawer_sliders_max_apps, 1, 200);
        c(&mut self.playback_app_drawer_icons_max_rows, 1, 200);
        c(&mut self.recording_app_drawer_sliders_max_apps, 1, 200);
        c(&mut self.recording_app_drawer_icons_max_rows, 1, 200);
    }

    pub fn effective_meter_peak_color(&self) -> Argb {
        self.meter_peak_color.unwrap_or(METER_PEAK_COLOR_DEFAULT)
    }

    pub fn effective_meter_peak_stereo_color(&self) -> Argb {
        self.meter_peak_stereo_color.unwrap_or(METER_PEAK_STEREO_COLOR_DEFAULT)
    }

    /// Wheel step in percent for a tray wheel action (`None` for `Nothing`).
    pub fn wheel_step_percent(&self, action: TrayWheelVolumeStep) -> Option<i32> {
        match action {
            TrayWheelVolumeStep::Nothing => None,
            TrayWheelVolumeStep::Default => Some(self.wheel_volume_step_percent),
            TrayWheelVolumeStep::Fine => Some(self.wheel_volume_step_fine_percent),
            TrayWheelVolumeStep::Coarse => Some(self.wheel_volume_step_coarse_percent),
        }
    }
}
