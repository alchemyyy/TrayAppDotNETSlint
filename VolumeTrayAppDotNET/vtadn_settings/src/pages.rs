//! Page builders: port of `UI/Settings/{General,Flyout,Devices,DeviceAppDrawers,TrayIcon,Hotkeys,
//! Theme,About}Page.cs` plus the shared sections of `SettingsWindowCommon.Sections.cs`,
//! `TrayAppDotNETGeneralSettingsSection.cs` (language, run on startup),
//! `TrayAppDotNETRenderingSettingsSection.cs` and `TrayAppDotNETAboutPage.cs` (no install/update
//! cards: this build has neither). Card order, wording, ranges and "only shown while" rules match
//! the C# pages.

use tadn_common::hotkey::HotkeyAction;
use tadn_common::settings::{
    AnimationMode, CONTEXT_MENU_FONT_SIZE_RANGE, ContextMenuPosition, RenderingBackend, TOOL_TIP_SHOW_DELAY_MS_RANGE,
    ThemeMode,
};
use tadn_common::theme::{Argb, NullableThemeColor, ThemeFile};
use vtadn_model::settings::{
    AppDrawerDisplayType, AppDrawerIconsCenterMode, AppDrawerStackDirection, AppSettings,
    BluetoothRadioButtonClickGesture, CaptureActivityIndicator, CommunicationsButtonVisibility, FlyoutDeviceLayout,
    FlyoutDeviceSort, FlyoutDeviceTitlePosition, FlyoutDisconnectedBluetoothDeviceVisibility, METER_PEAK_COLOR_DEFAULT,
    METER_PEAK_STEREO_COLOR_DEFAULT, SliderThumbOption, SoundSettingsTarget, TrayClickAction, TrayMenuDeviceNameStyle,
    TrayWheelVolumeStep,
};

use crate::SettingsPage;
use crate::hotkeys::HotkeyUi;
use crate::items::{
    Action, ColorTarget, Item, PageBuilder, Role, base_view, hotkey_view, model, text_option, thumb_option,
};
use crate::strings as s;
use crate::ui::ItemKind;

/// `LocalizationManager.AutoLanguage`.
pub(crate) const AUTO_LANGUAGE: &str = "Auto";
/// `LocalizationManager.NeutralLanguage`: the shipped English resources.
pub(crate) const NEUTRAL_LANGUAGE: &str = "en";
/// Native display name of the neutral language (`CultureInfo("en").NativeName`).
const NEUTRAL_LANGUAGE_NAME: &str = "English";
/// `TrayAppDotNETRenderingSettingsSection.RenderingBackendComboWidth`.
const RENDERING_BACKEND_COMBO_WIDTH: f32 = 172.0;
/// `TrayAppDotNETAboutPage.DevelopmentBuildLabel`.
const DEVELOPMENT_BUILD_LABEL: &str = "dev";

/// Everything a page needs besides the settings.
pub(crate) struct BuildContext<'a> {
    pub settings: &'a AppSettings,
    /// Effective light/dark mode (which variant swatch the color cards show).
    pub is_light: bool,
    /// Theme defaults for the color fallbacks.
    pub theme: &'a ThemeFile,
    pub startup_enabled: bool,
    pub startup_target: Option<String>,
    pub hotkeys: &'a HotkeyUi,
}

/// Every page, in sidebar order.
pub(crate) const PAGES: [SettingsPage; 8] = [
    SettingsPage::General,
    SettingsPage::Flyout,
    SettingsPage::Devices,
    SettingsPage::AppDrawers,
    SettingsPage::TrayIcon,
    SettingsPage::Hotkeys,
    SettingsPage::Theme,
    SettingsPage::About,
];

pub(crate) fn page_label(page: SettingsPage) -> &'static str {
    match page {
        SettingsPage::General => s::Settings_Common_Page_General,
        SettingsPage::Flyout => s::Settings_Common_Page_Flyout,
        SettingsPage::Devices => s::Settings_Common_Page_Devices,
        SettingsPage::AppDrawers => s::Settings_Common_Page_DeviceAppDrawers,
        SettingsPage::TrayIcon => s::Settings_Common_Page_TrayIcon,
        SettingsPage::Hotkeys => s::Settings_Common_Page_Hotkeys,
        SettingsPage::Theme => s::Settings_Common_Page_Theme,
        SettingsPage::About => s::Settings_Common_Page_About,
    }
}

/// `SettingsNavigationGlyphs` (shown with the Windows 11-style menu).
pub(crate) fn page_glyph(page: SettingsPage) -> &'static str {
    match page {
        SettingsPage::General => "\u{E71D}",
        SettingsPage::Flyout => "\u{E8A7}",
        SettingsPage::Devices => "\u{EDB5}",
        SettingsPage::AppDrawers => "\u{E81E}",
        SettingsPage::TrayIcon => "\u{E957}",
        SettingsPage::Hotkeys => "\u{E92E}",
        SettingsPage::Theme => "\u{E790}",
        SettingsPage::About => "\u{E946}",
    }
}

pub(crate) fn build_page(page: SettingsPage, ctx: &BuildContext<'_>) -> Vec<Item> {
    match page {
        SettingsPage::General => general(ctx),
        SettingsPage::Flyout => flyout(ctx),
        SettingsPage::Devices => devices(ctx),
        SettingsPage::AppDrawers => app_drawers(ctx),
        SettingsPage::TrayIcon => tray_icon(ctx),
        SettingsPage::Hotkeys => hotkeys(ctx),
        SettingsPage::Theme => theme(ctx),
        SettingsPage::About => about(ctx),
    }
}

// --- General -------------------------------------------------------------------------------------

/// Language options: Auto, then the neutral language (no translations ship).
pub(crate) fn language_options() -> Vec<(&'static str, &'static str)> {
    vec![(AUTO_LANGUAGE, s::Settings_General_Language_Auto), (NEUTRAL_LANGUAGE, NEUTRAL_LANGUAGE_NAME)]
}

/// `RunOnStartupDescription`.
pub(crate) fn startup_description(target: Option<&str>) -> String {
    match target {
        Some(target) if !target.is_empty() => format!("{}\n{}", s::Settings_General_RunOnStartup_OnHeaderLine, target),
        _ => s::Settings_General_RunOnStartup_Description.to_owned(),
    }
}

