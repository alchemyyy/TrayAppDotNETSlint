//! Bluetooth state behind the device cards: radio, pairing lineage, connect/disconnect with the
//! 30 s observation window, battery, codec. Owns the Bluetooth logic of `AudioDeviceManager`
//! (connect 193-388, disconnect 390-452, fan-out 1922-2080), the Bluetooth state of
//! `AudioDevice` (138-357) and `BluetoothBatteryMonitor` as the [`BluetoothCoordinator`].
//!
//! Threading: the coordinator lives on the audio thread (`&mut self`). Every slow call
//! (cfgmgr32 reconciliation, Classic Bluetooth queries, RadioMgr, KS one-shot requests, ETW)
//! runs on worker threads that report back through the [`BtSink`] as [`BtEvent`]s, which the
//! audio thread feeds into [`BluetoothCoordinator::handle`]. Timers are driven by the caller via
//! [`BluetoothCoordinator::next_deadline`] / [`BluetoothCoordinator::tick`].

mod cfgmgr;
mod classic;
mod codec;
mod connector;
mod pairing;
mod radio;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use vtadn_model::snapshot::{BtState, DeviceId, DeviceState, Flow, RadioState};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_NOTIFY_ACTION, CM_NOTIFY_ACTION_DEVICEINSTANCEENUMERATED, CM_NOTIFY_ACTION_DEVICEINSTANCEREMOVED,
    CM_NOTIFY_ACTION_DEVICEINSTANCESTARTED, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER,
    CM_NOTIFY_FILTER_FLAG_ALL_DEVICE_INSTANCES, CM_NOTIFY_FILTER_TYPE_DEVICEINSTANCE, CM_Register_Notification,
    CM_Unregister_Notification, CR_SUCCESS, HCMNOTIFICATION,
};
use windows::Win32::Devices::Properties::{DEVPKEY_Device_ClassGuid, DEVPKEY_Device_ContainerId};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows_core::GUID;

pub use codec::BluetoothCodec;
pub use pairing::{LineageNode, find_radio_adapter_index, is_radio_off_pairing, parse_recorded_adapter_instance_id};

use crate::util::{MtaGuard, guid_braced_upper};

/// `BluetoothConnectionAttemptTimeoutMs`: VTADN's reconnect observation window. Windows exposes
/// no completion, progress or deadline for KSPROPERTY_ONESHOT_RECONNECT.
pub const BLUETOOTH_CONNECTION_ATTEMPT_TIMEOUT_MS: u64 = 30_000;
/// `BluetoothConnectionCountdownTickMs`.
const BLUETOOTH_CONNECTION_COUNTDOWN_TICK: Duration = Duration::from_millis(100);
/// `BluetoothConnectionStatePollIntervalMs`: battery/connection re-poll while a window is open,
/// and the Classic connection poll while the flyout is visible.
const BLUETOOTH_CONNECTION_STATE_POLL_INTERVAL_MS: u64 = 500;
/// `BluetoothBatteryPollIntervalMs`.
const BLUETOOTH_BATTERY_POLL_INTERVAL: Duration = Duration::from_millis(30_000);
/// `BluetoothRadioStatePollIntervalMs`.
const BLUETOOTH_RADIO_STATE_POLL_INTERVAL: Duration = Duration::from_millis(1_000);

const BLUETOOTH_ENUMERATOR_PREFIX: &str = "BTH";
const BLUETOOTH_NAME_TOKENS: [&str; 3] = ["Bluetooth", "Hands-Free", "A2DP"];
/// Devnodes that do not belong to a real physical-device container carry this id.
const NO_CONTAINER_SENTINEL: GUID = GUID::from_u128(0x00000000_0000_0000_ffff_ffffffffffff);
/// `CM_NOTIFY_EVENT_DATA` header before the instance id (FilterType + Reserved).
const CM_NOTIFY_EVENT_DATA_HEADER_SIZE: usize = 8;

/// Endpoint facts the coordinator needs, read by the audio service from the property store.
#[derive(Clone, Debug, PartialEq)]
pub struct BtEndpointInfo {
    pub id: DeviceId,
    pub flow: Flow,
    pub state: DeviceState,
    /// `PKEY_Device_EnumeratorName`.
    pub enumerator: String,
    pub name: String,
    pub description: String,
    pub interface_name: String,
    /// `PKEY_Device_ContainerId`.
    pub container_id: Option<GUID>,
    /// `PKEY_Endpoint_AdapterInstancePath` (start of the pairing lineage walk).
    pub adapter_instance_path: Option<String>,
}

/// Monitor/worker notification routed back into [`BluetoothCoordinator::handle`]. Opaque.
pub struct BtEvent(BtEventKind);

pub type BtSink = Arc<dyn Fn(BtEvent) + Send + Sync>;

pub(crate) enum BtEventKind {
    /// One cfgmgr32 reconciliation pass finished.
    Reconciled {
        generation: u64,
        connection_generation: u64,
        lease: Option<u64>,
        result: Box<Reconciliation>,
    },
    /// Classic `fConnected` poll finished.
    ConnectionState {
        generation: u64,
        connected: HashSet<GUID>,
    },
    /// Radio query/toggle result; `generation` None for toggles (always applied).
    Radio {
        generation: Option<u64>,
        state: RadioState,
    },
    /// No driver accepted an initial reconnect: close that window.
    ConnectRejected {
        endpoint_ids: Vec<String>,
        deadline_ms: u64,
    },
    /// A disconnect finished: re-read the physical state.
    DisconnectDone,
    /// CM notification for a BTH* devnode.
    PnpChanged,
    Codec {
        generation: u64,
        codec: BluetoothCodec,
    },
    CodecRetired,
    /// Pairing-store query finished.
    Paired {
        containers: HashSet<GUID>,
    },
}

/// `AudioDevice.ResolveBluetoothConnectionSecondsRemaining`: whole seconds left, rounded up.
pub fn resolve_connection_seconds_remaining(deadline_ms: u64, now_ms: u64) -> u32 {
    let remaining = deadline_ms.saturating_sub(now_ms);
    u32::try_from(remaining.div_ceil(1_000)).unwrap_or(u32::MAX)
}

/// `AudioDeviceManager.ResolveBluetoothConnectionAttemptDeadline`: now + 30 s, strictly after
/// the previous deadline so every retry gets a fresh, unique window.
pub fn resolve_connection_attempt_deadline(now_ms: u64, previous_deadline_ms: u64) -> u64 {
    let deadline = now_ms + BLUETOOTH_CONNECTION_ATTEMPT_TIMEOUT_MS;
    if deadline > previous_deadline_ms { deadline } else { previous_deadline_ms + 1 }
}

/// `VolumeFlyoutWindow.ResolveBluetoothBatteryLevel`: the current level while connected, the
/// last known one while disconnected.
pub fn resolve_battery_level(is_disconnected: bool, current: Option<u8>, last_known: Option<u8>) -> Option<u8> {
    if is_disconnected { last_known } else { current }
}

/// `AudioDevice.IsBluetoothOrphanedState`: a NotPresent Bluetooth endpoint is orphaned unless it
/// was classified as the current pairing of a paired device whose radio is off.
pub fn is_orphaned_state(is_bluetooth: bool, state: DeviceState, radio_off_pairing: bool) -> bool {
    is_bluetooth && state.is_not_present() && !radio_off_pairing
}

