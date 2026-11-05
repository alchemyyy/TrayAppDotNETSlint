//! Theme colors, `theme.xml`, light/dark resolution and the resolved [`Palette`] (port of
//! `Visuals/AppTheme.cs`, `Visuals/AppTheme.axaml`, `UI/AppSettingsPalette.cs`,
//! `UI/Models/NullableThemeColor.cs`).
//!
//! UI crates push a [`Palette`] into their Slint `Theme` global with [`apply_palette!`]; the
//! global's property names (see `TrayAppDotNETCommon/ui/theme.slint`) match the fields here.

use std::path::Path;
use std::sync::LazyLock;

use crate::settings::{AnimationMode, CommonSettings, ThemeMode};
use crate::xml::{LoadOutcome, XmlElement, XmlError, XmlWriter, try_load};

/// A color encoded `0xAARRGGBB` (the C# hex order). Slint takes it through
/// `slint::Color::from_argb_encoded`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Argb(pub u32);

impl Argb {
    pub const fn opaque(rgb: u32) -> Self {
        Self(0xFF00_0000 | (rgb & 0x00FF_FFFF))
    }

    pub const fn alpha(self) -> u8 {
        (self.0 >> 24) as u8
    }

    /// Same color with alpha multiplied by `factor` (0..1), `Math.Round` (ties to even).
    pub fn with_opacity(self, factor: f32) -> Self {
        let a = (f32::from(self.alpha()) * factor.clamp(0.0, 1.0)).round_ties_even() as u32;
        Self((a << 24) | (self.0 & 0x00FF_FFFF))
    }

    /// `ColorMath.TryParseHexOrNull`: trimmed, one optional leading `#`, then 6 (`RRGGBB`,
    /// opaque) or 8 (`AARRGGBB`) hex digits. Blank or malformed text is None.
    pub fn parse_hex(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        let digits = trimmed.strip_prefix('#').unwrap_or(trimmed);
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        match digits.len() {
            6 => u32::from_str_radix(digits, 16).ok().map(Self::opaque),
            8 => u32::from_str_radix(digits, 16).ok().map(Self),
            _ => None,
        }
    }

    /// `ThemeColor` hex rule (`Normalize` + `ParseHexStrict`): a `#` is prepended when missing,
    /// then the text must parse with [`Self::parse_hex`].
    pub fn parse_hex_strict(text: &str) -> Option<Self> {
        if text.starts_with('#') { Self::parse_hex(text) } else { Self::parse_hex(&format!("#{text}")) }
    }

    /// `#RRGGBB` when opaque, else `#AARRGGBB` (C# `ThemeColor.ToHex`).
    pub fn to_hex(self) -> String {
        if self.alpha() == 0xFF { format!("#{:06X}", self.0 & 0x00FF_FFFF) } else { format!("#{:08X}", self.0) }
    }

    /// Always `#AARRGGBB` (C# `ToArgbHex`, used by the meter color settings).
    pub fn to_argb_hex(self) -> String {
        format!("#{:08X}", self.0)
    }
}

/// Light/dark color pair (`ThemeColor`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThemeColor {
    pub light: Argb,
    pub dark: Argb,
}

impl ThemeColor {
    pub const fn new(light: u32, dark: u32) -> Self {
        Self { light: Argb(light), dark: Argb(dark) }
    }

    pub fn pick(self, is_light: bool) -> Argb {
        if is_light { self.light } else { self.dark }
    }
}

/// Optional per-mode override (`NullableThemeColor`): `<X><LightHex/><DarkHex/></X>`, an empty
/// element means "use the theme".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NullableThemeColor {
    pub light: Option<Argb>,
    pub dark: Option<Argb>,
}

impl NullableThemeColor {
    pub fn pick(self, is_light: bool) -> Option<Argb> {
        if is_light { self.light } else { self.dark }
    }

    /// `IsUnset`: neither side set.
    pub fn is_unset(self) -> bool {
        self.light.is_none() && self.dark.is_none()
    }

    /// Reads `<X><LightHex>..</LightHex><DarkHex>..</DarkHex></X>`: a blank or malformed hex is
    /// unset (`ColorMath.TryParseHexOrNull`); other children are skipped.
    pub fn from_xml(element: &XmlElement) -> Result<Self, XmlError> {
        let mut color = Self::default();
        for child in &element.children {
            match child.name.as_str() {
                "LightHex" => color.light = Argb::parse_hex(child.content()?),
                "DarkHex" => color.dark = Argb::parse_hex(child.content()?),
                _ => {}
            }
        }
        Ok(color)
    }

