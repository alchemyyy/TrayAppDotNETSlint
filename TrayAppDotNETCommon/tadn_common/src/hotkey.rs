//! Hotkey model (port of `Models/Hotkey.cs`, `UI/Hotkeys/HotkeyKeys.cs`). Registration with
//! `RegisterHotKey` lives in `tadn_shell::hotkeys`.
//!
//! XML: `<Hotkeys><Binding Action="OpenFlyout" Parameter="" Modifiers="3" VirtualKey="86"
//! Enabled="true" BindingID="1" RemovedByUser="false" /></Hotkeys>`, attributes in that order.

use std::collections::HashSet;

use crate::settings::XmlEnum;
use crate::xml::{XmlElement, XmlWriter, format_bool, parse_bool, parse_i32, parse_u32};

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

/// `HotkeyVirtualKeys.DebuggerReserved` (F12): rejected by key capture and registration.
pub const VK_DEBUGGER_RESERVED: u32 = 0x7B;
/// `VK_ESCAPE`: rejected by key capture ("not supported").
pub const VK_ESCAPE: u32 = 0x1B;

/// `TrayAppDotNETCommon.Models.HotkeyAction`. Member 0 (`OpenSettings`) is the XML default when
/// the `Action` attribute is missing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HotkeyAction {
    #[default]
    OpenSettings,
    OpenFlyout,
}

impl XmlEnum for HotkeyAction {
    const VARIANTS: &'static [(Self, &'static str)] =
        &[(Self::OpenSettings, "OpenSettings"), (Self::OpenFlyout, "OpenFlyout")];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub action: HotkeyAction,
    pub parameter: String,
    /// `MOD_*` flags (never `MOD_NOREPEAT`).
    pub modifiers: u32,
    /// `VK_*` code; 0 means unbound.
    pub virtual_key: u32,
    pub enabled: bool,
    pub binding_id: i32,
    pub removed_by_user: bool,
}

impl Default for HotkeyBinding {
    fn default() -> Self {
        Self {
            action: HotkeyAction::OpenSettings,
            parameter: String::new(),
            modifiers: 0,
            virtual_key: 0,
            enabled: true,
            binding_id: 0,
            removed_by_user: false,
        }
    }
}

impl HotkeyBinding {
    /// Identity used for dedupe: (Action, Parameter, BindingID).
    pub fn identity(&self) -> (HotkeyAction, &str, i32) {
        (self.action, self.parameter.as_str(), self.binding_id)
    }

    /// `IsBound`: a key and at least one modifier.
    pub fn is_bound(&self) -> bool {
        self.virtual_key != 0 && self.modifiers != 0
    }

    /// `HasSameChord`: live (not tombstoned) and the same modifiers and key.
    pub fn has_same_chord(&self, modifiers: u32, virtual_key: u32) -> bool {
        !self.removed_by_user && self.modifiers == modifiers && self.virtual_key == virtual_key
    }

    /// Reads one `<Binding>` element. Returns None when the `Action` attribute names an action
    /// this build does not know (`HasUnknownAction`: such a binding is dropped on load).
    pub fn from_xml(element: &XmlElement) -> Option<Self> {
        let mut binding = Self::default();
        if let Some(action) = element.attr("Action") {
            binding.action = HotkeyAction::from_xml_name(action)?;
        }
        if let Some(parameter) = element.attr("Parameter") {
            binding.parameter = parameter.to_owned();
        }
        if let Some(value) = element.attr("Modifiers").and_then(parse_u32) {
            binding.modifiers = value;
        }
        if let Some(value) = element.attr("VirtualKey").and_then(parse_u32) {
            binding.virtual_key = value;
        }
        if let Some(value) = element.attr("Enabled").and_then(parse_bool) {
            binding.enabled = value;
        }
        if let Some(value) = element.attr("BindingID").and_then(parse_i32) {
            binding.binding_id = value;
        }
        if let Some(value) = element.attr("RemovedByUser").and_then(parse_bool) {
            binding.removed_by_user = value;
        }
        Some(binding)
    }

    /// Writes `<Binding .../>` with the 7 attributes in C# declaration order.
    pub fn write_xml(&self, writer: &mut XmlWriter) {
        writer.start_element("Binding");
        writer.attribute("Action", self.action.xml_name());
        writer.attribute("Parameter", &self.parameter);
        writer.attribute("Modifiers", &self.modifiers.to_string());
        writer.attribute("VirtualKey", &self.virtual_key.to_string());
        writer.attribute("Enabled", format_bool(self.enabled));
        writer.attribute("BindingID", &self.binding_id.to_string());
        writer.attribute("RemovedByUser", format_bool(self.removed_by_user));
        writer.end_element();
    }
}