/// `AudioDevice.DetectIsBluetooth`: BTH* enumerator, or a name containing a Bluetooth token.
pub fn detect_is_bluetooth(enumerator: &str, name: &str, description: &str, interface_name: &str) -> bool {
    if starts_with_ignore_case(enumerator, BLUETOOTH_ENUMERATOR_PREFIX) {
        return true;
    }
    let texts = [name.to_lowercase(), description.to_lowercase(), interface_name.to_lowercase()];
    BLUETOOTH_NAME_TOKENS.iter().any(|token| {
        let token = token.to_lowercase();
        texts.iter().any(|text| text.contains(&token))
    })
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.is_char_boundary(prefix.len())
        && text[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// `CanPromoteToBluetooth`: an endpoint whose own bus is something else (HDAUDIO, USB, ...) is
/// never promoted by a container match; an empty enumerator defers to the container claim.
fn can_promote(enumerator: &str) -> bool {
    enumerator.is_empty() || starts_with_ignore_case(enumerator, BLUETOOTH_ENUMERATOR_PREFIX)
}

fn is_real_container(guid: &GUID) -> bool {
    *guid != GUID::zeroed() && *guid != NO_CONTAINER_SENTINEL
}

/// Result of one background reconciliation pass (`BuildCurrentState`).
pub(crate) struct Reconciliation {
    /// Bluetooth-classifying present devnodes → container, in enumeration order.
    current_ids: Vec<(String, GUID)>,
    current_containers: HashSet<GUID>,
    current_batteries: HashMap<GUID, u8>,
    connected_containers: HashSet<GUID>,
    has_connection_data: bool,
    radio_off_lineages: HashSet<String>,
    scanned: usize,
    bluetooth_class_matches: usize,
    battery_matches: usize,
}

/// None when cfgmgr32 returned no present devnodes (keep the previous state).
fn build_current_state(tracked_lineages: &[String]) -> Option<Reconciliation> {
    let ids = cfgmgr::enumerate_present_devnode_ids();
    if ids.is_empty() {
        return None;
    }
    let present: HashSet<String> = ids.iter().map(|id| pairing::instance_key(id)).collect();
    let radio_off_lineages = pairing::resolve_radio_off_lineages(tracked_lineages, &present);

    let mut current_ids: Vec<(String, GUID)> = Vec::new();
    let mut current_containers = HashSet::new();
    let mut current_batteries = HashMap::new();
    let mut bluetooth_class_matches = 0;
    let mut battery_matches = 0;
    for id in &ids {
        let Some(devinst) = cfgmgr::locate(id, false) else { continue };
        let Some(container) = cfgmgr::read_guid_property(devinst, &DEVPKEY_Device_ContainerId) else { continue };
        if !is_real_container(&container) {
            continue;
        }
        let insert = |current_ids: &mut Vec<(String, GUID)>| {
            if current_ids.last().is_none_or(|(last, _)| last != id) {
                current_ids.push((id.clone(), container));
            }
        };
        if cfgmgr::read_guid_property(devinst, &DEVPKEY_Device_ClassGuid) == Some(pairing::BLUETOOTH_CLASS_GUID) {
            insert(&mut current_ids);
            current_containers.insert(container);
            bluetooth_class_matches += 1;
        }
        let Some(battery) = cfgmgr::read_byte_property(devinst, &cfgmgr::DEVPKEY_BLUETOOTH_BATTERY) else { continue };
        insert(&mut current_ids);
        current_containers.insert(container);
        current_batteries.insert(container, battery);
        battery_matches += 1;
    }
    let connected = classic::query_connected_classic_addresses();
    let connected_containers = connected
        .as_ref()
        .map(|addresses| classic::resolve_connected_containers(current_ids.iter().map(|(i, c)| (i, c)), addresses))
        .unwrap_or_default();
    Some(Reconciliation {
        current_ids,
        current_containers,
        current_batteries,
        connected_containers,
        has_connection_data: connected.is_some(),
        radio_off_lineages,
        scanned: ids.len(),
        bluetooth_class_matches,
        battery_matches,
    })
}

/// A request that can absorb a newer one while queued.
trait Coalesce {
    fn coalesce(self, newer: Self) -> Self;
}

struct CoalesceState<R> {
    running: bool,
    pending: Option<R>,
    stopped: bool,
}

type CoalescedWork<R> = Arc<dyn Fn(R, &dyn Fn() -> bool) + Send + Sync>;

/// Single-flight background runner (the `AsyncThrottler` keys of the C# monitor): at most one
/// pass runs; requests arriving meanwhile coalesce into one follow-up pass, and the running
/// pass can see that it was replaced (its result is then stale and dropped).
struct Coalescer<R> {
    name: &'static str,
    state: Arc<Mutex<CoalesceState<R>>>,
    work: CoalescedWork<R>,
}

impl<R: Coalesce + Send + 'static> Coalescer<R> {
    fn new(name: &'static str, work: CoalescedWork<R>) -> Self {
        Self {
            name,
            state: Arc::new(Mutex::new(CoalesceState { running: false, pending: None, stopped: false })),
            work,
        }
    }

    fn request(&self, request: R) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.stopped {
            return;
        }
        state.pending = Some(match state.pending.take() {
            Some(older) => older.coalesce(request),
            None => request,
        });
        if state.running {
            return;
        }
        state.running = true;
        drop(state);
        let shared = Arc::clone(&self.state);
        let work = Arc::clone(&self.work);
        let spawned = std::thread::Builder::new().name(self.name.into()).spawn(move || {
            let _mta = MtaGuard::enter();
            loop {
                let next = {
                    let mut state = shared.lock().unwrap_or_else(PoisonError::into_inner);
                    match state.pending.take() {
                        Some(next) if !state.stopped => next,
                        _ => {
                            state.running = false;
                            break;
                        }
                    }
                };
                let replaced = || shared.lock().unwrap_or_else(PoisonError::into_inner).pending.is_some();
                work(next, &replaced);
            }
        });
        if let Err(error) = spawned {
            log::warn!("{}: worker failed to start: {error}", self.name);
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.running = false;
            state.pending = None;
        }
    }

    fn stop(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.stopped = true;
        state.pending = None;
    }
}

struct RefreshRequest {
    generation: u64,
    connection_generation: u64,
    /// Poll generation for flyout-polling passes; None for explicit refreshes.
    lease: Option<u64>,
    lineages: Vec<String>,
}

impl Coalesce for RefreshRequest {
    fn coalesce(self, newer: Self) -> Self {
        // An explicit refresh must not be dropped because a poll lease expired.
        let lease = if self.lease.is_none() { None } else { newer.lease };
        Self { lease, ..newer }
    }
}

struct ConnectionRequest {
    generation: u64,
    id_to_container: Vec<(String, GUID)>,
}

impl Coalesce for ConnectionRequest {
    fn coalesce(self, newer: Self) -> Self {
        newer
    }
}

/// Pairing-store query (`QueryPairedContainers`). `DevGetObjects` over association endpoints
/// blocks until every AEP provider answers (measured: 30 s on the development machine, any
/// filter), so it runs on its own single-flight worker instead of inside the reconciliation
/// pass, which would otherwise hold battery/connection results back for that long.
struct PairedRequest;

impl Coalesce for PairedRequest {
    fn coalesce(self, newer: Self) -> Self {
        newer
    }
}

/// Bluetooth facts of one endpoint (`AudioDevice` 138-357).
struct Endpoint {
    info: BtEndpointInfo,
    container_text: Option<Arc<str>>,
    lineage_start: String,
    detected: bool,
    promoted: bool,
    connected: bool,
    radio_off_pairing: bool,
    /// `GetTickCount64` deadline; 0 = no attempt pending.
    deadline_ms: u64,
    seconds_remaining: u32,
    battery: Option<u8>,
    last_known_battery: Option<u8>,
    codec: Option<BluetoothCodec>,
}

impl Endpoint {
    fn new(info: BtEndpointInfo) -> Self {
        let detected = detect_is_bluetooth(&info.enumerator, &info.name, &info.description, &info.interface_name);
        let lineage_start = pairing::resolve_lineage_start(&info.id, info.adapter_instance_path.as_deref());
        let container_text = info.container_id.as_ref().map(|guid| Arc::from(guid_braced_upper(guid)));
        Self {
            info,
            container_text,
            lineage_start,
            detected,
            promoted: false,
            connected: false,
            radio_off_pairing: false,
            deadline_ms: 0,
            seconds_remaining: 0,
            battery: None,
            last_known_battery: None,
            codec: None,
        }
    }

    fn update_info(&mut self, info: BtEndpointInfo) {
        if self.info.container_id != info.container_id {
            self.container_text = info.container_id.as_ref().map(|guid| Arc::from(guid_braced_upper(guid)));
        }
        if self.info.adapter_instance_path != info.adapter_instance_path {
            self.lineage_start = pairing::resolve_lineage_start(&info.id, info.adapter_instance_path.as_deref());
        }
        self.detected = detect_is_bluetooth(&info.enumerator, &info.name, &info.description, &info.interface_name);
        self.info = info;
    }

