//! A2DP codec listener (ports of `Audio/BluetoothCodecMonitor.cs` and `Audio/BluetoothCodec.cs`).
//!
//! Microsoft's Bluetooth stack publishes the negotiated codec on every AVDTP SET_CONFIGURATION /
//! RECONFIGURE through the ETW provider `Microsoft.Windows.Bluetooth.BthA2dp` (event
//! `A2dpStreaming`, payload fields 3/4/5 = standard codec id, vendor id, vendor codec id). A
//! realtime session needs Administrator (or Performance Log Users); without it the monitor logs
//! once and stays inert. The session is a kernel object that outlives a crashed process, so a
//! start first stops a stale session of the same name.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError, mpsc};
use std::time::Duration;

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Diagnostics::Etw::{
    CONTROLTRACE_HANDLE, CloseTrace, ControlTraceW, EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_PROPERTY_INFO,
    EVENT_RECORD, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE,
    EnableTraceEx2, OpenTraceW, PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_REAL_TIME, PROCESSTRACE_HANDLE,
    PROPERTY_DATA_DESCRIPTOR, ProcessTrace, StartTraceW, TRACE_EVENT_INFO, TRACE_LEVEL_VERBOSE, TdhGetEventInformation,
    TdhGetProperty, TdhGetPropertySize, WNODE_FLAG_TRACED_GUID,
};
use windows_core::{GUID, PCWSTR, PWSTR};

use super::{BtEvent, BtEventKind, BtSink};
use crate::util::{is_process_elevated, wide};

/// Kernel session name. The .NET app uses "VolumeTrayAppDotNET-BthA2dp"; this build uses its
/// own name so the orphan cleanup never stops the .NET app's live session when both run.
const SESSION_NAME: &str = "VolumeTrayAppDotNETSlint-BthA2dp";
/// `Microsoft.Windows.Bluetooth.BthA2dp`.
const PROVIDER_GUID: GUID = GUID::from_u128(0x8776ad1e_5022_4451_a566_f47e708b9075);
/// Legacy provider from the Win8.1 BT-diagnostics KB; enabled alongside (no-op when absent).
const LEGACY_PROVIDER_GUID: GUID = GUID::from_u128(0xddb6da39_08a7_4579_8d0c_68011146e205);
const STREAMING_EVENT_NAME: &str = "A2dpStreaming";
const FIELD_STANDARD_CODEC_ID: usize = 3;
const FIELD_VENDOR_ID: usize = 4;
const FIELD_VENDOR_CODEC_ID: usize = 5;
/// `BluetoothCodecWorkerJoinTimeoutMs`.
const BLUETOOTH_CODEC_WORKER_JOIN_TIMEOUT: Duration = Duration::from_millis(2_000);

/// One A2DP codec choice (`BluetoothCodec`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BluetoothCodec {
    pub standard_codec_id: u8,
    pub vendor_id: i32,
    pub vendor_codec_id: i32,
}