/// Element name of the binding list (`[XmlArray("Hotkeys")]`).
pub const HOTKEYS_ELEMENT: &str = "Hotkeys";

/// Reads the `<Hotkeys>` wrapper: `<Binding>` items in order, other elements skipped. Returns the
/// bindings and whether any binding with an unknown action was dropped.
pub fn read_bindings_xml(wrapper: &XmlElement) -> (Vec<HotkeyBinding>, bool) {
    let mut dropped = false;
    let bindings = wrapper
        .children_named("Binding")
        .filter_map(|element| {
            let binding = HotkeyBinding::from_xml(element);
            dropped |= binding.is_none();
            binding
        })
        .collect();
    (bindings, dropped)
}

/// Writes `<Hotkeys>` with one `<Binding>` per entry (`<Hotkeys />` when empty).
pub fn write_bindings_xml(writer: &mut XmlWriter, bindings: &[HotkeyBinding]) {
    writer.start_element(HOTKEYS_ELEMENT);
    for binding in bindings {
        binding.write_xml(writer);
    }
    writer.end_element();
}

/// `Normalize` on load: drops duplicate identities (first wins). Returns true when anything changed
/// (the caller then saves once). Volume has no default bindings to append. Bindings with an
/// unknown action never reach the list (see [`read_bindings_xml`]).
pub fn normalize(bindings: &mut Vec<HotkeyBinding>) -> bool {
    let before = bindings.len();
    let mut seen = HashSet::new();
    bindings.retain(|b| seen.insert((b.action, b.parameter.clone(), b.binding_id)));
    bindings.len() != before
}

/// `HotkeyDefaults.Normalize` with an app's fresh defaults: dedupe as [`normalize`], then append
/// every default whose identity is absent (a tombstone counts as present).
pub fn normalize_with_defaults(bindings: &mut Vec<HotkeyBinding>, defaults: &[HotkeyBinding]) -> bool {
    let mut changed = normalize(bindings);
    for default in defaults {
        if !bindings.iter().any(|b| b.identity() == default.identity()) {
            bindings.push(default.clone());
            changed = true;
        }
    }
    changed
}

/// `ModifierText`: "Ctrl + Alt + Shift + Win" subset in that order.
pub fn modifier_text(modifiers: u32) -> String {
    let parts: Vec<&str> = [(MOD_CONTROL, "Ctrl"), (MOD_ALT, "Alt"), (MOD_SHIFT, "Shift"), (MOD_WIN, "Win")]
        .into_iter()
        .filter(|(flag, _)| modifiers & flag != 0)
        .map(|(_, name)| name)
        .collect();
    parts.join(" + ")
}

/// `TrayAppDotNETHotkeyModifierOptions.Create`: the 15 non-empty combinations in dropdown order.
pub const MODIFIER_OPTIONS: [(&str, u32); 15] = [
    ("Ctrl", MOD_CONTROL),
    ("Alt", MOD_ALT),
    ("Shift", MOD_SHIFT),
    ("Win", MOD_WIN),
    ("Ctrl + Alt", MOD_CONTROL | MOD_ALT),
    ("Ctrl + Shift", MOD_CONTROL | MOD_SHIFT),
    ("Ctrl + Win", MOD_CONTROL | MOD_WIN),
    ("Alt + Shift", MOD_ALT | MOD_SHIFT),
    ("Alt + Win", MOD_ALT | MOD_WIN),
    ("Shift + Win", MOD_SHIFT | MOD_WIN),
    ("Ctrl + Alt + Shift", MOD_CONTROL | MOD_ALT | MOD_SHIFT),
    ("Ctrl + Alt + Win", MOD_CONTROL | MOD_ALT | MOD_WIN),
    ("Ctrl + Shift + Win", MOD_CONTROL | MOD_SHIFT | MOD_WIN),
    ("Alt + Shift + Win", MOD_ALT | MOD_SHIFT | MOD_WIN),
    ("Ctrl + Alt + Shift + Win", MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_WIN),
];

fn vk_format(vk: u32) -> String {
    format!("VK {vk}")
}

