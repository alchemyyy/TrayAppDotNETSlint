//! Small Win32 helpers shared by the native modules.

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{CreateEventW, SetEvent};

/// Kernel event handle owned by this wrapper and closed on drop.
pub(crate) struct OwnedEvent(HANDLE);

// SAFETY: a kernel event handle is a process-wide object reference that may be waited on,
// signalled and closed from any thread.
unsafe impl Send for OwnedEvent {}
// SAFETY: see `Send`; every operation used on it (wait, set, reset) is thread-safe in the kernel.
unsafe impl Sync for OwnedEvent {}

impl OwnedEvent {
    pub(crate) fn new(manual_reset: bool) -> windows_core::Result<Self> {
        // SAFETY: plain event creation with no security attributes and no name.
        let handle = unsafe { CreateEventW(None, manual_reset, false, None)? };
        Ok(Self(handle))
    }

    pub(crate) fn handle(&self) -> HANDLE {
        self.0
    }

    pub(crate) fn set(&self) {
        // SAFETY: the handle is a valid event owned by `self`.
        if let Err(error) = unsafe { SetEvent(self.0) } {
            log::warn!("SetEvent failed: {error}");
        }
    }
}

impl Drop for OwnedEvent {
    fn drop(&mut self) {
        // SAFETY: the handle was created by `CreateEventW` and is closed exactly once here.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Joins the calling worker thread to the MTA for its lifetime.
pub(crate) struct MtaGuard {
    initialized: bool,
}

impl MtaGuard {
    pub(crate) fn enter() -> Self {
        // SAFETY: called on a worker thread that has not been initialized for another apartment
        // model; a failure (e.g. RPC_E_CHANGED_MODE) is tolerated and simply not balanced.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            log::warn!("CoInitializeEx(MTA) failed: {hr:?}");
        }
        Self { initialized: hr.is_ok() }
    }
}

impl Drop for MtaGuard {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: balances the successful `CoInitializeEx` on this same thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// UTF-16, NUL-terminated copy of `text`.
pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Text before the first NUL of a UTF-16 buffer.
pub(crate) fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` (braced, uppercase).
pub(crate) fn guid_braced_upper(guid: &windows_core::GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    )
}

/// .NET `Guid.ToString("B")`: braced, lowercase.
pub(crate) fn guid_braced_lower(guid: &windows_core::GUID) -> String {
    guid_braced_upper(guid).to_lowercase()
}

/// Whether the process token is elevated (`TokenElevation`), the check TraceEvent's
/// `IsElevated` performs. Failure to determine reads as not elevated.
pub(crate) fn is_process_elevated() -> bool {
    use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle of the current process is always valid; `token` receives a
    // handle that is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    // SAFETY: the buffer is a correctly sized TOKEN_ELEVATION for the TokenElevation class.
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::from_mut(&mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    // SAFETY: closes the token opened above exactly once.
    let _ = unsafe { CloseHandle(token) };
    result.is_ok() && elevation.TokenIsElevated != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_formats_like_dotnet() {
        let guid = windows_core::GUID::from_u128(0xeacd2258_fcac_4ff4_b36d_419e924a6d79);
        assert_eq!(guid_braced_lower(&guid), "{eacd2258-fcac-4ff4-b36d-419e924a6d79}");
        assert_eq!(guid_braced_upper(&guid), "{EACD2258-FCAC-4FF4-B36D-419E924A6D79}");
    }
}
