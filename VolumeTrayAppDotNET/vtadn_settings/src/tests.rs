//! Page model tests: search metadata on every card, every documented setting reachable from a
//! card, the "only shown while" rules, and search results.

use std::collections::{BTreeSet, HashMap};

use tadn_common::hotkey::{HotkeyBinding, MOD_CONTROL};
use tadn_common::settings::RenderingBackend;
use tadn_common::theme::{Argb, ThemeFile};
use vtadn_model::settings::{AppDrawerDisplayType, AppDrawerStackDirection, AppSettings};

use crate::SettingsPage;
use crate::hotkeys::HotkeyUi;
use crate::items::{Action, Item, Role};
use crate::pages::{BuildContext, PAGES, build_page};
use crate::search::{all_entries, build_results};
use crate::ui::ItemKind;

fn with_ctx<R>(settings: &AppSettings, f: impl FnOnce(&BuildContext<'_>) -> R) -> R {
    let theme = ThemeFile::default();
    let hotkeys = HotkeyUi::new();
    let ctx = BuildContext {
        settings,
        is_light: false,
        theme: &theme,
        startup_enabled: true,
        startup_target: Some(r"C:\Apps\VolumeTrayAppDotNET.exe".to_owned()),
        hotkeys: &hotkeys,
    };
    f(&ctx)
}

fn page_items(settings: &AppSettings, page: SettingsPage) -> Vec<Item> {
    with_ctx(settings, |ctx| build_page(page, ctx))
}

fn titles(items: &[Item]) -> Vec<String> {
    items.iter().map(|i| i.view.title.to_string()).collect()
}

fn has_card(settings: &AppSettings, page: SettingsPage, id: &str) -> bool {
    page_items(settings, page).iter().any(|i| i.id == id)
}

/// Every settings state one card edit away from `settings`.
fn edits(item: &Item, settings: &AppSettings) -> Vec<AppSettings> {
    let v = &item.view;
    let mut out = Vec::new();
    let mut with = |f: &dyn Fn(&mut AppSettings)| {
        let mut next = settings.clone();
        f(&mut next);
        next.clamp_all();
        out.push(next);
    };
    match &item.action {
        Action::Bool(f) => with(&|s| f(s, !v.checked)),
        Action::Int(f) => with(&|s| f(s, if v.number == v.minimum { v.maximum } else { v.minimum })),
        Action::Combo(f) => {
            for index in 0..slint::Model::row_count(&v.options) {
                with(&|s| f(s, index));
            }
        }
        Action::Pair { left, right } => {
            if let Some(f) = left {
                with(&|s| f(s, !v.left_value));
            }
            if let Some(f) = right {
                with(&|s| f(s, !v.right_value));
            }
        }
        Action::Color(target) => with(&|s| {
            if target.is_variant() {
                let pair = crate::pages::variant_value_mut(s, *target);
                pair.light = Some(Argb(0xFF12_3456));
                pair.dark = Some(Argb(0xFF12_3456));
            } else {
                *crate::pages::single_value_mut(s, *target) = Some(Argb(0xFF12_3456));
            }
        }),
        Action::SliderThumb(catalog) => {
            for option in catalog {
                with(&|s| s.slider_thumb = option.clone());
            }
        }
        Action::Startup => with(&|s| s.common.run_on_startup = !s.common.run_on_startup),
        Action::Language(tags) => {
            for tag in tags {
                with(&|s| s.common.language = (*tag).to_owned());
            }
        }
        Action::RenderingBackend => with(&|s| s.common.rendering_backend = RenderingBackend::Software),
        Action::Hotkey(action) => with(&|s| {
            s.common.hotkeys.push(HotkeyBinding {
                action: *action,
                modifiers: MOD_CONTROL,
                virtual_key: 0x41,
                binding_id: 1,
                ..HotkeyBinding::default()
            });
        }),
        Action::None | Action::OpenFolder | Action::OpenLink(_) | Action::Navigate(..) => {}
    }
    out
}

/// Top-level field paths (`common.x` for common fields) with their Debug text.
fn field_values(settings: &AppSettings) -> HashMap<String, String> {
    let text = format!("{settings:#?}");
    let mut values: HashMap<String, String> = HashMap::new();
    let (mut level1, mut level2) = (String::new(), String::new());
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();
        let name = trimmed.split_once(": ").map(|(n, _)| n.to_owned());
        if indent == 4 {
            if let Some(name) = &name {
                level1 = name.clone();
                level2.clear();
            }
        } else if indent == 8
            && level1 == "common"
            && let Some(name) = &name
        {
            level2 = name.clone();
        }
        let key = if level1 == "common" && !level2.is_empty() { format!("common.{level2}") } else { level1.clone() };
        values.entry(key).or_default().push_str(trimmed);
    }
    values
}