fn general(ctx: &BuildContext<'_>) -> Vec<Item> {
    let st = ctx.settings;
    let mut b = PageBuilder::new("general");
    b.header(s::Settings_General_SectionHeader);

    // TrayAppDotNETGeneralSettingsSection.BuildLanguageCard: a saved language that is not shipped
    // shows as Auto without rewriting the setting.
    let languages = language_options();
    let selected = languages.iter().position(|(tag, _)| *tag == st.common.language).unwrap_or(0);
    let mut view =
        base_view(ItemKind::ComboCard, s::Settings_General_Language_Title, s::Settings_General_Language_Description);
    view.options = model(languages.iter().map(|(_, text)| text_option(text)).collect());
    view.selected = selected as i32;
    b.push(
        "language",
        Role::Card,
        view,
        Action::Language(languages.iter().map(|(tag, _)| *tag).collect()),
        &[s::Settings_General_Language_SearchKeywords],
    );

    // BuildStartupCard (no start-minimized sub-option in Volume).
    let mut view = base_view(
        ItemKind::ToggleCard,
        s::Settings_General_RunOnStartup_Title,
        &startup_description(ctx.startup_target.as_deref()),
    );
    view.checked = ctx.startup_enabled;
    b.push("run_on_startup", Role::Card, view, Action::Startup, &[s::Settings_General_RunOnStartup_SearchKeywords]);

    // AddRenderingSection(supportsFlyoutWarmWindow: true, supportsTrayContextMenuWarmWindow: true).
    b.section("rendering_header", s::Settings_General_Rendering_Header);
    let backends = [
        (RenderingBackend::GPUPreferred, s::Settings_General_RenderingBackend_GPUPreferred),
        (RenderingBackend::Software, s::Settings_General_RenderingBackend_Software),
    ];
    let mut view = base_view(
        ItemKind::ComboCard,
        s::Settings_General_RenderingBackend_Title,
        s::Settings_General_RenderingBackend_Description,
    );
    view.options = model(backends.iter().map(|(_, text)| text_option(text)).collect());
    view.selected = backends.iter().position(|(v, _)| *v == st.common.rendering_backend).map_or(-1, |i| i as i32);
    view.combo_width = RENDERING_BACKEND_COMBO_WIDTH;
    b.push(
        "rendering_backend",
        Role::Card,
        view,
        Action::RenderingBackend,
        &[s::Settings_General_RenderingBackend_SearchKeywords],
    );
    b.section("performance_header", s::Settings_General_Performance_Header);
    b.toggle(
        "keep_flyout_warm",
        s::Settings_General_KeepFlyoutWarm_Title,
        s::Settings_General_KeepFlyoutWarm_Description,
        st.common.keep_flyout_warm,
        |s, v| s.common.keep_flyout_warm = v,
        &[s::Settings_General_KeepWarm_SearchKeywords],
    );
    b.toggle(
        "keep_tray_context_menu_warm",
        s::Settings_General_KeepTrayContextMenuWarm_Title,
        s::Settings_General_KeepTrayContextMenuWarm_Description,
        st.common.keep_tray_context_menu_warm,
        |s, v| s.common.keep_tray_context_menu_warm = v,
        &[s::Settings_General_KeepWarm_SearchKeywords],
    );

    b.section("notifications_header", s::Settings_General_Notifications_Header);
    b.toggle(
        "play_device_volume_change_sound",
        s::Settings_General_PlayDeviceVolumeChangeSound_Title,
        s::Settings_General_PlayDeviceVolumeChangeSound_Description,
        st.play_device_volume_change_sound,
        |s, v| s.play_device_volume_change_sound = v,
        &[s::Settings_General_PlayDeviceVolumeChangeSound_SearchKeywords],
    );
    if st.play_device_volume_change_sound {
        b.toggle(
            "play_tray_scroll_volume_change_sound",
            s::Settings_General_PlayTrayScrollVolumeChangeSound_Title,
            s::Settings_General_PlayTrayScrollVolumeChangeSound_Description,
            st.play_tray_scroll_volume_change_sound,
            |s, v| s.play_tray_scroll_volume_change_sound = v,
            &[s::Settings_General_PlayTrayScrollVolumeChangeSound_SearchKeywords],
        );
    }
    b.toggle(
        "play_app_volume_change_sound",
        s::Settings_General_PlayAppVolumeChangeSound_Title,
        s::Settings_General_PlayAppVolumeChangeSound_Description,
        st.play_app_volume_change_sound,
        |s, v| s.play_app_volume_change_sound = v,
        &[s::Settings_General_PlayAppVolumeChangeSound_SearchKeywords],
    );
    if st.play_device_volume_change_sound {
        b.toggle(
            "suppress_ding_when_playing",
            s::Settings_General_SuppressDeviceVolumeChangeSoundWhenAudioPlaying_Title,
            s::Settings_General_SuppressDeviceVolumeChangeSoundWhenAudioPlaying_Description,
            st.suppress_device_volume_change_sound_when_audio_playing,
            |s, v| s.suppress_device_volume_change_sound_when_audio_playing = v,
            &[s::Settings_General_SuppressDeviceVolumeChangeSoundWhenAudioPlaying_SearchKeywords],
        );
    }
    if st.play_device_volume_change_sound && st.suppress_device_volume_change_sound_when_audio_playing {
        b.number(
            "ding_suppression_peak_threshold",
            s::Settings_General_DingSuppressionPeakThreshold_Title,
            s::Settings_General_DingSuppressionPeakThreshold_Description,
            st.ding_suppression_peak_threshold_percent,
            (0, 100),
            "",
            |s, v| s.ding_suppression_peak_threshold_percent = v,
            &[s::Settings_General_DingSuppressionPeakThreshold_SearchKeywords],
        );
    }

    b.section("other_header", s::Settings_General_Other_Header);
    b.toggle(
        "logarithmic_volume_scale",
        s::Settings_General_LogarithmicVolumeScale_Title,
        s::Settings_General_LogarithmicVolumeScale_Description,
        st.use_logarithmic_volume_scale,
        |s, v| s.use_logarithmic_volume_scale = v,
        &[s::Settings_General_LogarithmicVolumeScale_SearchKeywords],
    );
    b.items
}

// --- Flyout --------------------------------------------------------------------------------------