fn locale_info(kind: u32, fallback: &str) -> String {
    use windows::Win32::Globalization::GetLocaleInfoEx;
    use windows_core::PCWSTR;
    let mut buffer = [0u16; 16];
    // SAFETY: LOCALE_NAME_USER_DEFAULT is a null name; `buffer` is a live local the call fills.
    let length = unsafe { GetLocaleInfoEx(PCWSTR::null(), kind, Some(&mut buffer)) };
    if length <= 1 {
        return fallback.to_owned();
    }
    String::from_utf16_lossy(&buffer[..(length - 1) as usize])
}

fn num_pad_operator_name(vk: u32) -> String {
    use windows::Win32::Globalization::{LOCALE_SDECIMAL, LOCALE_SLIST};
    let symbol = match vk {
        0x6A => "*".to_owned(),
        0x6B => "+".to_owned(),
        0x6C => locale_info(LOCALE_SLIST, ","),
        0x6D => "-".to_owned(),
        0x6E => locale_info(LOCALE_SDECIMAL, "."),
        _ => "/".to_owned(),
    };
    format!("Num {symbol}")
}

/// Punctuation keycaps depend on the keyboard layout, so the layout names them.
fn layout_character_name(vk: u32) -> String {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_CHAR, MapVirtualKeyW};
    // SAFETY: MapVirtualKeyW has no pointer arguments.
    let mapped = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_CHAR) };
    match char::from_u32(mapped & 0xFFFF) {
        Some(c) if c >= ' ' => c.to_uppercase().collect(),
        _ => vk_format(vk),
    }
}

/// `KeyName`: letters, digits and function keys keep their keycap text, punctuation keys show the
/// character of the current keyboard layout, named keys use the English resource names.
pub fn key_name(vk: u32) -> String {
    match vk {
        0x41..=0x5A | 0x30..=0x39 => char::from_u32(vk).map_or_else(|| vk_format(vk), String::from),
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x6A..=0x6F => num_pad_operator_name(vk),
        0xBA..=0xC0 | 0xDB..=0xDF | 0xE2 => layout_character_name(vk),
        0x08 => "Backspace".to_owned(),
        0x09 => "Tab".to_owned(),
        0x0D => "Enter".to_owned(),
        0x13 => "Pause".to_owned(),
        0x1B => "Esc".to_owned(),
        0x20 => "Space".to_owned(),
        0x21 => "Page Up".to_owned(),
        0x22 => "Page Down".to_owned(),
        0x23 => "End".to_owned(),
        0x24 => "Home".to_owned(),
        0x25 => "Left".to_owned(),
        0x26 => "Up".to_owned(),
        0x27 => "Right".to_owned(),
        0x28 => "Down".to_owned(),
        0x2C => "Print Screen".to_owned(),
        0x2D => "Insert".to_owned(),
        0x2E => "Delete".to_owned(),
        0x5D => "Menu".to_owned(),
        _ => vk_format(vk),
    }
}

/// "Ctrl + Alt + V" style text (HotkeyKeys names, modifiers joined by " + ").
pub fn display_text(modifiers: u32, virtual_key: u32) -> String {
    let modifiers = modifier_text(modifiers);
    let key = key_name(virtual_key);
    if modifiers.is_empty() { key } else { format!("{modifiers} + {key}") }
}

/// Virtual keys `VirtualKeyFromKey` can produce (the capture box's supported set).
pub fn is_capturable_virtual_key(vk: u32) -> bool {
    matches!(
        vk,
        0x41..=0x5A
            | 0x30..=0x39
            | 0x60..=0x6F
            | 0x70..=0x87
            | 0x08
            | 0x09
            | 0x0D
            | 0x13
            | 0x1B
            | 0x20..=0x28
            | 0x2C..=0x2E
            | 0x5D
            | 0xBA..=0xC0
            | 0xDB..=0xDF
            | 0xE2
    )
}

/// True for Slint key texts of pure modifier keys (Shift, Control, Alt, AltGr, Meta, left and
/// right): key capture swallows them silently while the user builds a chord.
pub fn is_modifier_slint_text(text: &str) -> bool {
    let mut chars = text.chars();
    matches!((chars.next(), chars.next()), (Some('\u{10}'..='\u{13}' | '\u{15}'..='\u{18}'), None))
}