    fn is_bluetooth(&self) -> bool {
        self.detected || self.promoted
    }

    fn is_active(&self) -> bool {
        self.info.state.is_active()
    }

    fn is_render(&self) -> bool {
        self.info.flow == Flow::Render
    }

    fn is_unplugged(&self) -> bool {
        self.is_bluetooth() && self.info.state.is_unplugged()
    }

    fn is_pending(&self) -> bool {
        self.deadline_ms != 0
    }

    fn begin_attempt(&mut self, deadline_ms: u64, now_ms: u64) {
        self.deadline_ms = deadline_ms;
        self.update_attempt(now_ms);
    }

    /// Updates the rounded-up countdown; ends the attempt once its deadline passed. True while
    /// still pending.
    fn update_attempt(&mut self, now_ms: u64) -> bool {
        if !self.is_pending() {
            return false;
        }
        let seconds = resolve_connection_seconds_remaining(self.deadline_ms, now_ms);
        if seconds == 0 {
            self.end_attempt();
            return false;
        }
        self.seconds_remaining = seconds;
        true
    }

    fn end_attempt(&mut self) {
        self.deadline_ms = 0;
        self.seconds_remaining = 0;
    }

    /// `BatteryLevel` setter: a known level also becomes the last known one.
    fn set_battery(&mut self, level: Option<u8>) {
        if level.is_some() {
            self.last_known_battery = level;
        }
        self.battery = level;
    }

    fn snapshot(&self) -> Option<BtState> {
        self.is_bluetooth().then(|| BtState {
            container_id: self.container_text.clone(),
            connected: self.connected,
            radio_off_pairing: self.radio_off_pairing,
            pending_deadline_ms: self.is_pending().then_some(self.deadline_ms),
            seconds_remaining: self.seconds_remaining,
            battery: self.battery,
            last_known_battery: self.last_known_battery,
            codec: self.codec.map(|codec| codec.friendly_name()),
        })
    }
}

/// Cached cfgmgr32 / Classic / pairing-store view (`BluetoothBatteryMonitor` state).
#[derive(Default)]
struct MonitorCache {
    /// Present devnodes that classify a container as Bluetooth (class or battery bearing).
    id_to_container: Vec<(String, GUID)>,
    batteries: HashMap<GUID, u8>,
    known: HashSet<GUID>,
    connected: HashSet<GUID>,
    paired: HashSet<GUID>,
    /// [`pairing::instance_key`]s of tracked lineage starts classified radio-off.
    radio_off_lineages: HashSet<String>,
    last_applied_refresh: u64,
    last_applied_connection: u64,
}

/// Monitor notifications, processed in order by the coordinator's fan-out handlers.
enum MonitorChange {
    Connection(GUID, bool),
    Battery(GUID, Option<u8>),
    Pairing,
}

impl MonitorCache {
    fn apply_current_state(
        &mut self,
        result: &Reconciliation,
        connection_generation: u64,
        changes: &mut Vec<MonitorChange>,
    ) {
        let newly_known: Vec<GUID> = result.current_containers.difference(&self.known).copied().collect();
        self.id_to_container.clone_from(&result.current_ids);
        self.known.clone_from(&result.current_containers);
        if result.has_connection_data && connection_generation > self.last_applied_connection {
            self.last_applied_connection = connection_generation;
            self.apply_connected_state(&result.connected_containers, changes);
        } else {
            let unknown: Vec<GUID> = self.connected.iter().filter(|c| !self.known.contains(c)).copied().collect();
            for container in unknown {
                self.connected.remove(&container);
                changes.push(MonitorChange::Connection(container, false));
            }
        }
        if result.has_connection_data {
            for container in newly_known {
                if !self.connected.contains(&container) {
                    changes.push(MonitorChange::Connection(container, false));
                }
            }
        }
        let stale: Vec<GUID> =
            self.batteries.keys().filter(|c| !result.current_batteries.contains_key(c)).copied().collect();
        for container in stale {
            self.apply_battery(container, None, changes);
        }
        for (&container, &level) in &result.current_batteries {
            self.apply_battery(container, Some(level), changes);
        }
    }

    fn apply_connected_state(&mut self, connected: &HashSet<GUID>, changes: &mut Vec<MonitorChange>) {
        let disconnected: Vec<GUID> =
            self.connected.iter().filter(|c| !self.known.contains(c) || !connected.contains(c)).copied().collect();
        let newly_connected: Vec<GUID> =
            connected.iter().filter(|c| self.known.contains(c) && !self.connected.contains(c)).copied().collect();
        for container in disconnected {
            self.connected.remove(&container);
            log::debug!("BluetoothBatteryMonitor: BT container={} connected=false", guid_braced_upper(&container));
            changes.push(MonitorChange::Connection(container, false));
        }
        for container in newly_connected {
            self.connected.insert(container);
            log::debug!("BluetoothBatteryMonitor: BT container={} connected=true", guid_braced_upper(&container));
            changes.push(MonitorChange::Connection(container, true));
        }
    }

    fn apply_battery(&mut self, container: GUID, level: Option<u8>, changes: &mut Vec<MonitorChange>) {
        let changed = match level {
            Some(level) => self.batteries.insert(container, level) != Some(level),
            None => self.batteries.remove(&container).is_some(),
        };
        if changed {
            log::debug!("BluetoothBatteryMonitor: container={} battery={level:?}", guid_braced_upper(&container));
            changes.push(MonitorChange::Battery(container, level));
        }
    }

    /// Radio-off lineage classification from a reconciliation pass.
    fn apply_radio_off_lineages(&mut self, radio_off: &HashSet<String>, changes: &mut Vec<MonitorChange>) {
        if self.radio_off_lineages != *radio_off {
            self.radio_off_lineages.clone_from(radio_off);
            changes.push(MonitorChange::Pairing);
        }
    }

    /// Paired-container set from the pairing store (a failed query never reaches here, so the
    /// previous set is kept).
    fn apply_paired_containers(&mut self, paired: &HashSet<GUID>, changes: &mut Vec<MonitorChange>) {
        if self.paired != *paired {
            self.paired.clone_from(paired);
            changes.push(MonitorChange::Pairing);
        }
    }
}

/// `CM_Register_Notification` registration; unregistered on drop.
struct DeviceNotification {
    handle: HCMNOTIFICATION,
    context: *mut BtSink,
}

// SAFETY: the notification handle is a process-wide registration usable from any thread; the
// context pointer is only freed after unregistering (which waits for running callbacks).
unsafe impl Send for DeviceNotification {}

impl DeviceNotification {
    fn register(sink: &BtSink) -> Option<Self> {
        let context = Box::into_raw(Box::new(Arc::clone(sink)));
        let filter = CM_NOTIFY_FILTER {
            cbSize: size_of::<CM_NOTIFY_FILTER>() as u32,
            Flags: CM_NOTIFY_FILTER_FLAG_ALL_DEVICE_INSTANCES,
            FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINSTANCE,
            ..Default::default()
        };
        let mut handle = HCMNOTIFICATION::default();
        // SAFETY: valid filter; `context` stays alive until after CM_Unregister_Notification.
        let cr = unsafe {
            CM_Register_Notification(
                &filter,
                Some(context.cast_const().cast()),
                Some(on_device_notification),
                &mut handle,
            )
        };
        if cr == CR_SUCCESS && !handle.is_invalid() {
            return Some(Self { handle, context });
        }
        log::warn!(
            "BluetoothBatteryMonitor: CM_Register_Notification failed, cr=0x{:08X}; connection presence will use flyout polling",
            cr.0
        );
        // SAFETY: registration failed, so no callback can observe the context.
        drop(unsafe { Box::from_raw(context) });
        None
    }
}

