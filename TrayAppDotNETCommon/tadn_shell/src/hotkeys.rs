//! Global hotkeys via `RegisterHotKey` on a hidden message window (port of
//! `Services/GlobalHotkeyService.cs`).
//!
//! [`HotkeyService::register_all`] diffs like the C# `Apply`: a key combination that stays bound
//! keeps its registration, so applying an unchanged list touches no Win32 state.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use tadn_common::hotkey::HotkeyBinding;
use windows::Win32::Foundation::{ERROR_HOTKEY_ALREADY_REGISTERED, HWND, LRESULT};
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::WindowsAndMessaging::WM_HOTKEY;

use crate::msgwnd::MessageWindow;
use crate::util::post;

/// `HotkeyModifiers.Required`: Alt, Ctrl, Shift, Win.
const REQUIRED_MODIFIERS: u32 = 0xF;
/// `HotkeyVirtualKeys.DebuggerReserved` (F12).
const DEBUGGER_RESERVED_KEY: u32 = 0x7B;

/// Result of registering one binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyRegistration {
    Registered,
    /// Disabled, unbound or removed: nothing registered.
    Skipped,
    /// Not registered; the text is the English settings message for the [`HotkeyFailure`]
    /// (the owner of a duplicate is named by its action's XML name; use
    /// [`HotkeyService::last_failures`] + [`HotkeyFailure::message`] to name it by its title).
    Failed(String),
}

/// `HotkeyRegistrationError` with its payload: why an enabled, bound binding is not registered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyFailure {
    NoKey,
    NoModifier,
    /// F12 is reserved by the debugger.
    ReservedKey,
    /// An earlier binding (index into the same slice) already uses the key combination.
    DuplicateInApp {
        owner: usize,
    },
    /// Windows refused the combination because another app (or Windows) registered it (1409).
    InUseByAnotherApp,
    /// Any other `RegisterHotKey` error.
    Failed {
        win32_error: u32,
    },
}

impl HotkeyFailure {
    /// `TrayAppDotNETHotkeysSection.DescribeFailure` with the English `CommonStrings`;
    /// `owner_title` names the binding that keeps a duplicated combination.
    pub fn message(&self, owner_title: &str) -> String {
        match self {
            Self::NoKey => "No key set.".to_owned(),
            Self::NoModifier => "At least one modifier (Ctrl, Alt, Shift, Win) is required.".to_owned(),
            Self::ReservedKey => "F12 is reserved by the debugger.".to_owned(),
            Self::InUseByAnotherApp => "Already in use by another app or by Windows.".to_owned(),
            Self::DuplicateInApp { .. } => format!("Also bound to {owner_title} in this app, which keeps it."),
            Self::Failed { win32_error } => format!("Registration failed (Windows error {win32_error})."),
        }
    }
}

/// `GlobalHotkeyService.Validate`: defence in depth against hand-edited settings.
pub fn validate(binding: &HotkeyBinding) -> Option<HotkeyFailure> {
    if binding.virtual_key == 0 {
        Some(HotkeyFailure::NoKey)
    } else if binding.modifiers & REQUIRED_MODIFIERS == 0 {
        Some(HotkeyFailure::NoModifier)
    } else if binding.virtual_key == DEBUGGER_RESERVED_KEY {
        Some(HotkeyFailure::ReservedKey)
    } else {
        None
    }
}

/// `HotkeyBindingBase.IsBound`: a key and at least one modifier bit.
fn is_bound(binding: &HotkeyBinding) -> bool {
    binding.virtual_key != 0 && binding.modifiers != 0
}

type Chord = (u32, u32);

struct Registration {
    id: i32,
    /// Index of the binding that owns the chord in the slice last passed to `register_all`.
    index: usize,
}

struct State {
    window: RefCell<Option<MessageWindow>>,
    registered: RefCell<HashMap<Chord, Registration>>,
    next_id: Cell<i32>,
    last_failures: RefCell<Vec<Option<HotkeyFailure>>>,
    handler: Box<dyn Fn(usize)>,
}

pub struct HotkeyService {
    state: Rc<State>,
}

impl HotkeyService {
    /// Creates the message window on the UI thread. `handler(index)` receives the index into the
    /// slice last passed to [`HotkeyService::register_all`], asynchronously on the UI thread.
    pub fn new(handler: impl Fn(usize) + 'static) -> windows_core::Result<Self> {
        let state = Rc::new(State {
            window: RefCell::new(None),
            registered: RefCell::new(HashMap::new()),
            next_id: Cell::new(1),
            last_failures: RefCell::new(Vec::new()),
            handler: Box::new(handler),
        });
        let weak = Rc::downgrade(&state);
        let window = MessageWindow::new(
            "TrayAppDotNETSlint.HotkeySink.MessageWindow",
            true,
            Rc::new(move |_, message, wparam, _| {
                if message != WM_HOTKEY {
                    return None;
                }
                let state = weak.upgrade()?;
                let id = wparam.0 as i32;
                let index = state.registered.borrow().values().find(|r| r.id == id).map(|r| r.index)?;
                let weak = Rc::downgrade(&state);
                post(move || {
                    if let Some(state) = weak.upgrade() {
                        (state.handler)(index);
                    }
                });
                Some(LRESULT(0))
            }),
        )?;
        *state.window.borrow_mut() = Some(window);
        Ok(Self { state })
    }