fn flyout(ctx: &BuildContext<'_>) -> Vec<Item> {
    let st = ctx.settings;
    let mut b = PageBuilder::new("flyout");
    b.header(s::Settings_Flyout_SectionHeader);

    b.toggle(
        "restore_undock_state",
        s::Settings_Flyout_RestoreUndockState_Title,
        s::Settings_Flyout_RestoreUndockState_Description,
        st.restore_flyout_undocked_on_startup,
        |s, v| s.restore_flyout_undocked_on_startup = v,
        &[s::Settings_Flyout_RestoreUndockState_SearchKeywords],
    );

    b.section("visibility_header", s::Settings_Flyout_Visibility_Header);
    b.toggle(
        "show_undock_button",
        s::Settings_Flyout_ShowUndockButton_Title,
        s::Settings_Flyout_ShowUndockButton_Description,
        st.allow_flyout_undock,
        |s, v| s.allow_flyout_undock = v,
        &[s::Settings_Flyout_ShowUndockButton_SearchKeywords],
    );
    if st.allow_flyout_undock {
        b.toggle(
            "clamp_undocked_to_screen",
            s::Settings_Flyout_ClampUndockedToScreen_Title,
            s::Settings_Flyout_ClampUndockedToScreen_Description,
            st.clamp_undocked_flyout_to_screen,
            |s, v| s.clamp_undocked_flyout_to_screen = v,
            &[s::Settings_Flyout_ClampUndockedToScreen_SearchKeywords],
        );
    }
    b.combo(
        "communications_button_visibility",
        s::Settings_Flyout_CommunicationsButtonVisibility_Title,
        s::Settings_Flyout_CommunicationsButtonVisibility_Description,
        &[
            (CommunicationsButtonVisibility::AlwaysShow, s::Settings_Flyout_CommunicationsButtonVisibility_AlwaysShow),
            (
                CommunicationsButtonVisibility::WhenDuckingOn,
                s::Settings_Flyout_CommunicationsButtonVisibility_WhenDuckingOn,
            ),
            (CommunicationsButtonVisibility::Hidden, s::Settings_Flyout_CommunicationsButtonVisibility_Hidden),
        ],
        st.flyout_communications_button_visibility,
        |s, v| s.flyout_communications_button_visibility = v,
        &[s::Settings_Flyout_CommunicationsButtonVisibility_SearchKeywords],
    );
    b.toggle(
        "show_recording_devices",
        s::Settings_Flyout_ShowRecordingDevices_Title,
        s::Settings_Flyout_ShowRecordingDevices_Description,
        st.show_recording_devices_in_flyout,
        |s, v| s.show_recording_devices_in_flyout = v,
        &[s::Settings_Flyout_ShowRecordingDevices_SearchKeywords],
    );
    b.combo(
        "disconnected_bluetooth_devices",
        s::Settings_Flyout_DisconnectedBluetoothDevices_Title,
        s::Settings_Flyout_DisconnectedBluetoothDevices_Description,
        &[
            (
                FlyoutDisconnectedBluetoothDeviceVisibility::NeverShow,
                s::Settings_Flyout_DisconnectedBluetoothDevices_NeverShow,
            ),
            (FlyoutDisconnectedBluetoothDeviceVisibility::Show, s::Settings_Flyout_DisconnectedBluetoothDevices_Show),
            (
                FlyoutDisconnectedBluetoothDeviceVisibility::AlwaysShow,
                s::Settings_Flyout_DisconnectedBluetoothDevices_AlwaysShow,
            ),
            (
                FlyoutDisconnectedBluetoothDeviceVisibility::AlwaysShowIntermixed,
                s::Settings_Flyout_DisconnectedBluetoothDevices_AlwaysShowIntermixed,
            ),
        ],
        st.flyout_disconnected_bluetooth_device_visibility,
        |s, v| s.flyout_disconnected_bluetooth_device_visibility = v,
        &[s::Settings_Flyout_DisconnectedBluetoothDevices_SearchKeywords],
    );
    b.toggle(
        "collapse_disconnected_bluetooth_devices",
        s::Settings_Flyout_CollapseDisconnectedBluetoothDevices_Title,
        s::Settings_Flyout_CollapseDisconnectedBluetoothDevices_Description,
        st.collapse_disconnected_bluetooth_devices_in_flyout,
        |s, v| s.collapse_disconnected_bluetooth_devices_in_flyout = v,
        &[s::Settings_Flyout_CollapseDisconnectedBluetoothDevices_SearchKeywords],
    );
    b.toggle(
        "show_bluetooth_devices_only_when_on",
        s::Settings_Flyout_ShowBluetoothDevicesOnlyWhenOn_Title,
        s::Settings_Flyout_ShowBluetoothDevicesOnlyWhenOn_Description,
        st.show_bluetooth_devices_only_when_bluetooth_is_on,
        |s, v| s.show_bluetooth_devices_only_when_bluetooth_is_on = v,
        &[s::Settings_Flyout_ShowBluetoothDevicesOnlyWhenOn_SearchKeywords],
    );
    b.toggle(
        "show_bluetooth_radio_button",
        s::Settings_Flyout_ShowBluetoothRadioButton_Title,
        s::Settings_Flyout_ShowBluetoothRadioButton_Description,
        st.show_bluetooth_radio_button_in_flyout_header,
        |s, v| s.show_bluetooth_radio_button_in_flyout_header = v,
        &[s::Settings_Flyout_ShowBluetoothRadioButton_SearchKeywords],
    );
    if st.show_bluetooth_radio_button_in_flyout_header {
        b.combo(
            "bluetooth_radio_button_click_gesture",
            s::Settings_Flyout_BluetoothRadioButtonClickGesture_Title,
            s::Settings_Flyout_BluetoothRadioButtonClickGesture_Description,
            &[
                (
                    BluetoothRadioButtonClickGesture::LeftClick,
                    s::Settings_Flyout_BluetoothRadioButtonClickGesture_LeftClick,
                ),
                (
                    BluetoothRadioButtonClickGesture::ControlLeftClick,
                    s::Settings_Flyout_BluetoothRadioButtonClickGesture_ControlLeftClick,
                ),
                (
                    BluetoothRadioButtonClickGesture::AltLeftClick,
                    s::Settings_Flyout_BluetoothRadioButtonClickGesture_AltLeftClick,
                ),
                (
                    BluetoothRadioButtonClickGesture::ShiftLeftClick,
                    s::Settings_Flyout_BluetoothRadioButtonClickGesture_ShiftLeftClick,
                ),
            ],
            st.flyout_bluetooth_radio_button_click_gesture,
            |s, v| s.flyout_bluetooth_radio_button_click_gesture = v,
            &[s::Settings_Flyout_BluetoothRadioButtonClickGesture_SearchKeywords],
        );
    }
    b.toggle(
        "use_dynamic_playback_volume_glyph",
        s::Settings_Flyout_UseDynamicPlaybackVolumeGlyph_Title,
        s::Settings_Flyout_UseDynamicPlaybackVolumeGlyph_Description,
        st.use_dynamic_playback_volume_glyph_in_flyout,
        |s, v| s.use_dynamic_playback_volume_glyph_in_flyout = v,
        &[s::Settings_Flyout_UseDynamicPlaybackVolumeGlyph_SearchKeywords],
    );
    b.toggle(
        "show_device_format_text",
        s::Settings_Flyout_ShowDeviceFormatText_Title,
        s::Settings_Flyout_ShowDeviceFormatText_Description,
        st.show_device_format_text,
        |s, v| s.show_device_format_text = v,
        &[s::Settings_Flyout_ShowDeviceFormatText_SearchKeywords],
    );
    b.toggle(
        "show_device_codec_text",
        s::Settings_Flyout_ShowDeviceCodecText_Title,
        s::Settings_Flyout_ShowDeviceCodecText_Description,
        st.show_device_codec_text,
        |s, v| s.show_device_codec_text = v,
        &[s::Settings_Flyout_ShowDeviceCodecText_SearchKeywords],
    );
    b.combo(
        "sound_settings_target",
        s::Settings_Flyout_SoundSettingsTarget_Title,
        s::Settings_Flyout_SoundSettingsTarget_Description,
        &[
            (SoundSettingsTarget::LegacySoundPanel, s::Settings_Flyout_SoundSettingsTarget_Legacy),
            (SoundSettingsTarget::WindowsSettingsApp, s::Settings_Flyout_SoundSettingsTarget_Modern),
        ],
        st.sound_settings_target,
        |s, v| s.sound_settings_target = v,
        &[s::Settings_Flyout_SoundSettingsTarget_SearchKeywords],
    );

    b.section("layout_header", s::Settings_Flyout_Layout_Header);
    b.combo(
        "device_layout",
        s::Settings_Flyout_DeviceLayout_Title,
        s::Settings_Flyout_DeviceLayout_Description,
        &[
            (FlyoutDeviceLayout::AppsAboveDevice, s::Settings_Flyout_DeviceLayout_AppsAbove),
            (FlyoutDeviceLayout::AppsBelowDevice, s::Settings_Flyout_DeviceLayout_AppsBelow),
        ],
        st.flyout_device_layout,
        |s, v| s.flyout_device_layout = v,
        &[s::Settings_Flyout_DeviceLayout_SearchKeywords],
    );
    b.combo(
        "device_title_position",
        s::Settings_Flyout_DeviceTitlePosition_Title,
        s::Settings_Flyout_DeviceTitlePosition_Description,
        &[
            (FlyoutDeviceTitlePosition::BelowSlider, s::Settings_Flyout_DeviceTitlePosition_BelowSlider),
            (FlyoutDeviceTitlePosition::AboveSlider, s::Settings_Flyout_DeviceTitlePosition_AboveSlider),
        ],
        st.flyout_device_title_position,
        |s, v| s.flyout_device_title_position = v,
        &[s::Settings_Flyout_DeviceTitlePosition_SearchKeywords],
    );
    b.combo(
        "device_sort",
        s::Settings_Flyout_DeviceSort_Title,
        s::Settings_Flyout_DeviceSort_Description,
        &[
            (FlyoutDeviceSort::StateGrouped, s::Settings_Flyout_DeviceSort_StateGrouped),
            (FlyoutDeviceSort::WindowsEnumeration, s::Settings_Flyout_DeviceSort_WindowsEnumeration),
        ],
        st.flyout_device_sort,
        |s, v| s.flyout_device_sort = v,
        &[s::Settings_Flyout_DeviceSort_SearchKeywords],
    );
    if st.show_recording_devices_in_flyout {
        b.toggle(
            "intermix_recording",
            s::Settings_Flyout_IntermixRecording_Title,
            s::Settings_Flyout_IntermixRecording_Description,
            st.intermix_recording_with_playback_in_flyout,
            |s, v| s.intermix_recording_with_playback_in_flyout = v,
            &[s::Settings_Flyout_IntermixRecording_SearchKeywords],
        );
    }
    b.toggle(
        "header_at_bottom",
        s::Settings_Flyout_HeaderAtBottom_Title,
        s::Settings_Flyout_HeaderAtBottom_Description,
        st.flyout_header_at_bottom,
        |s, v| s.flyout_header_at_bottom = v,
        &[s::Settings_Flyout_HeaderAtBottom_SearchKeywords],
    );

    b.section("peak_meter_header", s::Settings_General_PeakMeter_Header);
    b.toggle(
        "unified_peak_meter",
        s::Settings_Theme_UnifiedPeakMeter_Title,
        s::Settings_Theme_UnifiedPeakMeter_Description,
        st.unified_peak_meter,
        |s, v| s.unified_peak_meter = v,
        &[s::Settings_Theme_UnifiedPeakMeter_SearchKeywords],
    );
    if st.unified_peak_meter {
        b.number(
            "unified_meter_bias",
            s::Settings_Theme_UnifiedMeterBias_Title,
            s::Settings_Theme_UnifiedMeterBias_Description,
            st.unified_meter_low_channel_bias_multiplier,
            (0, 100),
            "",
            |s, v| s.unified_meter_low_channel_bias_multiplier = v,
            &[s::Settings_Theme_UnifiedMeterBias_SearchKeywords],
        );
    }
    b.number(
        "meter_peak_fps",
        s::Settings_Theme_MeterPeakFps_Title,
        s::Settings_Theme_MeterPeakFps_Description,
        st.meter_peak_fps,
        (1, 1000),
        "",
        |s, v| s.meter_peak_fps = v,
        &[s::Settings_Theme_MeterPeakFps_SearchKeywords],
    );
    b.number(
        "meter_peak_sample_rate",
        s::Settings_Theme_MeterPeakSampleRate_Title,
        s::Settings_Theme_MeterPeakSampleRate_Description,
        st.meter_peak_sample_rate,
        (1, 1000),
        "",
        |s, v| s.meter_peak_sample_rate = v,
        &[s::Settings_Theme_MeterPeakSampleRate_SearchKeywords],
    );
    b.number(
        "meter_peak_change_ceiling",
        s::Settings_Theme_MeterPeakChangeCeiling_Title,
        s::Settings_Theme_MeterPeakChangeCeiling_Description,
        st.meter_peak_change_ceiling,
        (0, 100),
        "",
        |s, v| s.meter_peak_change_ceiling = v,
        &[s::Settings_Theme_MeterPeakChangeCeiling_SearchKeywords],
    );
    b.items
}

