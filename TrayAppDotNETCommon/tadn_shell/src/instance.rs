//! One running copy per session with replace-running semantics (port of
//! `SingleInstanceCoordinator.cs`, `ApplicationInstanceCoordinator.cs` and
//! `TrayAppDotNETExitRequest.cs`, without the crash watcher).
//!
//! A new launch asks the running copy to exit through its PID-keyed exit event, waits for it, and
//! kills it (after verifying the PID still belongs to the same exe) if it does not exit in time.
//!
//! The running copy's PID is published in a 12-byte PID bulletin (generation @0, owner PID @4,
//! monitored PID @8, the C# layout; both PIDs are the app itself since there is no watcher), named
//! after this build's identity so the .NET app never appears in it.

use std::cell::RefCell;
use std::sync::atomic::{Ordering, fence};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tadn_common::identity::AppIdentity;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, FILETIME, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, GetCurrentProcessId, GetProcessTimes, INFINITE, OpenEventW,
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, ReleaseMutex, ResetEvent, SetEvent, TerminateProcess, WaitForMultipleObjects,
    WaitForSingleObject,
};
use windows_core::{PCWSTR, PWSTR};

use crate::util::wide;

/// `SingleInstanceMutexAcquireTimeoutMs`: wait for the mutex after killing the old copy.
const SINGLE_INSTANCE_MUTEX_ACQUIRE_TIMEOUT_MS: u32 = 5_000;
/// `SingleInstancePidBulletinReadTimeoutMs`: how long to wait for the owner to publish its PID.
const SINGLE_INSTANCE_PID_BULLETIN_READ_TIMEOUT_MS: u64 = 1_000;
/// `SingleInstancePidBulletinReadRetryMs`.
const SINGLE_INSTANCE_PID_BULLETIN_READ_RETRY_MS: u64 = 25;
/// `GracefulExitRequestTimeoutMs`: wait for the old copy to run its own exit after asking.
const GRACEFUL_EXIT_REQUEST_TIMEOUT_MS: u32 = 5_000;
/// `GracefulExitKillWaitMs`: wait for a killed process to end.
const GRACEFUL_EXIT_KILL_WAIT_MS: u32 = 5_000;

const BULLETIN_SIZE: usize = 12;
const OFFSET_GENERATION: usize = 0;
const OFFSET_OWNER_PID: usize = 4;
const OFFSET_MONITORED_PID: usize = 8;

pub struct SingleInstance {
    mutex: HANDLE,
    bulletin: Option<Bulletin>,
    /// `AppIdentity::exit_event_name` of this process.
    exit_event_name: String,
    listener: RefCell<Option<ExitListener>>,
}

#[derive(Debug)]
pub enum InstanceError {
    /// The running copy neither exited nor could be terminated.
    CouldNotReplace(String),
    Win32(windows_core::Error),
}

impl std::fmt::Display for InstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CouldNotReplace(message) => write!(f, "{message}"),
            Self::Win32(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for InstanceError {}

impl From<windows_core::Error> for InstanceError {
    fn from(error: windows_core::Error) -> Self {
        Self::Win32(error)
    }
}

impl SingleInstance {
    /// Acquires the instance mutex, replacing a running copy first. Holds it until dropped.
    pub fn acquire(identity: &AppIdentity) -> Result<Self, InstanceError> {
        let name = wide(&identity.instance_mutex_name());
        // SAFETY: NUL-terminated name; the handle is closed in Drop or on the error path.
        let mutex = unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr())) }?;
        // SAFETY: no preconditions; read right after CreateMutexW.
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

        if existed && let Err(error) = take_over(identity, mutex) {
            close(mutex);
            return Err(error);
        }

        let bulletin = match Bulletin::create(&bulletin_name(identity)) {
            Ok(bulletin) => {
                bulletin.publish(current_pid());
                Some(bulletin)
            }
            Err(error) => {
                // Only a later replacing launch needs it; that launch then waits for the mutex.
                log::warn!("SingleInstance: PID bulletin unavailable: {error}");
                None
            }
        };
        Ok(Self {
            mutex,
            bulletin,
            exit_event_name: identity.exit_event_name(current_pid()),
            listener: RefCell::new(None),
        })
    }

    /// Starts listening for an exit request from a newer copy; `on_exit` runs on the UI thread
    /// (via `slint::invoke_from_event_loop`) when it arrives.
    pub fn listen_for_exit_request(&self, on_exit: impl FnOnce() + Send + 'static) {
        let previous = self.listener.borrow_mut().take();
        drop(previous);
        match ExitListener::start(&self.exit_event_name, on_exit) {
            Ok(listener) => *self.listener.borrow_mut() = Some(listener),
            Err(error) => log::warn!("SingleInstance: exit request listener not started: {error}"),
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        self.listener.borrow_mut().take();
        self.bulletin.take();
        // SAFETY: this process owns the mutex since `acquire` succeeded.
        if let Err(error) = unsafe { ReleaseMutex(self.mutex) } {
            log::debug!("SingleInstance: ReleaseMutex failed: {error}");
        }
        close(self.mutex);
    }
}