    /// Writes `<name><LightHex>..</LightHex><DarkHex>..</DarkHex></name>`; an unset side is an
    /// empty element, a set side the color picker's hex (`#RRGGBB` or `#AARRGGBB`).
    pub fn write_xml(self, writer: &mut XmlWriter, name: &str) {
        writer.start_element(name);
        writer.element("LightHex", &self.light.map(Argb::to_hex).unwrap_or_default());
        writer.element("DarkHex", &self.dark.map(Argb::to_hex).unwrap_or_default());
        writer.end_element();
    }
}

/// Every color `theme.xml` can define, with the built-in defaults from `AppTheme.axaml`.
/// `theme.xml` is user-authored, read once at startup, never written.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeFile {
    pub background: ThemeColor,
    pub foreground: ThemeColor,
    pub border: ThemeColor,
    pub separator: ThemeColor,
    pub hover: ThemeColor,
    pub hover_deep: ThemeColor,
    pub pressed: ThemeColor,
    pub pressed_deep: ThemeColor,
    pub control_background: ThemeColor,
    pub control_background_deep: ThemeColor,
    pub control_border: ThemeColor,
    pub disabled_foreground: ThemeColor,
    pub accent: ThemeColor,
    pub acrylic: ThemeColor,
    pub secondary_foreground: ThemeColor,
    pub footer_background: ThemeColor,
    pub slider_track: ThemeColor,
    pub slider_progress: ThemeColor,
    pub slider_thumb: ThemeColor,
    pub button_hover: ThemeColor,
    pub button_pressed: ThemeColor,
    pub icon_foreground: ThemeColor,
    pub card_background: ThemeColor,
    pub text_box_focused: ThemeColor,
    pub search_list_item_selected: ThemeColor,
    pub search_list_item_hover: ThemeColor,
    pub toggle_switch_on_track: ThemeColor,
    pub toggle_switch_on_thumb: ThemeColor,
    pub close_button_hover: ThemeColor,
    pub close_button_pressed: ThemeColor,
    pub close_button_glyph_active: ThemeColor,
    pub flyout_overlay_backdrop: ThemeColor,
    pub flyout_shadow: ThemeColor,
    pub menu_shadow: ThemeColor,
}

impl Default for ThemeFile {
    fn default() -> Self {
        Self {
            background: ThemeColor::new(0xFFF3F3F3, 0xFF202020),
            foreground: ThemeColor::new(0xFF000000, 0xFFFFFFFF),
            border: ThemeColor::new(0xFFE0E0E0, 0xFF454545),
            separator: ThemeColor::new(0xFFE5E5E5, 0xFF3A3A3A),
            hover: ThemeColor::new(0xFFE9E9E9, 0xFF363636),
            hover_deep: ThemeColor::new(0xFFE9E9E9, 0xFF363636),
            pressed: ThemeColor::new(0xFFDFDFDF, 0xFF242424),
            pressed_deep: ThemeColor::new(0xFFDFDFDF, 0xFF181818),
            control_background: ThemeColor::new(0xFFFFFFFF, 0xFF3C3C3C),
            control_background_deep: ThemeColor::new(0xFFFFFFFF, 0xFF323232),
            control_border: ThemeColor::new(0xFF808080, 0xFF444444),
            disabled_foreground: ThemeColor::new(0xFF808080, 0xFF808080),
            accent: ThemeColor::new(0xFF222222, 0xFFDDDDDD),
            acrylic: ThemeColor::new(0xD0F3F3F3, 0xD0202020),
            secondary_foreground: ThemeColor::new(0xFF222222, 0xFFDDDDDD),
            footer_background: ThemeColor::new(0xFFE8E8E8, 0xFF1A1A1A),
            slider_track: ThemeColor::new(0xFFC0C0C0, 0xFF3A3A3A),
            slider_progress: ThemeColor::new(0xFF606060, 0xFF6A6A6A),
            slider_thumb: ThemeColor::new(0xFF404040, 0xFFF0F0F0),
            button_hover: ThemeColor::new(0xFFD5D5D5, 0xFF3A3A3A),
            button_pressed: ThemeColor::new(0xFFCACACA, 0xFF4A4A4A),
            icon_foreground: ThemeColor::new(0xFF222222, 0xFFDDDDDD),
            card_background: ThemeColor::new(0xFFFBFBFB, 0xFF2B2B2B),
            text_box_focused: ThemeColor::new(0xFFF5F5F5, 0xFF363636),
            search_list_item_selected: ThemeColor::new(0xFFDFDFDF, 0xFF454545),
            search_list_item_hover: ThemeColor::new(0xFFE9E9E9, 0xFF292929),
            toggle_switch_on_track: ThemeColor::new(0xFF5B5B5B, 0xFF5B5B5B),
            toggle_switch_on_thumb: ThemeColor::new(0xFFFFFFFF, 0xFFFFFFFF),
            close_button_hover: ThemeColor::new(0xFFC42B1C, 0xFFC42B1C),
            close_button_pressed: ThemeColor::new(0xFFA42B1C, 0xFFA42B1C),
            close_button_glyph_active: ThemeColor::new(0xFFFFFFFF, 0xFFFFFFFF),
            flyout_overlay_backdrop: ThemeColor::new(0xA0000000, 0x60000000),
            flyout_shadow: ThemeColor::new(0x99000000, 0x99000000),
            menu_shadow: ThemeColor::new(0x80000000, 0x80000000),
        }
    }
}