impl BluetoothCodec {
    /// Display name ("SBC", "AAC", "Sony LDAC", ...); unknown pairs show their raw ids with the
    /// exact `Audio_BluetoothCodecUnknown*Format` strings.
    pub fn friendly_name(&self) -> String {
        match self.standard_codec_id {
            0x00 => return "SBC".into(),
            0x01 => return "MP3".into(),
            0x02 => return "AAC".into(),
            0x04 => return "ATRAC".into(),
            0xFF => {}
            standard => {
                // "Unknown codec (invalid vendor): 0x{0:X2} {1}:{2}"
                return format!(
                    "Unknown codec (invalid vendor): 0x{standard:02X} {}:{}",
                    self.vendor_id, self.vendor_codec_id
                );
            }
        }
        let known = match (self.vendor_id, self.vendor_codec_id) {
            (0x004F, 0x0001) => "Qualcomm/CSR aptX",
            (0x00D7, 0x0024) => "Qualcomm/CSR aptX HD",
            (0x00D7, 0x0002) | (0x000A, 0x0002) => "Qualcomm/CSR aptX LL",
            (0x000A, 0x0001) => "Qualcomm/CSR FastStream",
            (0x000A, 0x0104) => "Qualcomm/CSR TWS v3 AAC",
            (0x000A, 0x0105) => "Qualcomm/CSR TWS v3 MP3",
            (0x000A, 0x0106) => "Qualcomm/CSR TWS v3 aptX",
            (0x00D7, 0x00AD) => "Qualcomm/CSR aptX Adaptive",
            (0x012D, 0x00AA) => "Sony LDAC",
            (0x0075, 0x0102) => "Samsung HD",
            (0x0075, 0x0103) => "Samsung Scalable",
            (0x0075, 0x0104) => "Samsung UHQ",
            (0x053A, 0x484C) => "Savitech LHDC",
            // "Unknown codec: 0x{0:X4}:0x{1:X4}" (.NET X4 of a negative int prints all 8 digits).
            (vendor, codec) => return format!("Unknown codec: 0x{vendor:04X}:0x{codec:04X}"),
        };
        known.into()
    }
}

/// Whether an event stamped with `event_generation` belongs to the live session.
pub(crate) fn is_current_generation(event_generation: u64, current_generation: u64) -> bool {
    event_generation == current_generation
}

/// Signals completion of one session retirement.
#[derive(Default)]
struct DoneFlag {
    done: Mutex<bool>,
    signal: Condvar,
}

impl DoneFlag {
    fn set(&self) {
        *self.done.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.signal.notify_all();
    }

    fn is_set(&self) -> bool {
        *self.done.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits up to `timeout`; true when done.
    fn wait(&self, timeout: Duration) -> bool {
        let guard = self.done.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) =
            self.signal.wait_timeout_while(guard, timeout, |done| !*done).unwrap_or_else(PoisonError::into_inner);
        *guard
    }
}

/// Live realtime session.
struct Session {
    control: CONTROLTRACE_HANDLE,
    /// Consumer handle published by the worker once `OpenTraceW` succeeded.
    process: Arc<Mutex<Option<PROCESSTRACE_HANDLE>>>,
    worker_done: mpsc::Receiver<()>,
}

/// Monitor state, owned by the coordinator on the audio thread.
pub(crate) struct CodecMonitor {
    sink: BtSink,
    /// Incremented by every start and stop; events of an earlier session are dropped.
    generation: u64,
    live_generation: Arc<AtomicU64>,
    session: Option<Session>,
    retirement: Option<Arc<DoneFlag>>,
    start_after_retirement: bool,
    elevation_missing_logged: bool,
    current: Option<BluetoothCodec>,
}

impl CodecMonitor {
    pub(crate) fn new(sink: BtSink) -> Self {
        Self {
            sink,
            generation: 0,
            live_generation: Arc::new(AtomicU64::new(0)),
            session: None,
            retirement: None,
            start_after_retirement: false,
            elevation_missing_logged: false,
            current: None,
        }
    }

    pub(crate) fn current(&self) -> Option<BluetoothCodec> {
        self.current
    }

    fn bump_generation(&mut self) -> u64 {
        self.generation += 1;
        self.live_generation.store(self.generation, Ordering::Release);
        self.generation
    }

    /// Opens the realtime session (idempotent). While a retirement still holds the session name,
    /// the start is deferred until it finished.
    pub(crate) fn start(&mut self) {
        if self.session.is_some() {
            return;
        }
        if self.retirement.as_ref().is_some_and(|r| !r.is_set()) {
            self.start_after_retirement = true;
            return;
        }
        if !is_process_elevated() {
            if !self.elevation_missing_logged {
                log::info!("BluetoothCodecMonitor.Start: requires admin / Performance Log Users; staying inert.");
            }
            self.elevation_missing_logged = true;
            return;
        }
        stop_orphaned_session();
        let generation = self.bump_generation();
        match open_session(generation, Arc::clone(&self.live_generation), Arc::clone(&self.sink)) {
            Ok(session) => {
                self.session = Some(session);
                log::info!(
                    "BluetoothCodecMonitor.Start: session opened on provider Microsoft.Windows.Bluetooth.BthA2dp"
                );
            }
            Err(error) => log::warn!("BluetoothCodecMonitor.Start: failed: {error}"),
        }
    }