// --- Devices -------------------------------------------------------------------------------------

fn devices(ctx: &BuildContext<'_>) -> Vec<Item> {
    let st = ctx.settings;
    let mut b = PageBuilder::new("devices");
    b.header(s::Settings_Devices_SectionHeader);

    b.toggle(
        "set_default_comms_to_default",
        s::Settings_Devices_SetDefaultCommsToDefault_Title,
        s::Settings_Devices_SetDefaultCommsToDefault_Description,
        st.set_default_comms_to_default,
        |s, v| s.set_default_comms_to_default = v,
        &[s::Settings_Devices_SetDefaultCommsToDefault_SearchKeywords],
    );
    b.toggle(
        "show_not_present",
        s::Settings_Devices_ShowNotPresent_Title,
        s::Settings_Devices_ShowNotPresent_Description,
        st.show_not_present_devices,
        |s, v| s.show_not_present_devices = v,
        &[s::Settings_Devices_ShowNotPresent_SearchKeywords],
    );
    b.toggle(
        "activate_recording_devices_for_peak_meters",
        s::Settings_Devices_ActivateRecordingDevicesForPeakMeters_Title,
        s::Settings_Devices_ActivateRecordingDevicesForPeakMeters_Description,
        st.activate_recording_devices_for_peak_meters,
        |s, v| s.activate_recording_devices_for_peak_meters = v,
        &[s::Settings_Devices_ActivateRecordingDevicesForPeakMeters_SearchKeywords],
    );

    b.pair_header("visibility_header", s::Settings_Devices_VisibilityColumn_Header);
    b.pair(
        "show_recording",
        s::Settings_Devices_ShowRecording_Title,
        s::Settings_Devices_ShowRecording_Description,
        None,
        Some((st.show_recording_devices, |s, v| s.show_recording_devices = v)),
        true,
        true,
        &[s::Settings_Devices_ShowRecording_SearchKeywords],
    );
    b.pair(
        "show_disabled",
        s::Settings_Devices_ShowDisabled_Title,
        s::Settings_Devices_ShowDisabled_Description,
        Some((st.show_disabled_playback_devices, |s, v| s.show_disabled_playback_devices = v)),
        Some((st.show_disabled_recording_devices, |s, v| s.show_disabled_recording_devices = v)),
        true,
        st.show_recording_devices,
        &[s::Settings_Devices_ShowDisabled_SearchKeywords],
    );
    let hide_default_cards =
        st.show_disabled_playback_devices && (!st.show_recording_devices || st.show_disabled_recording_devices);
    let show_default_right = st.show_recording_devices && !st.show_disabled_recording_devices;
    if !hide_default_cards {
        b.pair(
            "show_default_even_if_disabled",
            s::Settings_Devices_ShowDefaultEvenIfDisabled_Title,
            s::Settings_Devices_ShowDefaultEvenIfDisabled_Description,
            Some((st.show_default_playback_device_even_if_disabled, |s, v| {
                s.show_default_playback_device_even_if_disabled = v;
            })),
            Some((st.show_default_recording_device_even_if_disabled, |s, v| {
                s.show_default_recording_device_even_if_disabled = v;
            })),
            !st.show_disabled_playback_devices,
            show_default_right,
            &[s::Settings_Devices_ShowDefaultEvenIfDisabled_SearchKeywords],
        );
        b.pair(
            "show_default_comms_even_if_disabled",
            s::Settings_Devices_ShowDefaultCommsEvenIfDisabled_Title,
            s::Settings_Devices_ShowDefaultCommsEvenIfDisabled_Description,
            Some((st.show_default_comms_playback_device_even_if_disabled, |s, v| {
                s.show_default_comms_playback_device_even_if_disabled = v;
            })),
            Some((st.show_default_comms_recording_device_even_if_disabled, |s, v| {
                s.show_default_comms_recording_device_even_if_disabled = v;
            })),
            !st.show_disabled_playback_devices,
            show_default_right,
            &[s::Settings_Devices_ShowDefaultCommsEvenIfDisabled_SearchKeywords],
        );
    }
    b.pair(
        "show_disconnected",
        s::Settings_Devices_ShowDisconnectedPlayback_Title,
        s::Settings_Devices_ShowDisconnectedPlayback_Description,
        Some((st.show_disconnected_playback_devices, |s, v| s.show_disconnected_playback_devices = v)),
        Some((st.show_disconnected_recording_devices, |s, v| s.show_disconnected_recording_devices = v)),
        true,
        st.show_recording_devices,
        &[s::Settings_Devices_ShowDisconnectedPlayback_SearchKeywords],
    );

    b.pair_header("row_buttons_header", s::Settings_Devices_RowButtons_Header);
    b.pair(
        "show_lock_button",
        s::Settings_Devices_ShowPlaybackLockButton_Title,
        s::Settings_Devices_ShowPlaybackLockButton_Description,
        Some((st.show_lock_button_for_playback, |s, v| s.show_lock_button_for_playback = v)),
        Some((st.show_lock_button_for_recording, |s, v| s.show_lock_button_for_recording = v)),
        true,
        true,
        &[s::Settings_Devices_ShowPlaybackLockButton_SearchKeywords],
    );
    b.pair(
        "show_equalizer_apo_button",
        s::Settings_Devices_ShowPlaybackEqualizerAPOButton_Title,
        s::Settings_Devices_ShowPlaybackEqualizerAPOButton_Description,
        Some((st.show_equalizer_apo_button_for_playback, |s, v| s.show_equalizer_apo_button_for_playback = v)),
        Some((st.show_equalizer_apo_button_for_recording, |s, v| s.show_equalizer_apo_button_for_recording = v)),
        true,
        true,
        &[s::Settings_Devices_ShowPlaybackEqualizerAPOButton_SearchKeywords],
    );
    b.pair(
        "show_default_device_button",
        s::Settings_Devices_ShowPlaybackDefaultDeviceButton_Title,
        s::Settings_Devices_ShowPlaybackDefaultDeviceButton_Description,
        Some((st.show_default_device_button_for_playback, |s, v| s.show_default_device_button_for_playback = v)),
        Some((st.show_default_device_button_for_recording, |s, v| s.show_default_device_button_for_recording = v)),
        true,
        true,
        &[s::Settings_Devices_ShowPlaybackDefaultDeviceButton_SearchKeywords],
    );
    b.pair(
        "show_battery_button",
        s::Settings_Devices_ShowPlaybackBatteryButton_Title,
        s::Settings_Devices_ShowPlaybackBatteryButton_Description,
        Some((st.show_battery_button_for_playback, |s, v| s.show_battery_button_for_playback = v)),
        Some((st.show_battery_button_for_recording, |s, v| s.show_battery_button_for_recording = v)),
        true,
        true,
        &[s::Settings_Devices_ShowPlaybackBatteryButton_SearchKeywords],
    );
    b.pair(
        "show_listen_button",
        s::Settings_Devices_ShowRecordingListenButton_Title,
        s::Settings_Devices_ShowRecordingListenButton_Description,
        None,
        Some((st.show_listen_button_for_recording, |s, v| s.show_listen_button_for_recording = v)),
        true,
        true,
        &[s::Settings_Devices_ShowRecordingListenButton_SearchKeywords],
    );
    b.items
}

// --- App drawers ---------------------------------------------------------------------------------