/// Text selection highlight (`AppTheme.TextSelectionHighlightAlpha`, `#66000000`).
pub const TEXT_SELECTION: Argb = Argb(0x6600_0000);

/// Fully resolved colors and flags for the current light/dark mode, settings overrides applied.
/// Field names match the Slint `Theme` global (snake_case of its kebab-case properties).
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub is_light: bool,
    pub background: Argb,
    pub foreground: Argb,
    pub border: Argb,
    pub separator: Argb,
    pub hover: Argb,
    pub hover_deep: Argb,
    pub pressed: Argb,
    pub pressed_deep: Argb,
    pub control_background: Argb,
    pub control_background_deep: Argb,
    pub control_border: Argb,
    pub disabled_foreground: Argb,
    pub accent: Argb,
    pub acrylic: Argb,
    pub secondary_foreground: Argb,
    pub footer_background: Argb,
    pub slider_track: Argb,
    pub slider_progress: Argb,
    pub slider_thumb: Argb,
    pub button_hover: Argb,
    pub button_pressed: Argb,
    pub icon_foreground: Argb,
    pub card_background: Argb,
    pub text_box_focused: Argb,
    pub search_list_item_selected: Argb,
    pub search_list_item_hover: Argb,
    pub toggle_switch_on_track: Argb,
    pub toggle_switch_on_thumb: Argb,
    pub close_button_hover: Argb,
    pub close_button_pressed: Argb,
    pub close_button_glyph_active: Argb,
    pub flyout_overlay_backdrop: Argb,
    pub flyout_shadow: Argb,
    pub menu_shadow: Argb,
    pub text_selection: Argb,
    /// Tray glyph color: `TrayIconColor` override, else the theme foreground (`VolumeTrayIcon`
    /// ignores the `TextColor` override).
    pub tray_icon: Argb,
    pub rounded_corners: bool,
    /// Resolved `AnimationMode` (System reads `SPI_GETCLIENTAREAANIMATION`).
    pub animations: bool,
    pub tool_tip_delay_ms: i32,
    pub ui_font: &'static str,
    /// "Segoe Fluent Icons" when installed, else "Segoe MDL2 Assets".
    pub icon_font: &'static str,
    /// `ContextMenuFontSize`.
    pub menu_font_size: i32,
}

/// Root element of `theme.xml`.
pub const THEME_ROOT_ELEMENT: &str = "Theme";