fn current_pid() -> u32 {
    // SAFETY: no preconditions.
    unsafe { GetCurrentProcessId() }
}

fn close(handle: HANDLE) {
    // SAFETY: the handle is owned by the caller and not used afterwards.
    let _ = unsafe { CloseHandle(handle) };
}

fn bulletin_name(identity: &AppIdentity) -> String {
    format!("Local\\{}-InstancePID-{:?}", identity.instance_name, identity.app_guid)
}

/// Waits for the instance mutex; an abandoned mutex (owner killed) counts as acquired.
fn wait_for_mutex(mutex: HANDLE, timeout_ms: u32) -> bool {
    // SAFETY: `mutex` is a live handle.
    let result = unsafe { WaitForSingleObject(mutex, timeout_ms) };
    result == WAIT_OBJECT_0 || result == WAIT_ABANDONED
}

/// `SingleInstanceCoordinator.TakeoverFromExistingOwner`: ask the running copy to exit, wait,
/// then kill it (identity re-verified) and wait for the mutex once more.
fn take_over(identity: &AppIdentity, mutex: HANDLE) -> Result<(), InstanceError> {
    let old = read_existing_owner(identity).and_then(|pid| ProcessIdentity::capture(pid, identity.app_name));

    if let Some(old) = &old
        && request_exit(identity, old)
    {
        log::info!("SingleInstance: asked PID {} to exit", old.pid);
        if wait_for_mutex(mutex, GRACEFUL_EXIT_REQUEST_TIMEOUT_MS) {
            return Ok(());
        }
        log::warn!("SingleInstance: PID {} did not exit in time", old.pid);
    }

    if let Some(old) = &old {
        old.try_kill();
    }

    if wait_for_mutex(mutex, SINGLE_INSTANCE_MUTEX_ACQUIRE_TIMEOUT_MS) {
        Ok(())
    } else {
        Err(InstanceError::CouldNotReplace(format!(
            "Timed out waiting for the running {} to exit (single-instance mutex).",
            identity.app_name
        )))
    }
}

/// The owner PID from the bulletin, retrying while the owner is still publishing it.
fn read_existing_owner(identity: &AppIdentity) -> Option<u32> {
    let name = wide(&bulletin_name(identity));
    let deadline = Instant::now() + Duration::from_millis(SINGLE_INSTANCE_PID_BULLETIN_READ_TIMEOUT_MS);
    loop {
        // SAFETY: NUL-terminated name; the mapping and view are released before the next attempt.
        unsafe {
            if let Ok(mapping) = OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(name.as_ptr())) {
                let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, BULLETIN_SIZE);
                let mut owner = None;
                if !view.Value.is_null() {
                    let base = view.Value.cast::<u8>();
                    let generation = base.add(OFFSET_GENERATION).cast::<i32>().read_volatile();
                    fence(Ordering::Acquire);
                    if generation != 0 {
                        let pid = base.add(OFFSET_MONITORED_PID).cast::<i32>().read_volatile();
                        let pid = if pid > 0 { pid } else { base.add(OFFSET_OWNER_PID).cast::<i32>().read_volatile() };
                        owner = Some(pid.max(0) as u32);
                    }
                    let _ = UnmapViewOfFile(view);
                }
                close(mapping);
                if owner.is_some() {
                    return owner;
                }
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(SINGLE_INSTANCE_PID_BULLETIN_READ_RETRY_MS));
    }
}

/// `TrayAppDotNETExitRequest.TryRequest`: signals the running copy's exit event. False when the
/// process is gone, no longer matches the captured identity, or does not listen yet.
fn request_exit(identity: &AppIdentity, process: &ProcessIdentity) -> bool {
    let Some(_running) = process.open() else { return false };
    let name = wide(&identity.exit_event_name(process.pid));
    // SAFETY: NUL-terminated name; the event handle is closed below.
    unsafe {
        let Ok(event) = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(name.as_ptr())) else { return false };
        let signalled = SetEvent(event);
        close(event);
        match signalled {
            Ok(()) => true,
            Err(error) => {
                log::warn!("SingleInstance: could not signal PID {}: {error}", process.pid);
                false
            }
        }
    }
}