fn app_drawers(ctx: &BuildContext<'_>) -> Vec<Item> {
    let st = ctx.settings;
    let mut b = PageBuilder::new("app_drawers");
    b.header(s::Settings_DeviceAppDrawers_SectionHeader);

    b.toggle(
        "default_app_drawer_expanded",
        s::Settings_DeviceAppDrawers_DefaultAppDrawerExpanded_Title,
        s::Settings_DeviceAppDrawers_DefaultAppDrawerExpanded_Description,
        st.default_app_drawer_expanded,
        |s, v| s.default_app_drawer_expanded = v,
        &[s::Settings_DeviceAppDrawers_DefaultAppDrawerExpanded_SearchKeywords],
    );
    b.number(
        "icon_retry_interval",
        s::Settings_General_IconRetryInterval_Title,
        s::Settings_General_IconRetryInterval_Description,
        st.icon_retry_interval_ms,
        (50, 5000),
        "",
        |s, v| s.icon_retry_interval_ms = v,
        &[s::Settings_General_IconRetryInterval_SearchKeywords],
    );

    b.section("playback_header", s::Settings_Flyout_PlaybackDrawer_Header);
    b.number(
        "playback_sliders_max_apps",
        s::Settings_Flyout_AppDrawerMaxApps_Sliders_Title,
        s::Settings_Flyout_AppDrawerMaxApps_Sliders_Description,
        st.playback_app_drawer_sliders_max_apps,
        (1, 200),
        "",
        |s, v| s.playback_app_drawer_sliders_max_apps = v,
        &[s::Settings_Flyout_AppDrawerMaxApps_Sliders_SearchKeywords],
    );

    b.section("recording_header", s::Settings_Flyout_RecordingDrawer_Header);
    b.combo(
        "recording_display_type",
        s::Settings_Flyout_RecordingAppDrawerDisplayType_Title,
        s::Settings_Flyout_RecordingAppDrawerDisplayType_Description,
        &[
            (AppDrawerDisplayType::Icons, s::Settings_Flyout_RecordingAppDrawerDisplayType_Icons),
            (AppDrawerDisplayType::Sliders, s::Settings_Flyout_RecordingAppDrawerDisplayType_Sliders),
        ],
        st.recording_app_drawer_display_type,
        |s, v| s.recording_app_drawer_display_type = v,
        &[s::Settings_Flyout_RecordingAppDrawerDisplayType_SearchKeywords],
    );
    let icons = st.recording_app_drawer_display_type == AppDrawerDisplayType::Icons;
    if !icons {
        b.number(
            "recording_sliders_max_apps",
            s::Settings_Flyout_AppDrawerMaxApps_Sliders_Title,
            s::Settings_Flyout_AppDrawerMaxApps_Sliders_Description,
            st.recording_app_drawer_sliders_max_apps,
            (1, 200),
            "",
            |s, v| s.recording_app_drawer_sliders_max_apps = v,
            &[s::Settings_Flyout_AppDrawerMaxApps_Sliders_SearchKeywords],
        );
    }
    if icons {
        b.number(
            "recording_icons_max_rows",
            s::Settings_Flyout_AppDrawerMaxApps_Icons_Title,
            s::Settings_Flyout_AppDrawerMaxApps_Icons_Description,
            st.recording_app_drawer_icons_max_rows,
            (1, 200),
            "",
            |s, v| s.recording_app_drawer_icons_max_rows = v,
            &[s::Settings_Flyout_AppDrawerMaxApps_Icons_SearchKeywords],
        );
    }
    b.combo(
        "capture_activity_indicator",
        s::Settings_General_CaptureActivityIndicator_Title,
        s::Settings_General_CaptureActivityIndicator_Description,
        &[
            (CaptureActivityIndicator::DimInactive, s::Settings_General_CaptureActivityIndicator_DimInactive),
            (CaptureActivityIndicator::ActiveGlyph, s::Settings_General_CaptureActivityIndicator_ActiveGlyph),
            (CaptureActivityIndicator::HideInactive, s::Settings_General_CaptureActivityIndicator_HideInactive),
            (CaptureActivityIndicator::None, s::Settings_General_CaptureActivityIndicator_None),
        ],
        st.capture_activity_indicator,
        |s, v| s.capture_activity_indicator = v,
        &[s::Settings_General_CaptureActivityIndicator_SearchKeywords],
    );
    if icons {
        b.combo(
            "icons_center_mode",
            s::Settings_Flyout_AppDrawerIconsCentered_Title,
            s::Settings_Flyout_AppDrawerIconsCentered_Description,
            &[
                (AppDrawerIconsCenterMode::Off, s::Settings_Flyout_AppDrawerIconsCenterMode_Off),
                (AppDrawerIconsCenterMode::Centered, s::Settings_Flyout_AppDrawerIconsCenterMode_Centered),
                (AppDrawerIconsCenterMode::CenteredSoftMax, s::Settings_Flyout_AppDrawerIconsCenterMode_SoftMax),
            ],
            st.app_drawer_icons_center_mode,
            |s, v| s.app_drawer_icons_center_mode = v,
            &[s::Settings_Flyout_AppDrawerIconsCentered_SearchKeywords],
        );
    }
    // C# gates this card on the center mode alone (Maybe(mode == CenteredSoftMax)).
    if st.app_drawer_icons_center_mode == AppDrawerIconsCenterMode::CenteredSoftMax {
        b.number(
            "icons_center_soft_max",
            s::Settings_Flyout_AppDrawerIconsCenterSoftMax_Title,
            s::Settings_Flyout_AppDrawerIconsCenterSoftMax_Description,
            st.app_drawer_icons_center_soft_max,
            (1, 16),
            "",
            |s, v| s.app_drawer_icons_center_soft_max = v,
            &[s::Settings_Flyout_AppDrawerIconsCenterSoftMax_SearchKeywords],
        );
    }
    if icons {
        b.number(
            "icon_scale",
            s::Settings_Flyout_AppDrawerIconScale_Title,
            s::Settings_Flyout_AppDrawerIconScale_Description,
            st.app_drawer_icon_scale_percent,
            (50, 200),
            "",
            |s, v| s.app_drawer_icon_scale_percent = v,
            &[s::Settings_Flyout_AppDrawerIconScale_SearchKeywords],
        );
        b.combo(
            "icon_stack_direction",
            s::Settings_Flyout_AppDrawerStackDirection_Title,
            s::Settings_Flyout_AppDrawerStackDirection_Description,
            &[
                (AppDrawerStackDirection::Auto, s::Settings_Flyout_AppDrawerStackDirection_Auto),
                (AppDrawerStackDirection::TopBottom, s::Settings_Flyout_AppDrawerStackDirection_TopBottom),
                (AppDrawerStackDirection::BottomTop, s::Settings_Flyout_AppDrawerStackDirection_BottomTop),
                (AppDrawerStackDirection::LeftRight, s::Settings_Flyout_AppDrawerStackDirection_LeftRight),
                (AppDrawerStackDirection::RightLeft, s::Settings_Flyout_AppDrawerStackDirection_RightLeft),
            ],
            st.app_drawer_stack_direction,
            |s, v| s.app_drawer_stack_direction = v,
            &[s::Settings_Flyout_AppDrawerStackDirection_SearchKeywords],
        );
    }
    let vertical = matches!(
        st.app_drawer_stack_direction,
        AppDrawerStackDirection::LeftRight | AppDrawerStackDirection::RightLeft
    );
    if icons && !vertical {
        b.number(
            "icons_per_row",
            s::Settings_Flyout_AppDrawerIconsPerRow_Title,
            s::Settings_Flyout_AppDrawerIconsPerRow_Description,
            st.app_drawer_icons_per_row,
            (1, 16),
            "",
            |s, v| s.app_drawer_icons_per_row = v,
            &[s::Settings_Flyout_AppDrawerIconsPerRow_SearchKeywords],
        );
    }
    if icons && vertical {
        b.number(
            "icons_per_column",
            s::Settings_Flyout_AppDrawerIconsPerColumn_Title,
            s::Settings_Flyout_AppDrawerIconsPerColumn_Description,
            st.app_drawer_icons_per_row,
            (1, 16),
            "",
            |s, v| s.app_drawer_icons_per_row = v,
            &[s::Settings_Flyout_AppDrawerIconsPerColumn_SearchKeywords],
        );
    }
    b.items
}

/// A modified-action card: id, title, current value, setter.
type ActionCard<T> = (&'static str, &'static str, T, fn(&mut AppSettings, T));

// --- Tray icon -----------------------------------------------------------------------------------