impl Drop for DeviceNotification {
    fn drop(&mut self) {
        // SAFETY: unregisters our own registration; this waits for in-flight callbacks, after
        // which the context can be freed.
        let cr = unsafe { CM_Unregister_Notification(self.handle) };
        if cr != CR_SUCCESS {
            log::warn!("BluetoothBatteryMonitor: CM_Unregister_Notification failed, cr=0x{:08X}", cr.0);
            // The callback may still run: leak the context rather than free it under it.
            return;
        }
        // SAFETY: created by Box::into_raw in `register` and no longer referenced.
        drop(unsafe { Box::from_raw(self.context) });
    }
}

/// `PCM_NOTIFY_CALLBACK`: a BTH* devnode was enumerated, started or removed → refresh.
unsafe extern "system" fn on_device_notification(
    _notify: HCMNOTIFICATION,
    context: *const core::ffi::c_void,
    action: CM_NOTIFY_ACTION,
    event_data: *const CM_NOTIFY_EVENT_DATA,
    event_data_size: u32,
) -> u32 {
    if context.is_null() || event_data.is_null() || (event_data_size as usize) <= CM_NOTIFY_EVENT_DATA_HEADER_SIZE {
        return 0;
    }
    if action != CM_NOTIFY_ACTION_DEVICEINSTANCEENUMERATED
        && action != CM_NOTIFY_ACTION_DEVICEINSTANCESTARTED
        && action != CM_NOTIFY_ACTION_DEVICEINSTANCEREMOVED
    {
        return 0;
    }
    // SAFETY: for device-instance filters the data is the header followed by a NUL-terminated
    // instance id within `event_data_size` bytes; `context` is the registered `BtSink`.
    unsafe {
        let units = (event_data_size as usize - CM_NOTIFY_EVENT_DATA_HEADER_SIZE) / 2;
        let id = std::slice::from_raw_parts(
            event_data.cast::<u8>().add(CM_NOTIFY_EVENT_DATA_HEADER_SIZE).cast::<u16>(),
            units,
        );
        let end = id.iter().position(|&c| c == 0).unwrap_or(id.len());
        let id = String::from_utf16_lossy(&id[..end]);
        if starts_with_ignore_case(&id, BLUETOOTH_ENUMERATOR_PREFIX) {
            let sink = &*context.cast::<BtSink>();
            sink(BtEvent(BtEventKind::PnpChanged));
        }
    }
    0
}

/// `Instant` ↔ `GetTickCount64` milliseconds, the clock the countdown deadlines use.
struct Clock {
    instant: Instant,
    tick_ms: u64,
}

impl Clock {
    fn new() -> Self {
        // SAFETY: no preconditions.
        let tick_ms = unsafe { GetTickCount64() };
        Self { instant: Instant::now(), tick_ms }
    }

    fn ms(&self, at: Instant) -> u64 {
        let delta = at.saturating_duration_since(self.instant).as_millis();
        self.tick_ms + u64::try_from(delta).unwrap_or(u64::MAX - self.tick_ms)
    }
}

/// Bluetooth state machine of the audio service. See the module docs for threading.
pub struct BluetoothCoordinator {
    sink: BtSink,
    endpoints: Vec<Endpoint>,
    /// Session-lifetime battery cache per container (survives disconnects).
    last_known_batteries: HashMap<GUID, u8>,
    monitor: MonitorCache,
    tracked_lineages: Vec<String>,
    refresh_generation: u64,
    connection_generation: u64,
    reconciler: Coalescer<RefreshRequest>,
    connection_poller: Coalescer<ConnectionRequest>,
    pairing_query: Coalescer<PairedRequest>,
    radio: RadioState,
    radio_generation: u64,
    radio_gate: Arc<Mutex<()>>,
    codec: codec::CodecMonitor,
    /// Active battery-poll lease (0 = flyout hidden).
    poll_generation: u64,
    poll_generation_counter: u64,
    countdown_due: Option<Instant>,
    last_state_poll_ms: u64,
    radio_poll_due: Option<Instant>,
    battery_poll_due: Option<Instant>,
    connection_poll_due: Option<Instant>,
    connect_call_lock: Arc<Mutex<()>>,
    disconnect_in_flight: Arc<AtomicBool>,
    clock: Clock,
    notification: Option<DeviceNotification>,
}

impl BluetoothCoordinator {
    pub fn new(sink: BtSink, show_codec: bool) -> Self {
        let reconcile_sink = Arc::clone(&sink);
        let reconciler = Coalescer::new(
            "vtadn-bt-reconcile",
            Arc::new(move |request: RefreshRequest, replaced: &dyn Fn() -> bool| {
                let Some(result) = build_current_state(&request.lineages) else {
                    log::debug!(
                        "BluetoothBatteryMonitor.Reconcile: cfgmgr32 returned no present devnodes; keeping previous state."
                    );
                    return;
                };
                if replaced() {
                    return;
                }
                reconcile_sink(BtEvent(BtEventKind::Reconciled {
                    generation: request.generation,
                    connection_generation: request.connection_generation,
                    lease: request.lease,
                    result: Box::new(result),
                }));
            }),
        );
        let connection_sink = Arc::clone(&sink);
        let connection_poller = Coalescer::new(
            "vtadn-bt-connection",
            Arc::new(move |request: ConnectionRequest, replaced: &dyn Fn() -> bool| {
                let Some(addresses) = classic::query_connected_classic_addresses() else { return };
                if replaced() {
                    return;
                }
                let connected = classic::resolve_connected_containers(
                    request.id_to_container.iter().map(|(i, c)| (i, c)),
                    &addresses,
                );
                connection_sink(BtEvent(BtEventKind::ConnectionState { generation: request.generation, connected }));
            }),
        );
        let paired_sink = Arc::clone(&sink);
        let pairing_query = Coalescer::new(
            "vtadn-bt-pairing",
            Arc::new(move |_: PairedRequest, _: &dyn Fn() -> bool| {
                // A newer request only re-runs the slow query; this (still valid) answer is
                // posted regardless so a steady stream of refreshes cannot starve it.
                if let Some(containers) = pairing::query_paired_containers() {
                    paired_sink(BtEvent(BtEventKind::Paired { containers }));
                }
            }),
        );
        let mut this = Self {
            codec: codec::CodecMonitor::new(Arc::clone(&sink)),
            notification: DeviceNotification::register(&sink),
            sink,
            endpoints: Vec::new(),
            last_known_batteries: HashMap::new(),
            monitor: MonitorCache::default(),
            tracked_lineages: Vec::new(),
            refresh_generation: 0,
            connection_generation: 0,
            reconciler,
            connection_poller,
            pairing_query,
            radio: RadioState::Unknown,
            radio_generation: 0,
            radio_gate: Arc::new(Mutex::new(())),
            poll_generation: 0,
            poll_generation_counter: 0,
            countdown_due: None,
            last_state_poll_ms: 0,
            radio_poll_due: None,
            battery_poll_due: None,
            connection_poll_due: None,
            connect_call_lock: Arc::new(Mutex::new(())),
            disconnect_in_flight: Arc::new(AtomicBool::new(false)),
            clock: Clock::new(),
        };
        // BluetoothBatteryMonitor.Start: first reconciliation pass.
        this.refresh();
        // The flyout constructs its radio controller with one immediate refresh.
        this.request_radio_refresh();
        if show_codec {
            this.codec.start();
        }
        this
    }

    /// The full endpoint list after every enumeration/state change. Returns true when any
    /// endpoint's Bluetooth state changed. Diffing against the previous list runs the C# add
    /// (`AddDeviceByID`), remove (`RemoveDeviceByID`) and state-change
    /// (`HandleDeviceStateChanged`) Bluetooth paths.
    pub fn set_endpoints(&mut self, endpoints: &[BtEndpointInfo]) -> bool {
        let before = self.fingerprint();
        let mut previous: HashMap<DeviceId, Endpoint> =
            self.endpoints.drain(..).map(|endpoint| (Arc::clone(&endpoint.info.id), endpoint)).collect();
        let mut added = Vec::new();
        let mut state_changed = Vec::new();
        for info in endpoints {
            match previous.remove(&info.id) {
                Some(mut endpoint) => {
                    let old_state = endpoint.info.state;
                    endpoint.update_info(info.clone());
                    if endpoint.info.state != old_state {
                        state_changed.push(Arc::clone(&info.id));
                    }
                    self.endpoints.push(endpoint);
                }
                None => {
                    self.endpoints.push(Endpoint::new(info.clone()));
                    added.push(Arc::clone(&info.id));
                }
            }
        }

        // Removals.
        let removed_bt_render = previous.values().any(|e| e.is_bluetooth() && e.is_render());
        if !previous.is_empty() {
            self.sync_tracked_lineages();
            self.stop_countdown_if_idle();
            if removed_bt_render && !self.has_active_bluetooth_render() && self.codec.reset() {
                self.propagate_codec();
            }
        }
        for id in &state_changed {
            self.handle_state_changed(id);
        }
        for id in &added {
            self.handle_added(id);
        }
        before != self.fingerprint()
    }

