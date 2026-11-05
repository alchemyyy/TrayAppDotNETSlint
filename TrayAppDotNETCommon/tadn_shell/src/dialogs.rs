//! Native message boxes for startup failures and fatal errors (the common-controls-6 themed
//! `MessageBoxW`; no Slint involvement so they work before the event loop starts).

use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW};
use windows_core::PCWSTR;

use crate::util::wide;

/// Modal error box (`MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND`); returns when dismissed.
pub fn error(title: &str, text: &str) {
    let title = wide(title);
    let text = wide(text);
    // SAFETY: both strings are NUL-terminated and outlive the call; no owner window.
    let result = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    if result.0 == 0 {
        log::error!("MessageBoxW failed: {}", windows_core::Error::from_thread());
    }
}