fn tray_icon(ctx: &BuildContext<'_>) -> Vec<Item> {
    let st = ctx.settings;
    let mut b = PageBuilder::new("tray_icon");
    b.header(s::Settings_TrayIcon_SectionHeader);

    b.toggle(
        "mouse_wheel",
        s::Settings_TrayIcon_MouseWheel_Title,
        s::Settings_TrayIcon_MouseWheel_Description,
        st.tray_scroll_enabled,
        |s, v| s.tray_scroll_enabled = v,
        &[s::Settings_TrayIcon_MouseWheel_SearchKeywords],
    );
    if st.tray_scroll_enabled {
        b.number(
            "wheel_volume_step",
            s::Settings_General_WheelVolumeStepPercent_Title,
            s::Settings_General_WheelVolumeStepPercent_Description,
            st.wheel_volume_step_percent,
            (1, 100),
            s::Common_PercentSuffix,
            |s, v| s.wheel_volume_step_percent = v,
            &[s::Settings_General_WheelVolumeStepPercent_SearchKeywords],
        );
        b.number(
            "wheel_volume_step_fine",
            s::Settings_General_WheelVolumeStepFinePercent_Title,
            s::Settings_General_WheelVolumeStepFinePercent_Description,
            st.wheel_volume_step_fine_percent,
            (1, 100),
            s::Common_PercentSuffix,
            |s, v| s.wheel_volume_step_fine_percent = v,
            &[s::Settings_General_WheelVolumeStepFinePercent_SearchKeywords],
        );
        b.number(
            "wheel_volume_step_coarse",
            s::Settings_General_WheelVolumeStepCoarsePercent_Title,
            s::Settings_General_WheelVolumeStepCoarsePercent_Description,
            st.wheel_volume_step_coarse_percent,
            (1, 100),
            s::Common_PercentSuffix,
            |s, v| s.wheel_volume_step_coarse_percent = v,
            &[s::Settings_General_WheelVolumeStepCoarsePercent_SearchKeywords],
        );
        b.toggle(
            "precision_touchpad",
            s::Settings_TrayIcon_PrecisionTouchpadScroll_Title,
            s::Settings_TrayIcon_PrecisionTouchpadScroll_Description,
            st.precision_touchpad_scroll_enabled,
            |s, v| s.precision_touchpad_scroll_enabled = v,
            &[s::Settings_TrayIcon_PrecisionTouchpadScroll_SearchKeywords],
        );
    }
    if st.tray_scroll_enabled && st.precision_touchpad_scroll_enabled {
        b.number(
            "touchpad_units_per_step",
            s::Settings_TrayIcon_PrecisionTouchpadUnitsPerScrollStep_Title,
            s::Settings_TrayIcon_PrecisionTouchpadUnitsPerScrollStep_Description,
            st.precision_touchpad_units_per_scroll_step,
            (1, 1000),
            s::Common_PercentSuffix,
            |s, v| s.precision_touchpad_units_per_scroll_step = v,
            &[s::Settings_TrayIcon_PrecisionTouchpadUnitsPerScrollStep_SearchKeywords],
        );
    }

    b.section("context_menu_header", s::Common_ContextMenu_Header);
    b.combo(
        "menu_position",
        s::Settings_TrayIcon_MenuPosition_Title,
        s::Settings_TrayIcon_MenuPosition_Description,
        &[
            (ContextMenuPosition::Classic, s::Settings_TrayIcon_MenuPosition_Classic),
            (ContextMenuPosition::Modern, s::Settings_TrayIcon_MenuPosition_Modern),
        ],
        st.common.context_menu_position,
        |s, v| s.common.context_menu_position = v,
        &[s::Settings_TrayIcon_MenuPosition_SearchKeywords],
    );
    let name_styles = [
        (TrayMenuDeviceNameStyle::NameAndModel, s::Settings_TrayIcon_DeviceName_NameAndModel),
        (TrayMenuDeviceNameStyle::Name, s::Settings_TrayIcon_DeviceName_Name),
        (TrayMenuDeviceNameStyle::Model, s::Settings_TrayIcon_DeviceName_Model),
    ];
    b.combo(
        "playback_device_name",
        s::Settings_TrayIcon_PlaybackDeviceName_Title,
        s::Settings_TrayIcon_PlaybackDeviceName_Description,
        &name_styles,
        st.tray_menu_playback_device_name_style,
        |s, v| s.tray_menu_playback_device_name_style = v,
        &[s::Settings_TrayIcon_PlaybackDeviceName_SearchKeywords],
    );
    b.combo(
        "recording_device_name",
        s::Settings_TrayIcon_RecordingDeviceName_Title,
        s::Settings_TrayIcon_RecordingDeviceName_Description,
        &name_styles,
        st.tray_menu_recording_device_name_style,
        |s, v| s.tray_menu_recording_device_name_style = v,
        &[s::Settings_TrayIcon_RecordingDeviceName_SearchKeywords],
    );
    b.number(
        "device_name_max_length",
        s::Settings_TrayIcon_DeviceNameMaxLength_Title,
        s::Settings_TrayIcon_DeviceNameMaxLength_Description,
        st.tray_menu_device_name_max_length,
        (3, 200),
        "",
        |s, v| s.tray_menu_device_name_max_length = v,
        &[s::Settings_TrayIcon_DeviceNameMaxLength_SearchKeywords],
    );
    b.toggle(
        "show_recording_link",
        s::Settings_Devices_ShowTrayRecordingLink_Title,
        s::Settings_Devices_ShowTrayRecordingLink_Description,
        st.show_tray_menu_recording_link,
        |s, v| s.show_tray_menu_recording_link = v,
        &[s::Settings_Devices_ShowTrayRecordingLink_SearchKeywords],
    );
    b.toggle(
        "show_sounds_link",
        s::Settings_Devices_ShowTraySoundsLink_Title,
        s::Settings_Devices_ShowTraySoundsLink_Description,
        st.show_tray_menu_sounds_link,
        |s, v| s.show_tray_menu_sounds_link = v,
        &[s::Settings_Devices_ShowTraySoundsLink_SearchKeywords],
    );
    b.toggle(
        "show_communications_link",
        s::Settings_Devices_ShowTrayCommunicationsLink_Title,
        s::Settings_Devices_ShowTrayCommunicationsLink_Description,
        st.show_tray_menu_communications_link,
        |s, v| s.show_tray_menu_communications_link = v,
        &[s::Settings_Devices_ShowTrayCommunicationsLink_SearchKeywords],
    );
    b.toggle(
        "show_device_links",
        s::Settings_Devices_ShowTrayDeviceLinks_Title,
        s::Settings_Devices_ShowTrayDeviceLinks_Description,
        st.show_tray_menu_device_links,
        |s, v| s.show_tray_menu_device_links = v,
        &[s::Settings_Devices_ShowTrayDeviceLinks_SearchKeywords],
    );

    b.section("modified_actions_header", s::Settings_TrayIcon_ModifiedActions_Header);
    b.text("modified_actions_description", s::Settings_TrayIcon_ModifiedActions_Description, 8.0);
    let wheel_options = [
        (TrayWheelVolumeStep::Nothing, s::Settings_TrayIcon_ClickAction_Nothing),
        (TrayWheelVolumeStep::Default, s::Settings_General_WheelVolumeStepPercent_Title),
        (TrayWheelVolumeStep::Fine, s::Settings_General_WheelVolumeStepFinePercent_Title),
        (TrayWheelVolumeStep::Coarse, s::Settings_General_WheelVolumeStepCoarsePercent_Title),
    ];
    let wheel_cards: [ActionCard<TrayWheelVolumeStep>; 3] = [
        ("wheel_action", s::Settings_TrayIcon_MouseWheel_Title, st.tray_wheel_action, |s, v| s.tray_wheel_action = v),
        ("ctrl_wheel_action", s::Settings_TrayIcon_CtrlMouseWheel_Title, st.tray_ctrl_wheel_action, |s, v| {
            s.tray_ctrl_wheel_action = v;
        }),
        ("alt_wheel_action", s::Settings_TrayIcon_AltMouseWheel_Title, st.tray_alt_wheel_action, |s, v| {
            s.tray_alt_wheel_action = v;
        }),
    ];
    for (id, title, value, set) in wheel_cards {
        b.combo(id, title, "", &wheel_options, value, set, &[s::Settings_TrayIcon_ModifiedWheelActions_SearchKeywords]);
        // AddTrayWheelActionCard: card.IsEnabled = TrayScrollEnabled.
        b.last_mut().view.enabled = st.tray_scroll_enabled;
    }
    let click_options = [
        (TrayClickAction::Nothing, s::Settings_TrayIcon_ClickAction_Nothing),
        (TrayClickAction::OpenSettings, s::Settings_TrayIcon_ClickAction_OpenSettings),
    ];
    let click_cards: [ActionCard<TrayClickAction>; 7] = [
        ("ctrl_left_click_action", s::Settings_TrayIcon_CtrlLeftClick_Title, st.tray_ctrl_left_click_action, |s, v| {
            s.tray_ctrl_left_click_action = v;
        }),
        ("alt_left_click_action", s::Settings_TrayIcon_AltLeftClick_Title, st.tray_alt_left_click_action, |s, v| {
            s.tray_alt_left_click_action = v;
        }),
        (
            "ctrl_right_click_action",
            s::Settings_TrayIcon_CtrlRightClick_Title,
            st.tray_ctrl_right_click_action,
            |s, v| {
                s.tray_ctrl_right_click_action = v;
            },
        ),
        ("alt_right_click_action", s::Settings_TrayIcon_AltRightClick_Title, st.tray_alt_right_click_action, |s, v| {
            s.tray_alt_right_click_action = v;
        }),
        ("double_click_action", s::Settings_TrayIcon_DoubleLeftClick_Title, st.tray_double_click_action, |s, v| {
            s.tray_double_click_action = v;
        }),
        (
            "ctrl_double_click_action",
            s::Settings_TrayIcon_CtrlDoubleLeftClick_Title,
            st.tray_ctrl_double_left_click_action,
            |s, v| s.tray_ctrl_double_left_click_action = v,
        ),
        (
            "alt_double_click_action",
            s::Settings_TrayIcon_AltDoubleLeftClick_Title,
            st.tray_alt_double_left_click_action,
            |s, v| s.tray_alt_double_left_click_action = v,
        ),
    ];
    for (id, title, value, set) in click_cards {
        b.combo(id, title, "", &click_options, value, set, &[s::Settings_TrayIcon_ModifiedClickActions_SearchKeywords]);
    }
    b.items
}

