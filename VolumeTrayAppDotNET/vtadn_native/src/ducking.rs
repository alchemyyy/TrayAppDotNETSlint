//! Communications ducking (`HKCU\Software\Microsoft\Multimedia\Audio\UserDuckingPreference`;
//! MuteAll 0, Reduce80 1, Reduce50 2, DoNothing 3; missing = Reduce80). Port of
//! `Audio/CommunicationsDucking.cs`.
//!
//! There is no documented Win32/COM API for this setting: mmsys.cpl writes the registry value
//! directly and the audio service reads it.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use vtadn_model::snapshot::DuckingMode;
use windows::Win32::Foundation::{ERROR_SUCCESS, WAIT_FAILED, WAIT_OBJECT_0};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, REG_NOTIFY_CHANGE_LAST_SET,
    REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegNotifyChangeKeyValue, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::System::Threading::{INFINITE, ResetEvent, WaitForMultipleObjects};
use windows_core::PCWSTR;

use crate::util::{OwnedEvent, wide};

const KEY_PATH: &str = r"Software\Microsoft\Multimedia\Audio";
const VALUE_NAME: &str = "UserDuckingPreference";

/// `StopJoinTimeoutMs`: how long dropping the watcher waits for its thread.
const STOP_JOIN_TIMEOUT: Duration = Duration::from_millis(500);

/// Registry integer → mode. Values outside 0..=3 are not written by Windows; like the C#
/// `IsActive` (anything but 3 is active) they read as the active OS default.
pub fn mode_from_registry(value: Option<u32>) -> DuckingMode {
    match value {
        Some(0) => DuckingMode::MuteAll,
        Some(2) => DuckingMode::Reduce50,
        Some(3) => DuckingMode::DoNothing,
        _ => DuckingMode::Reduce80,
    }
}

/// Mode → the integer mmsys.cpl writes.
pub fn mode_to_registry(mode: DuckingMode) -> u32 {
    match mode {
        DuckingMode::MuteAll => 0,
        DuckingMode::Reduce80 => 1,
        DuckingMode::Reduce50 => 2,
        DuckingMode::DoNothing => 3,
    }
}

/// Pure registry read; a missing key/value or a non-DWORD value is the OS default (Reduce80).
pub fn read_mode() -> DuckingMode {
    mode_from_registry(read_dword())
}

fn read_dword() -> Option<u32> {
    let path = wide(KEY_PATH);
    let name = wide(VALUE_NAME);
    let mut key = HKEY::default();
    // SAFETY: valid NUL-terminated path; `key` receives an owned handle closed below.
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), None, KEY_QUERY_VALUE, &mut key) };
    if status != ERROR_SUCCESS {
        return None;
    }
    let mut kind = REG_VALUE_TYPE::default();
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the data buffer is a u32 whose size is passed in `size`.
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            Some(std::ptr::from_mut(&mut data).cast()),
            Some(&mut size),
        )
    };
    // SAFETY: closes the key opened above.
    let _ = unsafe { RegCloseKey(key) };
    (status == ERROR_SUCCESS && kind == REG_DWORD && size == size_of::<u32>() as u32).then_some(data)
}

/// Writes the preference (opens or creates the key, like `Registry.CreateSubKey`). A running
/// [`DuckingWatcher`] reports the change.
pub fn write_mode(mode: DuckingMode) -> windows_core::Result<()> {
    let path = wide(KEY_PATH);
    let name = wide(VALUE_NAME);
    let mut key = HKEY::default();
    // SAFETY: valid NUL-terminated path; `key` receives an owned handle closed below.
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            None,
            &mut key,
            None,
        )
    }
    .ok()?;
    let value = mode_to_registry(mode).to_le_bytes();
    // SAFETY: writes a 4-byte DWORD from a live buffer.
    let result = unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_DWORD, Some(&value)) }.ok();
    // SAFETY: closes the key opened above.
    let _ = unsafe { RegCloseKey(key) };
    result
}

/// Registry key handle shared between the watcher and its thread.
struct OwnedKey(HKEY);

// SAFETY: registry key handles are process-wide and usable from any thread.
unsafe impl Send for OwnedKey {}
// SAFETY: see `Send`; the only operation used after creation is `RegNotifyChangeKeyValue`.
unsafe impl Sync for OwnedKey {}

