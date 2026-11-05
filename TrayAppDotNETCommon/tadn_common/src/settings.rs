//! Settings every TrayAppDotNET app shares (port of `Models/AppSettingsCommon.cs`). Apps embed
//! [`CommonSettings`] as the first part of their own settings struct; the XML element order is
//! common fields first, in the order declared here.
//!
//! Defaults and clamps mirror the C# setters. `clamp_all` applies every range rule (load and
//! programmatic writes both go through it).

use crate::hotkey::{HOTKEYS_ELEMENT, HotkeyBinding, read_bindings_xml, write_bindings_xml};
use crate::theme::NullableThemeColor;
use crate::xml::{XmlElement, XmlError, XmlWriter};

/// `<ThemeMode>`: `System` follows the Windows app mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// `<AnimationMode>`: `System` follows `SPI_GETCLIENTAREAANIMATION`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AnimationMode {
    #[default]
    System,
    Disabled,
    Enabled,
}

/// `<ContextMenuPosition>`: `Modern` docks above the taskbar, `Classic` opens at the cursor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ContextMenuPosition {
    Classic,
    #[default]
    Modern,
}

/// `<RenderingBackend>`: `GPUPreferred` selects Slint's Skia renderer (falls back to software),
/// `Software` selects the software renderer. Read before the UI starts; changes need a restart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RenderingBackend {
    #[default]
    GPUPreferred,
    Software,
}

/// XML enum spelling helpers. Names are the exact C# member names; parsing is case-sensitive like
/// `Enum.TryParse` without `ignoreCase`, and also accepts the numeric value of a defined member.
/// `VARIANTS` lists every member in declaration order, so a member's numeric value is its index.
pub trait XmlEnum: Sized + Copy + 'static {
    const VARIANTS: &'static [(Self, &'static str)];

    fn xml_name(self) -> &'static str
    where
        Self: PartialEq,
    {
        Self::VARIANTS.iter().find(|(v, _)| *v == self).map(|(_, n)| *n).unwrap_or("")
    }

    /// `Enum.TryParse(text, ignoreCase: false)` followed by `Enum.IsDefined`: surrounding
    /// whitespace is ignored, a numeric value or a comma-separated list of names/values (OR-ed)
    /// is accepted, and the result must be a defined member.
    fn from_xml_name(text: &str) -> Option<Self> {
        let mut combined: i64 = 0;
        for part in text.split(',') {
            let part = part.trim();
            let value = if part.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+') {
                part.parse::<i64>().ok()?
            } else {
                Self::VARIANTS.iter().position(|(_, n)| *n == part)? as i64
            };
            combined |= value;
        }
        usize::try_from(combined).ok().and_then(|index| Self::VARIANTS.get(index)).map(|(v, _)| *v)
    }
}

impl XmlEnum for ThemeMode {
    const VARIANTS: &'static [(Self, &'static str)] =
        &[(Self::System, "System"), (Self::Light, "Light"), (Self::Dark, "Dark")];
}

impl XmlEnum for AnimationMode {
    const VARIANTS: &'static [(Self, &'static str)] =
        &[(Self::System, "System"), (Self::Disabled, "Disabled"), (Self::Enabled, "Enabled")];
}

impl XmlEnum for ContextMenuPosition {
    const VARIANTS: &'static [(Self, &'static str)] = &[(Self::Classic, "Classic"), (Self::Modern, "Modern")];
}

impl XmlEnum for RenderingBackend {
    const VARIANTS: &'static [(Self, &'static str)] =
        &[(Self::GPUPreferred, "GPUPreferred"), (Self::Software, "Software")];
}

pub const TOOL_TIP_SHOW_DELAY_MS_RANGE: (i32, i32) = (0, 10_000);
pub const CONTEXT_MENU_FONT_SIZE_RANGE: (i32, i32) = (8, 48);
pub const SUBMENU_SHOW_DELAY_MS_RANGE: (i32, i32) = (0, 5_000);
pub const UPDATE_CHECK_INTERVAL_MS_DEFAULT: i32 = 3_600_000;
pub const UPDATE_CHECK_INTERVAL_MS_RANGE: (i32, i32) = (60_000, 86_400_000);

/// Root element of every app's settings file.
pub const SETTINGS_ROOT_ELEMENT: &str = "AppSettings";

