//! COM apartment scope for the audio thread and its workers.

use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

/// Joins the process MTA on the current thread; leaves it on drop.
pub struct MtaScope(());

impl MtaScope {
    pub fn enter(label: &str) -> Option<Self> {
        // SAFETY: initializes COM on the calling thread; balanced by `Drop`.
        match unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
            Ok(()) => Some(Self(())),
            Err(error) => {
                log::error!("{label}: CoInitializeEx(MTA) failed: {error}");
                None
            }
        }
    }
}

impl Drop for MtaScope {
    fn drop(&mut self) {
        // SAFETY: paired with the successful CoInitializeEx in `enter` on this thread.
        unsafe { CoUninitialize() };
    }
}