    /// `HandleDefaultDeviceChanged`: a default changed somewhere; re-read battery/connection.
    pub fn on_default_device_changed(&mut self) {
        self.refresh();
    }

    pub fn handle(&mut self, event: BtEvent) -> bool {
        let before = self.fingerprint();
        match event.0 {
            BtEventKind::Reconciled { generation, connection_generation, lease, result } => {
                self.apply_reconciliation(generation, connection_generation, lease, &result);
            }
            BtEventKind::ConnectionState { generation, connected } => {
                if generation > self.monitor.last_applied_connection {
                    self.monitor.last_applied_connection = generation;
                    let mut changes = Vec::new();
                    self.monitor.apply_connected_state(&connected, &mut changes);
                    self.apply_monitor_changes(changes);
                }
            }
            BtEventKind::Radio { generation, state } => {
                if generation.is_none_or(|g| g == self.radio_generation) {
                    self.radio = state;
                }
            }
            BtEventKind::ConnectRejected { endpoint_ids, deadline_ms } => {
                self.end_attempts_for_ids(&endpoint_ids, Some(deadline_ms));
            }
            BtEventKind::DisconnectDone | BtEventKind::PnpChanged => self.refresh(),
            BtEventKind::Codec { generation, codec } => {
                if self.codec.apply(generation, codec) {
                    self.propagate_codec();
                }
            }
            BtEventKind::CodecRetired => self.codec.on_retired(),
            BtEventKind::Paired { containers } => {
                let mut changes = Vec::new();
                self.monitor.apply_paired_containers(&containers, &mut changes);
                self.apply_monitor_changes(changes);
            }
        }
        before != self.fingerprint()
    }

    /// Some for Bluetooth endpoints (detection + promotion rules), None otherwise.
    pub fn state(&self, id: &DeviceId) -> Option<BtState> {
        self.find(id).and_then(|index| self.endpoints[index].snapshot())
    }

    pub fn radio(&self) -> RadioState {
        self.radio
    }

    /// Connect the endpoint's device (only from the Unplugged state; restarts a pending window).
    /// The window starts at once (re-read [`Self::state`]); the driver requests run on a worker.
    pub fn connect(&mut self, id: &DeviceId) {
        let Some(index) = self.find(id) else { return };
        let device = &self.endpoints[index];
        if !device.is_bluetooth() || !device.info.state.is_disconnected() || device.info.id.is_empty() {
            return;
        }
        let container = device.info.container_id;
        // Only Unplugged endpoints accept the request; NotPresent ones have no driver loaded.
        let mut related: Vec<usize> = (0..self.endpoints.len())
            .filter(|&i| {
                let candidate = &self.endpoints[i];
                candidate.is_unplugged()
                    && match container {
                        None => i == index,
                        Some(container) => candidate.info.container_id == Some(container),
                    }
            })
            .collect();
        related.sort_by_key(|&i| i != index);
        if related.is_empty() {
            related.push(index);
        }
        let is_retry = related.iter().any(|&i| self.endpoints[i].is_pending());
        let mut endpoint_ids: Vec<String> = Vec::new();
        for &i in &related {
            let endpoint_id = self.endpoints[i].info.id.to_string();
            if !endpoint_id.is_empty() && !endpoint_ids.contains(&endpoint_id) {
                endpoint_ids.push(endpoint_id);
            }
        }
        if endpoint_ids.is_empty() {
            endpoint_ids.push(id.to_string());
        }
        if is_retry {
            log::debug!("ConnectBluetoothDevice: retrying pending reconnect");
        }

        let now = Instant::now();
        let now_ms = self.clock.ms(now);
        let previous = related.iter().map(|&i| self.endpoints[i].deadline_ms).max().unwrap_or(0);
        let deadline_ms = resolve_connection_attempt_deadline(now_ms, previous);
        for &i in &related {
            self.endpoints[i].begin_attempt(deadline_ms, now_ms);
        }
        self.start_countdown(now, now_ms);
        self.refresh();

        let lock = Arc::clone(&self.connect_call_lock);
        let sink = Arc::clone(&self.sink);
        let spawned = std::thread::Builder::new().name("vtadn-bt-connect".into()).spawn(move || {
            let _mta = MtaGuard::enter();
            // The request cannot be cancelled; serialize so a rapid second click is a queued retry.
            let requested = {
                let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
                connector::request_connection(&endpoint_ids, true)
            };
            if !requested {
                log::info!("ConnectBluetoothDevice: no related endpoint accepted reconnect");
                // A rejected retry may mean the earlier attempt still owns the target: keep its
                // window. Only a rejected initial request is known to have no work.
                if !is_retry {
                    sink(BtEvent(BtEventKind::ConnectRejected { endpoint_ids, deadline_ms }));
                }
            }
        });
        if let Err(error) = spawned {
            log::warn!("ConnectBluetoothDevice: worker failed to start: {error}");
        }
    }

    /// Disconnect the whole device the endpoint belongs to (driver one-shot disconnect on every
    /// active/Unplugged endpoint of the container, then the radio drops the link). Single-flight.
    pub fn disconnect(&mut self, id: &DeviceId) {
        let Some(index) = self.find(id) else { return };
        let device = &self.endpoints[index];
        let (true, Some(container)) = (device.is_bluetooth(), device.info.container_id) else { return };
        let mut endpoint_ids: Vec<String> = Vec::new();
        for candidate in &self.endpoints {
            if candidate.is_bluetooth()
                && candidate.info.container_id == Some(container)
                && (candidate.is_active() || candidate.is_unplugged())
                && !candidate.info.id.is_empty()
            {
                let candidate_id = candidate.info.id.to_string();
                if !endpoint_ids.contains(&candidate_id) {
                    endpoint_ids.push(candidate_id);
                }
            }
        }
        let address = classic::address_for_container(&self.monitor.id_to_container, &container);
        if address.is_none() {
            log::info!("DisconnectBluetoothDevice: no address for container {}", guid_braced_upper(&container));
            self.refresh();
            if endpoint_ids.is_empty() {
                return;
            }
        }
        if self.disconnect_in_flight.swap(true, Ordering::AcqRel) {
            log::debug!("DisconnectBluetoothDevice: prior disconnect still in flight");
            return;
        }
        let lock = Arc::clone(&self.connect_call_lock);
        let in_flight = Arc::clone(&self.disconnect_in_flight);
        let sink = Arc::clone(&self.sink);
        let spawned = std::thread::Builder::new().name("vtadn-bt-disconnect".into()).spawn(move || {
            {
                let _mta = MtaGuard::enter();
                // Settings only logs the driver results and drops the link regardless.
                let _guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
                connector::request_connection(&endpoint_ids, false);
                if let Some(address) = address {
                    classic::try_disconnect(address);
                }
            }
            in_flight.store(false, Ordering::Release);
            sink(BtEvent(BtEventKind::DisconnectDone));
        });
        if let Err(error) = spawned {
            log::warn!("DisconnectBluetoothDevice: worker failed to start: {error}");
            self.disconnect_in_flight.store(false, Ordering::Release);
        }
    }