impl ThemeFile {
    /// The `ThemeColor` slot written as `<name LightHex=".." DarkHex=".." />` in `theme.xml`.
    pub fn slot_mut(&mut self, element: &str) -> Option<&mut ThemeColor> {
        Some(match element {
            "Background" => &mut self.background,
            "Foreground" => &mut self.foreground,
            "Border" => &mut self.border,
            "Separator" => &mut self.separator,
            "Hover" => &mut self.hover,
            "HoverDeep" => &mut self.hover_deep,
            "Pressed" => &mut self.pressed,
            "PressedDeep" => &mut self.pressed_deep,
            "ControlBackground" => &mut self.control_background,
            "ControlBackgroundDeep" => &mut self.control_background_deep,
            "ControlBorder" => &mut self.control_border,
            "DisabledForeground" => &mut self.disabled_foreground,
            "Accent" => &mut self.accent,
            "Acrylic" => &mut self.acrylic,
            "SecondaryForeground" => &mut self.secondary_foreground,
            "FooterBackground" => &mut self.footer_background,
            "SliderTrack" => &mut self.slider_track,
            "SliderProgress" => &mut self.slider_progress,
            "SliderThumb" => &mut self.slider_thumb,
            "ButtonHover" => &mut self.button_hover,
            "ButtonPressed" => &mut self.button_pressed,
            "IconForeground" => &mut self.icon_foreground,
            "CardBackground" => &mut self.card_background,
            "TextBoxFocused" => &mut self.text_box_focused,
            "SearchListItemSelected" => &mut self.search_list_item_selected,
            "SearchListItemHover" => &mut self.search_list_item_hover,
            "ToggleSwitchOnTrack" => &mut self.toggle_switch_on_track,
            "ToggleSwitchOnThumb" => &mut self.toggle_switch_on_thumb,
            "CloseButtonHover" => &mut self.close_button_hover,
            "CloseButtonPressed" => &mut self.close_button_pressed,
            "CloseButtonGlyphActive" => &mut self.close_button_glyph_active,
            "FlyoutOverlayBackdrop" => &mut self.flyout_overlay_backdrop,
            "FlyoutShadow" => &mut self.flyout_shadow,
            "MenuShadow" => &mut self.menu_shadow,
            _ => return None,
        })
    }

    /// The generated reader for `AppTheme`: every known `ThemeColor` element starts from the
    /// `ThemeColor()` constructor (`#000000` both sides) and takes its `LightHex`/`DarkHex`
    /// attributes through the strict setter; an invalid hex throws, so the whole file is rejected.
    /// `GlyphSettings`/`GlyphPower` are strings (only checked for being scalar); other elements are
    /// skipped.
    pub fn from_xml(root: &XmlElement) -> Result<Self, XmlError> {
        let mut theme = Self::default();
        for element in &root.children {
            if matches!(element.name.as_str(), "GlyphSettings" | "GlyphPower") {
                element.content()?;
                continue;
            }
            let Some(slot) = theme.slot_mut(&element.name) else { continue };
            let side = |name: &str| -> Result<Argb, XmlError> {
                match element.attr(name) {
                    None => Ok(Argb::opaque(0)),
                    Some(hex) => Argb::parse_hex_strict(hex)
                        .ok_or_else(|| XmlError(format!("Invalid color hex '{hex}' in <{} {name}>.", element.name))),
                }
            };
            *slot = ThemeColor { light: side("LightHex")?, dark: side("DarkHex")? };
        }
        Ok(theme)
    }
}

/// Loads `theme.xml` (root `Theme`, `ThemeColor` children with `LightHex`/`DarkHex` attributes).
/// Missing file → defaults. Any invalid hex → whole file rejected (logged), defaults used. An
/// element present with one attribute missing gets `#000000` for that side (C# ctor default).
/// The file is never created, moved or rewritten.
pub fn load_theme_file(path: &Path) -> ThemeFile {
    let outcome = try_load(path, |bytes| {
        let root = XmlElement::parse(bytes)?;
        if root.name != THEME_ROOT_ELEMENT {
            return Err(XmlError(format!("Unexpected XML root <{}>, expected <{THEME_ROOT_ELEMENT}>.", root.name)));
        }
        ThemeFile::from_xml(&root)
    });
    match outcome {
        LoadOutcome::Loaded(theme) => theme,
        LoadOutcome::Missing | LoadOutcome::Corrupt(_) | LoadOutcome::Locked(_) => ThemeFile::default(),
    }
}