    /// Retires the session at once (late events are dropped by generation) and stops it on a
    /// worker; clears the cached codec. Returns true when the cached codec changed.
    pub(crate) fn stop(&mut self) -> bool {
        self.start_after_retirement = false;
        let Some(session) = self.session.take() else { return false };
        self.bump_generation();
        let previous = self.retirement.take();
        let done = Arc::new(DoneFlag::default());
        let thread_done = Arc::clone(&done);
        let sink = Arc::clone(&self.sink);
        let spawned = std::thread::Builder::new().name("BluetoothCodecMonitor.Retire".into()).spawn(move || {
            if let Some(previous) = previous {
                previous.wait(Duration::MAX);
            }
            retire_session(session);
            thread_done.set();
            sink(BtEvent(BtEventKind::CodecRetired));
        });
        if let Err(error) = spawned {
            log::warn!("BluetoothCodecMonitor.Stop: retirement thread failed to start: {error}");
            done.set();
        }
        self.retirement = Some(done);
        self.reset()
    }

    /// A retirement finished: run a deferred start.
    pub(crate) fn on_retired(&mut self) {
        if self.start_after_retirement && self.retirement.as_ref().is_none_or(|r| r.is_set()) {
            self.start_after_retirement = false;
            self.start();
        }
    }

    /// Clears the cached codec (last Bluetooth render endpoint went away). True when it changed.
    pub(crate) fn reset(&mut self) -> bool {
        self.current.take().is_some()
    }

    /// A codec event from the worker; true when the cached codec changed.
    pub(crate) fn apply(&mut self, generation: u64, codec: BluetoothCodec) -> bool {
        if !is_current_generation(generation, self.generation) || self.current == Some(codec) {
            return false;
        }
        self.current = Some(codec);
        true
    }

    /// App exit: stop and wait (bounded) so the kernel session does not outlive the process.
    pub(crate) fn shutdown(&mut self) {
        self.stop();
        if let Some(retirement) = self.retirement.take()
            && !retirement.wait(BLUETOOTH_CODEC_WORKER_JOIN_TIMEOUT * 2)
        {
            log::warn!("BluetoothCodecMonitor.Dispose: the session did not retire in time.");
        }
    }
}

/// `EVENT_TRACE_PROPERTIES` followed by room for the session name, 8-byte aligned.
struct TraceProperties {
    buffer: Vec<u64>,
}

impl TraceProperties {
    fn new() -> Self {
        let name_bytes = (SESSION_NAME.len() + 1) * 2;
        let total = size_of::<EVENT_TRACE_PROPERTIES>() + name_bytes + 1024;
        let words = total.div_ceil(8);
        let mut this = Self { buffer: vec![0u64; words] };
        let properties = this.properties();
        properties.Wnode.BufferSize = (words * 8) as u32;
        properties.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        properties.Wnode.ClientContext = 1; // QPC timestamps
        properties.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        properties.LoggerNameOffset = size_of::<EVENT_TRACE_PROPERTIES>() as u32;
        this
    }

    fn properties(&mut self) -> &mut EVENT_TRACE_PROPERTIES {
        // SAFETY: the buffer is 8-byte aligned and larger than EVENT_TRACE_PROPERTIES.
        unsafe { &mut *self.buffer.as_mut_ptr().cast::<EVENT_TRACE_PROPERTIES>() }
    }
}