    /// Toggles every controllable Bluetooth radio based on a fresh query (worker thread; the
    /// result arrives through the sink).
    pub fn toggle_radio(&mut self) {
        let gate = Arc::clone(&self.radio_gate);
        let sink = Arc::clone(&self.sink);
        let spawned = std::thread::Builder::new().name("vtadn-bt-radio-toggle".into()).spawn(move || {
            let _gate = gate.lock().unwrap_or_else(PoisonError::into_inner);
            let _mta = MtaGuard::enter();
            let current = radio::query();
            sink(BtEvent(BtEventKind::Radio { generation: None, state: current }));
            if matches!(current, RadioState::Unknown | RadioState::Unavailable) {
                return;
            }
            let result = radio::set_enabled(current != RadioState::On);
            sink(BtEvent(BtEventKind::Radio { generation: None, state: result }));
        });
        if let Err(error) = spawned {
            log::warn!("BluetoothRadioController.ToggleAsync failed: {error}");
        }
    }

    /// While the flyout is visible: radio polling (1 s), battery polling (30 s) and the Classic
    /// connection poll (500 ms); radio and battery start with an immediate pass.
    pub fn set_flyout_active(&mut self, active: bool) {
        let now = Instant::now();
        if active {
            if self.battery_poll_due.is_none() {
                self.poll_generation_counter += 1;
                self.poll_generation = self.poll_generation_counter;
                log::debug!(
                    "BluetoothBatteryMonitor.StartPolling: tracking {} devnodes",
                    self.monitor.id_to_container.len()
                );
                self.battery_poll_due = Some(now + BLUETOOTH_BATTERY_POLL_INTERVAL);
                self.connection_poll_due =
                    Some(now + Duration::from_millis(BLUETOOTH_CONNECTION_STATE_POLL_INTERVAL_MS));
                self.refresh_with_lease(Some(self.poll_generation));
            }
            if self.radio_poll_due.is_none() {
                self.radio_generation += 1;
                self.radio_poll_due = Some(now + BLUETOOTH_RADIO_STATE_POLL_INTERVAL);
                self.request_radio_refresh();
            }
        } else {
            self.poll_generation = 0;
            self.battery_poll_due = None;
            self.connection_poll_due = None;
            self.radio_generation += 1;
            self.radio_poll_due = None;
        }
    }

    /// `ShowDeviceCodecText`: start/stop the codec monitor (needs elevation). Stopping clears the
    /// codec of every Bluetooth render endpoint (re-read [`Self::state`]).
    pub fn set_show_codec(&mut self, show: bool) {
        if show {
            self.codec.start();
        } else if self.codec.stop() {
            self.propagate_codec();
        }
    }

    /// Earliest pending timer: the 100 ms countdown while a connection attempt is pending, plus
    /// the flyout polls (radio 1 s, Classic connection 500 ms, battery 30 s) while visible.
    pub fn next_deadline(&self) -> Option<Instant> {
        [self.countdown_due, self.radio_poll_due, self.battery_poll_due, self.connection_poll_due]
            .into_iter()
            .flatten()
            .min()
    }

