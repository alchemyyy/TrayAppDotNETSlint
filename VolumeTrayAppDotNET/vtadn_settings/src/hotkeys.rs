//! Hotkey editor state (port of `TrayAppDotNETHotkeysSection.cs` + `HotkeyKeys.AttachKeyCapture`):
//! per-row modifier/key selection, the Add/Exists state, the in-app chord conflict feedback, and
//! each binding's registration status from the last `SettingsHost::register_hotkeys`.

use tadn_common::hotkey::{self, HotkeyAction, HotkeyBinding, MODIFIER_OPTIONS, VK_DEBUGGER_RESERVED, VK_ESCAPE};
use tadn_shell::hotkeys::HotkeyRegistration;

use crate::items::{HotkeyView, model, text_option};
use crate::pages::{HOTKEY_ROWS, hotkey_title};
use crate::strings as s;
use crate::ui::{ComboOption, HotkeyEntry};

/// Capture state of one hotkey row (`RowState`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RowState {
    action: Option<HotkeyAction>,
    selected_modifiers: u32,
    selected_virtual_key: u32,
    key_text: String,
    rejected: Option<&'static str>,
}

/// What a key press in a capture box did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyCapture {
    /// A modifier alone: swallowed silently.
    Ignored,
    Captured(u32),
    Rejected(&'static str),
}

/// `AttachKeyCapture` rules: modifiers are swallowed, Escape and keys without a virtual key are
/// "not supported", F12 is reserved by the debugger.
pub(crate) fn classify_key(text: &str) -> KeyCapture {
    if hotkey::is_modifier_slint_text(text) {
        return KeyCapture::Ignored;
    }
    match hotkey::virtual_key_from_slint_text(text) {
        Some(VK_DEBUGGER_RESERVED) => KeyCapture::Rejected(s::Settings_Hotkeys_Error_ReservedKey),
        Some(VK_ESCAPE) | None => KeyCapture::Rejected(s::Settings_Hotkeys_KeyNotSupported),
        Some(vk) => KeyCapture::Captured(vk),
    }
}

pub(crate) struct HotkeyUi {
    rows: Vec<RowState>,
    /// Bindings and results of the last registration (one result per binding, in order).
    last: Option<(Vec<HotkeyBinding>, Vec<HotkeyRegistration>)>,
    modifier_options: slint::ModelRc<ComboOption>,
}

impl HotkeyUi {
    pub(crate) fn new() -> Self {
        Self {
            rows: HOTKEY_ROWS.iter().map(|row| RowState { action: Some(row.0), ..RowState::default() }).collect(),
            last: None,
            modifier_options: model(MODIFIER_OPTIONS.iter().map(|(label, _)| text_option(label)).collect()),
        }
    }

    /// Clears every row's pending modifier/key selection (a reopened window starts fresh).
    pub(crate) fn reset_rows(&mut self) {
        for row in &mut self.rows {
            *row = RowState { action: row.action, ..RowState::default() };
        }
    }

    pub(crate) fn modifier_options(&self) -> slint::ModelRc<ComboOption> {
        self.modifier_options.clone()
    }

    fn row_mut(&mut self, action: HotkeyAction) -> Option<&mut RowState> {
        self.rows.iter_mut().find(|r| r.action == Some(action))
    }

    fn row(&self, action: HotkeyAction) -> Option<&RowState> {
        self.rows.iter().find(|r| r.action == Some(action))
    }

    /// Stores the result of `SettingsHost::register_hotkeys` for `bindings`.
    pub(crate) fn set_results(&mut self, bindings: &[HotkeyBinding], results: Vec<HotkeyRegistration>) {
        self.last = Some((bindings.to_vec(), results));
    }

    pub(crate) fn select_modifiers(&mut self, action: HotkeyAction, index: usize) {
        if let Some(row) = self.row_mut(action) {
            row.selected_modifiers = MODIFIER_OPTIONS.get(index).map_or(0, |(_, m)| *m);
            row.rejected = None;
        }
    }

    /// A key pressed in the row's capture box. Returns true when the row changed.
    pub(crate) fn key_pressed(&mut self, action: HotkeyAction, text: &str) -> bool {
        let Some(row) = self.row_mut(action) else { return false };
        match classify_key(text) {
            KeyCapture::Ignored => false,
            KeyCapture::Captured(vk) => {
                row.selected_virtual_key = vk;
                row.key_text = hotkey::key_name(vk);
                row.rejected = None;
                true
            }
            KeyCapture::Rejected(message) => {
                row.rejected = Some(message);
                true
            }
        }
    }