// --- Hotkeys -------------------------------------------------------------------------------------

/// Volume's hotkey rows (`HotkeysPage.cs`): title, description, keywords.
pub(crate) const HOTKEY_ROWS: [(HotkeyAction, &str, &str, &str); 2] = [
    (
        HotkeyAction::OpenFlyout,
        s::Settings_Hotkeys_OpenFlyout_Title,
        s::Settings_Hotkeys_OpenFlyout_Description,
        s::Settings_Hotkeys_OpenFlyout_SearchKeywords,
    ),
    (
        HotkeyAction::OpenSettings,
        s::Settings_Hotkeys_OpenSettings_Title,
        s::Settings_Hotkeys_OpenSettings_Description,
        s::Settings_Hotkeys_OpenSettings_SearchKeywords,
    ),
];

/// `TitleOf`: the row title of an action (the XML name for an action without a row).
pub(crate) fn hotkey_title(action: HotkeyAction) -> &'static str {
    HOTKEY_ROWS
        .iter()
        .find(|row| row.0 == action)
        .map_or_else(|| tadn_common::settings::XmlEnum::xml_name(action), |row| row.1)
}

fn hotkeys(ctx: &BuildContext<'_>) -> Vec<Item> {
    let mut b = PageBuilder::new("hotkeys");
    b.header(s::Settings_Hotkeys_SectionHeader);
    // HotkeysSection.DescriptionMargin 0,0,0,16.
    b.text("description", s::Settings_Hotkeys_SectionDescription, 16.0);
    let modifier_options = ctx.hotkeys.modifier_options();
    for (action, title, description, keywords) in HOTKEY_ROWS {
        let data = ctx.hotkeys.row_view(action, &ctx.settings.common.hotkeys);
        let view = hotkey_view(title, description, data, modifier_options.clone());
        let id = format!("{action:?}");
        // MarkCard(card, row.Title, [row.Description, .. row.SearchKeywords]).
        b.push(&id, Role::Card, view, Action::Hotkey(action), &[description, keywords]);
    }
    b.items
}

// --- Theme ---------------------------------------------------------------------------------------

/// `SliderThumbDisplayName`.
pub(crate) fn slider_thumb_display_name(name: &str) -> &str {
    match name {
        "Capsule" => s::Settings_Theme_SliderThumb_Capsule,
        "Circle" => s::Settings_Theme_SliderThumb_Circle,
        "Diamond" => s::Settings_Theme_SliderThumb_Diamond,
        "Star" => s::Settings_Theme_SliderThumb_Star,
        "Square" => s::Settings_Theme_SliderThumb_Square,
        "Heart" => s::Settings_Theme_SliderThumb_Heart,
        _ => name,
    }
}

/// The fallback (theme default) colors of a variant target: (light, dark).
pub(crate) fn variant_fallback(theme: &ThemeFile, target: ColorTarget) -> (Argb, Argb) {
    match target {
        ColorTarget::BackgroundColor => (theme.background.light, theme.background.dark),
        _ => (theme.foreground.light, theme.foreground.dark),
    }
}

pub(crate) fn variant_value(settings: &AppSettings, target: ColorTarget) -> NullableThemeColor {
    match target {
        ColorTarget::TextColor => settings.common.text_color,
        ColorTarget::BackgroundColor => settings.common.background_color,
        _ => settings.common.tray_icon_color,
    }
}

pub(crate) fn variant_value_mut(settings: &mut AppSettings, target: ColorTarget) -> &mut NullableThemeColor {
    match target {
        ColorTarget::TextColor => &mut settings.common.text_color,
        ColorTarget::BackgroundColor => &mut settings.common.background_color,
        _ => &mut settings.common.tray_icon_color,
    }
}

/// Single-color targets: (stored value, default).
pub(crate) fn single_value(settings: &AppSettings, target: ColorTarget) -> (Option<Argb>, Argb) {
    match target {
        ColorTarget::MeterPeakStereo => (settings.meter_peak_stereo_color, METER_PEAK_STEREO_COLOR_DEFAULT),
        _ => (settings.meter_peak_color, METER_PEAK_COLOR_DEFAULT),
    }
}

pub(crate) fn single_value_mut(settings: &mut AppSettings, target: ColorTarget) -> &mut Option<Argb> {
    match target {
        ColorTarget::MeterPeakStereo => &mut settings.meter_peak_stereo_color,
        _ => &mut settings.meter_peak_color,
    }
}

#[allow(clippy::too_many_arguments)]
fn variant_color_card(
    b: &mut PageBuilder,
    ctx: &BuildContext<'_>,
    id: &str,
    title: &str,
    description: &str,
    light_tooltip: &str,
    dark_tooltip: &str,
    target: ColorTarget,
    keywords: &str,
) {
    let fallback = variant_fallback(ctx.theme, target);
    let value = variant_value(ctx.settings, target);
    let (current, default, tooltip) =
        if ctx.is_light { (value.light, fallback.0, light_tooltip) } else { (value.dark, fallback.1, dark_tooltip) };
    b.color(id, title, description, current.unwrap_or(default), current.is_none(), tooltip, target, &[keywords]);
}

