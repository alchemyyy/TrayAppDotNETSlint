//! Native helpers for Slint windows: HWND access, DWM attributes, activation, focus tracking and
//! physical positioning (port of `Interop/ForegroundWindowFunctions.cs` plus the DWM calls of the
//! C# window helpers).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
    DwmSetWindowAttribute,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, INPUT, INPUT_0, INPUT_MOUSE, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, SetForegroundWindow,
};

/// The window's HWND once its winit window exists (after the first `show()` has been processed by
/// the event loop); `None` before that.
pub fn hwnd(window: &slint::Window) -> Option<HWND> {
    hwnd_of(&window.window_handle())
}

fn hwnd_of(handle: &slint::WindowHandle) -> Option<HWND> {
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(win32) => Some(HWND(win32.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

type CreatedCallback = Box<dyn FnOnce(HWND)>;

thread_local! {
    /// Callbacks waiting for their window to be created, keyed by the id their waker carries.
    static PENDING_CREATION: RefCell<HashMap<usize, (slint::WindowHandle, CreatedCallback)>> =
        RefCell::new(HashMap::new());
    static NEXT_PENDING_ID: Cell<usize> = const { Cell::new(1) };
}

/// Calls `f` with the HWND as soon as the winit window exists (immediately if it already does).
/// Runs on the UI thread: the winit adapter's window-existence future
/// (`WinitWindowAccessor::winit_window()`) is polled once to register a waker, and the wake posts
/// `f` to the event loop once the window is created. Call it around the window's first `show()`;
/// until the window exists the callback keeps the window adapter alive.
pub fn when_created(window: &slint::Window, f: impl FnOnce(HWND) + 'static) {
    if let Some(hwnd) = hwnd(window) {
        f(hwnd);
        return;
    }

    let id = NEXT_PENDING_ID.with(|next| next.replace(next.get().wrapping_add(1)));
    PENDING_CREATION.with(|pending| pending.borrow_mut().insert(id, (window.window_handle(), Box::new(f))));

    let waker = creation_waker(id);
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(window.winit_window());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(Ok(_)) => run_pending_creation(id),
        Poll::Ready(Err(error)) => {
            log::warn!("window::when_created: no winit window: {error}");
            PENDING_CREATION.with(|pending| pending.borrow_mut().remove(&id));
        }
        // The adapter keeps the waker and wakes it when it creates the window.
        Poll::Pending => {}
    }
}

fn run_pending_creation(id: usize) {
    let Some((handle, callback)) = PENDING_CREATION.with(|pending| pending.borrow_mut().remove(&id)) else {
        return;
    };
    match hwnd_of(&handle) {
        Some(hwnd) => callback(hwnd),
        None => log::warn!("window::when_created: the window was created but reports no Win32 handle"),
    }
}

fn creation_waker(id: usize) -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone_waker, wake_waker, wake_waker_by_ref, drop_waker);

    fn clone_waker(data: *const ()) -> RawWaker {
        RawWaker::new(data, &VTABLE)
    }
    fn wake_waker(data: *const ()) {
        wake_waker_by_ref(data);
    }
    fn wake_waker_by_ref(data: *const ()) {
        let id = data as usize;
        // The wake happens inside window creation; run the callback after it returns.
        if let Err(error) = slint::invoke_from_event_loop(move || run_pending_creation(id)) {
            log::warn!("window::when_created: could not post the creation callback: {error}");
        }
    }
    fn drop_waker(_: *const ()) {}

    // SAFETY: the data is a plain integer id; every vtable function treats it as such and never
    // dereferences it, so the waker is trivially Send + Sync.
    unsafe { Waker::from_raw(RawWaker::new(id as *const (), &VTABLE)) }
}