    /// `AddBinding`: the new binding (or None when Add is not enabled); clears the row.
    pub(crate) fn take_new_binding(
        &mut self,
        action: HotkeyAction,
        bindings: &[HotkeyBinding],
    ) -> Option<HotkeyBinding> {
        let state = self.row(action)?.clone();
        if state.selected_modifiers == 0 || state.selected_virtual_key == 0 {
            return None;
        }
        if find_chord_owner(bindings, state.selected_modifiers, state.selected_virtual_key).is_some() {
            return None;
        }
        let binding_id = bindings
            .iter()
            .filter(|b| b.action == action && b.parameter.is_empty())
            .map(|b| b.binding_id)
            .max()
            .unwrap_or(0)
            + 1;
        if let Some(row) = self.row_mut(action) {
            *row = RowState { action: Some(action), ..RowState::default() };
        }
        Some(HotkeyBinding {
            action,
            parameter: String::new(),
            modifiers: state.selected_modifiers,
            virtual_key: state.selected_virtual_key,
            enabled: true,
            binding_id,
            removed_by_user: false,
        })
    }

    /// The bindings a row lists (`!RemovedByUser && IsBound`, by BindingID).
    pub(crate) fn row_bindings(action: HotkeyAction, bindings: &[HotkeyBinding]) -> Vec<&HotkeyBinding> {
        let mut list: Vec<&HotkeyBinding> = bindings
            .iter()
            .filter(|b| !b.removed_by_user && b.is_bound() && b.action == action && b.parameter.is_empty())
            .collect();
        list.sort_by_key(|b| b.binding_id);
        list
    }

    /// Registration problem text of `binding`, or None when it registered.
    fn problem(&self, binding: &HotkeyBinding) -> Option<String> {
        let Some((registered, results)) = &self.last else {
            return Some(s::Settings_Hotkeys_Status_HotkeyServiceUnavailable.to_owned());
        };
        if registered.len() != results.len() {
            return Some(s::Settings_Hotkeys_Status_HotkeyServiceUnavailable.to_owned());
        }
        let index = registered.iter().position(|b| b == binding)?;
        match &results[index] {
            HotkeyRegistration::Failed(message) => {
                // DuplicateInApp names the owning row by its title (C# TitleOf); the service only
                // knows the action's XML name.
                let owner = registered[..index].iter().find(|other| {
                    other.enabled
                        && !other.removed_by_user
                        && other.is_bound()
                        && other.has_same_chord(binding.modifiers, binding.virtual_key)
                });
                Some(match owner {
                    Some(owner) => {
                        s::Settings_Hotkeys_Error_DuplicateInApp_Format.replace("{0}", hotkey_title(owner.action))
                    }
                    None => message.clone(),
                })
            }
            HotkeyRegistration::Registered | HotkeyRegistration::Skipped => None,
        }
    }

    /// View data of one row (`RefreshRows` + `UpdateAddState`).
    pub(crate) fn row_view(&self, action: HotkeyAction, bindings: &[HotkeyBinding]) -> HotkeyView {
        let state = self.row(action).cloned().unwrap_or_default();
        let mut feedback = state.rejected.map(str::to_owned);
        let mut add_text = s::Settings_Hotkeys_Add_Button;
        let mut add_enabled = false;
        if state.selected_modifiers != 0 && state.selected_virtual_key != 0 {
            match find_chord_owner(bindings, state.selected_modifiers, state.selected_virtual_key) {
                None => add_enabled = true,
                Some(owner) if owner.action == action && owner.parameter.is_empty() => {
                    add_text = s::Settings_Hotkeys_Exists_Button;
                }
                Some(owner) => {
                    feedback = Some(
                        s::Settings_Hotkeys_ChordInUse_Format
                            .replace("{0}", &hotkey::display_text(state.selected_modifiers, state.selected_virtual_key))
                            .replace("{1}", hotkey_title(owner.action)),
                    );
                }
            }
        }
        let entries = Self::row_bindings(action, bindings)
            .into_iter()
            .map(|binding| {
                let problem = self.problem(binding);
                HotkeyEntry {
                    text: hotkey::display_text(binding.modifiers, binding.virtual_key).into(),
                    tooltip: problem.clone().unwrap_or_else(|| s::Settings_Hotkeys_Status_Registered.to_owned()).into(),
                    problem: problem.unwrap_or_default().into(),
                }
            })
            .collect();
        HotkeyView {
            modifier_selected: MODIFIER_OPTIONS
                .iter()
                .position(|(_, m)| *m == state.selected_modifiers && state.selected_modifiers != 0)
                .map_or(-1, |i| i as i32),
            key_text: state.key_text,
            add_text,
            add_enabled,
            feedback: feedback.unwrap_or_default(),
            entries,
        }
    }
}