impl Drop for OwnedKey {
    fn drop(&mut self) {
        // SAFETY: the key was opened by `RegOpenKeyExW` and is closed exactly once here.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// Handles shared by the watcher and its thread; whichever releases the last reference closes
/// them (C#: a watcher that misses the join timeout keeps its handles).
struct WatchHandles {
    key: OwnedKey,
    reg_event: OwnedEvent,
    wake_event: OwnedEvent,
}

/// `RegNotifyChangeKeyValue` watcher thread; stops when dropped. `on_change` runs on the watcher
/// thread after any value under `Multimedia\Audio` was written; listeners re-read [`read_mode`].
pub struct DuckingWatcher {
    handles: Arc<WatchHandles>,
    done: mpsc::Receiver<()>,
}

impl DuckingWatcher {
    pub fn start(on_change: Arc<dyn Fn() + Send + Sync>) -> windows_core::Result<Self> {
        let path = wide(KEY_PATH);
        let mut key = HKEY::default();
        // SAFETY: valid NUL-terminated path; ownership of `key` moves into `OwnedKey`.
        let status = unsafe {
            RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), None, KEY_NOTIFY | KEY_QUERY_VALUE, &mut key)
        };
        if status != ERROR_SUCCESS {
            log::warn!("CommunicationsDucking: RegOpenKeyExW failed with {}", status.0);
            return Err(status.into());
        }
        let key = OwnedKey(key);
        // Manual-reset registry event so the signal sticks across the re-arm.
        let reg_event = OwnedEvent::new(true)?;
        // Auto-reset wake event so one SetEvent wakes the thread exactly once.
        let wake_event = OwnedEvent::new(false)?;
        let handles = Arc::new(WatchHandles { key, reg_event, wake_event });

        let (done_tx, done) = mpsc::channel();
        let thread_handles = Arc::clone(&handles);
        std::thread::Builder::new()
            .name("VolumeTrayApp.CommunicationsDucking".into())
            .spawn(move || {
                watch_loop(&thread_handles, on_change.as_ref());
                drop(thread_handles);
                let _ = done_tx.send(());
            })
            .map_err(|error| {
                log::warn!("CommunicationsDucking: watcher start failed: {error}");
                windows_core::Error::from_thread()
            })?;
        Ok(Self { handles, done })
    }
}

impl Drop for DuckingWatcher {
    fn drop(&mut self) {
        self.handles.wake_event.set();
        if self.done.recv_timeout(STOP_JOIN_TIMEOUT).is_err() {
            log::warn!("CommunicationsDucking.Stop: watcher did not stop before timeout; leaving handles with watcher");
        }
    }
}

fn watch_loop(handles: &WatchHandles, on_change: &(dyn Fn() + Send + Sync)) {
    let waits = [handles.wake_event.handle(), handles.reg_event.handle()];
    loop {
        // SAFETY: the key and event stay open while `handles` is alive (held by this thread).
        let status = unsafe {
            RegNotifyChangeKeyValue(
                handles.key.0,
                false,
                REG_NOTIFY_CHANGE_LAST_SET,
                Some(handles.reg_event.handle()),
                true,
            )
        };
        if status != ERROR_SUCCESS {
            log::warn!("CommunicationsDucking: RegNotifyChangeKeyValue failed with {}", status.0);
            return;
        }
        // Lowest index wins a tie, so a stop is seen even when a write lands in the same pass.
        // SAFETY: both handles are valid events for the duration of the wait.
        let result = unsafe { WaitForMultipleObjects(&waits, false, INFINITE) };
        if result == WAIT_FAILED {
            log::warn!("CommunicationsDucking: WaitForMultipleObjects failed");
            return;
        }
        if result == WAIT_OBJECT_0 {
            return;
        }
        // SAFETY: valid manual-reset event.
        if let Err(error) = unsafe { ResetEvent(handles.reg_event.handle()) } {
            log::warn!("CommunicationsDucking: ResetEvent failed: {error}");
        }
        on_change();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_mapping_matches_mmsys() {
        assert_eq!(mode_from_registry(Some(0)), DuckingMode::MuteAll);
        assert_eq!(mode_from_registry(Some(1)), DuckingMode::Reduce80);
        assert_eq!(mode_from_registry(Some(2)), DuckingMode::Reduce50);
        assert_eq!(mode_from_registry(Some(3)), DuckingMode::DoNothing);
        for mode in [DuckingMode::MuteAll, DuckingMode::Reduce80, DuckingMode::Reduce50, DuckingMode::DoNothing] {
            assert_eq!(mode_from_registry(Some(mode_to_registry(mode))), mode);
        }
    }

    #[test]
    fn missing_or_unknown_value_is_active_os_default() {
        assert_eq!(mode_from_registry(None), DuckingMode::Reduce80);
        assert!(mode_from_registry(None).is_active());
        assert!(mode_from_registry(Some(7)).is_active());
        assert!(!mode_from_registry(Some(3)).is_active());
    }
}