    /// Brings the registrations in line with `bindings`: unregisters combinations no longer bound,
    /// registers new ones (`MOD_NOREPEAT` added), and keeps the rest. The first binding of a
    /// combination wins; later ones fail with [`HotkeyFailure::DuplicateInApp`]. Returns one result
    /// per input binding, in order; every failure is logged.
    pub fn register_all(&self, bindings: &[HotkeyBinding]) -> Vec<HotkeyRegistration> {
        let Some(hwnd) = self.hwnd() else {
            return vec![HotkeyRegistration::Skipped; bindings.len()];
        };
        let mut failures: Vec<Option<HotkeyFailure>> = vec![None; bindings.len()];
        let mut skipped = vec![false; bindings.len()];
        let mut wanted: Vec<(Chord, usize)> = Vec::new();
        for (index, binding) in bindings.iter().enumerate() {
            if binding.removed_by_user || !binding.enabled || !is_bound(binding) {
                skipped[index] = true;
                continue;
            }
            if let Some(invalid) = validate(binding) {
                failures[index] = Some(invalid);
                continue;
            }
            let chord = (binding.modifiers, binding.virtual_key);
            match wanted.iter().find(|(existing, _)| *existing == chord) {
                Some(&(_, owner)) => failures[index] = Some(HotkeyFailure::DuplicateInApp { owner }),
                None => wanted.push((chord, index)),
            }
        }

        {
            let mut registered = self.state.registered.borrow_mut();
            registered.retain(|chord, registration| {
                let keep = wanted.iter().any(|(wanted_chord, _)| wanted_chord == chord);
                if !keep {
                    unregister(hwnd, registration.id);
                }
                keep
            });
            if registered.is_empty() {
                self.state.next_id.set(1);
            }

            for &(chord, index) in &wanted {
                if let Some(existing) = registered.get_mut(&chord) {
                    existing.index = index;
                    continue;
                }
                let id = self.state.next_id.get();
                self.state.next_id.set(id + 1);
                // SAFETY: `hwnd` is this service's message window.
                match unsafe { RegisterHotKey(Some(hwnd), id, HOT_KEY_MODIFIERS(chord.0) | MOD_NOREPEAT, chord.1) } {
                    Ok(()) => {
                        registered.insert(chord, Registration { id, index });
                    }
                    Err(error) => {
                        let code = (error.code().0 as u32) & 0xFFFF;
                        failures[index] = Some(if code == ERROR_HOTKEY_ALREADY_REGISTERED.0 {
                            HotkeyFailure::InUseByAnotherApp
                        } else {
                            HotkeyFailure::Failed { win32_error: code }
                        });
                    }
                }
            }
        }

        let results = bindings
            .iter()
            .enumerate()
            .map(|(index, binding)| match failures[index] {
                Some(failure) => {
                    let owner_name = match failure {
                        HotkeyFailure::DuplicateInApp { owner } => format!("{:?}", bindings[owner].action),
                        _ => String::new(),
                    };
                    log::warn!(
                        "Hotkey {:?}({}) #{} not registered: {failure:?}",
                        binding.action,
                        binding.parameter,
                        binding.binding_id
                    );
                    HotkeyRegistration::Failed(failure.message(&owner_name))
                }
                None if skipped[index] => HotkeyRegistration::Skipped,
                None => HotkeyRegistration::Registered,
            })
            .collect();
        *self.state.last_failures.borrow_mut() = failures;
        results
    }

    /// Failures of the latest [`HotkeyService::register_all`], one entry per input binding
    /// (`LastResult.Failed`); settings pages read it instead of applying again.
    pub fn last_failures(&self) -> Vec<Option<HotkeyFailure>> {
        self.state.last_failures.borrow().clone()
    }

    /// The `RegisterHotKey` id the binding at `index` is registered under (for tests/diagnostics).
    pub fn registration_id(&self, index: usize) -> Option<i32> {
        self.state.registered.borrow().values().find(|r| r.index == index).map(|r| r.id)
    }

    pub fn unregister_all(&self) {
        if let Some(hwnd) = self.hwnd() {
            for registration in self.state.registered.borrow().values() {
                unregister(hwnd, registration.id);
            }
        }
        self.state.registered.borrow_mut().clear();
        self.state.next_id.set(1);
    }