fn stop_session_by_name() -> WIN32_ERROR {
    let name = wide(SESSION_NAME);
    let mut properties = TraceProperties::new();
    // SAFETY: valid properties buffer with room for the logger name; stop by name.
    unsafe {
        ControlTraceW(
            CONTROLTRACE_HANDLE::default(),
            PCWSTR(name.as_ptr()),
            properties.properties(),
            EVENT_TRACE_CONTROL_STOP,
        )
    }
}

/// Stops a stale session of a crashed earlier run (realtime sessions live in the kernel).
fn stop_orphaned_session() {
    if stop_session_by_name() == ERROR_SUCCESS {
        log::info!("BluetoothCodecMonitor: stopped orphaned ETW session {SESSION_NAME}");
    }
}

fn open_session(generation: u64, live: Arc<AtomicU64>, sink: BtSink) -> Result<Session, String> {
    let name = wide(SESSION_NAME);
    let mut properties = TraceProperties::new();
    let mut control = CONTROLTRACE_HANDLE::default();
    // SAFETY: valid properties buffer, name and out-handle.
    let status = unsafe { StartTraceW(&mut control, PCWSTR(name.as_ptr()), properties.properties()) };
    if status == ERROR_ALREADY_EXISTS {
        return Err("ETW session already exists".into());
    }
    if status != ERROR_SUCCESS {
        return Err(format!("StartTraceW failed with {}", status.0));
    }
    // matchAnyKeyword = 0 selects every event on the provider.
    // SAFETY: live session handle and provider GUID.
    let status = unsafe {
        EnableTraceEx2(
            control,
            &PROVIDER_GUID,
            EVENT_CONTROL_CODE_ENABLE_PROVIDER.0,
            TRACE_LEVEL_VERBOSE as u8,
            0,
            0,
            0,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        let _ = stop_session_by_name();
        return Err(format!("EnableTraceEx2 failed with {}", status.0));
    }
    // SAFETY: live session handle and provider GUID.
    let status = unsafe {
        EnableTraceEx2(
            control,
            &LEGACY_PROVIDER_GUID,
            EVENT_CONTROL_CODE_ENABLE_PROVIDER.0,
            TRACE_LEVEL_VERBOSE as u8,
            0,
            0,
            0,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        log::info!("BluetoothCodecMonitor.Start: legacy provider enable: error {}", status.0);
    }

    let process = Arc::new(Mutex::new(None));
    let worker_process = Arc::clone(&process);
    let (done_tx, worker_done) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("BluetoothCodecMonitor.ETW".into()).spawn(move || {
        worker_loop(generation, live, sink, &worker_process);
        let _ = done_tx.send(());
    });
    if let Err(error) = spawned {
        let _ = stop_session_by_name();
        return Err(format!("worker failed to start: {error}"));
    }
    Ok(Session { control, process, worker_done })
}

/// Stops the session (which makes `ProcessTrace` return) and joins the worker, bounded.
fn retire_session(session: Session) {
    let mut properties = TraceProperties::new();
    // SAFETY: stops our own session through its control handle.
    let status =
        unsafe { ControlTraceW(session.control, PCWSTR::null(), properties.properties(), EVENT_TRACE_CONTROL_STOP) };
    if status != ERROR_SUCCESS {
        log::warn!("BluetoothCodecMonitor.Stop: ControlTrace(stop) failed with {}", status.0);
    }
    if let Some(handle) = session.process.lock().unwrap_or_else(PoisonError::into_inner).take() {
        // SAFETY: closes the consumer handle opened by the worker; ProcessTrace then returns.
        let _ = unsafe { CloseTrace(handle) };
    }
    if session.worker_done.recv_timeout(BLUETOOTH_CODEC_WORKER_JOIN_TIMEOUT).is_err() {
        log::warn!("BluetoothCodecMonitor.Stop: the ETW worker did not stop in time.");
    }
    log::info!("BluetoothCodecMonitor.Stop: session closed.");
}

/// Context handed to the ETW callback through `EVENT_TRACE_LOGFILEW.Context`.
struct CallbackContext {
    generation: u64,
    live: Arc<AtomicU64>,
    sink: BtSink,
}

fn worker_loop(generation: u64, live: Arc<AtomicU64>, sink: BtSink, process: &Mutex<Option<PROCESSTRACE_HANDLE>>) {
    let context = Box::new(CallbackContext { generation, live, sink });
    let mut name = wide(SESSION_NAME);
    let mut logfile = EVENT_TRACE_LOGFILEW { LoggerName: PWSTR(name.as_mut_ptr()), ..Default::default() };
    logfile.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
    logfile.Anonymous2.EventRecordCallback = Some(on_event_record);
    logfile.Context = std::ptr::from_ref::<CallbackContext>(&context).cast_mut().cast();
    // SAFETY: `logfile` and `context` outlive ProcessTrace, which runs on this thread.
    let handle = unsafe { OpenTraceW(&mut logfile) };
    if handle.Value == u64::MAX {
        log::warn!("BluetoothCodecMonitor.WorkerLoop: OpenTrace failed: {}", windows_core::Error::from_thread());
        return;
    }
    *process.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
    // Blocks until the session stops or the handle is closed.
    // SAFETY: valid consumer handle; the callback context is alive for the whole call.
    let status = unsafe { ProcessTrace(&[handle], None, None) };
    if status != ERROR_SUCCESS && status.0 != 1223 {
        log::warn!("BluetoothCodecMonitor.WorkerLoop: ProcessTrace returned {}", status.0);
    }
    if let Some(handle) = process.lock().unwrap_or_else(PoisonError::into_inner).take() {
        // SAFETY: closes the consumer handle if the retirement did not already.
        let _ = unsafe { CloseTrace(handle) };
    }
    drop(context);
}

/// `PEVENT_RECORD_CALLBACK`.
unsafe extern "system" fn on_event_record(record: *mut EVENT_RECORD) {
    // SAFETY: ETW passes a valid record for the duration of the callback; UserContext is the
    // `CallbackContext` owned by the worker thread that is running ProcessTrace.
    let (record, context) = unsafe {
        let Some(record) = record.as_ref() else { return };
        let Some(context) = record.UserContext.cast::<CallbackContext>().as_ref() else { return };
        (record, context)
    };
    if !is_current_generation(context.generation, context.live.load(Ordering::Acquire)) {
        return;
    }
    // TraceEvent matches provider *name* and event name; only the modern provider carries it.
    if record.EventHeader.ProviderId != PROVIDER_GUID {
        return;
    }
    let Some(info) = EventInfo::read(record) else { return };
    if !info.is_streaming_event() {
        return;
    }
    info.log_schema_once(record);
    let codec = (|| {
        let standard = u8::try_from(info.integer_property(record, FIELD_STANDARD_CODEC_ID)?).ok()?;
        let vendor = i32::try_from(info.integer_property(record, FIELD_VENDOR_ID)?).ok()?;
        let vendor_codec = i32::try_from(info.integer_property(record, FIELD_VENDOR_CODEC_ID)?).ok()?;
        Some(BluetoothCodec { standard_codec_id: standard, vendor_id: vendor, vendor_codec_id: vendor_codec })
    })();
    let Some(codec) = codec else {
        log::warn!("BluetoothCodecMonitor.OnStreamingEvent: unreadable codec payload");
        return;
    };
    log::info!(
        "BluetoothCodecMonitor: A2DP codec = {} (std=0x{:02X} vendor=0x{:04X} codec=0x{:04X})",
        codec.friendly_name(),
        codec.standard_codec_id,
        codec.vendor_id,
        codec.vendor_codec_id
    );
    (context.sink)(BtEvent(BtEventKind::Codec { generation: context.generation, codec }));
}

/// `TRACE_EVENT_INFO` buffer for one record.
struct EventInfo {
    buffer: Vec<u64>,
}

static SCHEMA_LOGGED: AtomicBool = AtomicBool::new(false);

impl EventInfo {
    fn read(record: &EVENT_RECORD) -> Option<Self> {
        let mut size = 0u32;
        // SAFETY: size query for a valid record.
        let status = unsafe { TdhGetEventInformation(record, None, None, &mut size) };
        if status != ERROR_INSUFFICIENT_BUFFER.0 || size == 0 {
            return None;
        }
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: `buffer` is at least `size` bytes and 8-byte aligned.
        let status = unsafe { TdhGetEventInformation(record, None, Some(buffer.as_mut_ptr().cast()), &mut size) };
        (status == ERROR_SUCCESS.0).then_some(Self { buffer })
    }

    fn info(&self) -> &TRACE_EVENT_INFO {
        // SAFETY: filled by TdhGetEventInformation; aligned and large enough.
        unsafe { &*self.buffer.as_ptr().cast::<TRACE_EVENT_INFO>() }
    }

    fn base(&self) -> *const u8 {
        self.buffer.as_ptr().cast()
    }

    fn string_at(&self, offset: u32) -> Option<String> {
        if offset == 0 || offset as usize >= self.buffer.len() * 8 {
            return None;
        }
        let max_units = (self.buffer.len() * 8 - offset as usize) / 2;
        // SAFETY: the offset lies inside the buffer; the string is NUL-terminated within it.
        let units = unsafe { std::slice::from_raw_parts(self.base().add(offset as usize).cast::<u16>(), max_units) };
        let end = units.iter().position(|&c| c == 0)?;
        Some(String::from_utf16_lossy(&units[..end]))
    }

    /// TraceEvent's event name: the TraceLogging event name, else the task name for an
    /// opcode-0/"Info" event (other opcodes are named "Task/Opcode").
    fn is_streaming_event(&self) -> bool {
        let info = self.info();
        // SAFETY: union field read as the documented EventNameOffset.
        let event_name = self.string_at(unsafe { info.Anonymous1.EventNameOffset });
        if let Some(name) = event_name.filter(|n| !n.is_empty()) {
            return name.trim() == STREAMING_EVENT_NAME;
        }
        let task = self.string_at(info.TaskNameOffset).map(|t| t.trim().to_owned());
        let opcode = self.string_at(info.OpcodeNameOffset).map(|o| o.trim().to_owned()).unwrap_or_default();
        let opcode_is_info =
            info.EventDescriptor.Opcode == 0 || opcode.is_empty() || opcode.eq_ignore_ascii_case("Info");
        task.as_deref() == Some(STREAMING_EVENT_NAME) && opcode_is_info
    }

    fn property(&self, index: usize) -> Option<&EVENT_PROPERTY_INFO> {
        let info = self.info();
        if index >= info.TopLevelPropertyCount as usize {
            return None;
        }
        let array = std::mem::offset_of!(TRACE_EVENT_INFO, EventPropertyInfoArray);
        let offset = array + index * size_of::<EVENT_PROPERTY_INFO>();
        if offset + size_of::<EVENT_PROPERTY_INFO>() > self.buffer.len() * 8 {
            return None;
        }
        // SAFETY: bounds checked above; the array is laid out contiguously by TDH.
        Some(unsafe { &*self.base().add(offset).cast::<EVENT_PROPERTY_INFO>() })
    }

    fn property_name(&self, index: usize) -> Option<String> {
        self.string_at(self.property(index)?.NameOffset)
    }

    /// Top-level property `index` as an unsigned little-endian integer (1/2/4/8 bytes).
    fn integer_property(&self, record: &EVENT_RECORD, index: usize) -> Option<u64> {
        let property = self.property(index)?;
        if property.NameOffset == 0 {
            return None;
        }
        let descriptor = [PROPERTY_DATA_DESCRIPTOR {
            // SAFETY: NameOffset points at a NUL-terminated name inside `buffer`.
            PropertyName: unsafe { self.base().add(property.NameOffset as usize) } as u64,
            ArrayIndex: u32::MAX,
            Reserved: 0,
        }];
        let mut size = 0u32;
        // SAFETY: valid record and descriptor; the name buffer outlives the call.
        if unsafe { TdhGetPropertySize(record, None, &descriptor, &mut size) } != ERROR_SUCCESS.0 {
            return None;
        }
        if !matches!(size, 1 | 2 | 4 | 8) {
            return None;
        }
        let mut bytes = [0u8; 8];
        // SAFETY: `bytes[..size]` receives exactly `size` bytes.
        if unsafe { TdhGetProperty(record, None, &descriptor, &mut bytes[..size as usize]) } != ERROR_SUCCESS.0 {
            return None;
        }
        Some(u64::from_le_bytes(bytes))
    }

    /// First-event diagnostic: payload field names, so a schema shift is visible in the log.
    fn log_schema_once(&self, record: &EVENT_RECORD) {
        if SCHEMA_LOGGED.swap(true, Ordering::AcqRel) {
            return;
        }
        let count = self.info().TopLevelPropertyCount as usize;
        let fields: Vec<String> = (0..count)
            .map(|i| {
                let name = self.property_name(i).unwrap_or_default();
                let value =
                    self.integer_property(record, i).map_or_else(|| "<unreadable>".to_owned(), |v| v.to_string());
                format!("[{i}]{name}={value}")
            })
            .collect();
        log::debug!("BluetoothCodecMonitor: A2dpStreaming schema - {}", fields.join(" "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec(standard: u8, vendor: i32, codec: i32) -> String {
        BluetoothCodec { standard_codec_id: standard, vendor_id: vendor, vendor_codec_id: codec }.friendly_name()
    }

    #[test]
    fn only_events_of_the_live_session_update_the_codec() {
        assert!(is_current_generation(3, 3));
        // Stop retired session 3 (generation 4) or a new start opened 5: a late event of 3 drops.
        assert!(!is_current_generation(3, 4));
        assert!(!is_current_generation(3, 5));
    }

    #[test]
    fn standard_and_vendor_codecs_have_names() {
        assert_eq!(codec(0x00, 0, 0), "SBC");
        assert_eq!(codec(0x01, 0, 0), "MP3");
        assert_eq!(codec(0x02, 0, 0), "AAC");
        assert_eq!(codec(0x04, 0, 0), "ATRAC");
        assert_eq!(codec(0xFF, 0x012D, 0x00AA), "Sony LDAC");
        assert_eq!(codec(0xFF, 0x000A, 0x0002), "Qualcomm/CSR aptX LL");
        assert_eq!(codec(0xFF, 0x053A, 0x484C), "Savitech LHDC");
    }

    #[test]
    fn unknown_codecs_use_the_exact_localized_formats() {
        assert_eq!(codec(0x03, 10, 20), "Unknown codec (invalid vendor): 0x03 10:20");
        assert_eq!(codec(0xFF, 0x1234, 0x0005), "Unknown codec: 0x1234:0x0005");
    }

    #[test]
    fn monitor_drops_stale_generations_and_duplicates() {
        let sink: BtSink = Arc::new(|_| {});
        let mut monitor = CodecMonitor::new(sink);
        let generation = monitor.bump_generation();
        let ldac = BluetoothCodec { standard_codec_id: 0xFF, vendor_id: 0x012D, vendor_codec_id: 0x00AA };
        assert!(monitor.apply(generation, ldac));
        assert!(!monitor.apply(generation, ldac));
        assert!(
            !monitor.apply(generation - 1, BluetoothCodec { standard_codec_id: 0, vendor_id: 0, vendor_codec_id: 0 })
        );
        assert_eq!(monitor.current(), Some(ldac));
        assert!(monitor.reset());
        assert!(!monitor.reset());
    }
}