/// Maps a Slint `KeyEvent.text` (printable character or a `slint::platform::Key` special
/// character) to a Windows virtual-key code for hotkey capture. Pure modifier keys return None.
/// Escape (0x1B) and F12 (0x7B) are returned; the capture UI rejects them like `AttachKeyCapture`.
/// Printable characters map through the current keyboard layout (`VkKeyScanW`), so shifted
/// symbols resolve to their key; Slint reports numpad digits and operators as their characters,
/// so they map to the main-keyboard keys.
pub fn virtual_key_from_slint_text(text: &str) -> Option<u32> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let vk = match c {
        '\u{8}' => 0x08,
        '\t' | '\u{19}' => 0x09,
        '\n' | '\r' => 0x0D,
        '\u{1B}' => 0x1B,
        '\u{7F}' => 0x2E,
        ' ' => 0x20,
        '\u{F700}' => 0x26,
        '\u{F701}' => 0x28,
        '\u{F702}' => 0x25,
        '\u{F703}' => 0x27,
        '\u{F704}'..='\u{F71B}' => 0x70 + (c as u32 - 0xF704),
        '\u{F727}' => 0x2D,
        '\u{F729}' => 0x24,
        '\u{F72B}' => 0x23,
        '\u{F72C}' => 0x21,
        '\u{F72D}' => 0x22,
        '\u{F730}' => 0x13,
        '\u{F731}' => 0x2C,
        '\u{F735}' => 0x5D,
        'a'..='z' => c as u32 - 'a' as u32 + 0x41,
        'A'..='Z' | '0'..='9' => c as u32,
        _ if c < ' ' || ('\u{F700}'..='\u{F8FF}').contains(&c) => return None,
        _ => layout_virtual_key(c)?,
    };
    is_capturable_virtual_key(vk).then_some(vk)
}

