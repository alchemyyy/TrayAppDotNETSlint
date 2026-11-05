//! App icon resolution with retries and an LRU (port of `Audio/AppIconResolver.cs`): Win32 exe
//! icons, packaged (UWP/MSIX) app images through their AUMID, `IAudioSessionControl::GetIconPath`
//! resources; up to `ICON_RETRY_ATTEMPTS` attempts spaced 1x, 2x, 3x the interval, then a generic
//! icon (None).
//!
//! Resolution chain: system sounds -> `audiosrv.dll` icon 203; packaged process -> shell image of
//! its AUMID (Apps folder); session icon path (or the process image) -> PE icon resource when the
//! location carries an index, else the shell icon (`SHDefExtractIconW`), else the shell image.
//! Entries are deduplicated by identity key and by pixel content; unused entries (no snapshot or
//! session holds the pixels) stay cached up to the LRU limit.
//!
//! Resolution runs on one background MTA worker; results reach the session's host as
//! [`SessionEvent`]s through the sink.

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use vtadn_model::settings::ICON_RETRY_ATTEMPTS;
use vtadn_model::snapshot::{AppId, DeviceId, IconPixels};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, FreeLibrary, MAX_PATH, SIZE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP,
    HGDIOBJ, ReleaseDC,
};
use windows::Win32::Storage::Packaging::Appx::{GetApplicationUserModelId, GetPackageId};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize, IBindCtx};
use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows::Win32::System::LibraryLoader::{
    FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW, LoadResource,
    LockResource, SizeofResource,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
use windows::Win32::UI::Shell::{
    FOLDERID_AppsFolder, IShellItemImageFactory, KF_FLAG_DONT_VERIFY, PathParseIconLocationW,
    SHCreateItemFromParsingName, SHCreateItemInKnownFolder, SHDefExtractIconW, SIIGBF_RESIZETOFIT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, GetIconInfo, HICON, ICONINFO, LR_DEFAULTCOLOR, LookupIconIdFromDirectoryEx,
    RT_GROUP_ICON, RT_ICON,
};
use windows_core::{HSTRING, PCWSTR, PWSTR};

use crate::host::{AgileControl, Event, SessionEvent, SessionSink};
use crate::process::{ProcessHandle, image_path};

/// `IconSize`: app icons are extracted at 48 px.
const ICON_SIZE: i32 = 48;
const SYSTEM_SOUNDS_DLL: &str = "audiosrv.dll";
const SYSTEM_SOUNDS_ICON_ORDINAL: u16 = 203;
const CORTANA_BAD_AUMID: &str = "MicrosoftWindows.Client.CBS_cw5n1h2txyewy!CortanaUI";
const CORTANA_GOOD_AUMID: &str = "MicrosoftWindows.Client.CBS_cw5n1h2txyewy!PackageMetadata";
const INVALID_ORDINAL_MARKER: &str = ",-";
/// `MAX_AUMID_LEN`.
const MAX_AUMID_LEN: u32 = 512;
/// `IconCursorVersion.Default` for `CreateIconFromResourceEx`.
const ICON_CURSOR_VERSION: u32 = 0x0003_0000;
/// `DefaultMaximumDimension`: the largest icon side the readers accept.
const MAXIMUM_DIMENSION: i32 = 256;

const ALPHA_THRESHOLD: u8 = 16;
const MIN_OPAQUE_RUN: i32 = 2;
const MIN_PAD_RATIO: f64 = 0.10;

const KEY_SYS: &str = "sys";
const KEY_PE: &str = "pe";
const KEY_SHELL_UWP: &str = "shell|uwp";
const KEY_SHELL_DESKTOP: &str = "shell|desktop";
/// `MaxIdentityAliasesPerEntry`.
const MAX_IDENTITY_ALIASES_PER_ENTRY: usize = 8;
/// `AppSettings.IconLRULimitMin` / `IconLRULimitMax`.
const LRU_LIMIT_MAX: u32 = 1000;
/// `TimeConstants.IconRetryIntervalMs` min / max.
const RETRY_INTERVAL_MIN_MS: u32 = 50;
const RETRY_INTERVAL_MAX_MS: u32 = 5000;

/// What a queued resolution does.
#[derive(Clone, Copy, Debug)]
pub(crate) enum IconJobKind {
    /// First resolution after the session appeared: icon (with retries) and, when the session did
    /// not name itself, the process display name.
    Initial { resolve_name: bool },
    /// Icon retry `attempt` (2..=ICON_RETRY_ATTEMPTS).
    Retry { attempt: u32 },
    /// The session went Active with a placeholder name or no icon (`TryReresolveProcessMetadata`).
    Reresolve { name: bool, icon: bool },
    /// `OnIconPathChanged`: re-run the chain once; a failure clears the icon.
    IconPathChanged,
}

pub(crate) struct IconJob {
    pub device: DeviceId,
    pub session: u64,
    pub app_id: AppId,
    pub pid: u32,
    pub is_system_sounds: bool,
    pub control: AgileControl,
    pub alive: Arc<AtomicBool>,
    pub sink: SessionSink,
    pub kind: IconJobKind,
}

struct Entry {
    pixels: Arc<IconPixels>,
    content_hash: u64,
    identity_keys: VecDeque<String>,
    last_used: u64,
}

#[derive(Default)]
struct CacheState {
    retry_interval_ms: u32,
    lru_limit: u32,
    by_identity: HashMap<String, u64>,
    by_content: HashMap<u64, u64>,
    entries: HashMap<u64, Entry>,
    by_app: HashMap<AppId, u64>,
    next_id: u64,
    tick: u64,
}

#[derive(Default)]
struct Queue {
    ready: VecDeque<IconJob>,
    delayed: Vec<(Instant, IconJob)>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<CacheState>,
    queue: Mutex<Queue>,
    wake: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct IconCache {
    shared: Arc<Shared>,
}

impl IconCache {
    pub fn new(retry_interval_ms: u32, lru_limit: u32) -> Arc<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(CacheState {
                retry_interval_ms: retry_interval_ms.clamp(RETRY_INTERVAL_MIN_MS, RETRY_INTERVAL_MAX_MS),
                lru_limit: lru_limit.min(LRU_LIMIT_MAX),
                ..CacheState::default()
            }),
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
        });
        let worker = shared.clone();
        if let Err(error) = std::thread::Builder::new().name("vtadn-app-icons".to_owned()).spawn(move || run(&worker)) {
            log::error!("IconCache: could not start the icon worker: {error}");
        }
        Arc::new(Self { shared })
    }

    pub fn configure(&self, retry_interval_ms: u32, lru_limit: u32) {
        let mut state = lock(&self.shared.state);
        state.retry_interval_ms = retry_interval_ms.clamp(RETRY_INTERVAL_MIN_MS, RETRY_INTERVAL_MAX_MS);
        state.lru_limit = lru_limit.min(LRU_LIMIT_MAX);
        // `TrimToLimit`: a reduced limit applies at once.
        state.trim();
    }

    /// Cached icon for an app id, if resolved.
    pub fn get(&self, app_id: &str) -> Option<Arc<IconPixels>> {
        let state = lock(&self.shared.state);
        let id = state.by_app.get(app_id)?;
        state.entries.get(id).map(|entry| entry.pixels.clone())
    }

    pub(crate) fn request(&self, job: IconJob) {
        let mut queue = lock(&self.shared.queue);
        if queue.shutdown {
            return;
        }
        queue.ready.push_back(job);
        self.shared.wake.notify_one();
    }
}