/// A running process pinned by its ID, image path and start time
/// (`TrayAppDotNETProcessIdentity`): a PID is reused once its process is gone, so every kill
/// first proves the PID still names the captured process.
struct ProcessIdentity {
    pid: u32,
    image_path: String,
    start_time: u64,
}

/// An open process handle, closed on drop.
struct OwnedProcess(HANDLE);

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        close(self.0);
    }
}

impl ProcessIdentity {
    /// Captures `pid` when it is another live process whose image file name is `app_name`.
    fn capture(pid: u32, app_name: &str) -> Option<Self> {
        if pid == 0 || pid == current_pid() {
            return None;
        }
        let process = open_process(pid)?;
        let image_path = image_path(process.0)?;
        let start_time = start_time(process.0)?;
        let file_stem = std::path::Path::new(&image_path).file_stem()?.to_string_lossy().into_owned();
        if !file_stem.eq_ignore_ascii_case(app_name) {
            log::warn!("SingleInstance: PID {pid} is {image_path}, not {app_name}");
            return None;
        }
        Some(Self { pid, image_path, start_time })
    }

    /// The process again, only while its PID still names the captured image and start time.
    fn open(&self) -> Option<OwnedProcess> {
        let process = open_process(self.pid)?;
        // SAFETY: live handle with SYNCHRONIZE access.
        let running = unsafe { WaitForSingleObject(process.0, 0) } == WAIT_TIMEOUT;
        let same = running
            && start_time(process.0) == Some(self.start_time)
            && image_path(process.0).is_some_and(|path| path.eq_ignore_ascii_case(&self.image_path));
        same.then_some(process)
    }

    /// `TrayAppDotNETExitRequest.TryKill`: terminate when still the captured process, then wait
    /// briefly. True when it is gone afterwards.
    fn try_kill(&self) -> bool {
        let Some(process) = self.open() else { return true };
        log::warn!("SingleInstance: killing PID {} ({})", self.pid, self.image_path);
        // SAFETY: live handle with PROCESS_TERMINATE | SYNCHRONIZE access, identity verified.
        unsafe {
            if let Err(error) = TerminateProcess(process.0, 1) {
                log::warn!("SingleInstance: could not kill PID {}: {error}", self.pid);
            }
            WaitForSingleObject(process.0, GRACEFUL_EXIT_KILL_WAIT_MS) == WAIT_OBJECT_0
        }
    }
}

fn open_process(pid: u32) -> Option<OwnedProcess> {
    // SAFETY: plain OpenProcess; the handle is owned by the returned value.
    unsafe {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid)
            .ok()
            .map(OwnedProcess)
    }
}

fn image_path(process: HANDLE) -> Option<String> {
    let mut buffer = vec![0u16; 32_768];
    let mut len = buffer.len() as u32;
    // SAFETY: `buffer` holds `len` UTF-16 units.
    unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len) }.ok()?;
    Some(String::from_utf16_lossy(&buffer[..len as usize]))
}

fn start_time(process: HANDLE) -> Option<u64> {
    let (mut creation, mut exit, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: out-pointers to locals.
    unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) }.ok()?;
    Some((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

/// The PID bulletin this process publishes for a later replacing launch.
struct Bulletin {
    mapping: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl Bulletin {
    fn create(name: &str) -> windows_core::Result<Self> {
        let wide_name = wide(name);
        // SAFETY: pagefile-backed mapping of BULLETIN_SIZE bytes with a NUL-terminated name.
        unsafe {
            let mapping = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                BULLETIN_SIZE as u32,
                PCWSTR(wide_name.as_ptr()),
            )?;
            let view = MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, BULLETIN_SIZE);
            if view.Value.is_null() {
                let error = windows_core::Error::from_thread();
                close(mapping);
                return Err(error);
            }
            Ok(Self { mapping, view })
        }
    }

    /// PIDs first, then a barrier, then the next generation (never 0).
    fn publish(&self, pid: u32) {
        let base = self.view.Value.cast::<u8>();
        // SAFETY: the view maps BULLETIN_SIZE writable bytes; offsets are 4-byte aligned.
        unsafe {
            let generation = base.add(OFFSET_GENERATION).cast::<i32>().read_volatile();
            base.add(OFFSET_OWNER_PID).cast::<i32>().write_volatile(pid as i32);
            base.add(OFFSET_MONITORED_PID).cast::<i32>().write_volatile(pid as i32);
            fence(Ordering::SeqCst);
            base.add(OFFSET_GENERATION).cast::<i32>().write_volatile(next_generation(generation));
        }
    }
}

impl Drop for Bulletin {
    fn drop(&mut self) {
        // SAFETY: the view and mapping were created in `create` and are released once.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
        }
        close(self.mapping);
    }
}