/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\SystemUsesLightTheme == 1`
/// (the shell/taskbar mode `AppTheme.DetectSystemLightTheme` reads); a missing value or any error
/// means dark.
pub fn system_uses_light_theme() -> bool {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows_core::w;
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `value`/`size` are live locals describing a 4-byte buffer; the key and value names
    // are static NUL-terminated wide strings.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&raw mut size),
        )
    };
    status.is_ok() && value == 1
}

/// `ThemeMode` Light/Dark force the mode; System asks [`system_uses_light_theme`].
pub fn resolve_is_light(settings: &CommonSettings) -> bool {
    match settings.theme_mode {
        ThemeMode::Light => true,
        ThemeMode::Dark => false,
        ThemeMode::System => system_uses_light_theme(),
    }
}

/// `SPI_GETCLIENTAREAANIMATION` for `AnimationMode::System`; animations stay on when the call
/// fails (`TrayAppDotNETAnimationPolicy`).
pub fn system_animations_enabled() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    use windows_core::BOOL;
    let mut enabled = BOOL(1);
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL into `enabled`, a live local.
    let result = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&raw mut enabled).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    result.is_err() || enabled.as_bool()
}

/// Resolved `AnimationMode`: Disabled → off, Enabled → on, System → [`system_animations_enabled`].
pub fn resolve_animations(settings: &CommonSettings) -> bool {
    match settings.animation_mode {
        AnimationMode::Disabled => false,
        AnimationMode::Enabled => true,
        AnimationMode::System => system_animations_enabled(),
    }
}

/// `TADNFontResolver.SegoeUIFamilyName`.
pub const UI_FONT_FAMILY: &str = "Segoe UI";
/// `TADNFontResolver.SegoeFluentIconsFamilyName`.
pub const FLUENT_ICONS_FAMILY: &str = "Segoe Fluent Icons";
/// `TADNFontResolver.SegoeMDL2AssetsFamilyName`.
pub const MDL2_ASSETS_FAMILY: &str = "Segoe MDL2 Assets";

/// True when GDI enumerates an installed font family named `family`.
pub fn is_font_installed(family: &str) -> bool {
    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::Graphics::Gdi::{
        DEFAULT_CHARSET, EnumFontFamiliesExW, GetDC, LOGFONTW, ReleaseDC, TEXTMETRICW,
    };

    unsafe extern "system" fn found(_: *const LOGFONTW, _: *const TEXTMETRICW, _: u32, lparam: LPARAM) -> i32 {
        // SAFETY: lparam is the address of the caller's `bool`, alive for the enumeration.
        unsafe { *(lparam.0 as *mut bool) = true };
        0
    }

    let mut font = LOGFONTW { lfCharSet: DEFAULT_CHARSET, ..Default::default() };
    let name: Vec<u16> = family.encode_utf16().take(font.lfFaceName.len() - 1).collect();
    font.lfFaceName[..name.len()].copy_from_slice(&name);
    let mut installed = false;
    // SAFETY: the screen DC is released below; `font` and `installed` outlive the synchronous
    // enumeration whose callback only writes `installed`.
    unsafe {
        let dc = GetDC(None);
        if dc.is_invalid() {
            return false;
        }
        EnumFontFamiliesExW(dc, &font, Some(found), LPARAM((&raw mut installed) as isize), 0);
        ReleaseDC(None, dc);
    }
    installed
}

/// "Segoe Fluent Icons" if that font is installed, else "Segoe MDL2 Assets". Cached.
pub fn icon_font_family() -> &'static str {
    static FAMILY: LazyLock<&'static str> =
        LazyLock::new(|| if is_font_installed(FLUENT_ICONS_FAMILY) { FLUENT_ICONS_FAMILY } else { MDL2_ASSETS_FAMILY });
    *FAMILY
}

/// `AppTheme.ResolveTextSelectionHighlight`: the accent's RGB at [`TEXT_SELECTION`]'s alpha.
pub fn text_selection_highlight(accent: Argb) -> Argb {
    Argb((TEXT_SELECTION.0 & 0xFF00_0000) | (accent.0 & 0x00FF_FFFF))
}

/// Builds the palette for the current mode: theme file colors, then `TextColor` /
/// `BackgroundColor` / `TrayIconColor` overrides exactly as `AppSettingsPalette.Create` derives them.
///
/// Every color field is the same-named theme slot for the mode, except `background`
/// (`BackgroundColor` override ?? theme), `foreground` (`TextColor` override ?? theme),
/// `text_selection` (accent RGB at alpha 0x66) and `tray_icon` (`TrayIconColor` override ?? theme
/// foreground). C# `SettingsPalette` aliases map onto fields as: `Separator`/`ControlBorder` →
/// `border`, `ButtonHover`/`ButtonPressed` → `hover`/`pressed`, `IconForeground` → `foreground`,
/// `FooterBackground` → `background`; the flyout and tray menu read the raw theme slots
/// (`button_hover`, `footer_background`, `icon_foreground`, `separator`, `menu_shadow`).
pub fn resolve_palette(theme: &ThemeFile, settings: &CommonSettings) -> Palette {
    resolve_palette_for(theme, settings, resolve_is_light(settings), resolve_animations(settings), icon_font_family())
}

/// [`resolve_palette`] with the system-dependent inputs supplied (light mode, animations, icon
/// font).
pub fn resolve_palette_for(
    theme: &ThemeFile,
    settings: &CommonSettings,
    is_light: bool,
    animations: bool,
    icon_font: &'static str,
) -> Palette {
    let t = |color: ThemeColor| color.pick(is_light);
    let foreground = settings.text_color.pick(is_light).unwrap_or_else(|| t(theme.foreground));
    let background = settings.background_color.pick(is_light).unwrap_or_else(|| t(theme.background));
    let accent = t(theme.accent);
    Palette {
        is_light,
        background,
        foreground,
        border: t(theme.border),
        separator: t(theme.separator),
        hover: t(theme.hover),
        hover_deep: t(theme.hover_deep),
        pressed: t(theme.pressed),
        pressed_deep: t(theme.pressed_deep),
        control_background: t(theme.control_background),
        control_background_deep: t(theme.control_background_deep),
        control_border: t(theme.control_border),
        disabled_foreground: t(theme.disabled_foreground),
        accent,
        acrylic: t(theme.acrylic),
        secondary_foreground: t(theme.secondary_foreground),
        footer_background: t(theme.footer_background),
        slider_track: t(theme.slider_track),
        slider_progress: t(theme.slider_progress),
        slider_thumb: t(theme.slider_thumb),
        button_hover: t(theme.button_hover),
        button_pressed: t(theme.button_pressed),
        icon_foreground: t(theme.icon_foreground),
        card_background: t(theme.card_background),
        text_box_focused: t(theme.text_box_focused),
        search_list_item_selected: t(theme.search_list_item_selected),
        search_list_item_hover: t(theme.search_list_item_hover),
        toggle_switch_on_track: t(theme.toggle_switch_on_track),
        toggle_switch_on_thumb: t(theme.toggle_switch_on_thumb),
        close_button_hover: t(theme.close_button_hover),
        close_button_pressed: t(theme.close_button_pressed),
        close_button_glyph_active: t(theme.close_button_glyph_active),
        flyout_overlay_backdrop: t(theme.flyout_overlay_backdrop),
        flyout_shadow: t(theme.flyout_shadow),
        menu_shadow: t(theme.menu_shadow),
        text_selection: text_selection_highlight(accent),
        tray_icon: settings.tray_icon_color.pick(is_light).unwrap_or_else(|| t(theme.foreground)),
        rounded_corners: settings.enable_rounded_corners,
        animations,
        tool_tip_delay_ms: settings.tool_tip_show_delay_ms,
        ui_font: UI_FONT_FAMILY,
        icon_font,
        menu_font_size: settings.context_menu_font_size,
    }
}

/// Pushes a [`Palette`] into a Slint `Theme` global (`TrayAppDotNETCommon/ui/theme.slint`).
/// Usage: `tadn_common::apply_palette!(window.global::<Theme>(), palette)`. The caller crate must
/// depend on `slint`.
#[macro_export]
macro_rules! apply_palette {
    ($theme:expr, $palette:expr) => {{
        let theme = $theme;
        let p: &$crate::theme::Palette = &$palette;
        let c = |a: $crate::theme::Argb| ::slint::Color::from_argb_encoded(a.0);
        theme.set_is_light(p.is_light);
        theme.set_background(c(p.background));
        theme.set_foreground(c(p.foreground));
        theme.set_border(c(p.border));
        theme.set_separator(c(p.separator));
        theme.set_hover(c(p.hover));
        theme.set_hover_deep(c(p.hover_deep));
        theme.set_pressed(c(p.pressed));
        theme.set_pressed_deep(c(p.pressed_deep));
        theme.set_control_background(c(p.control_background));
        theme.set_control_background_deep(c(p.control_background_deep));
        theme.set_control_border(c(p.control_border));
        theme.set_disabled_foreground(c(p.disabled_foreground));
        theme.set_accent(c(p.accent));
        theme.set_acrylic(c(p.acrylic));
        theme.set_secondary_foreground(c(p.secondary_foreground));
        theme.set_footer_background(c(p.footer_background));
        theme.set_slider_track(c(p.slider_track));
        theme.set_slider_progress(c(p.slider_progress));
        theme.set_slider_thumb(c(p.slider_thumb));
        theme.set_button_hover(c(p.button_hover));
        theme.set_button_pressed(c(p.button_pressed));
        theme.set_icon_foreground(c(p.icon_foreground));
        theme.set_card_background(c(p.card_background));
        theme.set_text_box_focused(c(p.text_box_focused));
        theme.set_search_list_item_selected(c(p.search_list_item_selected));
        theme.set_search_list_item_hover(c(p.search_list_item_hover));
        theme.set_toggle_switch_on_track(c(p.toggle_switch_on_track));
        theme.set_toggle_switch_on_thumb(c(p.toggle_switch_on_thumb));
        theme.set_close_button_hover(c(p.close_button_hover));
        theme.set_close_button_pressed(c(p.close_button_pressed));
        theme.set_close_button_glyph_active(c(p.close_button_glyph_active));
        theme.set_flyout_overlay_backdrop(c(p.flyout_overlay_backdrop));
        theme.set_flyout_shadow(c(p.flyout_shadow));
        theme.set_menu_shadow(c(p.menu_shadow));
        theme.set_text_selection(c(p.text_selection));
        theme.set_tray_icon(c(p.tray_icon));
        theme.set_rounded_corners(p.rounded_corners);
        theme.set_animations(p.animations);
        theme.set_tooltip_delay(i64::from(p.tool_tip_delay_ms));
        theme.set_ui_font(p.ui_font.into());
        theme.set_icon_font(p.icon_font.into());
        theme.set_menu_font_size(p.menu_font_size as f32);
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tadn-theme-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_default();
        let path = dir.join("theme.xml");
        std::fs::write(&path, content).unwrap_or_default();
        path
    }

    fn cleanup(path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn background_uses_neutral_light_and_dark_defaults() {
        let theme = ThemeFile::default();
        assert_eq!(theme.background.light.to_hex(), "#F3F3F3");
        assert_eq!(theme.background.dark.to_hex(), "#202020");
    }

    #[test]
    fn interactive_colors_use_mode_appropriate_neutral_defaults() {
        let theme = ThemeFile::default();
        assert_eq!((theme.accent.light.to_hex(), theme.accent.dark.to_hex()), ("#222222".into(), "#DDDDDD".into()));
        assert_eq!(
            (theme.search_list_item_selected.light.to_hex(), theme.search_list_item_selected.dark.to_hex()),
            ("#DFDFDF".into(), "#454545".into())
        );
        assert_eq!(
            (theme.search_list_item_hover.light.to_hex(), theme.search_list_item_hover.dark.to_hex()),
            ("#E9E9E9".into(), "#292929".into())
        );
        assert_eq!(TEXT_SELECTION.alpha(), 0x66);
    }

    #[test]
    fn structural_fallbacks_remain_achromatic() {
        let t = ThemeFile::default();
        let structural = [
            t.background,
            t.foreground,
            t.border,
            t.separator,
            t.hover,
            t.hover_deep,
            t.pressed,
            t.pressed_deep,
            t.control_background,
            t.control_background_deep,
            t.control_border,
            t.disabled_foreground,
            t.accent,
            t.acrylic,
            t.secondary_foreground,
            t.footer_background,
            t.slider_track,
            t.slider_progress,
            t.slider_thumb,
            t.button_hover,
            t.button_pressed,
            t.icon_foreground,
            t.card_background,
            t.text_box_focused,
            t.search_list_item_selected,
            t.search_list_item_hover,
            t.toggle_switch_on_track,
            t.toggle_switch_on_thumb,
            t.close_button_glyph_active,
            t.flyout_overlay_backdrop,
            t.flyout_shadow,
            t.menu_shadow,
        ];
        for color in structural {
            for c in [color.light, color.dark] {
                let (r, g, b) = ((c.0 >> 16) & 0xFF, (c.0 >> 8) & 0xFF, c.0 & 0xFF);
                assert!(r == g && g == b, "{c:?} must remain neutral");
            }
        }
    }

    #[test]
    fn hex_parsing_follows_color_math() {
        assert_eq!(Argb::parse_hex(" #102030 "), Some(Argb(0xFF10_2030)));
        assert_eq!(Argb::parse_hex("80405060"), Some(Argb(0x8040_5060)));
        assert_eq!(Argb::parse_hex("##102030"), None);
        assert_eq!(Argb::parse_hex("#12345"), None);
        assert_eq!(Argb::parse_hex("#GG0000"), None);
        assert_eq!(Argb::parse_hex("   "), None);
        assert_eq!(Argb::parse_hex_strict("123456"), Some(Argb(0xFF12_3456)));
        assert_eq!(Argb::parse_hex_strict(" #123456"), None);
        assert_eq!(Argb(0xFF12_3456).to_hex(), "#123456");
        assert_eq!(Argb(0x8040_5060).to_hex(), "#80405060");
        assert_eq!(Argb(0xFF12_3456).to_argb_hex(), "#FF123456");
        assert_eq!(Argb(0xFF00_0000).with_opacity(0.5).alpha(), 128);
        assert_eq!(Argb(0x0500_0000).with_opacity(0.5).alpha(), 2);
    }

    #[test]
    fn theme_file_overrides_known_colors_and_keeps_the_rest() {
        let path = temp_file(
            "valid",
            "<Theme Name=\"Custom\" Version=\"x\">\r\n  <Background LightHex=\"123456\" DarkHex=\"#ABCDEF\" />\r\n  \
             <Accent LightHex=\"#80FF0000\" />\r\n  <Unknown LightHex=\"zz\" />\r\n  <GlyphSettings>&#xE713;</GlyphSettings>\r\n</Theme>",
        );
        let theme = load_theme_file(&path);
        assert_eq!(theme.background, ThemeColor::new(0xFF12_3456, 0xFFAB_CDEF));
        assert_eq!(theme.accent, ThemeColor::new(0x80FF_0000, 0xFF00_0000), "missing DarkHex is #000000");
        assert_eq!(theme.foreground, ThemeFile::default().foreground);
        assert_eq!(ThemeFile::default().background.light.to_hex(), "#F3F3F3");
        cleanup(&path);
    }

    #[test]
    fn invalid_theme_file_uses_defaults_and_is_left_alone() {
        for (name, content) in [
            ("hex", "<Theme><Background LightHex=\"#12345G\" DarkHex=\"#000000\" /></Theme>"),
            ("root", "<NotTheme><Background LightHex=\"#123456\" /></NotTheme>"),
            ("syntax", "<Theme><Background LightHex=\"#123456\" </Theme>"),
        ] {
            let path = temp_file(name, content);
            assert_eq!(load_theme_file(&path), ThemeFile::default(), "{name}");
            assert_eq!(std::fs::read_to_string(&path).unwrap_or_default(), content);
            cleanup(&path);
        }
        assert_eq!(load_theme_file(Path::new(r"C:\definitely\missing\theme.xml")), ThemeFile::default());
    }

    #[test]
    fn palette_applies_overrides_like_app_settings_palette() {
        let theme = ThemeFile::default();
        let mut settings = CommonSettings::default();
        let dark = resolve_palette_for(&theme, &settings, false, true, FLUENT_ICONS_FAMILY);
        assert_eq!((dark.background, dark.foreground), (theme.background.dark, theme.foreground.dark));
        assert_eq!(dark.footer_background, theme.footer_background.dark);
        assert_eq!(dark.icon_foreground, theme.icon_foreground.dark);
        assert_eq!(dark.tray_icon, theme.foreground.dark);
        assert_eq!(dark.text_selection, Argb(0x66DD_DDDD));
        assert_eq!((dark.menu_font_size, dark.tool_tip_delay_ms, dark.ui_font), (15, 750, "Segoe UI"));

        settings.text_color = NullableThemeColor { light: Some(Argb(0xFF11_1111)), dark: None };
        settings.background_color = NullableThemeColor { light: None, dark: Some(Argb(0xFF22_2222)) };
        settings.tray_icon_color = NullableThemeColor { light: Some(Argb(0xFFFF_0000)), dark: None };
        let light = resolve_palette_for(&theme, &settings, true, false, MDL2_ASSETS_FAMILY);
        assert_eq!(light.foreground, Argb(0xFF11_1111));
        assert_eq!(light.background, theme.background.light);
        assert_eq!(light.tray_icon, Argb(0xFFFF_0000));
        assert_eq!(light.icon_foreground, theme.icon_foreground.light, "IconForeground ignores TextColor");
        let dark = resolve_palette_for(&theme, &settings, false, false, MDL2_ASSETS_FAMILY);
        assert_eq!(dark.background, Argb(0xFF22_2222));
        assert_eq!(dark.foreground, theme.foreground.dark);
        assert_eq!(dark.tray_icon, theme.foreground.dark, "tray icon ignores TextColor");
    }

    #[test]
    fn forced_modes_and_system_queries() {
        let mut settings = CommonSettings { theme_mode: ThemeMode::Light, ..CommonSettings::default() };
        assert!(resolve_is_light(&settings));
        settings.theme_mode = ThemeMode::Dark;
        assert!(!resolve_is_light(&settings));
        settings.animation_mode = AnimationMode::Disabled;
        assert!(!resolve_animations(&settings));
        settings.animation_mode = AnimationMode::Enabled;
        assert!(resolve_animations(&settings));
        let _ = system_uses_light_theme();
        let _ = system_animations_enabled();
        assert!(is_font_installed("Segoe UI"));
        assert!([FLUENT_ICONS_FAMILY, MDL2_ASSETS_FAMILY].contains(&icon_font_family()));
        assert!(!is_font_installed("No Such Font Family 12345"));
    }
}