impl Drop for IconCache {
    fn drop(&mut self) {
        let mut queue = lock(&self.shared.queue);
        queue.shutdown = true;
        queue.ready.clear();
        queue.delayed.clear();
        self.shared.wake.notify_one();
    }
}

impl CacheState {
    fn alias(&mut self, id: u64, key: &str) {
        if let Some(previous) = self.by_identity.get(key).copied()
            && previous != id
            && let Some(entry) = self.entries.get_mut(&previous)
        {
            entry.identity_keys.retain(|k| k != key);
        }
        let Some(entry) = self.entries.get_mut(&id) else { return };
        entry.identity_keys.retain(|k| k != key);
        entry.identity_keys.push_back(key.to_owned());
        self.by_identity.insert(key.to_owned(), id);
        while entry.identity_keys.len() > MAX_IDENTITY_ALIASES_PER_ENTRY {
            let Some(oldest) = entry.identity_keys.pop_front() else { break };
            if self.by_identity.get(&oldest) == Some(&id) {
                self.by_identity.remove(&oldest);
            }
        }
    }

    fn touch(&mut self, id: u64) -> Option<Arc<IconPixels>> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.entries.get_mut(&id)?;
        entry.last_used = tick;
        Some(entry.pixels.clone())
    }

    fn hit(&mut self, key: &str) -> Option<(u64, Arc<IconPixels>)> {
        let id = *self.by_identity.get(key)?;
        self.alias(id, key);
        self.touch(id).map(|pixels| (id, pixels))
    }

    /// Keeps at most `lru_limit` unused entries (pixels referenced only by the cache), dropping the
    /// least recently used.
    fn trim(&mut self) {
        let mut unused: Vec<(u64, u64)> = self
            .entries
            .iter()
            .filter(|(_, entry)| Arc::strong_count(&entry.pixels) == 1)
            .map(|(id, entry)| (entry.last_used, *id))
            .collect();
        let limit = self.lru_limit as usize;
        if unused.len() <= limit {
            return;
        }
        unused.sort_unstable();
        let excess = unused.len() - limit;
        for (_, id) in unused.into_iter().take(excess) {
            let Some(entry) = self.entries.remove(&id) else { continue };
            if self.by_content.get(&entry.content_hash) == Some(&id) {
                self.by_content.remove(&entry.content_hash);
            }
            for key in &entry.identity_keys {
                if self.by_identity.get(key) == Some(&id) {
                    self.by_identity.remove(key);
                }
            }
            self.by_app.retain(|_, mapped| *mapped != id);
        }
    }
}