fn changed_fields(a: &AppSettings, b: &AppSettings) -> BTreeSet<String> {
    let (a, b) = (field_values(a), field_values(b));
    a.iter().filter(|(k, v)| b.get(*k) != Some(*v)).map(|(k, _)| k.clone()).collect()
}

/// User-facing settings listed in `documentation/docs/apps/volume/settings.md` plus the shared
/// cards of `shared/{settings-window,themes,hotkeys}.md` (installation/update cards excluded).
const DOCUMENTED_FIELDS: &[&str] = &[
    "play_device_volume_change_sound",
    "play_tray_scroll_volume_change_sound",
    "play_app_volume_change_sound",
    "suppress_device_volume_change_sound_when_audio_playing",
    "ding_suppression_peak_threshold_percent",
    "use_logarithmic_volume_scale",
    "wheel_volume_step_percent",
    "restore_flyout_undocked_on_startup",
    "allow_flyout_undock",
    "clamp_undocked_flyout_to_screen",
    "flyout_communications_button_visibility",
    "show_recording_devices_in_flyout",
    "flyout_disconnected_bluetooth_device_visibility",
    "collapse_disconnected_bluetooth_devices_in_flyout",
    "show_bluetooth_devices_only_when_bluetooth_is_on",
    "show_bluetooth_radio_button_in_flyout_header",
    "flyout_bluetooth_radio_button_click_gesture",
    "use_dynamic_playback_volume_glyph_in_flyout",
    "show_device_format_text",
    "show_device_codec_text",
    "sound_settings_target",
    "flyout_device_layout",
    "flyout_device_title_position",
    "flyout_device_sort",
    "intermix_recording_with_playback_in_flyout",
    "flyout_header_at_bottom",
    "unified_peak_meter",
    "unified_meter_low_channel_bias_multiplier",
    "meter_peak_fps",
    "meter_peak_sample_rate",
    "meter_peak_change_ceiling",
    "set_default_comms_to_default",
    "show_not_present_devices",
    "activate_recording_devices_for_peak_meters",
    "show_recording_devices",
    "show_disabled_playback_devices",
    "show_disabled_recording_devices",
    "show_default_playback_device_even_if_disabled",
    "show_default_recording_device_even_if_disabled",
    "show_default_comms_playback_device_even_if_disabled",
    "show_default_comms_recording_device_even_if_disabled",
    "show_disconnected_playback_devices",
    "show_disconnected_recording_devices",
    "show_lock_button_for_playback",
    "show_lock_button_for_recording",
    "show_equalizer_apo_button_for_playback",
    "show_equalizer_apo_button_for_recording",
    "show_default_device_button_for_playback",
    "show_default_device_button_for_recording",
    "show_battery_button_for_playback",
    "show_battery_button_for_recording",
    "show_listen_button_for_recording",
    "default_app_drawer_expanded",
    "icon_retry_interval_ms",
    "playback_app_drawer_sliders_max_apps",
    "recording_app_drawer_display_type",
    "recording_app_drawer_sliders_max_apps",
    "recording_app_drawer_icons_max_rows",
    "capture_activity_indicator",
    "app_drawer_icons_center_mode",
    "app_drawer_icons_center_soft_max",
    "app_drawer_icon_scale_percent",
    "app_drawer_stack_direction",
    "app_drawer_icons_per_row",
    "tray_scroll_enabled",
    "wheel_volume_step_fine_percent",
    "wheel_volume_step_coarse_percent",
    "precision_touchpad_scroll_enabled",
    "precision_touchpad_units_per_scroll_step",
    "tray_menu_playback_device_name_style",
    "tray_menu_recording_device_name_style",
    "tray_menu_device_name_max_length",
    "show_tray_menu_recording_link",
    "show_tray_menu_sounds_link",
    "show_tray_menu_communications_link",
    "show_tray_menu_device_links",
    "tray_wheel_action",
    "tray_ctrl_wheel_action",
    "tray_alt_wheel_action",
    "tray_ctrl_left_click_action",
    "tray_alt_left_click_action",
    "tray_ctrl_right_click_action",
    "tray_alt_right_click_action",
    "tray_double_click_action",
    "tray_ctrl_double_left_click_action",
    "tray_alt_double_left_click_action",
    "slider_thumb",
    "meter_peak_color",
    "meter_peak_stereo_color",
    "common.language",
    "common.run_on_startup",
    "common.rendering_backend",
    "common.keep_flyout_warm",
    "common.keep_tray_context_menu_warm",
    "common.theme_mode",
    "common.use_windows11_settings_navigation",
    "common.text_color",
    "common.background_color",
    "common.enable_rounded_corners",
    "common.animation_mode",
    "common.tool_tip_show_delay_ms",
    "common.context_menu_font_size",
    "common.context_menu_position",
    "common.tray_icon_color",
    "common.hotkeys",
];