    /// Advances countdowns and runs due polls; true when any state changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let before = self.fingerprint();
        if self.countdown_due.is_some_and(|due| due <= now) {
            self.countdown_tick(now);
        }
        if self.radio_poll_due.is_some_and(|due| due <= now) {
            self.radio_poll_due = Some(now + BLUETOOTH_RADIO_STATE_POLL_INTERVAL);
            self.request_radio_refresh();
        }
        if self.battery_poll_due.is_some_and(|due| due <= now) {
            self.battery_poll_due = Some(now + BLUETOOTH_BATTERY_POLL_INTERVAL);
            self.refresh_with_lease(Some(self.poll_generation));
        }
        if self.connection_poll_due.is_some_and(|due| due <= now) {
            self.connection_poll_due = Some(now + Duration::from_millis(BLUETOOTH_CONNECTION_STATE_POLL_INTERVAL_MS));
            self.refresh_connection_state();
        }
        before != self.fingerprint()
    }

    pub fn shutdown(self) {
        drop(self);
    }

    // ---- internals -------------------------------------------------------------------------

    fn find(&self, id: &str) -> Option<usize> {
        self.endpoints.iter().position(|endpoint| &*endpoint.info.id == id)
    }

    fn fingerprint(&self) -> (RadioState, Vec<(DeviceId, Option<BtState>)>) {
        (self.radio, self.endpoints.iter().map(|e| (Arc::clone(&e.info.id), e.snapshot())).collect())
    }

    fn refresh(&mut self) {
        self.refresh_with_lease(None);
    }

    /// `BluetoothBatteryMonitor.Refresh` / `RefreshCore(pollLease)`.
    fn refresh_with_lease(&mut self, lease: Option<u64>) {
        self.refresh_generation += 1;
        self.connection_generation += 1;
        self.reconciler.request(RefreshRequest {
            generation: self.refresh_generation,
            connection_generation: self.connection_generation,
            lease,
            lineages: self.tracked_lineages.clone(),
        });
        self.pairing_query.request(PairedRequest);
    }

    /// `RefreshConnectionState`: the cheap Classic `fConnected` query.
    fn refresh_connection_state(&mut self) {
        if self.monitor.id_to_container.is_empty() {
            return;
        }
        self.connection_generation += 1;
        self.connection_poller.request(ConnectionRequest {
            generation: self.connection_generation,
            id_to_container: self.monitor.id_to_container.clone(),
        });
    }

    /// `BluetoothRadioController.RefreshAsync`: skipped while another radio operation runs.
    fn request_radio_refresh(&self) {
        let generation = self.radio_generation;
        let gate = Arc::clone(&self.radio_gate);
        let sink = Arc::clone(&self.sink);
        let spawned = std::thread::Builder::new().name("vtadn-bt-radio".into()).spawn(move || {
            let Ok(_gate) = gate.try_lock() else { return };
            let _mta = MtaGuard::enter();
            let state = radio::query();
            sink(BtEvent(BtEventKind::Radio { generation: Some(generation), state }));
        });
        if let Err(error) = spawned {
            log::warn!("BluetoothRadioController.RefreshAsync failed: {error}");
        }
    }

    fn apply_reconciliation(
        &mut self,
        generation: u64,
        connection_generation: u64,
        lease: Option<u64>,
        result: &Reconciliation,
    ) {
        if lease.is_some_and(|lease| lease == 0 || lease != self.poll_generation) {
            return;
        }
        if generation <= self.monitor.last_applied_refresh {
            return;
        }
        self.monitor.last_applied_refresh = generation;
        let mut changes = Vec::new();
        self.monitor.apply_current_state(result, connection_generation, &mut changes);
        self.monitor.apply_radio_off_lineages(&result.radio_off_lineages, &mut changes);
        log::debug!(
            "BluetoothBatteryMonitor.Reconcile: scanned={} bluetoothClass={} battery={} knownContainers={} connectedContainers={} pairedContainers={} radioOffLineages={}",
            result.scanned,
            result.bluetooth_class_matches,
            result.battery_matches,
            self.monitor.known.len(),
            self.monitor.connected.len(),
            self.monitor.paired.len(),
            self.monitor.radio_off_lineages.len()
        );
        self.apply_monitor_changes(changes);
    }

    fn apply_monitor_changes(&mut self, changes: Vec<MonitorChange>) {
        for change in changes {
            match change {
                MonitorChange::Connection(container, connected) => {
                    self.on_container_connection_changed(container, connected)
                }
                MonitorChange::Battery(container, level) => self.on_battery_changed(container, level),
                MonitorChange::Pairing => {
                    for index in 0..self.endpoints.len() {
                        self.apply_pairing(index);
                    }
                }
            }
        }
    }

    /// `AddDeviceByID` Bluetooth follow-up.
    fn handle_added(&mut self, id: &DeviceId) {
        self.sync_tracked_lineages();
        self.refresh();
        let Some(index) = self.find(id) else { return };
        self.promote_if_classified(index);
        self.apply_pairing(index);
        let endpoint = &mut self.endpoints[index];
        if endpoint.is_bluetooth() && endpoint.is_render() {
            endpoint.codec = self.codec.current();
        }
        if endpoint.is_bluetooth() && endpoint.is_active() {
            endpoint.connected = true;
            self.end_attempts_for_endpoint(index);
        }
    }

    /// `HandleDeviceStateChanged` Bluetooth parts.
    fn handle_state_changed(&mut self, id: &DeviceId) {
        let Some(index) = self.find(id) else { return };
        if self.endpoints[index].is_active() {
            self.refresh();
            self.promote_if_classified(index);
            if self.endpoints[index].is_bluetooth() {
                self.endpoints[index].connected = true;
                self.end_attempts_for_endpoint(index);
            }
        } else if self.endpoints[index].is_bluetooth() {
            self.refresh();
        }
        // The codec cache survives the active/inactive flicker of a paired headset; a newly
        // active Bluetooth render endpoint inherits it.
        let endpoint = &self.endpoints[index];
        if endpoint.is_bluetooth() && endpoint.is_render() && self.has_active_bluetooth_render() {
            self.propagate_codec();
        }
    }

    /// Active endpoint: end the attempts of its container (or of the endpoint alone).
    fn end_attempts_for_endpoint(&mut self, index: usize) {
        match self.endpoints[index].info.container_id {
            Some(container) => self.end_attempts_for_container(container),
            None => {
                let id = self.endpoints[index].info.id.to_string();
                self.end_attempts_for_ids(&[id], None);
            }
        }
    }

    fn end_attempts_for_ids(&mut self, ids: &[String], expected_deadline_ms: Option<u64>) {
        for endpoint in &mut self.endpoints {
            if !ids.iter().any(|id| **id == *endpoint.info.id) {
                continue;
            }
            if expected_deadline_ms.is_some_and(|expected| endpoint.deadline_ms != expected) {
                continue;
            }
            endpoint.end_attempt();
        }
        self.stop_countdown_if_idle();
    }

    fn end_attempts_for_container(&mut self, container: GUID) {
        for endpoint in &mut self.endpoints {
            if endpoint.info.container_id == Some(container) {
                endpoint.end_attempt();
            }
        }
        self.stop_countdown_if_idle();
    }

    fn start_countdown(&mut self, now: Instant, now_ms: u64) {
        self.last_state_poll_ms = now_ms;
        if self.countdown_due.is_none() {
            self.countdown_due = Some(now + BLUETOOTH_CONNECTION_COUNTDOWN_TICK);
        }
    }

    fn stop_countdown_if_idle(&mut self) {
        if !self.endpoints.iter().any(Endpoint::is_pending) {
            self.countdown_due = None;
        }
    }

    /// `OnBluetoothConnectionCountdownTick`.
    fn countdown_tick(&mut self, now: Instant) {
        let now_ms = self.clock.ms(now);
        let mut has_pending = false;
        for endpoint in &mut self.endpoints {
            let was_pending = endpoint.is_pending();
            if endpoint.update_attempt(now_ms) {
                has_pending = true;
            } else if was_pending {
                log::info!("Bluetooth reconnect timed out for '{}'", endpoint.info.id);
            }
        }
        if !has_pending {
            self.countdown_due = None;
            return;
        }
        self.countdown_due = Some(now + BLUETOOTH_CONNECTION_COUNTDOWN_TICK);
        if now_ms.saturating_sub(self.last_state_poll_ms) < BLUETOOTH_CONNECTION_STATE_POLL_INTERVAL_MS {
            return;
        }
        self.last_state_poll_ms = now_ms;
        self.refresh();
    }

    fn has_active_bluetooth_render(&self) -> bool {
        self.endpoints.iter().any(|e| e.is_bluetooth() && e.is_render() && e.is_active())
    }

    /// `PropagateCodecToBluetoothDevices`: render only (A2DP covers playback, not HFP capture).
    fn propagate_codec(&mut self) {
        let codec = self.codec.current();
        for endpoint in &mut self.endpoints {
            if endpoint.is_bluetooth() && endpoint.is_render() {
                endpoint.codec = codec;
            }
        }
    }

    /// `PromoteBluetoothIfClassified`.
    fn promote_if_classified(&mut self, index: usize) {
        let endpoint = &self.endpoints[index];
        let Some(container) = endpoint.info.container_id else { return };
        if !self.monitor.known.contains(&container) || !can_promote(&endpoint.info.enumerator) {
            return;
        }
        if !endpoint.is_bluetooth() {
            self.endpoints[index].promoted = true;
            self.sync_tracked_lineages();
        }
        let endpoint = &mut self.endpoints[index];
        endpoint.connected = endpoint.is_active() || self.monitor.connected.contains(&container);
        let level = self.monitor.batteries.get(&container).copied();
        self.apply_battery(index, container, level);
    }

    /// `OnBluetoothBatteryChanged`: fan out to every endpoint of the container.
    fn on_battery_changed(&mut self, container: GUID, level: Option<u8>) {
        if let Some(level) = level {
            self.last_known_batteries.insert(container, level);
        }
        for index in 0..self.endpoints.len() {
            if self.endpoints[index].info.container_id == Some(container) {
                self.apply_battery(index, container, level);
            }
        }
    }

    /// `OnBluetoothContainerConnectionChanged`: promote, keep the physical link flag independent
    /// of Core Audio, and end the reconnect window once the link is up.
    fn on_container_connection_changed(&mut self, container: GUID, connected: bool) {
        let mut promoted = false;
        let codec = self.codec.current();
        let level = self.monitor.batteries.get(&container).copied();
        for index in 0..self.endpoints.len() {
            let endpoint = &mut self.endpoints[index];
            if endpoint.info.container_id != Some(container) {
                continue;
            }
            if !endpoint.is_bluetooth() {
                if !can_promote(&endpoint.info.enumerator) {
                    continue;
                }
                endpoint.promoted = true;
                promoted = true;
            }
            endpoint.connected = connected;
            if !connected {
                continue;
            }
            if endpoint.is_render() {
                endpoint.codec = codec;
            }
            self.apply_battery(index, container, level);
        }
        if promoted {
            self.sync_tracked_lineages();
        }
        if connected {
            self.end_attempts_for_container(container);
        }
    }

    /// `ApplyBluetoothPairing`.
    fn apply_pairing(&mut self, index: usize) {
        let endpoint = &self.endpoints[index];
        let radio_off = endpoint.info.container_id.is_some_and(|c| self.monitor.paired.contains(&c))
            && self.monitor.radio_off_lineages.contains(&pairing::instance_key(&endpoint.lineage_start));
        self.endpoints[index].radio_off_pairing = radio_off;
    }

    /// `ApplyBluetoothBattery`.
    fn apply_battery(&mut self, index: usize, container: GUID, level: Option<u8>) {
        match level {
            Some(level) => {
                self.last_known_batteries.insert(container, level);
            }
            None => {
                if let Some(&last_known) = self.last_known_batteries.get(&container) {
                    self.endpoints[index].last_known_battery = Some(last_known);
                }
            }
        }
        self.endpoints[index].set_battery(level);
    }

    /// `SyncTrackedBluetoothLineages` + `SetTrackedBluetoothLineages`: Bluetooth endpoints and
    /// BTH* enumerators (before promotion confirms them); refreshes when the set changed.
    fn sync_tracked_lineages(&mut self) {
        let mut lineages: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for endpoint in &self.endpoints {
            if !endpoint.is_bluetooth()
                && !starts_with_ignore_case(&endpoint.info.enumerator, BLUETOOTH_ENUMERATOR_PREFIX)
            {
                continue;
            }
            if endpoint.lineage_start.is_empty() || !seen.insert(pairing::instance_key(&endpoint.lineage_start)) {
                continue;
            }
            lineages.push(endpoint.lineage_start.clone());
        }
        lineages.sort_by_key(|lineage| pairing::instance_key(lineage));
        let unchanged = lineages.len() == self.tracked_lineages.len()
            && lineages
                .iter()
                .zip(&self.tracked_lineages)
                .all(|(a, b)| pairing::instance_key(a) == pairing::instance_key(b));
        if unchanged {
            return;
        }
        self.tracked_lineages = lineages;
        self.refresh();
    }
}