#[derive(Clone, Debug, PartialEq)]
pub struct CommonSettings {
    /// `RunOnStartup` (true). Applied on first run only; the UI toggle mirrors the shortcut.
    pub run_on_startup: bool,
    /// `Autosave` (true). Gates implicit saves.
    pub autosave: bool,
    /// `ThemeMode` (System).
    pub theme_mode: ThemeMode,
    /// `TextColor` override; unset sides use the theme.
    pub text_color: NullableThemeColor,
    /// `BackgroundColor` override.
    pub background_color: NullableThemeColor,
    /// `TrayIconColor` override (tray glyph color).
    pub tray_icon_color: NullableThemeColor,
    /// `EnableRoundedCorners` (true).
    pub enable_rounded_corners: bool,
    /// `UseWindows11SettingsNavigation` (false).
    pub use_windows11_settings_navigation: bool,
    /// `SettingsSidebarWidth` (0 = app default), DIPs.
    pub settings_sidebar_width: f64,
    /// `AnimationMode` (System).
    pub animation_mode: AnimationMode,
    /// `ToolTipShowDelayMs` (750, 0..10000). In-app tooltips only.
    pub tool_tip_show_delay_ms: i32,
    /// `ContextMenuPosition` (Modern).
    pub context_menu_position: ContextMenuPosition,
    /// `ContextMenuFontSize` (15, 8..48).
    pub context_menu_font_size: i32,
    /// `UseSystemSubmenuShowDelay` (false).
    pub use_system_submenu_show_delay: bool,
    /// `SubmenuShowDelayMs` (150, 0..5000).
    pub submenu_show_delay_ms: i32,
    /// `RenderingBackend` (GPUPreferred). Restart needed.
    pub rendering_backend: RenderingBackend,
    /// `Language` ("Auto"). Restart needed. Only English ships.
    pub language: String,
    /// `CheckForUpdatesEnabled` (true). Kept for file compatibility; this build has no updater.
    pub check_for_updates_enabled: bool,
    /// `ShowUpdateNotificationsEnabled` (false). Kept for file compatibility.
    pub show_update_notifications_enabled: bool,
    /// `ShowUpdateButtonInFlyout` (true). Kept for file compatibility.
    pub show_update_button_in_flyout: bool,
    /// `UpdateCheckIntervalMs` (3600000; <=0 -> default; else 60000..86400000). Kept for file compatibility.
    pub update_check_interval_ms: i32,
    /// `SkippedUpdateVersion` (0). Kept for file compatibility.
    pub skipped_update_version: i32,
    /// `KeepFlyoutWarm` (true).
    pub keep_flyout_warm: bool,
    /// `KeepTrayContextMenuWarm` (true).
    pub keep_tray_context_menu_warm: bool,
    /// `<Hotkeys><Binding .../></Hotkeys>`. Volume ships no default bindings.
    pub hotkeys: Vec<HotkeyBinding>,
}

impl Default for CommonSettings {
    fn default() -> Self {
        Self {
            run_on_startup: true,
            autosave: true,
            theme_mode: ThemeMode::System,
            text_color: NullableThemeColor::default(),
            background_color: NullableThemeColor::default(),
            tray_icon_color: NullableThemeColor::default(),
            enable_rounded_corners: true,
            use_windows11_settings_navigation: false,
            settings_sidebar_width: 0.0,
            animation_mode: AnimationMode::System,
            tool_tip_show_delay_ms: 750,
            context_menu_position: ContextMenuPosition::Modern,
            context_menu_font_size: 15,
            use_system_submenu_show_delay: false,
            submenu_show_delay_ms: 150,
            rendering_backend: RenderingBackend::GPUPreferred,
            language: "Auto".to_owned(),
            check_for_updates_enabled: true,
            show_update_notifications_enabled: false,
            show_update_button_in_flyout: true,
            update_check_interval_ms: UPDATE_CHECK_INTERVAL_MS_DEFAULT,
            skipped_update_version: 0,
            keep_flyout_warm: true,
            keep_tray_context_menu_warm: true,
            hotkeys: Vec::new(),
        }
    }
}