#[test]
fn every_documented_setting_is_reachable_from_a_card() {
    // Breadth-first over card edits (two levels reach every "only shown while" card).
    let mut reached: BTreeSet<String> = BTreeSet::new();
    let mut frontier = vec![AppSettings::default()];
    for _ in 0..2 {
        let mut next = Vec::new();
        for state in &frontier {
            for page in PAGES {
                for item in page_items(state, page) {
                    for edited in edits(&item, state) {
                        reached.extend(changed_fields(state, &edited));
                        next.push(edited);
                    }
                }
            }
        }
        frontier = next;
    }
    let missing: Vec<&&str> = DOCUMENTED_FIELDS.iter().filter(|f| !reached.contains(**f)).collect();
    assert!(missing.is_empty(), "settings without a card: {missing:?}");
}

#[test]
fn every_card_has_search_metadata() {
    let mut states = vec![AppSettings::default()];
    states.push(AppSettings {
        recording_app_drawer_display_type: AppDrawerDisplayType::Sliders,
        app_drawer_stack_direction: AppDrawerStackDirection::LeftRight,
        show_disabled_playback_devices: true,
        ..AppSettings::default()
    });
    for settings in &states {
        let entries = with_ctx(settings, all_entries);
        for page in PAGES {
            let items = page_items(settings, page);
            for card in items.iter().filter(|i| i.role == Role::Card) {
                let entry = entries
                    .iter()
                    .find(|(p, id, _)| *p == page && *id == card.id)
                    .map(|(_, _, e)| e)
                    .unwrap_or_else(|| panic!("{} has no search entry", card.id));
                assert!(!entry.title.trim().is_empty(), "{} has no search title", card.id);
                assert!(!entry.keywords.is_empty(), "{} has no search keywords", card.id);
                assert!(!entry.page.is_empty(), "{} has no page", card.id);
            }
            // Every page has at least one searchable document.
            assert!(entries.iter().any(|(p, _, _)| *p == page), "{page:?} has no search entries");
        }
    }
}