fn theme(ctx: &BuildContext<'_>) -> Vec<Item> {
    let st = ctx.settings;
    let mut b = PageBuilder::new("theme");
    b.header(s::Settings_Theme_SectionHeader);

    // AddAppearanceSection.
    b.section("appearance_header", s::Settings_Theme_Appearance_Header);
    b.combo(
        "theme_mode",
        s::Settings_Theme_ThemeStyle_Title,
        s::Settings_Theme_ThemeStyle_Description,
        &[
            (ThemeMode::System, s::Settings_Theme_ThemeStyle_System),
            (ThemeMode::Light, s::Settings_Theme_ThemeStyle_Light),
            (ThemeMode::Dark, s::Settings_Theme_ThemeStyle_Dark),
        ],
        st.common.theme_mode,
        |s, v| s.common.theme_mode = v,
        &[s::Settings_Theme_ThemeStyle_SearchKeywords],
    );
    b.toggle(
        "windows11_navigation",
        s::Settings_Theme_Windows11Navigation_Title,
        s::Settings_Theme_Windows11Navigation_Description,
        st.common.use_windows11_settings_navigation,
        |s, v| s.common.use_windows11_settings_navigation = v,
        &[s::Settings_Theme_Windows11Navigation_SearchKeywords],
    );
    variant_color_card(
        &mut b,
        ctx,
        "text_color",
        s::Settings_Theme_TextColor_Title,
        s::Settings_Theme_TextColor_Description,
        s::Settings_Theme_TextColor_LightTooltip,
        s::Settings_Theme_TextColor_DarkTooltip,
        ColorTarget::TextColor,
        s::Settings_Theme_TextColor_SearchKeywords,
    );
    variant_color_card(
        &mut b,
        ctx,
        "background_color",
        s::Settings_Theme_BackgroundColor_Title,
        s::Settings_Theme_BackgroundColor_Description,
        s::Settings_Theme_BackgroundColor_LightTooltip,
        s::Settings_Theme_BackgroundColor_DarkTooltip,
        ColorTarget::BackgroundColor,
        s::Settings_Theme_BackgroundColor_SearchKeywords,
    );
    b.toggle(
        "rounded_corners",
        s::Settings_Theme_RoundedCorners_Title,
        s::Settings_Theme_RoundedCorners_Description,
        st.common.enable_rounded_corners,
        |s, v| s.common.enable_rounded_corners = v,
        &[s::Settings_Theme_RoundedCorners_SearchKeywords],
    );
    b.combo(
        "animations",
        s::Settings_Theme_Animations_Title,
        s::Settings_Theme_Animations_Description,
        &[
            (AnimationMode::System, s::Settings_Theme_Animations_System),
            (AnimationMode::Disabled, s::Settings_Theme_Animations_Disabled),
            (AnimationMode::Enabled, s::Settings_Theme_Animations_Enabled),
        ],
        st.common.animation_mode,
        |s, v| s.common.animation_mode = v,
        &[s::Settings_Theme_Animations_SearchKeywords],
    );
    b.number(
        "tool_tip_show_delay",
        s::Settings_Theme_ToolTipShowDelay_Title,
        s::Settings_Theme_ToolTipShowDelay_Description,
        st.common.tool_tip_show_delay_ms,
        TOOL_TIP_SHOW_DELAY_MS_RANGE,
        s::Common_MillisecondsSuffix,
        |s, v| s.common.tool_tip_show_delay_ms = v,
        &[s::Settings_Theme_ToolTipShowDelay_SearchKeywords],
    );

    // AddContextMenuAppearanceSection.
    b.section("context_menu_header", s::Settings_General_ContextMenu_Header);
    b.number(
        "context_menu_font_size",
        s::Settings_Theme_FontSize_Title,
        s::Settings_Theme_FontSize_Description,
        st.common.context_menu_font_size,
        CONTEXT_MENU_FONT_SIZE_RANGE,
        "",
        |s, v| s.common.context_menu_font_size = v,
        &[s::Settings_Theme_FontSize_SearchKeywords],
    );

    b.section("flyout_header", s::Settings_Theme_Flyout_Header);
    let catalog = SliderThumbOption::catalog_with(&st.slider_thumb);
    let mut view = base_view(
        ItemKind::ComboCard,
        s::Settings_Theme_SliderIndicator_Title,
        s::Settings_Theme_SliderIndicator_Description,
    );
    view.options =
        model(catalog.iter().map(|option| thumb_option(option, slider_thumb_display_name(&option.name))).collect());
    view.selected = catalog.iter().position(|o| o.name == st.slider_thumb.name).map_or(-1, |i| i as i32);
    view.show_previews = true;
    b.push(
        "slider_indicator",
        Role::Card,
        view,
        Action::SliderThumb(catalog),
        &[s::Settings_Theme_SliderIndicator_SearchKeywords],
    );

    b.section("tray_icon_header", s::Settings_Theme_TrayIcon_Header);
    variant_color_card(
        &mut b,
        ctx,
        "tray_icon_color",
        s::Settings_Theme_StaticIconColor_Title,
        s::Settings_Theme_StaticIconColor_Description,
        s::Settings_Theme_StaticIconColor_LightTooltip,
        s::Settings_Theme_StaticIconColor_DarkTooltip,
        ColorTarget::TrayIconColor,
        s::Settings_Theme_StaticIconColor_SearchKeywords,
    );

    b.section("meter_peak_header", s::Settings_Theme_MeterPeak_Header);
    b.color(
        "meter_peak_color",
        s::Settings_Theme_MeterPeakColor_Title,
        s::Settings_Theme_MeterPeakColor_Description,
        st.effective_meter_peak_color(),
        false,
        s::Settings_Theme_MeterPeakColor_Tooltip,
        ColorTarget::MeterPeak,
        &[s::Settings_Theme_MeterPeakColor_SearchKeywords],
    );
    b.color(
        "meter_peak_stereo_color",
        s::Settings_Theme_MeterPeakStereoColor_Title,
        s::Settings_Theme_MeterPeakStereoColor_Description,
        st.effective_meter_peak_stereo_color(),
        false,
        s::Settings_Theme_MeterPeakStereoColor_Tooltip,
        ColorTarget::MeterPeakStereo,
        &[s::Settings_Theme_MeterPeakStereoColor_SearchKeywords],
    );
    b.items
}

// --- About ---------------------------------------------------------------------------------------

/// Build facts shown on the About page (`BuildInfo`), stamped by `build.rs`.
pub(crate) struct BuildFacts {
    pub build_number: u32,
    pub commit_hash: &'static str,
    pub runtime: &'static str,
}

pub(crate) const BUILD: BuildFacts = BuildFacts {
    build_number: match option_env!("TADN_BUILD_NUMBER") {
        Some(text) => parse_u32(text),
        None => 0,
    },
    commit_hash: match option_env!("TADN_COMMIT_HASH") {
        Some(hash) => hash,
        None => "",
    },
    runtime: env!("TADN_RUNTIME_DESCRIPTION"),
};

const fn parse_u32(text: &str) -> u32 {
    let bytes = text.as_bytes();
    let mut value = 0u32;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if !b.is_ascii_digit() {
            return 0;
        }
        value = value * 10 + (b - b'0') as u32;
        i += 1;
    }
    value
}

fn about(_ctx: &BuildContext<'_>) -> Vec<Item> {
    let mut b = PageBuilder::new("about");
    b.header(s::Settings_About_SectionHeader);

    b.push(
        "app",
        Role::Supporting,
        base_view(ItemKind::AboutApp, vtadn_model::IDENTITY.app_name, s::Settings_About_Tagline),
        Action::None,
        &[],
    );
    let about_row = |b: &mut PageBuilder, id: &str, label: &str, value: &str, link: Option<&'static str>| {
        let mut view = base_view(ItemKind::AboutRow, label, value);
        view.is_link = link.is_some();
        let action = link.map_or(Action::None, Action::OpenLink);
        b.push(id, Role::Supporting, view, action, &[]);
    };
    let build =
        if BUILD.build_number == 0 { DEVELOPMENT_BUILD_LABEL.to_owned() } else { BUILD.build_number.to_string() };
    about_row(&mut b, "build", s::Settings_About_BuildLabel, &build, None);
    if BUILD.build_number == 0 {
        let hash = if BUILD.commit_hash.trim().is_empty() {
            s::Settings_About_CommitHashUnavailable
        } else {
            BUILD.commit_hash
        };
        about_row(&mut b, "commit", s::Settings_About_CommitHashLabel, hash, None);
    }
    about_row(&mut b, "runtime", s::Settings_About_RuntimeLabel, BUILD.runtime, None);
    about_row(&mut b, "author", s::Settings_About_AuthorLabel, tadn_common::identity::PUBLISHER, None);
    about_row(
        &mut b,
        "github",
        s::Settings_About_GithubLabel,
        tadn_common::identity::REPOSITORY_URL,
        Some(tadn_common::identity::REPOSITORY_URL),
    );

    b.section("files_header", s::Settings_About_Files_Header);
    b.button(
        "open_settings_folder",
        s::Settings_About_OpenSettingsFolder_Title,
        s::Settings_About_OpenSettingsFolder_Description,
        s::Settings_About_OpenSettingsFolder_Button,
        Action::OpenFolder,
        &[s::Settings_About_OpenSettingsFolder_SearchKeywords],
    );

    b.section("known_issues_header", s::Settings_About_KnownIssues_Header);
    // BuildKnownIssueCard: a raw card (not search-marked; the section's content is one entry).
    b.push(
        "known_issue_bluetooth_codec",
        Role::Supporting,
        base_view(
            ItemKind::TextCard,
            s::Settings_About_BluetoothCodecNotDisplaying_Title,
            s::Settings_About_BluetoothCodecNotDisplaying_Description,
        ),
        Action::None,
        &[],
    );
    b.items
}