/// Raw 32-bit BGRA pixels (straight alpha, top-down rows).
struct RawPixels {
    width: i32,
    height: i32,
    bgra: Vec<u8>,
}

fn run(shared: &Shared) {
    // SAFETY: initializes COM for this worker thread; balanced by CoUninitialize below.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if com.is_err() {
        log::error!("IconCache worker: CoInitializeEx failed: {com:?}");
    }
    while let Some(job) = next_job(shared) {
        process(shared, job);
    }
    if com.is_ok() {
        // SAFETY: matches the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
}

fn next_job(shared: &Shared) -> Option<IconJob> {
    let mut queue = lock(&shared.queue);
    loop {
        if queue.shutdown {
            return None;
        }
        let now = Instant::now();
        let mut index = 0;
        while index < queue.delayed.len() {
            if queue.delayed[index].0 <= now {
                let (_, job) = queue.delayed.swap_remove(index);
                queue.ready.push_back(job);
            } else {
                index += 1;
            }
        }
        if let Some(job) = queue.ready.pop_front() {
            return Some(job);
        }
        let next_due = queue.delayed.iter().map(|(due, _)| *due).min();
        queue = match next_due {
            Some(due) => {
                let timeout = due.saturating_duration_since(now);
                shared.wake.wait_timeout(queue, timeout).unwrap_or_else(PoisonError::into_inner).0
            }
            None => shared.wake.wait(queue).unwrap_or_else(PoisonError::into_inner),
        };
    }
}

fn post(job: &IconJob, icon: Option<Arc<IconPixels>>, name: Option<String>, clear_icon_on_none: bool) {
    if !job.alive.load(Ordering::Acquire) {
        return;
    }
    (job.sink)(
        job.device.clone(),
        SessionEvent(Event::Metadata { session: job.session, icon, name, clear_icon_on_none }),
    );
}

fn schedule_retry(shared: &Shared, mut job: IconJob, attempt: u32) {
    let interval = lock(&shared.state).retry_interval_ms;
    // Linear backoff: attempt 2 waits 1x the interval, attempt 3 2x, attempt 4 3x.
    let delay = Duration::from_millis(u64::from(interval) * u64::from(attempt - 1));
    job.kind = IconJobKind::Retry { attempt };
    let mut queue = lock(&shared.queue);
    if queue.shutdown {
        return;
    }
    queue.delayed.push((Instant::now() + delay, job));
}

fn process(shared: &Shared, job: IconJob) {
    if !job.alive.load(Ordering::Acquire) {
        return;
    }
    match job.kind {
        IconJobKind::Initial { resolve_name } => {
            let icon = acquire(shared, &job);
            let name = if resolve_name && !job.is_system_sounds && job.alive.load(Ordering::Acquire) {
                crate::process::display_name_for_process(job.pid)
            } else {
                None
            };
            let missing = icon.is_none();
            if icon.is_some() || name.is_some() {
                post(&job, icon, name, false);
            }
            if missing && !job.is_system_sounds && ICON_RETRY_ATTEMPTS >= 2 {
                schedule_retry(shared, job, 2);
            }
        }
        IconJobKind::Retry { attempt } => match acquire(shared, &job) {
            Some(icon) => post(&job, Some(icon), None, false),
            None if attempt < ICON_RETRY_ATTEMPTS => schedule_retry(shared, job, attempt + 1),
            None => {}
        },
        IconJobKind::Reresolve { name, icon } => {
            let resolved_name = if name {
                crate::host::read_session_display_name(&job.control.0)
                    .or_else(|| crate::process::display_name_for_process(job.pid))
            } else {
                None
            };
            let resolved_icon = if icon { acquire(shared, &job) } else { None };
            if resolved_name.is_some() || resolved_icon.is_some() {
                post(&job, resolved_icon, resolved_name, false);
            }
        }
        IconJobKind::IconPathChanged => {
            let icon = acquire(shared, &job);
            post(&job, icon, None, true);
        }
    }
}