#[test]
fn item_ids_are_unique_per_page() {
    for page in PAGES {
        let items = page_items(&AppSettings::default(), page);
        let ids: BTreeSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids.len(), items.len(), "{page:?}");
    }
}

#[test]
fn general_page_order_and_dependencies() {
    let mut settings = AppSettings::default();
    let items = page_items(&settings, SettingsPage::General);
    assert_eq!(
        titles(&items),
        [
            "General",
            "Language",
            "Run on startup",
            "Rendering",
            "Rendering backend",
            "Performance",
            "Keep flyout warm",
            "Keep tray context menu warm",
            "Volume changed ping",
            "Play a sound when changing playback device volume",
            "Play a sound when scrolling the tray icon",
            "Play a sound when changing app volume",
            "Skip the sound when audio is already playing",
            "Peak meter threshold (%)",
            "Other",
            "Use logarithmic volume scale",
        ]
    );
    // The startup card shows the shortcut target.
    let startup = items.iter().find(|i| i.id == "general.run_on_startup").expect("startup card");
    assert_eq!(startup.view.description.as_str(), "Startup shortcut target:\nC:\\Apps\\VolumeTrayAppDotNET.exe");

    settings.suppress_device_volume_change_sound_when_audio_playing = false;
    assert!(!has_card(&settings, SettingsPage::General, "general.ding_suppression_peak_threshold"));
    settings.play_device_volume_change_sound = false;
    assert!(!has_card(&settings, SettingsPage::General, "general.play_tray_scroll_volume_change_sound"));
    assert!(!has_card(&settings, SettingsPage::General, "general.suppress_ding_when_playing"));
}

#[test]
fn dependent_cards_follow_their_settings() {
    let mut settings = AppSettings::default();
    assert!(has_card(&settings, SettingsPage::AppDrawers, "app_drawers.icons_per_row"));
    assert!(!has_card(&settings, SettingsPage::AppDrawers, "app_drawers.icons_per_column"));
    assert!(!has_card(&settings, SettingsPage::AppDrawers, "app_drawers.recording_sliders_max_apps"));
    settings.app_drawer_stack_direction = AppDrawerStackDirection::RightLeft;
    assert!(has_card(&settings, SettingsPage::AppDrawers, "app_drawers.icons_per_column"));
    settings.recording_app_drawer_display_type = AppDrawerDisplayType::Sliders;
    assert!(has_card(&settings, SettingsPage::AppDrawers, "app_drawers.recording_sliders_max_apps"));
    assert!(!has_card(&settings, SettingsPage::AppDrawers, "app_drawers.icon_scale"));

    settings.tray_scroll_enabled = false;
    let tray = page_items(&settings, SettingsPage::TrayIcon);
    assert!(!tray.iter().any(|i| i.id == "tray_icon.wheel_volume_step"));
    let wheel = tray.iter().find(|i| i.id == "tray_icon.ctrl_wheel_action").expect("wheel action card");
    assert!(!wheel.view.enabled, "modified wheel cards grey out while the mouse wheel is off");

    settings.show_disabled_playback_devices = true;
    settings.show_recording_devices = false;
    assert!(!has_card(&settings, SettingsPage::Devices, "devices.show_default_even_if_disabled"));
    let devices = page_items(&settings, SettingsPage::Devices);
    let disabled = devices.iter().find(|i| i.id == "devices.show_disabled").expect("show disabled");
    assert!(disabled.view.show_left && !disabled.view.show_right);
    let listen = devices.iter().find(|i| i.id == "devices.show_listen_button").expect("listen");
    assert!(!listen.view.has_left && listen.view.has_right);

    settings.allow_flyout_undock = false;
    settings.show_bluetooth_radio_button_in_flyout_header = false;
    let flyout = page_items(&settings, SettingsPage::Flyout);
    assert!(!flyout.iter().any(|i| i.id == "flyout.clamp_undocked_to_screen"));
    assert!(!flyout.iter().any(|i| i.id == "flyout.bluetooth_radio_button_click_gesture"));
}