impl Drop for BluetoothCoordinator {
    fn drop(&mut self) {
        self.battery_poll_due = None;
        self.connection_poll_due = None;
        self.radio_poll_due = None;
        self.countdown_due = None;
        self.reconciler.stop();
        self.connection_poller.stop();
        self.pairing_query.stop();
        self.codec.shutdown();
        self.notification = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_display_uses_current_only_while_connected() {
        let cases = [
            (false, Some(64), Some(23), Some(64)),
            (false, None, Some(23), None),
            (true, None, Some(23), Some(23)),
            (true, None, None, None),
            (true, Some(64), Some(23), Some(23)),
        ];
        for (disconnected, current, last_known, expected) in cases {
            assert_eq!(resolve_battery_level(disconnected, current, last_known), expected);
        }
    }

    #[test]
    fn seconds_remaining_round_up_until_deadline() {
        let cases = [
            (30_000, 0, 30),
            (30_000, 1, 30),
            (30_000, 29_000, 1),
            (30_000, 29_999, 1),
            (30_000, 30_000, 0),
            (30_000, 31_000, 0),
        ];
        for (deadline, now, expected) in cases {
            assert_eq!(resolve_connection_seconds_remaining(deadline, now), expected, "{deadline} {now}");
        }
    }

    #[test]
    fn retry_deadline_is_fresh_and_unique() {
        assert_eq!(resolve_connection_attempt_deadline(1_000, 0), 31_000);
        assert_eq!(resolve_connection_attempt_deadline(1_000, 31_000), 31_001);
        assert_eq!(resolve_connection_attempt_deadline(2_000, 31_000), 32_000);
    }

    #[test]
    fn only_unclassified_not_present_bluetooth_endpoints_are_orphaned() {
        let cases = [
            (true, DeviceState::NOT_PRESENT, false, true),
            (true, DeviceState::NOT_PRESENT, true, false),
            (true, DeviceState::UNPLUGGED, false, false),
            (true, DeviceState::ACTIVE, false, false),
            (true, DeviceState::DISABLED, false, false),
            (false, DeviceState::NOT_PRESENT, false, false),
        ];
        for (bluetooth, state, radio_off, expected) in cases {
            assert_eq!(is_orphaned_state(bluetooth, DeviceState(state), radio_off), expected);
        }
    }

    #[test]
    fn detection_uses_enumerator_then_name_tokens() {
        assert!(detect_is_bluetooth("BTHENUM", "Headphones (WH-1000XM4)", "", ""));
        assert!(detect_is_bluetooth("bthhfenum", "", "", ""));
        assert!(detect_is_bluetooth("", "Headset (Hands-Free AG Audio)", "", ""));
        assert!(detect_is_bluetooth("", "", "", "Stereo a2dp sink"));
        assert!(detect_is_bluetooth("", "", "Bluetooth Speaker", ""));
        assert!(!detect_is_bluetooth("HDAUDIO", "Speakers (Realtek(R) Audio)", "Speakers", "Realtek(R) Audio"));
        assert!(can_promote(""));
        assert!(can_promote("BTHLEDevice"));
        assert!(!can_promote("USB"));
    }

    #[test]
    fn notification_filter_matches_native_layout() {
        assert_eq!(size_of::<CM_NOTIFY_FILTER>(), 416);
    }

    fn info(id: &str, flow: Flow, state: u32, enumerator: &str, container: Option<GUID>) -> BtEndpointInfo {
        BtEndpointInfo {
            id: Arc::from(id),
            flow,
            state: DeviceState(state),
            enumerator: enumerator.into(),
            name: "Headphones".into(),
            description: "Headphones".into(),
            interface_name: "WH-1000XM4".into(),
            container_id: container,
            adapter_instance_path: None,
        }
    }

    /// Exercises the pure endpoint/monitor state machine without starting any worker.
    fn endpoint_set(infos: Vec<BtEndpointInfo>) -> Vec<Endpoint> {
        infos.into_iter().map(Endpoint::new).collect()
    }

    #[test]
    fn countdown_ends_attempt_at_deadline() {
        let mut endpoints =
            endpoint_set(vec![info("{0.0.0.00000000}.{a}", Flow::Render, DeviceState::UNPLUGGED, "BTHENUM", None)]);
        let endpoint = &mut endpoints[0];
        endpoint.begin_attempt(31_000, 1_000);
        assert!(endpoint.is_pending());
        assert_eq!(endpoint.seconds_remaining, 30);
        assert!(endpoint.update_attempt(30_500));
        assert_eq!(endpoint.seconds_remaining, 1);
        assert!(!endpoint.update_attempt(31_000));
        assert!(!endpoint.is_pending());
        assert_eq!(endpoint.snapshot().and_then(|s| s.pending_deadline_ms), None);
    }

    #[test]
    fn battery_setter_keeps_last_known_level() {
        let mut endpoints =
            endpoint_set(vec![info("{0.0.0.00000000}.{a}", Flow::Render, DeviceState::ACTIVE, "BTHENUM", None)]);
        let endpoint = &mut endpoints[0];
        endpoint.set_battery(Some(80));
        endpoint.set_battery(None);
        let snapshot = endpoint.snapshot().expect("bluetooth");
        assert_eq!(snapshot.battery, None);
        assert_eq!(snapshot.last_known_battery, Some(80));
    }

    #[test]
    fn monitor_reports_connection_and_battery_transitions() {
        let container = GUID::from_u128(42);
        let mut cache = MonitorCache::default();
        let result = Reconciliation {
            current_ids: vec![(r"BTHENUM\DEV_8099E75CA52C\x".into(), container)],
            current_containers: HashSet::from([container]),
            current_batteries: HashMap::from([(container, 70)]),
            connected_containers: HashSet::from([container]),
            has_connection_data: true,
            radio_off_lineages: HashSet::new(),
            scanned: 1,
            bluetooth_class_matches: 1,
            battery_matches: 1,
        };
        let mut changes = Vec::new();
        cache.apply_current_state(&result, 1, &mut changes);
        assert!(matches!(changes[0], MonitorChange::Connection(c, true) if c == container));
        assert!(matches!(changes[1], MonitorChange::Battery(c, Some(70)) if c == container));
        assert_eq!(changes.len(), 2);
        cache.apply_paired_containers(&HashSet::from([container]), &mut changes);
        assert!(matches!(changes[2], MonitorChange::Pairing));
        assert_eq!(changes.len(), 3);

        // Same data again: no transitions.
        let mut changes = Vec::new();
        cache.apply_current_state(&result, 2, &mut changes);
        assert!(changes.is_empty());

        // Container gone: disconnected and battery cleared; a stale connection generation is
        // ignored in favour of dropping unknown containers.
        let gone = Reconciliation {
            current_ids: Vec::new(),
            current_containers: HashSet::new(),
            current_batteries: HashMap::new(),
            connected_containers: HashSet::new(),
            has_connection_data: true,
            radio_off_lineages: HashSet::new(),
            scanned: 1,
            bluetooth_class_matches: 0,
            battery_matches: 0,
        };
        let mut changes = Vec::new();
        cache.apply_current_state(&gone, 1, &mut changes);
        assert!(matches!(changes[0], MonitorChange::Connection(c, false) if c == container));
        assert!(matches!(changes[1], MonitorChange::Battery(c, None) if c == container));
        // Same paired set and radio-off classification: no pairing change.
        let mut changes = Vec::new();
        cache.apply_paired_containers(&HashSet::from([container]), &mut changes);
        cache.apply_radio_off_lineages(&HashSet::new(), &mut changes);
        assert!(changes.is_empty());
        cache.apply_radio_off_lineages(&HashSet::from(["X".to_owned()]), &mut changes);
        assert!(matches!(changes[0], MonitorChange::Pairing));
    }

    #[test]
    fn coalesced_refresh_keeps_an_explicit_request() {
        let explicit = RefreshRequest { generation: 1, connection_generation: 1, lease: None, lineages: Vec::new() };
        let poll = RefreshRequest { generation: 2, connection_generation: 2, lease: Some(5), lineages: Vec::new() };
        let merged = explicit.coalesce(poll);
        assert_eq!((merged.generation, merged.lease), (2, None));
        let poll_a = RefreshRequest { generation: 3, connection_generation: 3, lease: Some(5), lineages: Vec::new() };
        let poll_b = RefreshRequest { generation: 4, connection_generation: 4, lease: Some(6), lineages: Vec::new() };
        assert_eq!(poll_a.coalesce(poll_b).lease, Some(6));
    }
}