/// `AppIconResolver.Acquire` for a queued session.
fn acquire(shared: &Shared, job: &IconJob) -> Option<Arc<IconPixels>> {
    if !job.alive.load(Ordering::Acquire) {
        return None;
    }
    let icon_path = if job.is_system_sounds { None } else { crate::host::read_icon_path(&job.control.0) };
    let (id, pixels) = resolve(shared, icon_path.as_deref(), job.pid, job.is_system_sounds)?;
    let mut state = lock(&shared.state);
    if state.entries.contains_key(&id) {
        state.by_app.insert(job.app_id.clone(), id);
    }
    Some(pixels)
}

fn resolve(
    shared: &Shared,
    session_icon_path: Option<&str>,
    pid: u32,
    is_system_sounds: bool,
) -> Option<(u64, Arc<IconPixels>)> {
    if is_system_sounds {
        if let Some(hit) = lock(&shared.state).hit(KEY_SYS) {
            return Some(hit);
        }
        let path = format!("{}\\{SYSTEM_SOUNDS_DLL}", system_directory()?);
        let raw = extract_from_pe_resource(&path, SYSTEM_SOUNDS_ICON_ORDINAL)?;
        return memoize_and_crop(shared, raw, KEY_SYS);
    }

    if is_packaged_process(pid)
        && let Some(aumid) = application_user_model_id(pid).filter(|a| !a.is_empty())
    {
        let canonical =
            if aumid.eq_ignore_ascii_case(CORTANA_BAD_AUMID) { CORTANA_GOOD_AUMID.to_owned() } else { aumid };
        let key = format!("{KEY_SHELL_UWP}|{canonical}");
        if let Some(hit) = lock(&shared.state).hit(&key) {
            return Some(hit);
        }
        if let Some(raw) = extract_from_shell(&canonical, true) {
            return memoize_and_crop(shared, raw, &key);
        }
    }

    let desktop_path = match session_icon_path.filter(|p| !p.trim().is_empty()) {
        Some(path) => expand_environment_variables(path.trim_start_matches('@')),
        None => image_path(pid).unwrap_or_default(),
    };
    if desktop_path.is_empty() {
        return None;
    }
    resolve_desktop(shared, &desktop_path)
}

fn resolve_desktop(shared: &Shared, path: &str) -> Option<(u64, Arc<IconPixels>)> {
    let (icon_file, icon_index) = parse_icon_location(path);
    if icon_index != 0 {
        let ordinal = icon_index.unsigned_abs();
        let key = format!("{KEY_PE}|{}|{ordinal}", normalize_path(&icon_file));
        if let Some(hit) = lock(&shared.state).hit(&key) {
            return Some(hit);
        }
        if let Ok(ordinal) = u16::try_from(ordinal)
            && let Some(raw) = extract_from_pe_resource(&icon_file, ordinal)
        {
            return memoize_and_crop(shared, raw, &key);
        }
    }

    let shell_path = match path.rfind(INVALID_ORDINAL_MARKER) {
        Some(position) => &path[..position],
        None => path,
    };
    let key = format!("{KEY_SHELL_DESKTOP}|{}", normalize_path(shell_path));
    if let Some(hit) = lock(&shared.state).hit(&key) {
        return Some(hit);
    }
    let raw = extract_from_shell(shell_path, false)?;
    memoize_and_crop(shared, raw, &key)
}

fn normalize_path(path: &str) -> String {
    match std::path::absolute(path) {
        Ok(full) => full.to_string_lossy().to_lowercase(),
        Err(_) => path.to_lowercase(),
    }
}

fn memoize_and_crop(shared: &Shared, raw: RawPixels, key: &str) -> Option<(u64, Arc<IconPixels>)> {
    let content_hash = hash_pixels(&raw);
    let mut state = lock(&shared.state);
    if let Some(hit) = state.hit(key) {
        return Some(hit);
    }
    if let Some(id) = state.by_content.get(&content_hash).copied() {
        state.alias(id, key);
        return state.touch(id).map(|pixels| (id, pixels));
    }

    let cropped = crop_transparent_border(raw);
    let pixels = Arc::new(to_rgba(cropped));
    state.next_id += 1;
    let id = state.next_id;
    state.tick += 1;
    let last_used = state.tick;
    state.entries.insert(id, Entry { pixels: pixels.clone(), content_hash, identity_keys: VecDeque::new(), last_used });
    state.alias(id, key);
    state.by_content.insert(content_hash, id);
    state.trim();
    Some((id, pixels))
}

