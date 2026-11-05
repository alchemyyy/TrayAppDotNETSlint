//! Small crate-private helpers shared by the shell modules.

use std::time::Duration;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::SystemInformation::GetTickCount64;

/// NUL-terminated UTF-16 copy of `text`.
pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 buffer up to (not including) its first NUL.
pub(crate) fn from_wide(buffer: &[u16]) -> String {
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..len])
}

/// Runs `f` on the UI thread from the event loop, after the current call stack (in particular any
/// window procedure) has returned.
pub(crate) fn post(f: impl FnOnce() + 'static) {
    slint::Timer::single_shot(Duration::ZERO, f);
}

/// Runs `f` on the UI thread after `delay`.
pub(crate) fn post_delayed(delay: Duration, f: impl FnOnce() + 'static) {
    slint::Timer::single_shot(delay, f);
}

/// Milliseconds since boot (`GetTickCount64`), the clock the C# code reads as
/// `Environment.TickCount64`.
pub(crate) fn tick_ms() -> u64 {
    // SAFETY: no preconditions.
    unsafe { GetTickCount64() }
}

/// A fresh random GUID as 32 lowercase hex digits (`Guid.NewGuid().ToString("N")`).
pub(crate) fn guid_n() -> String {
    match windows_core::GUID::new() {
        Ok(guid) => format!(
            "{:08x}{:04x}{:04x}{}",
            guid.data1,
            guid.data2,
            guid.data3,
            guid.data4.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
        Err(error) => {
            log::warn!("CoCreateGuid failed ({error}); using the tick count for a window class suffix");
            format!("{:032x}", tick_ms())
        }
    }
}

/// Initializes COM (STA) on the current thread for the scope when it is not initialized yet. A
/// thread that already joined the MTA keeps it (`RPC_E_CHANGED_MODE`); COM is usable either way.
pub(crate) struct ComScope {
    initialized: bool,
}

impl ComScope {
    pub(crate) fn enter() -> Self {
        // SAFETY: balanced with CoUninitialize in Drop only when this call succeeded.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        Self { initialized: hr.is_ok() }
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: pairs with the successful CoInitializeEx in `enter` on this thread.
            unsafe { CoUninitialize() };
        }
    }
}