    fn hwnd(&self) -> Option<HWND> {
        self.state.window.borrow().as_ref().map(MessageWindow::hwnd)
    }
}

impl Drop for HotkeyService {
    fn drop(&mut self) {
        self.unregister_all();
        let window = self.state.window.borrow_mut().take();
        drop(window);
    }
}

fn unregister(hwnd: HWND, id: i32) {
    // SAFETY: `hwnd` is this service's message window.
    if let Err(error) = unsafe { UnregisterHotKey(Some(hwnd), id) } {
        log::warn!("UnregisterHotKey({id}) failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use tadn_common::hotkey::{HotkeyAction, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};

    use super::*;

    // Combinations no app or Windows binds: Ctrl+Alt+Shift+Win with F22 and F23.
    const ALL_MODIFIERS: u32 = MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_WIN;
    const VK_F22: u32 = 0x85;
    const VK_F23: u32 = 0x86;

    /// The tests register the same system-wide combinations, so they run one at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn service() -> (std::sync::MutexGuard<'static, ()>, HotkeyService) {
        let guard = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (guard, HotkeyService::new(|_| {}).expect("message window"))
    }

    fn binding(action: HotkeyAction, virtual_key: u32) -> HotkeyBinding {
        HotkeyBinding { action, modifiers: ALL_MODIFIERS, virtual_key, ..HotkeyBinding::default() }
    }

    #[test]
    #[ignore = "registers system-wide hotkeys on a message window; run with --ignored"]
    fn second_binding_of_one_combination_reports_the_binding_that_keeps_it() {
        let (_serial, service) = service();
        let bindings = [binding(HotkeyAction::OpenFlyout, VK_F22), binding(HotkeyAction::OpenSettings, VK_F22)];

        let results = service.register_all(&bindings);

        assert_eq!(results[0], HotkeyRegistration::Registered);
        assert_eq!(
            results[1],
            HotkeyRegistration::Failed("Also bound to OpenFlyout in this app, which keeps it.".to_owned())
        );
        assert_eq!(service.last_failures()[1], Some(HotkeyFailure::DuplicateInApp { owner: 0 }));
    }

    #[test]
    #[ignore = "registers system-wide hotkeys on a message window; run with --ignored"]
    fn apply_keeps_registrations_of_unchanged_combinations() {
        let (_serial, service) = service();
        let mut bindings = [binding(HotkeyAction::OpenFlyout, VK_F22), binding(HotkeyAction::OpenSettings, VK_F23)];
        service.register_all(&bindings);
        let kept_id = service.registration_id(0);
        assert!(kept_id.is_some());

        bindings[1].enabled = false;
        let results = service.register_all(&bindings);

        assert_eq!(service.registration_id(0), kept_id);
        assert_eq!(service.registration_id(1), None);
        assert_eq!(results[1], HotkeyRegistration::Skipped);
    }

    #[test]
    #[ignore = "registers system-wide hotkeys on a message window; run with --ignored"]
    fn ids_start_over_once_nothing_is_registered() {
        let (_serial, service) = service();
        let bindings = [binding(HotkeyAction::OpenFlyout, VK_F22)];
        service.register_all(&bindings);
        let first_id = service.registration_id(0);

        service.register_all(&[]);
        service.register_all(&bindings);

        assert_eq!(service.registration_id(0), first_id);
    }

    #[test]
    #[ignore = "registers system-wide hotkeys on a message window; run with --ignored"]
    fn invalid_bindings_are_reported_without_registering() {
        let (_serial, service) = service();
        let reserved = HotkeyBinding {
            action: HotkeyAction::OpenFlyout,
            modifiers: MOD_CONTROL,
            virtual_key: DEBUGGER_RESERVED_KEY,
            ..HotkeyBinding::default()
        };

        let results = service.register_all(&[reserved]);

        assert_eq!(results[0], HotkeyRegistration::Failed("F12 is reserved by the debugger.".to_owned()));
        assert_eq!(service.registration_id(0), None);
    }

    #[test]
    #[ignore = "registers system-wide hotkeys on a message window; run with --ignored"]
    fn removed_disabled_and_unbound_bindings_are_skipped() {
        let (_serial, service) = service();
        let removed = HotkeyBinding { removed_by_user: true, ..binding(HotkeyAction::OpenFlyout, VK_F22) };
        let disabled = HotkeyBinding { enabled: false, ..binding(HotkeyAction::OpenFlyout, VK_F22) };
        let unbound = HotkeyBinding { modifiers: 0, ..binding(HotkeyAction::OpenFlyout, VK_F22) };

        let results = service.register_all(&[removed, disabled, unbound]);

        assert_eq!(results, vec![HotkeyRegistration::Skipped; 3]);
    }
}
