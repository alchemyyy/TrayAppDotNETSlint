//! Process helpers: image path (`ProcessImagePath`), display name (`ProcessHelper`) and exit
//! notifications (`ProcessExitWatch`).

use std::ffi::c_void;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Threading::{
    INFINITE, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, RegisterWaitForSingleObject, UnregisterWaitEx, WT_EXECUTEONLYONCE,
    WaitForSingleObject,
};
use windows_core::{HSTRING, PCWSTR, PWSTR};

/// MAX_PATH covers almost every image; long-path images grow the buffer up to the NT path limit.
const IMAGE_PATH_INITIAL_CAPACITY: usize = 260;
const IMAGE_PATH_MAXIMUM_CAPACITY: usize = 32_767;

/// Owned process handle.
pub(crate) struct ProcessHandle(HANDLE);

impl ProcessHandle {
    pub(crate) fn open(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> windows_core::Result<Self> {
        // SAFETY: plain Win32 call; the returned handle is owned by the wrapper.
        unsafe { OpenProcess(access, false, pid) }.map(Self)
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open` and is closed exactly once.
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            log::warn!("CloseHandle(process) failed: {error}");
        }
    }
}

/// The full image path of a process, or None when it is gone or refuses limited query access.
pub(crate) fn image_path(pid: u32) -> Option<String> {
    // 0 is the idle process, which has no image.
    if pid == 0 {
        return None;
    }
    let process = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    image_path_of(&process)
}

fn image_path_of(process: &ProcessHandle) -> Option<String> {
    let mut buffer = vec![0u16; IMAGE_PATH_INITIAL_CAPACITY];
    loop {
        let mut length = buffer.len() as u32;
        // SAFETY: `buffer` holds `length` UTF-16 units; the call writes at most that many.
        let result = unsafe {
            QueryFullProcessImageNameW(process.raw(), PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut length)
        };
        match result {
            Ok(()) => return Some(String::from_utf16_lossy(&buffer[..length as usize])),
            Err(error)
                if error.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult()
                    && buffer.len() < IMAGE_PATH_MAXIMUM_CAPACITY =>
            {
                let grown = (buffer.len() * 2).min(IMAGE_PATH_MAXIMUM_CAPACITY);
                buffer.resize(grown, 0);
            }
            Err(_) => return None,
        }
    }
}

/// .NET `FileVersionInfo` language/code-page fallbacks when the translation table is missing or
/// wrong.
const FALLBACK_LANGUAGE_CODE_PAGES: [u32; 3] = [0x0409_04B0, 0x0409_04E4, 0x0409_0000];

/// `ProcessHelper.GetDisplayNameForProcess`: the image's `FileDescription`, else the file name
/// without extension. None means unresolved ("Unknown").
pub(crate) fn display_name_for_process(pid: u32) -> Option<String> {
    let path = image_path(pid).filter(|p| !p.is_empty())?;
    if let Some(description) = file_description(&path).filter(|d| !d.trim().is_empty()) {
        return Some(description);
    }
    std::path::Path::new(&path).file_stem().map(|stem| stem.to_string_lossy().into_owned())
}

fn file_description(path: &str) -> Option<String> {
    let path = HSTRING::from(path);
    // SAFETY: `path` is a valid NUL-terminated wide string.
    let size = unsafe { GetFileVersionInfoSizeW(&path, None) };
    if size == 0 {
        return None;
    }
    let mut block = vec![0u8; size as usize];
    // SAFETY: `block` holds `size` bytes as the call requires.
    unsafe { GetFileVersionInfoW(&path, None, size, block.as_mut_ptr().cast()) }.ok()?;

    let translation = query_translation(&block);
    if let Some(found) = translation.and_then(|code_page| query_string(&block, code_page, "FileDescription")) {
        return Some(found);
    }
    FALLBACK_LANGUAGE_CODE_PAGES
        .iter()
        .filter(|candidate| Some(**candidate) != translation)
        .find_map(|candidate| query_string(&block, *candidate, "FileDescription"))
}

/// First `\VarFileInfo\Translation` entry as `(language << 16) | code page`.
fn query_translation(block: &[u8]) -> Option<u32> {
    let mut pointer: *mut c_void = std::ptr::null_mut();
    let mut length = 0u32;
    // SAFETY: `block` is a version-info block filled by GetFileVersionInfoW; the out pointer
    // points into it.
    let found = unsafe {
        VerQueryValueW(block.as_ptr().cast(), windows_core::w!("\\VarFileInfo\\Translation"), &mut pointer, &mut length)
    };
    if !found.as_bool() || pointer.is_null() || length < 4 {
        return None;
    }
    // SAFETY: the translation entry is two u16 values (language, code page) inside `block`.
    let words = unsafe { std::slice::from_raw_parts(pointer.cast::<u16>(), 2) };
    Some((u32::from(words[0]) << 16) | u32::from(words[1]))
}

fn query_string(block: &[u8], language_code_page: u32, name: &str) -> Option<String> {
    let sub_block = HSTRING::from(format!("\\StringFileInfo\\{language_code_page:08x}\\{name}"));
    let mut pointer: *mut c_void = std::ptr::null_mut();
    let mut length = 0u32;
    // SAFETY: see `query_translation`.
    let found = unsafe { VerQueryValueW(block.as_ptr().cast(), &sub_block, &mut pointer, &mut length) };
    if !found.as_bool() || pointer.is_null() || length == 0 {
        return None;
    }
    // SAFETY: string values are NUL-terminated UTF-16 inside `block`.
    let text = unsafe { PCWSTR(pointer.cast()).to_string() }.ok()?;
    (!text.is_empty()).then_some(text)
}

/// What [`watch_exit`] found.
pub(crate) enum ExitWatchResult {
    /// Running; the callback runs once (on a thread-pool thread) when it exits.
    Watching(ExitWatch),
    /// Already exited (or no process has the id).
    Exited,
    /// Exists but cannot be watched (access denied); not treated as gone.
    Unavailable,
}

struct ExitContext {
    on_exit: Box<dyn Fn() + Send + Sync>,
}

/// A registered process-exit wait. Dropping it unregisters the wait (waiting for a running
/// callback to finish) without invoking the callback.
pub(crate) struct ExitWatch {
    wait: HANDLE,
    /// Kept open (and closed after the wait is unregistered) so the id cannot be reused meanwhile.
    _process: ProcessHandle,
    context: *mut ExitContext,
}

unsafe extern "system" fn on_process_signaled(context: *mut c_void, _timed_out: bool) {
    // SAFETY: `context` is the `ExitContext` registered in `watch_exit`; it stays alive until
    // `UnregisterWaitEx(INVALID_HANDLE_VALUE)` returned, which waits for this callback.
    let context = unsafe { &*context.cast::<ExitContext>() };
    (context.on_exit)();
}

/// `ProcessExitWatch.TryRegister` (without start-time validation, which the session path does not
/// use).
pub(crate) fn watch_exit(pid: u32, on_exit: Box<dyn Fn() + Send + Sync>) -> ExitWatchResult {
    if pid == 0 {
        return ExitWatchResult::Exited;
    }
    let process = match ProcessHandle::open(pid, PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION) {
        Ok(process) => process,
        Err(error) if error.code() == ERROR_ACCESS_DENIED.to_hresult() => {
            // Some protected processes grant SYNCHRONIZE without limited query access.
            match ProcessHandle::open(pid, PROCESS_SYNCHRONIZE) {
                Ok(process) => process,
                Err(error) => return classify_open_failure(error.code()),
            }
        }
        Err(error) => return classify_open_failure(error.code()),
    };

    // A process that exited while someone still holds a handle opens fine but is signaled.
    // SAFETY: valid process handle.
    if unsafe { WaitForSingleObject(process.raw(), 0) } == WAIT_OBJECT_0 {
        return ExitWatchResult::Exited;
    }

    let context = Box::into_raw(Box::new(ExitContext { on_exit }));
    let mut wait = HANDLE::default();
    // SAFETY: `context` outlives the registration (freed in `ExitWatch::drop` after the wait is
    // unregistered with completion); the process handle stays open for the same span.
    let registered = unsafe {
        RegisterWaitForSingleObject(
            &mut wait,
            process.raw(),
            Some(on_process_signaled),
            Some(context.cast_const().cast()),
            INFINITE,
            WT_EXECUTEONLYONCE,
        )
    };
    match registered {
        Ok(()) => ExitWatchResult::Watching(ExitWatch { wait, _process: process, context }),
        Err(error) => {
            // SAFETY: registration failed, so no callback can reference the context.
            drop(unsafe { Box::from_raw(context) });
            log::warn!("RegisterWaitForSingleObject(pid {pid}) failed: {error}");
            ExitWatchResult::Unavailable
        }
    }
}

/// Only ERROR_INVALID_PARAMETER means no process has the id; anything else (access denied above
/// all) means it may well be running.
fn classify_open_failure(code: windows_core::HRESULT) -> ExitWatchResult {
    if code == ERROR_INVALID_PARAMETER.to_hresult() { ExitWatchResult::Exited } else { ExitWatchResult::Unavailable }
}

impl Drop for ExitWatch {
    fn drop(&mut self) {
        // SAFETY: `wait` came from RegisterWaitForSingleObject; INVALID_HANDLE_VALUE blocks until a
        // running callback returned, after which the context can be freed.
        if let Err(error) = unsafe { UnregisterWaitEx(self.wait, Some(INVALID_HANDLE_VALUE)) } {
            log::warn!("UnregisterWaitEx failed: {error}");
        }
        // SAFETY: allocated by `Box::into_raw` in `watch_exit`; no callback can run any more.
        drop(unsafe { Box::from_raw(self.context) });
    }
}
