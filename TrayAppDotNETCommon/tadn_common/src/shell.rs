//! `ShellExecuteW` launching: URIs (`ms-settings:`), control panel applets and folders.

use std::path::Path;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows_core::{HSTRING, PCWSTR, w};

/// Runs `ShellExecuteW(open, file, args)`. Returns an error when the shell reports failure
/// (`HINSTANCE <= 32`).
pub fn shell_execute(file: &str, args: Option<&str>) -> windows_core::Result<()> {
    let file = HSTRING::from(file);
    let args = args.map(HSTRING::from);
    let args_ptr = args.as_ref().map_or(PCWSTR::null(), |a| PCWSTR(a.as_ptr()));
    // SAFETY: every pointer is a live, NUL-terminated HSTRING for the duration of the call.
    let result =
        unsafe { ShellExecuteW(Some(HWND::default()), w!("open"), &file, args_ptr, PCWSTR::null(), SW_SHOWNORMAL) };
    let code = result.0 as isize;
    if code <= 32 {
        return Err(windows_core::Error::new(
            windows::Win32::Foundation::E_FAIL,
            format!("ShellExecuteW({file}) failed with code {code}"),
        ));
    }
    Ok(())
}

/// Opens a URI with its registered handler, e.g. `ms-settings:sound`.
pub fn open_uri(uri: &str) -> windows_core::Result<()> {
    shell_execute(uri, None)
}

/// Opens a folder in Explorer.
pub fn open_folder(path: &Path) -> windows_core::Result<()> {
    shell_execute(&path.to_string_lossy(), None)
}

/// Opens a control panel applet page through `rundll32.exe shell32.dll,Control_RunDLL <args>`.
pub fn open_control_panel(applet_args: &str) -> windows_core::Result<()> {
    shell_execute("rundll32.exe", Some(&format!("shell32.dll,Control_RunDLL {applet_args}")))
}
