//! System change notifications from a hidden top-level message window: light/dark and accent
//! changes (`WM_SETTINGCHANGE` "ImmersiveColorSet"), animation/system parameter changes,
//! display/DPI changes, and session end.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    PBT_APMRESUMEAUTOMATIC, SPI_SETWORKAREA, WM_DISPLAYCHANGE, WM_ENDSESSION, WM_POWERBROADCAST, WM_SETTINGCHANGE,
};

use crate::msgwnd::MessageWindow;
use crate::util::post;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemEvent {
    /// Apps light/dark mode or accent may have changed: recompute the palette.
    ThemeChanged,
    /// `SystemParametersInfo` values changed (animations, double-click time, ...).
    ParametersChanged,
    /// Monitors, resolution, work area or DPI changed (`WM_DISPLAYCHANGE`, `SPI_SETWORKAREA`).
    DisplayChanged,
    /// `WM_ENDSESSION` with `wParam = TRUE`: save and exit now.
    SessionEnding,
    /// `PBT_APMRESUMEAUTOMATIC`: the machine resumed from sleep.
    Resumed,
}

impl SystemEvent {
    const ALL: [Self; 5] =
        [Self::ThemeChanged, Self::ParametersChanged, Self::DisplayChanged, Self::SessionEnding, Self::Resumed];

    fn bit(self) -> u8 {
        1 << Self::ALL.iter().position(|e| *e == self).unwrap_or(0)
    }
}

struct State {
    window: RefCell<Option<MessageWindow>>,
    handler: Box<dyn Fn(SystemEvent)>,
    /// Events posted but not delivered yet (one bit per kind), so bursts coalesce.
    pending: Cell<u8>,
}

pub struct SystemEvents {
    state: Rc<State>,
}

impl SystemEvents {
    /// Creates the message window on the UI thread; `handler` runs asynchronously on the UI thread.
    ///
    /// Exception: [`SystemEvent::SessionEnding`] is delivered synchronously from `WM_ENDSESSION`,
    /// because Windows may end the process as soon as that message returns.
    pub fn new(handler: impl Fn(SystemEvent) + 'static) -> windows_core::Result<Self> {
        let state = Rc::new(State { window: RefCell::new(None), handler: Box::new(handler), pending: Cell::new(0) });
        let weak = Rc::downgrade(&state);
        // Broadcasts (WM_SETTINGCHANGE, WM_DISPLAYCHANGE, WM_ENDSESSION, WM_POWERBROADCAST) reach
        // top-level windows only, not HWND_MESSAGE windows.
        let window = MessageWindow::new(
            "TrayAppDotNETSlint.SystemEvents",
            false,
            Rc::new(move |_, message, wparam, lparam| on_message(&weak, message, wparam, lparam)),
        )?;
        *state.window.borrow_mut() = Some(window);
        Ok(Self { state })
    }
}

impl Drop for SystemEvents {
    fn drop(&mut self) {
        let window = self.state.window.borrow_mut().take();
        drop(window);
    }
}

fn on_message(state: &Weak<State>, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let state = state.upgrade()?;
    match message {
        WM_SETTINGCHANGE => {
            if let Some(event) = classify_setting_change(wparam.0 as u32, setting_name(lparam).as_deref()) {
                deliver(&state, event);
            }
            None
        }
        WM_DISPLAYCHANGE => {
            deliver(&state, SystemEvent::DisplayChanged);
            None
        }
        WM_POWERBROADCAST => {
            if wparam.0 as u32 == PBT_APMRESUMEAUTOMATIC {
                deliver(&state, SystemEvent::Resumed);
            }
            None
        }
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                log::info!("SystemEvents: the Windows session is ending");
                (state.handler)(SystemEvent::SessionEnding);
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// `WM_SETTINGCHANGE`: "ImmersiveColorSet" is the apps theme/accent; `SPI_SETWORKAREA` moves the
/// work area; any other `SPI_*` code is a system parameter change.
pub fn classify_setting_change(spi: u32, name: Option<&str>) -> Option<SystemEvent> {
    if name == Some("ImmersiveColorSet") {
        Some(SystemEvent::ThemeChanged)
    } else if spi == SPI_SETWORKAREA.0 {
        Some(SystemEvent::DisplayChanged)
    } else if spi != 0 {
        Some(SystemEvent::ParametersChanged)
    } else {
        None
    }
}

fn setting_name(lparam: LPARAM) -> Option<String> {
    if lparam.0 == 0 {
        return None;
    }
    // SAFETY: for WM_SETTINGCHANGE a non-zero lParam points to a NUL-terminated UTF-16 string
    // valid for the duration of the message.
    unsafe { windows_core::PCWSTR(lparam.0 as *const u16).to_string() }.ok()
}

fn deliver(state: &Rc<State>, event: SystemEvent) {
    let bit = event.bit();
    if state.pending.get() & bit != 0 {
        return;
    }
    state.pending.set(state.pending.get() | bit);
    let weak = Rc::downgrade(state);
    post(move || {
        if let Some(state) = weak.upgrade() {
            state.pending.set(state.pending.get() & !bit);
            (state.handler)(event);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_changes_are_classified() {
        assert_eq!(classify_setting_change(0, Some("ImmersiveColorSet")), Some(SystemEvent::ThemeChanged));
        assert_eq!(classify_setting_change(SPI_SETWORKAREA.0, None), Some(SystemEvent::DisplayChanged));
        // SPI_SETCLIENTAREAANIMATION
        assert_eq!(classify_setting_change(0x1043, None), Some(SystemEvent::ParametersChanged));
        assert_eq!(classify_setting_change(0, Some("Environment")), None);
    }
}