#[test]
fn theme_page_shows_the_effective_variant_and_slider_catalog() {
    let mut settings = AppSettings::default();
    settings.common.text_color.dark = Some(Argb(0xFF11_2233));
    let items = page_items(&settings, SettingsPage::Theme);
    let text = items.iter().find(|i| i.id == "theme.text_color").expect("text color");
    assert_eq!(text.view.color, crate::items::slint_color(Argb(0xFF11_2233)));
    assert!(!text.view.color_unset);
    let background = items.iter().find(|i| i.id == "theme.background_color").expect("background color");
    assert!(background.view.color_unset);
    assert_eq!(background.view.color, crate::items::slint_color(Argb(0xFF20_2020)));
    let thumb = items.iter().find(|i| i.id == "theme.slider_indicator").expect("slider indicator");
    assert_eq!(slint::Model::row_count(&thumb.view.options), 6);
    assert_eq!(thumb.view.selected, 0);
}

#[test]
fn search_filters_cards_under_their_page_and_section() {
    let settings = AppSettings::default();
    let results = with_ctx(&settings, |ctx| build_results("autostart", ctx));
    assert_eq!(results[0].view.title.as_str(), "Search results");
    assert!(results.iter().any(|i| i.id == "general.run_on_startup"), "synonym match");
    let link = results.iter().find(|i| i.view.kind == ItemKind::PageLink).expect("page link");
    assert_eq!(link.view.title.as_str(), "General");
    assert!(
        matches!(&link.action, Action::Navigate(SettingsPage::General, ids) if ids.contains(&"general.run_on_startup".to_owned()))
    );

    let results = with_ctx(&settings, |ctx| build_results("peak meter color", ctx));
    assert!(results.iter().any(|i| i.id == "theme.meter_peak_color"));
    assert!(results.iter().any(|i| i.id == "theme.meter_peak_header"), "section header shown with its match");

    let results = with_ctx(&settings, |ctx| build_results("zzzqqqxxx", ctx));
    assert_eq!(results.len(), 2);
    assert_eq!(results[1].view.description.as_str(), "No settings match \"zzzqqqxxx\".");
}

#[test]
fn sidebar_width_resolution() {
    assert_eq!(crate::resolve_sidebar_width(0.0), 230.0);
    assert_eq!(crate::resolve_sidebar_width(f64::NAN), 230.0);
    assert_eq!(crate::resolve_sidebar_width(100.0), 140.0);
    assert_eq!(crate::resolve_sidebar_width(900.0), 520.0);
    assert_eq!(crate::resolve_sidebar_width(300.0), 300.0);
}

#[test]
fn keyboard_focus_lands_on_the_first_card_control() {
    use crate::items::{first_focus_row, is_focus_stop};
    let settings = AppSettings::default();
    for page in PAGES {
        let items = page_items(&settings, page);
        let row = first_focus_row(&items).unwrap_or_else(|| panic!("{page:?} has no focus stop"));
        // Nothing before it is focusable, and it is a control row.
        assert!(items[..row].iter().all(|i| !is_focus_stop(&i.view)));
        assert!(matches!(items[row].role, Role::Card | Role::Supporting));
    }
    assert_eq!(
        page_items(&settings, SettingsPage::General)
            [first_focus_row(&page_items(&settings, SettingsPage::General)).unwrap_or(0)]
        .id,
        "general.language"
    );
    let about = page_items(&settings, SettingsPage::About);
    assert_eq!(
        about[first_focus_row(&about).unwrap_or(0)].id,
        "about.github",
        "the GitHub link is the About page's first stop"
    );

    // Search results: page headings are skipped, the first matched card's control takes focus.
    let results = with_ctx(&settings, |ctx| build_results("peak meter color", ctx));
    let row = first_focus_row(&results).expect("focus stop in results");
    assert_eq!(results[row].id, "theme.meter_peak_color");
    // No matches: nothing to focus (the window keeps focus on the page).
    let results = with_ctx(&settings, |ctx| build_results("zzzqqqxxx", ctx));
    assert_eq!(first_focus_row(&results), None);
}

