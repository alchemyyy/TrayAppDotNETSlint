//! Hidden windows with their own class and window procedure (port of `Interop/Win32Window.cs`),
//! for messages the Slint windows do not receive: tray callbacks, hotkeys, broadcasts.
//!
//! The procedure only forwards to a Rust closure. The closures in this crate never call app code
//! synchronously; they update `Cell` state and post work to the event loop, so a message sent to
//! the window re-entrantly (for example while `Shell_NotifyIconW` waits on Explorer) is harmless.

use std::panic::AssertUnwindSafe;
use std::rc::Rc;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, HWND_MESSAGE, RegisterClassExW,
    SetWindowLongPtrW, UnregisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSEXW,
};
use windows_core::PCWSTR;

use crate::util::{guid_n, wide};

/// Handles one message; `None` falls through to `DefWindowProcW`.
pub(crate) type WindowProc = dyn Fn(HWND, u32, WPARAM, LPARAM) -> Option<LRESULT>;

pub(crate) struct MessageWindow {
    hwnd: HWND,
    class_name: Vec<u16>,
    hinstance: HINSTANCE,
    procedure: *mut Rc<WindowProc>,
}

impl MessageWindow {
    /// Registers `<class_prefix>.<random>` and creates a size-0, style-0 window: message-only
    /// (`HWND_MESSAGE`) or a hidden top-level window (which also receives broadcasts such as
    /// `TaskbarCreated` and `WM_SETTINGCHANGE`).
    pub(crate) fn new(class_prefix: &str, message_only: bool, procedure: Rc<WindowProc>) -> windows_core::Result<Self> {
        // SAFETY: querying the module handle of the running exe has no preconditions.
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None) }?.into();
        let class_name = wide(&format!("{class_prefix}.{}", guid_n()));
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_procedure),
            hInstance: hinstance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: `class` and the class name it points to outlive the call.
        if unsafe { RegisterClassExW(&class) } == 0 {
            return Err(windows_core::Error::from_thread());
        }

        let parent = message_only.then_some(HWND_MESSAGE);
        // SAFETY: the class was registered above with this module's instance.
        let created = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                parent,
                None,
                Some(hinstance),
                None,
            )
        };
        let hwnd = match created {
            Ok(hwnd) => hwnd,
            Err(error) => {
                // SAFETY: unregisters the class registered above; no window uses it.
                let _ = unsafe { UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(hinstance)) };
                return Err(error);
            }
        };

        let procedure = Box::into_raw(Box::new(procedure));
        // SAFETY: `hwnd` belongs to this thread; the pointer stays valid until Drop clears it.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, procedure as isize) };
        Ok(Self { hwnd, class_name, hinstance, procedure })
    }

    pub(crate) fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for MessageWindow {
    fn drop(&mut self) {
        // SAFETY: the window and class were created by `new` on this thread. No message reaches the
        // closure once the user data is cleared (DestroyWindow still sends WM_DESTROY, which falls
        // through to DefWindowProcW). The boxed Rc is freed exactly once here; a procedure call in
        // progress further up the stack holds its own Rc clone.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            if let Err(error) = DestroyWindow(self.hwnd) {
                log::warn!("MessageWindow: DestroyWindow failed: {error}");
            }
            drop(Box::from_raw(self.procedure));
            let _ = UnregisterClassW(PCWSTR(self.class_name.as_ptr()), Some(self.hinstance));
        }
    }
}

extern "system" fn window_procedure(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: the user data is either 0 or the `Box<Rc<WindowProc>>` pointer set by `new`, which
    // stays valid until Drop clears it first.
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Rc<WindowProc>;
    if !pointer.is_null() {
        // SAFETY: see above; cloning keeps the closure alive for this call even if the window is
        // dropped re-entrantly.
        let procedure = unsafe { (*pointer).clone() };
        match std::panic::catch_unwind(AssertUnwindSafe(|| procedure(hwnd, message, wparam, lparam))) {
            Ok(Some(result)) => return result,
            Ok(None) => {}
            Err(_) => log::error!("MessageWindow: window procedure panicked on message 0x{message:04X}"),
        }
    }
    // SAFETY: default processing for a window of this thread.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