/// `FindChordOwner`: the live binding that already uses the combination.
fn find_chord_owner(bindings: &[HotkeyBinding], modifiers: u32, virtual_key: u32) -> Option<&HotkeyBinding> {
    bindings.iter().find(|b| b.has_same_chord(modifiers, virtual_key))
}

/// `DeleteBinding`: Volume has no default bindings, so the binding is removed outright.
pub(crate) fn delete_binding(bindings: &mut Vec<HotkeyBinding>, action: HotkeyAction, binding_id: i32) {
    bindings.retain(|b| !(b.action == action && b.parameter.is_empty() && b.binding_id == binding_id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tadn_common::hotkey::{MOD_ALT, MOD_CONTROL};

    fn binding(action: HotkeyAction, modifiers: u32, vk: u32, id: i32) -> HotkeyBinding {
        HotkeyBinding { action, modifiers, virtual_key: vk, binding_id: id, ..HotkeyBinding::default() }
    }

    #[test]
    fn key_capture_rules() {
        assert_eq!(classify_key("\u{10}"), KeyCapture::Ignored);
        assert_eq!(classify_key("\u{1b}"), KeyCapture::Rejected(s::Settings_Hotkeys_KeyNotSupported));
        assert_eq!(classify_key("\u{F70F}"), KeyCapture::Rejected(s::Settings_Hotkeys_Error_ReservedKey));
        assert_eq!(classify_key("v"), KeyCapture::Captured(0x56));
    }

    #[test]
    fn add_assigns_next_binding_id_and_detects_exists_and_conflicts() {
        let mut ui = HotkeyUi::new();
        let mut bindings = vec![binding(HotkeyAction::OpenFlyout, MOD_CONTROL | MOD_ALT, 0x56, 3)];

        // Same chord on the same row: "Exists", Add disabled.
        ui.select_modifiers(HotkeyAction::OpenFlyout, 4); // Ctrl + Alt
        assert!(ui.key_pressed(HotkeyAction::OpenFlyout, "v"));
        let view = ui.row_view(HotkeyAction::OpenFlyout, &bindings);
        assert_eq!(view.add_text, s::Settings_Hotkeys_Exists_Button);
        assert!(!view.add_enabled);

        // Same chord on the other row: conflict feedback naming the owner.
        ui.select_modifiers(HotkeyAction::OpenSettings, 4);
        ui.key_pressed(HotkeyAction::OpenSettings, "v");
        let view = ui.row_view(HotkeyAction::OpenSettings, &bindings);
        assert_eq!(view.feedback, "Ctrl + Alt + V is already used by Open flyout.");
        assert!(!view.add_enabled);

        // A free chord adds with BindingID = max + 1 and clears the row.
        ui.key_pressed(HotkeyAction::OpenFlyout, "b");
        let added = ui.take_new_binding(HotkeyAction::OpenFlyout, &bindings).expect("binding");
        assert_eq!(added.binding_id, 4);
        assert_eq!(added.virtual_key, 0x42);
        bindings.push(added);
        assert_eq!(ui.row_view(HotkeyAction::OpenFlyout, &bindings).modifier_selected, -1);

        delete_binding(&mut bindings, HotkeyAction::OpenFlyout, 3);
        assert_eq!(bindings.len(), 1);
    }

    #[test]
    fn registration_problems_are_described() {
        let mut ui = HotkeyUi::new();
        let bindings = vec![
            binding(HotkeyAction::OpenFlyout, MOD_CONTROL, 0x41, 1),
            binding(HotkeyAction::OpenSettings, MOD_CONTROL, 0x41, 1),
        ];
        // No result yet: the service is unavailable.
        assert_eq!(
            ui.row_view(HotkeyAction::OpenFlyout, &bindings).entries[0].problem.as_str(),
            s::Settings_Hotkeys_Status_HotkeyServiceUnavailable
        );
        ui.set_results(
            &bindings,
            vec![
                HotkeyRegistration::Registered,
                HotkeyRegistration::Failed("Also bound to OpenFlyout in this app, which keeps it.".to_owned()),
            ],
        );
        let flyout = ui.row_view(HotkeyAction::OpenFlyout, &bindings);
        assert_eq!(flyout.entries[0].problem.as_str(), "");
        assert_eq!(flyout.entries[0].tooltip.as_str(), s::Settings_Hotkeys_Status_Registered);
        let settings = ui.row_view(HotkeyAction::OpenSettings, &bindings);
        assert_eq!(settings.entries[0].problem.as_str(), "Also bound to Open flyout in this app, which keeps it.");
    }
}
