//! Tray-click toggling of a window (port of `UI/Tray/TaskbarForegroundTracker.cs` and
//! `UI/Tray/TrayWindowToggle.cs`).
//!
//! A tray click first makes the taskbar the foreground window, which deactivates (and may hide)
//! the flyout before the click arrives. The tracker remembers the last non-taskbar foreground
//! window and when the taskbar was activated so the click can tell "was showing, hide it" from
//! "show it".

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_FOREGROUND, GW_OWNER, GetClassNameW, GetForegroundWindow, GetWindow, WINEVENT_OUTOFCONTEXT,
};

use crate::window::double_click_time_ms;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayToggleAction {
    /// Nothing: the window was foreground and this click's taskbar activation already hid it.
    None,
    Show,
    Hide,
    /// Visible but not foreground: bring it forward in place.
    Activate,
}

/// The primary and secondary taskbars, and the notification overflow flyouts of Windows 11 and
/// Windows 10.
const TASKBAR_WINDOW_CLASS_NAMES: [&str; 4] =
    ["Shell_TrayWnd", "Shell_SecondaryTrayWnd", "TopLevelWindowForOverflowXamlIsland", "NotifyIconOverflowWindow"];
/// `ClassNameCapacity`.
const CLASS_NAME_CAPACITY: usize = 256;

/// `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` on the UI thread. Taskbar windows are
/// `Shell_TrayWnd`, `Shell_SecondaryTrayWnd`, `TopLevelWindowForOverflowXamlIsland`,
/// `NotifyIconOverflowWindow`.
pub struct TaskbarForegroundTracker {
    state: Rc<TrackerState>,
}

struct TrackerState {
    last_window_outside_taskbar: Cell<HWND>,
    /// `GetTickCount` time of the last move of the foreground to a taskbar window.
    taskbar_activation_time: Cell<Option<u32>>,
}

/// One out-of-context hook per thread, shared by the thread's trackers; its callback runs from
/// the thread's message loop.
#[derive(Default)]
struct HookRegistry {
    hook: Option<HWINEVENTHOOK>,
    trackers: Vec<Weak<TrackerState>>,
}

thread_local! {
    static REGISTRY: RefCell<HookRegistry> = RefCell::new(HookRegistry::default());
}