fn next_generation(generation: i32) -> i32 {
    if generation == i32::MAX || generation.wrapping_add(1) == 0 { 1 } else { generation + 1 }
}

/// `TrayAppDotNETExitRequestListener`: waits on this process's exit event on a background thread
/// until dropped.
struct ExitListener {
    exit_event: usize,
    stop_event: usize,
    thread: Option<JoinHandle<()>>,
}

impl ExitListener {
    fn start(event_name: &str, on_exit: impl FnOnce() + Send + 'static) -> windows_core::Result<Self> {
        let name = wide(event_name);
        // SAFETY: NUL-terminated name; both handles are closed in Drop (or below on error).
        let (exit_event, stop_event) = unsafe {
            let exit_event = CreateEventW(None, true, false, PCWSTR(name.as_ptr()))?;
            // A request that arrived before this process got here was addressed to an earlier
            // owner of the PID.
            let _ = ResetEvent(exit_event);
            let stop_event = match CreateEventW(None, true, false, PCWSTR::null()) {
                Ok(event) => event,
                Err(error) => {
                    close(exit_event);
                    return Err(error);
                }
            };
            (exit_event.0 as usize, stop_event.0 as usize)
        };
        let thread = std::thread::Builder::new().name("exit-request".into()).spawn(move || {
            let handles = [HANDLE(exit_event as *mut _), HANDLE(stop_event as *mut _)];
            // SAFETY: both handles stay open until this thread is joined in Drop.
            let result = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
            if result == WAIT_OBJECT_0 {
                log::info!("SingleInstance: exit requested by a newer launch");
                if let Err(error) = slint::invoke_from_event_loop(on_exit) {
                    log::error!("SingleInstance: could not post the exit request: {error}");
                }
            }
        });
        match thread {
            Ok(thread) => Ok(Self { exit_event, stop_event, thread: Some(thread) }),
            Err(error) => {
                close(HANDLE(exit_event as *mut _));
                close(HANDLE(stop_event as *mut _));
                Err(windows_core::Error::new(windows::Win32::Foundation::E_FAIL, error.to_string()))
            }
        }
    }
}

impl Drop for ExitListener {
    fn drop(&mut self) {
        // SAFETY: the stop event is open until closed below.
        let _ = unsafe { SetEvent(HANDLE(self.stop_event as *mut _)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        close(HANDLE(self.exit_event as *mut _));
        close(HANDLE(self.stop_event as *mut _));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_IDENTITY: AppIdentity = AppIdentity {
        app_name: "tadn_shell_instance_test",
        instance_name: "TadnShellInstanceTest",
        tray_icon_guid: windows_core::GUID::from_u128(0x0b3e_6a41_6c55_4c8e_9d0b_6b9e_0f57_2a11),
        app_guid: windows_core::GUID::from_u128(0x5f0a_2d1e_7c43_4b9a_8e61_3a7d_c2b4_9e05),
    };

    #[test]
    fn generations_wrap_to_one_and_never_zero() {
        assert_eq!(next_generation(0), 1);
        assert_eq!(next_generation(41), 42);
        assert_eq!(next_generation(i32::MAX), 1);
        assert_eq!(next_generation(-1), 1);
    }

    #[test]
    fn identity_names_are_side_by_side_with_the_dotnet_build() {
        assert_eq!(
            TEST_IDENTITY.instance_mutex_name(),
            "Local\\TadnShellInstanceTest-Instance-5F0A2D1E-7C43-4B9A-8E61-3A7DC2B49E05"
        );
        assert_eq!(TEST_IDENTITY.exit_event_name(1234), "Local\\TadnShellInstanceTest-Exit-1234");
        assert_eq!(
            bulletin_name(&TEST_IDENTITY),
            "Local\\TadnShellInstanceTest-InstancePID-5F0A2D1E-7C43-4B9A-8E61-3A7DC2B49E05"
        );
    }

    #[test]
    fn acquire_publishes_this_process_in_the_bulletin() {
        let instance = SingleInstance::acquire(&TEST_IDENTITY).expect("first acquire");
        assert_eq!(read_existing_owner(&TEST_IDENTITY), Some(current_pid()));
        drop(instance);
    }

    #[test]
    fn the_own_process_is_never_captured_for_a_kill() {
        assert!(ProcessIdentity::capture(current_pid(), TEST_IDENTITY.app_name).is_none());
        assert!(ProcessIdentity::capture(0, TEST_IDENTITY.app_name).is_none());
    }
}