fn layout_virtual_key(c: char) -> Option<u32> {
    use windows::Win32::UI::Input::KeyboardAndMouse::VkKeyScanW;
    let mut units = [0u16; 2];
    let encoded = c.encode_utf16(&mut units);
    if encoded.len() != 1 {
        return None;
    }
    // SAFETY: VkKeyScanW has no pointer arguments.
    let scan = unsafe { VkKeyScanW(encoded[0]) };
    if scan == -1 {
        return None;
    }
    Some(u32::from(scan as u16 & 0xFF))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::XmlElement;

    fn binding(action: HotkeyAction, modifiers: u32, vk: u32, id: i32) -> HotkeyBinding {
        HotkeyBinding { action, modifiers, virtual_key: vk, binding_id: id, ..HotkeyBinding::default() }
    }

    #[test]
    fn normalize_drops_duplicate_identities_first_wins() {
        let mut list = vec![
            binding(HotkeyAction::OpenFlyout, 3, 70, 0),
            binding(HotkeyAction::OpenFlyout, 6, 71, 0),
            binding(HotkeyAction::OpenFlyout, 6, 72, 1),
            binding(HotkeyAction::OpenSettings, 6, 73, 0),
        ];
        assert!(normalize(&mut list));
        assert_eq!(list.iter().map(|b| b.virtual_key).collect::<Vec<_>>(), [70, 72, 73]);
        assert!(!normalize(&mut list));
        let tombstone = HotkeyBinding { removed_by_user: true, ..binding(HotkeyAction::OpenSettings, 0, 0, 0) };
        let mut with_defaults = vec![tombstone];
        assert!(!normalize_with_defaults(&mut with_defaults, &[binding(HotkeyAction::OpenSettings, 3, 83, 0)]));
        assert!(normalize_with_defaults(&mut with_defaults, &[binding(HotkeyAction::OpenFlyout, 3, 86, 0)]));
        assert_eq!(with_defaults.len(), 2);
    }

    #[test]
    fn bindings_xml_reads_attributes_and_drops_unknown_actions() {
        let root = XmlElement::parse(
            b"<Hotkeys><Binding Action=\"OpenFlyout\" Parameter=\"\" Modifiers=\"3\" VirtualKey=\"70\" BindingID=\"0\" />\
              <Binding Action=\"ActionFromANewerBuild\" Modifiers=\"3\" VirtualKey=\"72\" /><Other /><Binding /></Hotkeys>",
        )
        .unwrap_or_default();
        let (list, dropped) = read_bindings_xml(&root);
        assert!(dropped);
        assert_eq!(list, vec![binding(HotkeyAction::OpenFlyout, 3, 70, 0), HotkeyBinding::default()]);
        let mut writer = crate::xml::XmlWriter::new();
        write_bindings_xml(
            &mut writer,
            &[HotkeyBinding { parameter: "p\"".into(), ..binding(HotkeyAction::OpenFlyout, 3, 86, 1) }],
        );
        let text = String::from_utf8(writer.finish()).unwrap_or_default();
        assert!(text.ends_with(
            "<Hotkeys>\r\n  <Binding Action=\"OpenFlyout\" Parameter=\"p&quot;\" Modifiers=\"3\" VirtualKey=\"86\" \
             Enabled=\"true\" BindingID=\"1\" RemovedByUser=\"false\" />\r\n</Hotkeys>"
        ));
    }

    #[test]
    fn binding_state_helpers() {
        assert!(!binding(HotkeyAction::OpenFlyout, 0, 86, 0).is_bound());
        assert!(binding(HotkeyAction::OpenFlyout, 3, 86, 0).is_bound());
        let removed = HotkeyBinding { removed_by_user: true, ..binding(HotkeyAction::OpenFlyout, 3, 86, 0) };
        assert!(!removed.has_same_chord(3, 86));
    }

    #[test]
    fn display_text_names_keys() {
        assert_eq!(display_text(MOD_CONTROL | MOD_ALT, 0x56), "Ctrl + Alt + V");
        assert_eq!(display_text(MOD_WIN | MOD_SHIFT | MOD_ALT | MOD_CONTROL, 0x70), "Ctrl + Alt + Shift + Win + F1");
        assert_eq!(display_text(0, 0x21), "Page Up");
        assert_eq!(key_name(0x65), "Num 5");
        assert_eq!(key_name(0x6A), "Num *");
        assert_eq!(key_name(0x87), "F24");
        assert_eq!(key_name(0x1B), "Esc");
        assert_eq!(key_name(0x5D), "Menu");
        assert_eq!(key_name(0xFF), "VK 255");
        assert_eq!(MODIFIER_OPTIONS.len(), 15);
        assert!(MODIFIER_OPTIONS.iter().all(|(label, mods)| modifier_text(*mods) == *label));
    }

    #[test]
    fn every_capturable_key_has_a_name() {
        let unnamed: Vec<u32> = (0..=0xFFu32)
            .filter(|&vk| is_capturable_virtual_key(vk))
            .filter(|&vk| !matches!(vk, 0xBA..=0xC0 | 0xDB..=0xDF | 0xE2))
            .filter(|&vk| key_name(vk) == format!("VK {vk}"))
            .collect();
        assert!(unnamed.is_empty(), "{unnamed:x?}");
    }

    #[test]
    fn slint_key_text_maps_to_virtual_keys() {
        for (text, vk) in [
            ("a", 0x41),
            ("V", 0x56),
            ("7", 0x37),
            (" ", 0x20),
            ("\u{8}", 0x08),
            ("\t", 0x09),
            ("\n", 0x0D),
            ("\u{1b}", 0x1B),
            ("\u{7f}", 0x2E),
            ("\u{F700}", 0x26),
            ("\u{F701}", 0x28),
            ("\u{F702}", 0x25),
            ("\u{F703}", 0x27),
            ("\u{F704}", 0x70),
            ("\u{F70F}", 0x7B),
            ("\u{F71B}", 0x87),
            ("\u{F727}", 0x2D),
            ("\u{F729}", 0x24),
            ("\u{F72B}", 0x23),
            ("\u{F72C}", 0x21),
            ("\u{F72D}", 0x22),
            ("\u{F730}", 0x13),
            ("\u{F731}", 0x2C),
            ("\u{F735}", 0x5D),
        ] {
            assert_eq!(virtual_key_from_slint_text(text), Some(vk), "{text:?}");
        }
        for text in ["", "ab", "\u{10}", "\u{11}", "\u{12}", "\u{14}", "\u{17}", "\u{F72F}"] {
            assert_eq!(virtual_key_from_slint_text(text), None, "{text:?}");
        }
        assert!(is_modifier_slint_text("\u{10}") && is_modifier_slint_text("\u{18}"));
        assert!(!is_modifier_slint_text("\u{14}") && !is_modifier_slint_text("a"));
        // Punctuation goes through the active layout; whatever it maps to must be a capturable key.
        if let Some(vk) = virtual_key_from_slint_text(";") {
            assert!(is_capturable_virtual_key(vk));
        }
    }
}