impl TaskbarForegroundTracker {
    pub fn install() -> Self {
        let state = Rc::new(TrackerState {
            last_window_outside_taskbar: Cell::new(HWND::default()),
            taskbar_activation_time: Cell::new(None),
        });
        // SAFETY: no preconditions.
        let foreground = unsafe { GetForegroundWindow() };
        if !is_taskbar_window(foreground) {
            state.record_window_outside_taskbar(foreground);
        }

        REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            registry.trackers.retain(|tracker| tracker.strong_count() > 0);
            registry.trackers.push(Rc::downgrade(&state));
            if registry.hook.is_none() {
                // SAFETY: out-of-context hook with a 'static callback, removed when the last
                // tracker of this thread drops.
                let hook = unsafe {
                    SetWinEventHook(
                        EVENT_SYSTEM_FOREGROUND,
                        EVENT_SYSTEM_FOREGROUND,
                        None,
                        Some(on_win_event),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT,
                    )
                };
                if hook.is_invalid() {
                    log::warn!(
                        "TaskbarForegroundTracker: SetWinEventHook failed; only the current foreground is known"
                    );
                } else {
                    registry.hook = Some(hook);
                }
            }
        });
        Self { state }
    }

    /// `TrayWindowToggle.Decide`, with `taskbarActivatedByThisClick` = taskbar activated within
    /// `GetDoubleClickTime()`. `Hide`/`None` forget the window.
    pub fn decide(&self, hwnd: Option<HWND>, is_visible: bool, is_minimized: bool) -> TrayToggleAction {
        let was_foreground = hwnd.is_some_and(|hwnd| self.was_foreground_before_taskbar(hwnd));
        let action = decide(
            is_visible,
            is_minimized,
            was_foreground,
            is_press_of_this_click(self.milliseconds_since_taskbar_activation(), double_click_time_ms()),
        );
        // The window is closed either way now, so the next click starts from "not foreground".
        if matches!(action, TrayToggleAction::Hide | TrayToggleAction::None)
            && let Some(hwnd) = hwnd
        {
            self.forget(hwnd);
        }
        action
    }

    /// Whether the window, or a window it owns, held the foreground before any taskbar
    /// interaction still in progress, such as the click that raised a notification icon callback.
    pub fn was_foreground_before_taskbar(&self, hwnd: HWND) -> bool {
        if hwnd.is_invalid() {
            return false;
        }
        // SAFETY: no preconditions.
        let foreground = unsafe { GetForegroundWindow() };
        let candidate =
            if is_taskbar_window(foreground) { self.state.last_window_outside_taskbar.get() } else { foreground };
        is_owned_by_or_same(candidate, hwnd)
    }

    /// Forgets that the window held the foreground. A window that hid while the taskbar was
    /// active hands the foreground to nothing, so without this the next click would still find
    /// it there.
    pub fn forget(&self, hwnd: HWND) {
        let last = self.state.last_window_outside_taskbar.get();
        if !hwnd.is_invalid() && is_owned_by_or_same(last, hwnd) {
            self.state.last_window_outside_taskbar.set(HWND::default());
        }
    }

    /// Milliseconds since the foreground last moved to a taskbar window, or None when it has not
    /// since this tracker started.
    pub fn milliseconds_since_taskbar_activation(&self) -> Option<u32> {
        // SAFETY: no preconditions; GetTickCount and the WinEvent time share one clock.
        let now = unsafe { GetTickCount() };
        self.state.taskbar_activation_time.get().map(|time| now.wrapping_sub(time))
    }
}

impl Drop for TaskbarForegroundTracker {
    fn drop(&mut self) {
        let state = Rc::downgrade(&self.state);
        REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            registry.trackers.retain(|tracker| !tracker.ptr_eq(&state) && tracker.strong_count() > 0);
            if registry.trackers.is_empty()
                && let Some(hook) = registry.hook.take()
            {
                // SAFETY: the hook was installed on this thread by `install`.
                let _ = unsafe { UnhookWinEvent(hook) };
            }
        });
    }
}

impl TrackerState {
    fn record_window_outside_taskbar(&self, hwnd: HWND) {
        if !hwnd.is_invalid() {
            self.last_window_outside_taskbar.set(hwnd);
        }
    }
}

/// The C# compares `GetAncestor(candidate, GA_ROOTOWNER)` with the window. Flyout and menu
/// windows here are owned by a hidden owner window (that is what keeps them out of the taskbar
/// and Alt+Tab), so the root owner is that hidden window; walking the owner chain instead keeps
/// "the window, or a window it owns".
fn is_owned_by_or_same(candidate: HWND, hwnd: HWND) -> bool {
    let mut current = candidate;
    while !current.is_invalid() {
        if current == hwnd {
            return true;
        }
        // SAFETY: user32 validates the handle; returns an error at the top of the chain.
        current = unsafe { GetWindow(current, GW_OWNER) }.unwrap_or_default();
    }
    false
}

/// Runs inside the WinEvent dispatch of the UI thread's message loop.
extern "system" fn on_win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _object_id: i32,
    _child_id: i32,
    _event_thread: u32,
    event_time: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND || hwnd.is_invalid() {
        return;
    }
    let taskbar = is_taskbar_window(hwnd);
    REGISTRY.with(|registry| {
        let Ok(registry) = registry.try_borrow() else { return };
        for tracker in registry.trackers.iter().filter_map(Weak::upgrade) {
            if taskbar {
                tracker.taskbar_activation_time.set(Some(event_time));
            } else {
                tracker.record_window_outside_taskbar(hwnd);
            }
        }
    });
}