impl CommonSettings {
    /// Applies every C# setter clamp. `SettingsSidebarWidth` has none: the settings window
    /// resolves the persisted width against its own limits.
    pub fn clamp_all(&mut self) {
        let clamp = |v: i32, (lo, hi): (i32, i32)| v.clamp(lo, hi);
        self.tool_tip_show_delay_ms = clamp(self.tool_tip_show_delay_ms, TOOL_TIP_SHOW_DELAY_MS_RANGE);
        self.context_menu_font_size = clamp(self.context_menu_font_size, CONTEXT_MENU_FONT_SIZE_RANGE);
        self.submenu_show_delay_ms = clamp(self.submenu_show_delay_ms, SUBMENU_SHOW_DELAY_MS_RANGE);
        self.update_check_interval_ms = if self.update_check_interval_ms <= 0 {
            UPDATE_CHECK_INTERVAL_MS_DEFAULT
        } else {
            clamp(self.update_check_interval_ms, UPDATE_CHECK_INTERVAL_MS_RANGE)
        };
    }

    /// Applies one child element of `<AppSettings>` when it is a common setting. Returns
    /// `Ok(true)` when the element was consumed; `dropped_hotkeys` is set when a binding with an
    /// unknown action was dropped. Clamps are not applied here (call [`Self::clamp_all`]).
    pub fn read_xml_element(&mut self, element: &XmlElement, dropped_hotkeys: &mut bool) -> Result<bool, XmlError> {
        match element.name.as_str() {
            "RunOnStartup" => element.read_bool(&mut self.run_on_startup)?,
            "Autosave" => element.read_bool(&mut self.autosave)?,
            "ThemeMode" => element.read_enum(&mut self.theme_mode)?,
            "TextColor" => self.text_color = NullableThemeColor::from_xml(element)?,
            "BackgroundColor" => self.background_color = NullableThemeColor::from_xml(element)?,
            "TrayIconColor" => self.tray_icon_color = NullableThemeColor::from_xml(element)?,
            "EnableRoundedCorners" => element.read_bool(&mut self.enable_rounded_corners)?,
            "UseWindows11SettingsNavigation" => element.read_bool(&mut self.use_windows11_settings_navigation)?,
            "SettingsSidebarWidth" => element.read_f64(&mut self.settings_sidebar_width)?,
            "AnimationMode" => element.read_enum(&mut self.animation_mode)?,
            "ToolTipShowDelayMs" => element.read_i32(&mut self.tool_tip_show_delay_ms)?,
            "ContextMenuPosition" => element.read_enum(&mut self.context_menu_position)?,
            "ContextMenuFontSize" => element.read_i32(&mut self.context_menu_font_size)?,
            "UseSystemSubmenuShowDelay" => element.read_bool(&mut self.use_system_submenu_show_delay)?,
            "SubmenuShowDelayMs" => element.read_i32(&mut self.submenu_show_delay_ms)?,
            "RenderingBackend" => element.read_enum(&mut self.rendering_backend)?,
            "Language" => element.read_string(&mut self.language)?,
            "CheckForUpdatesEnabled" => element.read_bool(&mut self.check_for_updates_enabled)?,
            "ShowUpdateNotificationsEnabled" => element.read_bool(&mut self.show_update_notifications_enabled)?,
            "ShowUpdateButtonInFlyout" => element.read_bool(&mut self.show_update_button_in_flyout)?,
            "UpdateCheckIntervalMs" => element.read_i32(&mut self.update_check_interval_ms)?,
            "SkippedUpdateVersion" => element.read_i32(&mut self.skipped_update_version)?,
            "KeepFlyoutWarm" => element.read_bool(&mut self.keep_flyout_warm)?,
            "KeepTrayContextMenuWarm" => element.read_bool(&mut self.keep_tray_context_menu_warm)?,
            HOTKEYS_ELEMENT => {
                let (bindings, dropped) = read_bindings_xml(element);
                self.hotkeys = bindings;
                *dropped_hotkeys |= dropped;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Writes the common elements in C# declaration order (`Hotkeys` last).
    pub fn write_xml(&self, writer: &mut XmlWriter) {
        writer.element_bool("RunOnStartup", self.run_on_startup);
        writer.element_bool("Autosave", self.autosave);
        writer.element_enum("ThemeMode", self.theme_mode);
        self.text_color.write_xml(writer, "TextColor");
        self.background_color.write_xml(writer, "BackgroundColor");
        self.tray_icon_color.write_xml(writer, "TrayIconColor");
        writer.element_bool("EnableRoundedCorners", self.enable_rounded_corners);
        writer.element_bool("UseWindows11SettingsNavigation", self.use_windows11_settings_navigation);
        writer.element_f64("SettingsSidebarWidth", self.settings_sidebar_width);
        writer.element_enum("AnimationMode", self.animation_mode);
        writer.element_i32("ToolTipShowDelayMs", self.tool_tip_show_delay_ms);
        writer.element_enum("ContextMenuPosition", self.context_menu_position);
        writer.element_i32("ContextMenuFontSize", self.context_menu_font_size);
        writer.element_bool("UseSystemSubmenuShowDelay", self.use_system_submenu_show_delay);
        writer.element_i32("SubmenuShowDelayMs", self.submenu_show_delay_ms);
        writer.element_enum("RenderingBackend", self.rendering_backend);
        writer.element("Language", &self.language);
        writer.element_bool("CheckForUpdatesEnabled", self.check_for_updates_enabled);
        writer.element_bool("ShowUpdateNotificationsEnabled", self.show_update_notifications_enabled);
        writer.element_bool("ShowUpdateButtonInFlyout", self.show_update_button_in_flyout);
        writer.element_i32("UpdateCheckIntervalMs", self.update_check_interval_ms);
        writer.element_i32("SkippedUpdateVersion", self.skipped_update_version);
        writer.element_bool("KeepFlyoutWarm", self.keep_flyout_warm);
        writer.element_bool("KeepTrayContextMenuWarm", self.keep_tray_context_menu_warm);
        write_bindings_xml(writer, &self.hotkeys);
    }
}

/// The runner's pre-read (`TrayAppDotNETAppRunner`): `RenderingBackend` and `Language` from the
/// top-level elements of `settings.xml`, before the UI starts. None when the file is missing or
/// unreadable, or the element is absent; the backend is matched ignoring case.
pub fn pre_read_startup_settings(path: &std::path::Path) -> (Option<RenderingBackend>, Option<String>) {
    let Some(root) = std::fs::read(path).ok().and_then(|bytes| XmlElement::parse(&bytes).ok()) else {
        return (None, None);
    };
    let text = |name: &str| root.children.iter().find(|c| c.name == name).map(|c| c.text.clone());
    let backend = text("RenderingBackend").and_then(|value| {
        let value = value.trim();
        RenderingBackend::VARIANTS.iter().find(|(_, n)| n.eq_ignore_ascii_case(value)).map(|(v, _)| *v)
    });
    (backend, text("Language"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_parsing_matches_enum_try_parse() {
        assert_eq!(ThemeMode::from_xml_name("Dark"), Some(ThemeMode::Dark));
        assert_eq!(ThemeMode::from_xml_name(" Light "), Some(ThemeMode::Light));
        assert_eq!(ThemeMode::from_xml_name("dark"), None, "case-sensitive");
        assert_eq!(ThemeMode::from_xml_name("2"), Some(ThemeMode::Dark));
        assert_eq!(ThemeMode::from_xml_name("+1"), Some(ThemeMode::Light));
        assert_eq!(ThemeMode::from_xml_name("3"), None, "undefined value");
        assert_eq!(ThemeMode::from_xml_name("-1"), None);
        assert_eq!(ThemeMode::from_xml_name("System, Light"), Some(ThemeMode::Light));
        assert_eq!(ThemeMode::from_xml_name("Light, Dark"), None, "1 | 2 = 3 is undefined");
        assert_eq!(ThemeMode::from_xml_name(""), None);
        assert_eq!(ThemeMode::Dark.xml_name(), "Dark");
    }

    #[test]
    fn clamps_match_the_setters() {
        let mut settings = CommonSettings {
            context_menu_font_size: 2,
            submenu_show_delay_ms: i32::MAX,
            tool_tip_show_delay_ms: -5,
            update_check_interval_ms: 0,
            settings_sidebar_width: -4.0,
            ..CommonSettings::default()
        };
        settings.clamp_all();
        assert_eq!(settings.context_menu_font_size, 8);
        assert_eq!(settings.submenu_show_delay_ms, 5_000);
        assert_eq!(settings.tool_tip_show_delay_ms, 0);
        assert_eq!(settings.update_check_interval_ms, UPDATE_CHECK_INTERVAL_MS_DEFAULT);
        assert_eq!(settings.settings_sidebar_width, -4.0, "no setter clamp in C#");
        settings.context_menu_font_size = 500;
        settings.update_check_interval_ms = 1;
        settings.clamp_all();
        assert_eq!(settings.context_menu_font_size, 48);
        assert_eq!(settings.update_check_interval_ms, 60_000);
        settings.update_check_interval_ms = i32::MAX;
        settings.clamp_all();
        assert_eq!(settings.update_check_interval_ms, 86_400_000);
    }
}