/// `DWMWA_WINDOW_CORNER_PREFERENCE`: rounded (`DWMWCP_ROUND`) or square (`DWMWCP_DONOTROUND`).
/// No-op on Windows 10.
pub fn set_rounded_corners(hwnd: HWND, rounded: bool) {
    let preference = if rounded { DWMWCP_ROUND } else { DWMWCP_DONOTROUND };
    // SAFETY: passes a pointer to a local i32-sized enum with its exact size.
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const c_void,
            size_of_val(&preference) as u32,
        )
    };
    // Windows 10 does not know the attribute; that is the documented no-op.
    if let Err(error) = result {
        log::debug!("DWMWA_WINDOW_CORNER_PREFERENCE not applied: {error}");
    }
}

/// `DWMWA_USE_IMMERSIVE_DARK_MODE` for the title bar of `Normal` windows.
pub fn set_dark_title_bar(hwnd: HWND, dark: bool) {
    let value = windows_core::BOOL::from(dark);
    // SAFETY: passes a pointer to a local BOOL with its exact size.
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &value as *const _ as *const c_void,
            size_of_val(&value) as u32,
        )
    };
    if let Err(error) = result {
        log::debug!("DWMWA_USE_IMMERSIVE_DARK_MODE not applied: {error}");
    }
}

/// Brings the window to the foreground reliably from a tray click (AllowSetForegroundWindow /
/// AttachThreadInput dance as the C# `ForegroundWindowFunctions` do).
pub fn activate(hwnd: HWND) {
    if !force_foreground(hwnd) {
        log::debug!("window::activate: Windows kept the foreground elsewhere");
    }
}

/// `ForegroundWindowFunctions.ForceForeground`. Windows only lets the foreground process, or the
/// process that provided the last input, take the foreground; an empty injected mouse input makes
/// this process the last input provider without moving the cursor. If Windows still refuses, the
/// switch runs while this thread shares the foreground thread's input state. Returns true when the
/// window ends up as the foreground window.
pub fn force_foreground(hwnd: HWND) -> bool {
    if hwnd.is_invalid() {
        return false;
    }
    if is_foreground(hwnd) {
        return true;
    }

    let empty_mouse_input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT::default() } };
    // SAFETY: one fully initialized INPUT with its exact size; the window handle is only passed to
    // user32, which validates it.
    unsafe {
        SendInput(&[empty_mouse_input], size_of::<INPUT>() as i32);
        if SetForegroundWindow(hwnd).as_bool() && GetForegroundWindow() == hwnd {
            return true;
        }

        let foreground_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let current_thread = GetCurrentThreadId();
        if foreground_thread == 0
            || foreground_thread == current_thread
            || !AttachThreadInput(current_thread, foreground_thread, true).as_bool()
        {
            return false;
        }

        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = AttachThreadInput(current_thread, foreground_thread, false);
        GetForegroundWindow() == hwnd
    }
}

pub fn is_foreground(hwnd: HWND) -> bool {
    // SAFETY: no preconditions.
    !hwnd.is_invalid() && unsafe { GetForegroundWindow() } == hwnd
}

pub fn is_minimized(hwnd: HWND) -> bool {
    // SAFETY: user32 validates the handle.
    unsafe { IsIconic(hwnd) }.as_bool()
}

/// Registers `f(focused)` for winit `Focused` events of this window
/// (`WinitWindowAccessor::on_winit_window_event`). Events still propagate to Slint. This installs
/// the window's one winit event filter, replacing any earlier filter of that window.
pub fn on_focus_changed(window: &slint::Window, mut f: impl FnMut(bool) + 'static) {
    window.on_winit_window_event(move |_, event| {
        if let winit::event::WindowEvent::Focused(focused) = event {
            f(*focused);
        }
        EventResult::Propagate
    });
}

/// Current cursor position in physical screen pixels.
pub fn cursor_pos() -> (i32, i32) {
    let mut point = POINT::default();
    // SAFETY: out-pointer to a local.
    if let Err(error) = unsafe { GetCursorPos(&mut point) } {
        log::debug!("GetCursorPos failed: {error}");
    }
    (point.x, point.y)
}

/// System double-click time in milliseconds.
pub fn double_click_time_ms() -> u32 {
    // SAFETY: no preconditions.
    unsafe { GetDoubleClickTime() }
}