fn is_taskbar_window(hwnd: HWND) -> bool {
    if hwnd.is_invalid() {
        return false;
    }
    let mut buffer = [0u16; CLASS_NAME_CAPACITY];
    // SAFETY: the buffer holds CLASS_NAME_CAPACITY units.
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    len > 0 && is_taskbar_window_class_name(&String::from_utf16_lossy(&buffer[..len as usize]))
}

/// `TaskbarForegroundTracker.IsTaskbarWindowClassName` (ordinal comparison).
pub fn is_taskbar_window_class_name(class_name: &str) -> bool {
    TASKBAR_WINDOW_CLASS_NAMES.contains(&class_name)
}

/// `TrayWindowToggle.IsPressOfThisClick`: the taskbar took the foreground recently enough to be
/// the press of the click being handled.
pub fn is_press_of_this_click(milliseconds_since_taskbar_activation: Option<u32>, double_click_time: u32) -> bool {
    milliseconds_since_taskbar_activation.is_some_and(|elapsed| elapsed <= double_click_time)
}

/// `TrayWindowToggle.Decide`.
pub fn decide(
    is_visible: bool,
    is_minimized: bool,
    was_foreground_before_taskbar: bool,
    taskbar_activated_by_this_click: bool,
) -> TrayToggleAction {
    if !is_visible {
        return if was_foreground_before_taskbar && taskbar_activated_by_this_click {
            TrayToggleAction::None
        } else {
            TrayToggleAction::Show
        };
    }
    if is_minimized {
        return TrayToggleAction::Show;
    }
    if was_foreground_before_taskbar { TrayToggleAction::Hide } else { TrayToggleAction::Activate }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_click_toggles_from_the_foreground_before_the_taskbar() {
        use TrayToggleAction::*;
        let cases = [
            // The user was working in the window: the click closes it.
            (true, false, true, true, Hide),
            (true, false, true, false, Hide),
            // Visible behind another window: it comes forward where it is.
            (true, false, false, true, Activate),
            (true, false, false, false, Activate),
            (true, true, true, true, Show),
            (true, true, false, false, Show),
            // This click's own press hid it: the click must not reopen it.
            (false, false, true, true, None),
            // An earlier taskbar click hid it, or something else had the foreground: it opens.
            (false, false, true, false, Show),
            (false, false, false, true, Show),
            (false, false, false, false, Show),
        ];
        for (visible, minimized, was_foreground, this_click, expected) in cases {
            assert_eq!(
                decide(visible, minimized, was_foreground, this_click),
                expected,
                "visible={visible} minimized={minimized} was_foreground={was_foreground} this_click={this_click}"
            );
        }
    }

    #[test]
    fn only_a_recent_taskbar_activation_is_the_press_of_this_click() {
        let cases = [
            (0, 500, true),
            (250, 500, true),
            (500, 500, true),
            (501, 500, false),
            (60_000, 500, false),
            (600, 900, true),
        ];
        for (elapsed, double_click_time, expected) in cases {
            assert_eq!(
                is_press_of_this_click(Some(elapsed), double_click_time),
                expected,
                "{elapsed} vs {double_click_time}"
            );
        }
        assert!(!is_press_of_this_click(None, 500));
    }

    #[test]
    fn taskbar_and_overflow_windows_count_as_taskbar() {
        for class_name in TASKBAR_WINDOW_CLASS_NAMES {
            assert!(is_taskbar_window_class_name(class_name));
        }
        for class_name in ["", "Progman", "CabinetWClass", "SystemTray_Main", "shell_traywnd"] {
            assert!(!is_taskbar_window_class_name(class_name));
        }
    }

    #[test]
    #[ignore = "installs a WinEvent hook; run with --ignored"]
    fn no_window_was_never_foreground() {
        let tracker = TaskbarForegroundTracker::install();
        assert!(!tracker.was_foreground_before_taskbar(HWND::default()));
        assert_eq!(tracker.decide(None, false, false), TrayToggleAction::Show);
    }
}