#[test]
fn disabled_and_hidden_controls_are_not_focus_stops() {
    use crate::items::is_focus_stop;
    let settings = AppSettings {
        tray_scroll_enabled: false,
        show_disabled_playback_devices: true,
        show_recording_devices: false,
        ..AppSettings::default()
    };
    let tray = page_items(&settings, SettingsPage::TrayIcon);
    let wheel = tray.iter().find(|i| i.id == "tray_icon.wheel_action").expect("wheel action card");
    assert!(!is_focus_stop(&wheel.view), "greyed-out card");
    let devices = page_items(&settings, SettingsPage::Devices);
    for item in &devices {
        if item.role != Role::Card {
            assert!(!is_focus_stop(&item.view), "{} is not a control row", item.id);
        }
    }
    let disabled = devices.iter().find(|i| i.id == "devices.show_disabled").expect("pair card");
    assert!(is_focus_stop(&disabled.view), "left side still shown");
    let mut view = disabled.view.clone();
    view.show_left = false;
    assert!(!is_focus_stop(&view), "no visible side");
    let known_issue = page_items(&settings, SettingsPage::About)
        .into_iter()
        .find(|i| i.id == "about.known_issue_bluetooth_codec")
        .expect("known issue");
    assert!(!is_focus_stop(&known_issue.view));
}

/// Off-screen Slint platform for this test thread (software renderer, no OS window).
struct HeadlessPlatform {
    window: std::rc::Rc<slint::platform::software_renderer::MinimalSoftwareWindow>,
}

impl slint::platform::Platform for HeadlessPlatform {
    fn create_window_adapter(&self) -> Result<std::rc::Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

struct TestHost;

impl crate::SettingsHost for TestHost {
    fn palette(&self) -> tadn_common::theme::Palette {
        tadn_common::theme::resolve_palette(&ThemeFile::default(), &AppSettings::default().common)
    }
    fn startup_enabled(&self) -> bool {
        false
    }
    fn set_startup_enabled(&self, _: bool) -> Result<(), String> {
        Ok(())
    }
    fn startup_target(&self) -> Option<String> {
        None
    }
    fn register_hotkeys(&self) -> Vec<tadn_shell::hotkeys::HotkeyRegistration> {
        Vec::new()
    }
    fn restart(&self) {}
    fn data_folder(&self) -> std::path::PathBuf {
        std::path::PathBuf::new()
    }
    fn settings_activated(&self) {}
    fn settings_closed(&self) {}
}

/// Every row takes its height in the page layout: a row without one stacks every card at the top
/// of the page (the page then fits the window and cannot scroll).
#[test]
fn page_rows_take_layout_height() {
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(HeadlessPlatform { window: window.clone() })).expect("platform");
    window.set_size(slint::PhysicalSize::new(960, 670));
    let handle = vtadn_model::settings_handle::SettingsHandle::new(
        AppSettings::default(),
        vtadn_model::device_settings::DeviceSettings::default(),
    );
    let settings_window = crate::SettingsWindow::new(handle, std::rc::Rc::new(TestHost)).expect("settings window");
    settings_window.show(Some(SettingsPage::General));
    window.set_size(slint::PhysicalSize::new(960, 670));

    let rows = settings_window.inner.items.borrow().len();
    assert!(rows > 10, "General has more than ten rows");
    let scroll_max = settings_window.inner.window.get_scroll_max();
    // The General page is taller than a 670px window: at least 40px per row on average.
    assert!(scroll_max > 0.0, "page content must exceed the window, got scroll max {scroll_max}");
    assert!(scroll_max + 670.0 > rows as f32 * 40.0, "rows too short: {scroll_max} for {rows} rows");
}
