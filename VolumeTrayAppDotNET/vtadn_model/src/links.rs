//! Windows Sound and Bluetooth surfaces (port of `Audio/DeviceShellLinks.cs`). Failures are
//! logged, never surfaced.

use tadn_common::shell;

use crate::settings::SoundSettingsTarget;

const URI_DEVICE_PROPERTIES_PREFIX: &str = "ms-settings:sound-properties?endpointId=";
const URI_MODERN_SOUND_SETTINGS: &str = "ms-settings:sound";
const URI_BLUETOOTH_QUICK_SETTINGS: &str = "ms-actioncenter:controlcenter/bluetooth";

/// Windows Settings page of one endpoint (format, enhancements, spatial sound, ...).
pub fn open_device_properties(device_id: &str) {
    let uri = format!("{URI_DEVICE_PROPERTIES_PREFIX}{}", escape_data_string(device_id));
    log_failure("open_device_properties", shell::open_uri(&uri));
}

pub fn open_playback_tab() {
    open_sound_panel("playback");
}

pub fn open_recording_tab() {
    open_sound_panel("recording");
}

pub fn open_sounds_tab() {
    open_sound_panel("sounds");
}

pub fn open_communications_tab() {
    open_sound_panel("communications");
}

pub fn open_modern_sound_settings() {
    log_failure("open_modern_sound_settings", shell::open_uri(URI_MODERN_SOUND_SETTINGS));
}

/// The flyout's sound settings button: brings an already open target window forward instead of
/// opening a second one.
pub fn open_sound_settings(target: SoundSettingsTarget) {
    match target {
        SoundSettingsTarget::WindowsSettingsApp => {
            if !try_focus_modern_sound_settings() {
                open_modern_sound_settings();
            }
        }
        SoundSettingsTarget::LegacySoundPanel => {
            if !try_focus_legacy_sound_panel() {
                open_playback_tab();
            }
        }
    }
}

/// Bluetooth page of the Windows quick settings panel (`explorer.exe
/// ms-actioncenter:controlcenter/bluetooth`).
pub fn open_bluetooth_quick_settings() {
    log_failure(
        "open_bluetooth_quick_settings",
        shell::shell_execute("explorer.exe", Some(URI_BLUETOOTH_QUICK_SETTINGS)),
    );
}

fn open_sound_panel(tab: &str) {
    log_failure("open_sound_panel", shell::open_control_panel(&format!("mmsys.cpl,,{tab}")));
}

/// Focuses the main window of a running `SystemSettings.exe` (restoring it when minimized).
fn try_focus_modern_sound_settings() -> bool {
    focus::try_focus_first_process("SystemSettings.exe", None)
}

/// Focuses a `rundll32.exe` main window whose process has `mmsys.cpl` loaded.
fn try_focus_legacy_sound_panel() -> bool {
    focus::try_focus_first_process("rundll32.exe", Some("mmsys.cpl"))
}

/// `Process.GetProcessesByName` + `Process.MainWindowHandle` + `Process.Modules`, over ToolHelp
/// snapshots and `EnumWindows`. Every failure counts as "no match", like the C# best-effort code.
mod focus {
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, PROCESSENTRY32W, Process32FirstW,
        Process32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE,
        SetForegroundWindow, ShowWindowAsync,
    };
    use windows_core::BOOL;

    struct Snapshot(HANDLE);

    impl Drop for Snapshot {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateToolhelp32Snapshot and is closed exactly once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn wide_name(buffer: &[u16]) -> String {
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..len])
    }

    /// Process ids whose image name equals `exe_name` (case-insensitive), in snapshot order.
    fn processes_named(exe_name: &str) -> Vec<u32> {
        let mut pids = Vec::new();
        // SAFETY: plain ToolHelp calls; the entry struct is sized as the API requires.
        unsafe {
            let Ok(handle) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return pids };
            let snapshot = Snapshot(handle);
            let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut ok = Process32FirstW(snapshot.0, &mut entry).is_ok();
            while ok {
                if wide_name(&entry.szExeFile).eq_ignore_ascii_case(exe_name) {
                    pids.push(entry.th32ProcessID);
                }
                ok = Process32NextW(snapshot.0, &mut entry).is_ok();
            }
        }
        pids
    }

    fn process_hosts_module(pid: u32, module: &str) -> bool {
        // SAFETY: plain ToolHelp calls; the entry struct is sized as the API requires.
        unsafe {
            let Ok(handle) = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) else {
                return false;
            };
            let snapshot = Snapshot(handle);
            let mut entry = MODULEENTRY32W { dwSize: size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
            let mut ok = Module32FirstW(snapshot.0, &mut entry).is_ok();
            while ok {
                if wide_name(&entry.szModule).eq_ignore_ascii_case(module) {
                    return true;
                }
                ok = Module32NextW(snapshot.0, &mut entry).is_ok();
            }
        }
        false
    }

    struct MainWindowSearch {
        pid: u32,
        found: HWND,
    }

    unsafe extern "system" fn enum_main_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the &mut MainWindowSearch passed to EnumWindows below, alive for the call.
        let search = unsafe { &mut *(lparam.0 as *mut MainWindowSearch) };
        let mut pid = 0u32;
        // SAFETY: valid out pointer; hwnd comes from EnumWindows.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        // .NET `MainWindowHandle`: the first visible, unowned top-level window of the process.
        // SAFETY: hwnd comes from EnumWindows.
        let is_main = unsafe { IsWindowVisible(hwnd).as_bool() && GetWindow(hwnd, GW_OWNER).is_err() };
        if pid == search.pid && is_main {
            search.found = hwnd;
            return BOOL(0);
        }
        BOOL(1)
    }

    fn main_window(pid: u32) -> Option<HWND> {
        let mut search = MainWindowSearch { pid, found: HWND::default() };
        // SAFETY: the callback only touches `search`, which outlives the call. EnumWindows reports
        // an error when the callback stops early; the result is in `search`.
        unsafe {
            let _ = EnumWindows(Some(enum_main_window), LPARAM(&raw mut search as isize));
        }
        (!search.found.is_invalid()).then_some(search.found)
    }

    /// Un-minimize (posted, so a hung target cannot stall the UI thread), then activate.
    fn focus_window(hwnd: HWND) -> bool {
        // SAFETY: plain user32 calls on a window handle (stale handles just fail).
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindowAsync(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd).as_bool()
        }
    }

    pub(super) fn try_focus_first_process(exe_name: &str, required_module: Option<&str>) -> bool {
        for pid in processes_named(exe_name) {
            let Some(hwnd) = main_window(pid) else { continue };
            if required_module.is_some_and(|module| !process_hosts_module(pid, module)) {
                continue;
            }
            if focus_window(hwnd) {
                return true;
            }
        }
        false
    }
}

/// `Uri.EscapeDataString`: percent-encode everything except RFC 3986 unreserved characters.
fn escape_data_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn log_failure(what: &str, result: windows_core::Result<()>) {
    if let Err(error) = result {
        log::warn!("links::{what}: {error}");
    }
}