fn to_rgba(raw: RawPixels) -> IconPixels {
    let mut rgba = raw.bgra;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    IconPixels { width: raw.width as u32, height: raw.height as u32, rgba }
}

/// FNV-1a over the BGRA bytes, then width and height (`HashPixels`).
fn hash_pixels(source: &RawPixels) -> u64 {
    const OFFSET: u64 = 14_695_981_039_346_656_037;
    const PRIME: u64 = 1_099_511_628_211;
    let mut hash = OFFSET;
    for byte in &source.bgra {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash ^= source.width as u64;
    hash = hash.wrapping_mul(PRIME);
    hash ^= source.height as u64;
    hash.wrapping_mul(PRIME)
}

/// `CropTransparentBorder`: crops a centred square around the opaque content when every side has
/// at least 10% transparent padding (runs of 2+ pixels above alpha 16 count as content).
fn crop_transparent_border(source: RawPixels) -> RawPixels {
    let (width, height) = (source.width, source.height);
    if width <= 0 || height <= 0 {
        return source;
    }
    let stride = (width * 4) as usize;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (width, height, -1, -1);
    for y in 0..height {
        let row = y as usize * stride;
        let mut run = 0;
        let (mut row_min_x, mut row_max_x) = (-1, -1);
        for x in 0..width {
            let alpha = source.bgra[row + x as usize * 4 + 3];
            if alpha > ALPHA_THRESHOLD {
                run += 1;
                if run < MIN_OPAQUE_RUN {
                    continue;
                }
                let first_in_run = x - run + 1;
                if row_min_x < 0 || first_in_run < row_min_x {
                    row_min_x = first_in_run;
                }
                row_max_x = x;
            } else {
                run = 0;
            }
        }
        if row_max_x < 0 {
            continue;
        }
        min_x = min_x.min(row_min_x);
        max_x = max_x.max(row_max_x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    if max_x < 0 {
        return source;
    }

    let min_pad = min_x.min(width - 1 - max_x).min(min_y.min(height - 1 - max_y));
    let pad_gate = (f64::from(width) * MIN_PAD_RATIO).ceil() as i32;
    if min_pad < pad_gate {
        return source;
    }

    let crop_w = max_x - min_x + 1;
    let crop_h = max_y - min_y + 1;
    let side = crop_w.max(crop_h);
    let cx = min_x + crop_w / 2;
    let cy = min_y + crop_h / 2;
    let half = side / 2;
    let sx = 0.max((width - side).min(cx - half));
    let sy = 0.max((height - side).min(cy - half));
    let sw = side.min(width - sx);
    let sh = side.min(height - sy);

    let row_bytes = sw as usize * 4;
    let mut cropped = Vec::with_capacity(row_bytes * sh as usize);
    for y in 0..sh {
        let start = (sy + y) as usize * stride + sx as usize * 4;
        cropped.extend_from_slice(&source.bgra[start..start + row_bytes]);
    }
    RawPixels { width: sw, height: sh, bgra: cropped }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

fn system_directory() -> Option<String> {
    let mut buffer = [0u16; MAX_PATH as usize];
    // SAFETY: the buffer length is passed through the slice.
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    (length > 0 && length < buffer.len()).then(|| String::from_utf16_lossy(&buffer[..length]))
}

fn expand_environment_variables(text: &str) -> String {
    let source = HSTRING::from(text);
    // SAFETY: a None destination asks for the required size.
    let needed = unsafe { ExpandEnvironmentStringsW(&source, None) } as usize;
    if needed == 0 {
        return text.to_owned();
    }
    let mut buffer = vec![0u16; needed];
    // SAFETY: the buffer holds `needed` units as the first call reported.
    let written = unsafe { ExpandEnvironmentStringsW(&source, Some(&mut buffer)) } as usize;
    if written == 0 || written > buffer.len() { text.to_owned() } else { from_wide(&buffer) }
}

/// `PathParseIconLocationW`: splits `"file,index"` into the file and the index.
fn parse_icon_location(path: &str) -> (String, i32) {
    let mut buffer = wide(path);
    if buffer.len() < MAX_PATH as usize {
        buffer.resize(MAX_PATH as usize, 0);
    }
    // SAFETY: NUL-terminated writable buffer of at least MAX_PATH units.
    let index = unsafe { PathParseIconLocationW(PWSTR(buffer.as_mut_ptr())) };
    (from_wide(&buffer), index)
}

fn is_packaged_process(pid: u32) -> bool {
    let Ok(process) = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else { return false };
    let mut length = 0u32;
    // SAFETY: a zero-length query only reports whether the process has a package identity.
    let result = unsafe { GetPackageId(process.raw(), &mut length, None) };
    result == ERROR_INSUFFICIENT_BUFFER
}

fn application_user_model_id(pid: u32) -> Option<String> {
    let process = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    let mut buffer = vec![0u16; MAX_AUMID_LEN as usize];
    let mut length = MAX_AUMID_LEN;
    // SAFETY: `buffer` holds `length` units.
    let result = unsafe { GetApplicationUserModelId(process.raw(), &mut length, Some(PWSTR(buffer.as_mut_ptr()))) };
    (result == ERROR_SUCCESS).then(|| from_wide(&buffer))
}

/// Loads `RT_GROUP_ICON` `ordinal` from a PE file as data and reads the best 48 px image.
fn extract_from_pe_resource(path: &str, ordinal: u16) -> Option<RawPixels> {
    let file = HSTRING::from(path);
    // SAFETY: loads the image as a resource-only data file; freed below.
    let module =
        unsafe { LoadLibraryExW(&file, None, LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE) }.ok()?;
    let result = (|| {
        // SAFETY: `module` is a loaded resource module; MAKEINTRESOURCE ordinals are passed as
        // pointer-sized integers; resource pointers stay valid while the module is loaded.
        unsafe {
            let group = FindResourceW(Some(module), PCWSTR(usize::from(ordinal) as *const u16), RT_GROUP_ICON);
            if group.is_invalid() {
                return None;
            }
            let group_data = LockResource(LoadResource(Some(module), group).ok()?);
            if group_data.is_null() {
                return None;
            }
            let icon_id = LookupIconIdFromDirectoryEx(group_data.cast(), true, ICON_SIZE, ICON_SIZE, LR_DEFAULTCOLOR);
            if icon_id <= 0 {
                return None;
            }
            let icon = FindResourceW(Some(module), PCWSTR(icon_id as usize as *const u16), RT_ICON);
            if icon.is_invalid() {
                return None;
            }
            let icon_data = LockResource(LoadResource(Some(module), icon).ok()?);
            let icon_size = SizeofResource(Some(module), icon);
            if icon_data.is_null() || icon_size == 0 {
                return None;
            }
            let bits = std::slice::from_raw_parts(icon_data.cast::<u8>(), icon_size as usize);
            let handle =
                CreateIconFromResourceEx(bits, true, ICON_CURSOR_VERSION, ICON_SIZE, ICON_SIZE, LR_DEFAULTCOLOR)
                    .ok()?;
            let pixels = read_icon_pixels(handle);
            let _ = DestroyIcon(handle);
            pixels
        }
    })();
    // SAFETY: matches the LoadLibraryExW above.
    let _ = unsafe { FreeLibrary(module) };
    result
}

fn extract_from_shell(path: &str, is_uwp: bool) -> Option<RawPixels> {
    let canonical = if is_uwp && path.eq_ignore_ascii_case(CORTANA_BAD_AUMID) { CORTANA_GOOD_AUMID } else { path };
    if !is_uwp && let Some(icon) = extract_with_shell_icon_api(canonical) {
        return Some(icon);
    }
    extract_shell_image(canonical, is_uwp)
}

fn extract_with_shell_icon_api(path: &str) -> Option<RawPixels> {
    let (parsed, index) = parse_icon_location(path);
    let icon_file = if parsed.is_empty() { path.to_owned() } else { parsed };
    if !std::path::Path::new(&icon_file).is_file() {
        return None;
    }
    let file = HSTRING::from(icon_file.as_str());
    let mut large = HICON::default();
    let mut small = HICON::default();
    let sizes = ((ICON_SIZE as u32) << 16) | ICON_SIZE as u32;
    // SAFETY: out pointers are valid; returned icons are destroyed below.
    let result = unsafe { SHDefExtractIconW(&file, index, 0, Some(&mut large), Some(&mut small), sizes) };
    let pixels = if result.is_ok() { read_icon_pixels(if large.is_invalid() { small } else { large }) } else { None };
    for icon in [large, small] {
        if !icon.is_invalid() {
            // SAFETY: icon handles returned by SHDefExtractIconW, destroyed once.
            let _ = unsafe { DestroyIcon(icon) };
        }
    }
    pixels
}

/// `TryExtractShellImage`: an AUMID through the Apps known folder (falling back to the parsing
/// name), or a file path through its parsing name.
fn extract_shell_image(identity: &str, is_application_id: bool) -> Option<RawPixels> {
    if identity.trim().is_empty() {
        return None;
    }
    let name = HSTRING::from(identity);
    // SAFETY: plain shell item creation with valid strings.
    let factory: IShellItemImageFactory = unsafe {
        if is_application_id {
            SHCreateItemInKnownFolder(&FOLDERID_AppsFolder, KF_FLAG_DONT_VERIFY, &name)
                .or_else(|_| SHCreateItemFromParsingName(&name, None::<&IBindCtx>))
        } else {
            SHCreateItemFromParsingName(&name, None::<&IBindCtx>)
        }
    }
    .ok()?;
    // SAFETY: valid factory; the returned bitmap is deleted below.
    let bitmap = unsafe { factory.GetImage(SIZE { cx: ICON_SIZE, cy: ICON_SIZE }, SIIGBF_RESIZETOFIT) }.ok()?;
    let pixels = read_bitmap_pixels(bitmap, MAXIMUM_DIMENSION);
    // SAFETY: the bitmap belongs to us.
    let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
    pixels
}

/// `TryReadIconPixels`: the colour bitmap (alpha from the AND mask when it carries none), or the
/// monochrome mask pair when the icon has no colour bitmap.
fn read_icon_pixels(icon: HICON) -> Option<RawPixels> {
    if icon.is_invalid() {
        return None;
    }
    let mut info = ICONINFO::default();
    // SAFETY: valid icon handle and out struct; the returned bitmaps are deleted below.
    unsafe { GetIconInfo(icon, &mut info) }.ok()?;
    let result = if info.hbmColor.is_invalid() {
        if info.hbmMask.is_invalid() {
            None
        } else {
            read_bitmap_pixels(info.hbmMask, MAXIMUM_DIMENSION * 2).and_then(convert_monochrome_mask)
        }
    } else {
        match read_bitmap_pixels(info.hbmColor, MAXIMUM_DIMENSION) {
            Some(mut color) if !info.hbmMask.is_invalid() && !has_any_alpha(&color.bgra) => {
                match read_bitmap_pixels(info.hbmMask, MAXIMUM_DIMENSION * 2) {
                    Some(mask) if mask.width == color.width && mask.height >= color.height => {
                        apply_mask_alpha(&mut color.bgra, &mask.bgra);
                    }
                    _ => set_opaque(&mut color.bgra),
                }
                Some(color)
            }
            other => other,
        }
    };
    for bitmap in [info.hbmColor, info.hbmMask] {
        if !bitmap.is_invalid() {
            // SAFETY: GetIconInfo handed us ownership of these bitmaps.
            let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
        }
    }
    result
}

/// `TryReadBitmapPixels`: an HBITMAP as 32-bit top-down BGRA.
fn read_bitmap_pixels(bitmap: HBITMAP, maximum_dimension: i32) -> Option<RawPixels> {
    if bitmap.is_invalid() {
        return None;
    }
    let mut info = BITMAP::default();
    // SAFETY: `info` is a BITMAP of the size passed.
    let read =
        unsafe { GetObjectW(HGDIOBJ(bitmap.0), size_of::<BITMAP>() as i32, Some((&raw mut info).cast::<c_void>())) };
    if read == 0 {
        return None;
    }
    let width = info.bmWidth;
    let height = info.bmHeight.abs();
    if width <= 0 || height <= 0 || width > maximum_dimension || height > maximum_dimension {
        return None;
    }
    let mut pixels = vec![0u8; width as usize * 4 * height as usize];
    // SAFETY: screen DC, released below.
    let dc = unsafe { GetDC(None) };
    if dc.is_invalid() {
        return None;
    }
    let mut format = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: pixels.len() as u32,
            ..BITMAPINFOHEADER::default()
        },
        ..BITMAPINFO::default()
    };
    // SAFETY: `pixels` holds width*height 32-bit pixels as the header describes.
    let lines = unsafe {
        GetDIBits(dc, bitmap, 0, height as u32, Some(pixels.as_mut_ptr().cast()), &mut format, DIB_RGB_COLORS)
    };
    // SAFETY: matches GetDC above.
    unsafe { ReleaseDC(None, dc) };
    (lines != 0).then_some(RawPixels { width, height, bgra: pixels })
}

fn has_any_alpha(bgra: &[u8]) -> bool {
    bgra.chunks_exact(4).any(|pixel| pixel[3] != 0)
}

/// Black AND-mask pixels are opaque, white ones transparent.
fn apply_mask_alpha(color: &mut [u8], mask: &[u8]) {
    for (pixel, mask) in color.chunks_exact_mut(4).zip(mask.chunks_exact(4)) {
        let transparent = mask[0] != 0 || mask[1] != 0 || mask[2] != 0;
        pixel[3] = if transparent { 0 } else { 255 };
    }
}

fn set_opaque(bgra: &mut [u8]) {
    for pixel in bgra.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
}

/// A monochrome icon: AND mask stacked over the XOR mask.
fn convert_monochrome_mask(pair: RawPixels) -> Option<RawPixels> {
    if pair.height < 2 || pair.height % 2 != 0 {
        return None;
    }
    let height = pair.height / 2;
    let half = pair.width as usize * 4 * height as usize;
    let mut pixels = vec![0u8; half];
    for offset in (0..half).step_by(4) {
        let transparent = pair.bgra[offset] != 0;
        let color = if pair.bgra[half + offset] != 0 { 255 } else { 0 };
        pixels[offset] = color;
        pixels[offset + 1] = color;
        pixels[offset + 2] = color;
        pixels[offset + 3] = if transparent { 0 } else { 255 };
    }
    Some(RawPixels { width: pair.width, height, bgra: pixels })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: i32, opaque: std::ops::Range<i32>) -> RawPixels {
        let mut bgra = vec![0u8; (size * size * 4) as usize];
        for y in opaque.clone() {
            for x in opaque.clone() {
                let i = ((y * size + x) * 4) as usize;
                bgra[i..i + 4].copy_from_slice(&[1, 2, 3, 255]);
            }
        }
        RawPixels { width: size, height: size, bgra }
    }

    #[test]
    fn crop_trims_padded_icons_to_a_centred_square() {
        let cropped = crop_transparent_border(square(48, 10..38));
        assert_eq!((cropped.width, cropped.height), (28, 28));
        assert!(cropped.bgra.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn crop_keeps_icons_without_enough_padding() {
        let cropped = crop_transparent_border(square(48, 3..45));
        assert_eq!((cropped.width, cropped.height), (48, 48));
        let empty = crop_transparent_border(square(48, 0..0));
        assert_eq!((empty.width, empty.height), (48, 48));
    }

    #[test]
    fn rgba_conversion_swaps_red_and_blue() {
        let pixels = to_rgba(RawPixels { width: 1, height: 1, bgra: vec![1, 2, 3, 4] });
        assert_eq!(pixels.rgba, [3, 2, 1, 4]);
    }

    #[test]
    fn mask_helpers_derive_alpha() {
        let mut color = vec![9, 9, 9, 0, 9, 9, 9, 0];
        apply_mask_alpha(&mut color, &[0, 0, 0, 0, 255, 255, 255, 0]);
        assert_eq!(color, [9, 9, 9, 255, 9, 9, 9, 0]);
        let pair = RawPixels { width: 1, height: 2, bgra: vec![0, 0, 0, 0, 255, 255, 255, 0] };
        let mono = convert_monochrome_mask(pair).map(|p| p.bgra);
        assert_eq!(mono, Some(vec![255, 255, 255, 255]));
    }

    #[test]
    fn lru_keeps_only_the_most_recent_unused_entries() {
        let mut state = CacheState { lru_limit: 1, ..CacheState::default() };
        let mut held = Vec::new();
        for id in 1..=3u64 {
            state.entries.insert(
                id,
                Entry {
                    pixels: Arc::new(IconPixels { width: 1, height: 1, rgba: vec![id as u8; 4] }),
                    content_hash: id,
                    identity_keys: VecDeque::from([format!("k{id}")]),
                    last_used: id,
                },
            );
            state.by_identity.insert(format!("k{id}"), id);
            state.by_content.insert(id, id);
        }
        // Entry 3 is in use; of the unused 1 and 2 only the most recent (2) stays.
        held.push(state.entries[&3].pixels.clone());
        state.trim();
        let mut ids: Vec<u64> = state.entries.keys().copied().collect();
        ids.sort_unstable();
        assert_eq!(ids, [2, 3]);
        assert!(!state.by_identity.contains_key("k1"));
        assert!(!state.by_content.contains_key(&1));
        drop(held);
        state.lru_limit = 0;
        state.trim();
        assert!(state.entries.is_empty());
    }
}
