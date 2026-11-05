//! One device's sessions grouped into apps (port of `AudioSession.cs`, `AudioAppGroup.cs` and the
//! session parts of `AudioDevice.cs`).
//!
//! - Sessions: one per `IAudioSessionControl`, deduplicated by session instance id; sessions that
//!   are already Expired are never published.
//! - Groups: one per app id (lower-cased image path, `pid:N`, or the system-sounds id) in insertion
//!   order; the first session is the representative (name, icon, pid, volume, mute).
//! - Removal: disconnect (any reason; `ExclusiveModeOverride` marks the endpoint exclusively held
//!   until a new session arrives), Expired state, process exit, a write failing with
//!   `AUDCLNT_E_DEVICE_INVALIDATED`, reconcile, detach.
//!
//! COM callbacks and workers only post [`SessionEvent`]s through the sink; all state changes run
//! in [`SessionHost::handle`] on the audio thread.

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use vtadn_model::policy::ding::DingSuppressionPeak;
use vtadn_model::snapshot::{AppGroupSnapshot, AppId, DeviceId, Flow, IconPixels, SessionState};
use vtadn_model::text::SYSTEM_SOUNDS_NAME;
use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    AUDCLNT_E_DEVICE_INVALIDATED, AudioSessionDisconnectReason, AudioSessionState, AudioSessionStateActive,
    AudioSessionStateExpired, DisconnectReasonExclusiveModeOverride, IAudioSessionControl, IAudioSessionControl2,
    IAudioSessionEvents, IAudioSessionEvents_Impl, IAudioSessionManager2, IAudioSessionNotification,
    IAudioSessionNotification_Impl, ISimpleAudioVolume,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows_core::{BOOL, GUID, Interface, PCWSTR, PWSTR, Ref, implement};

use crate::icons::{IconCache, IconJob, IconJobKind};
use crate::meter::read_meter_peaks;
use crate::process::{ExitWatch, ExitWatchResult, image_path, watch_exit};

/// `AudioSession.VolumeEqualityEpsilon`: kept below the level change of a 0.01% move at the bottom
/// of the logarithmic app slider so the first wheel steps up from silence are never swallowed.
pub const VOLUME_EQUALITY_EPSILON: f32 = 0.000_001;
/// `TimeConstants.VolumeWriteRateDefaultMs`: throttled session volume writes, latest wins.
pub const VOLUME_WRITE_COOLDOWN: Duration = Duration::from_millis(30);
/// `AudioSession.SystemSoundsAppID`: groups the system-sounds session with itself.
pub const SYSTEM_SOUNDS_APP_ID: &str = "System.SystemSoundsSession";

/// A callback/worker notification for one device's host. Opaque to the service, which routes it
/// back with [`SessionHost::handle`]. Must be `Send` (COM pointers inside are MTA-agile wrappers).
pub struct SessionEvent(pub(crate) Event);

impl fmt::Debug for SessionEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Event::Created(_) => f.write_str("SessionCreated"),
            Event::DisplayNameChanged { session, .. } => write!(f, "DisplayNameChanged({session})"),
            Event::IconPathChanged { session } => write!(f, "IconPathChanged({session})"),
            Event::SimpleVolumeChanged { session, volume, muted } => {
                write!(f, "SimpleVolumeChanged({session}, {volume}, {muted})")
            }
            Event::StateChanged { session, state } => write!(f, "StateChanged({session}, {state:?})"),
            Event::Disconnected { session, reason } => write!(f, "Disconnected({session}, {reason:?})"),
            Event::WriteFailed { session } => write!(f, "WriteFailed({session})"),
            Event::Metadata { session, .. } => write!(f, "Metadata({session})"),
        }
    }
}

pub(crate) enum Event {
    /// `IAudioSessionNotification::OnSessionCreated`.
    Created(AgileControl),
    DisplayNameChanged {
        session: u64,
        name: String,
    },
    IconPathChanged {
        session: u64,
    },
    SimpleVolumeChanged {
        session: u64,
        volume: f32,
        muted: bool,
    },
    StateChanged {
        session: u64,
        state: SessionState,
    },
    /// `OnSessionDisconnected` (`Some(reason)`) or the process exited (None).
    Disconnected {
        session: u64,
        reason: Option<i32>,
    },
    /// A volume or mute write failed with `AUDCLNT_E_DEVICE_INVALIDATED`.
    WriteFailed {
        session: u64,
    },
    /// Icon / display name resolved by the icon worker.
    Metadata {
        session: u64,
        icon: Option<Arc<IconPixels>>,
        name: Option<String>,
        clear_icon_on_none: bool,
    },
}

/// A session control handed between threads.
#[derive(Clone)]
pub(crate) struct AgileControl(pub(crate) IAudioSessionControl);

// SAFETY: audioses.dll session controls aggregate the free-threaded marshaler (the .NET app relies
// on the same property for its worker reads), and every thread that touches them here (COM
// callback threads, the audio thread, the icon worker) is in the MTA.
unsafe impl Send for AgileControl {}
// SAFETY: see above; the wrapped interface is only used through its thread-safe COM methods.
unsafe impl Sync for AgileControl {}

/// Posts `(device, event)` into the audio service thread.
pub type SessionSink = Arc<dyn Fn(DeviceId, SessionEvent) + Send + Sync>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostChange {
    /// Group membership, state, icon, name or mute changed (flyout rebuild class).
    pub structural: bool,
    /// Only group volumes changed.
    pub values: bool,
}

impl HostChange {
    pub fn any(self) -> bool {
        self.structural || self.values
    }

    fn structural() -> Self {
        Self { structural: true, values: false }
    }

    fn values() -> Self {
        Self { structural: false, values: true }
    }
}

impl std::ops::BitOrAssign for HostChange {
    fn bitor_assign(&mut self, other: Self) {
        self.structural |= other.structural;
        self.values |= other.values;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SessionHostConfig {
    /// Event context passed to `SetMasterVolume`/`SetMute`; echoes carrying it are ignored.
    pub event_context: GUID,
    /// This process: its own sessions (the ding) are tracked but flagged for hiding.
    pub own_pid: u32,
}

fn session_state(state: AudioSessionState) -> SessionState {
    if state == AudioSessionStateActive {
        SessionState::Active
    } else if state == AudioSessionStateExpired {
        SessionState::Expired
    } else {
        SessionState::Inactive
    }
}

/// Takes ownership of a CoTaskMem string.
fn take_co_string(value: PWSTR) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: COM returned a NUL-terminated string we own; freed right after the copy.
    let text = unsafe { value.to_string() }.ok();
    // SAFETY: allocated by the COM callee with CoTaskMemAlloc.
    unsafe { CoTaskMemFree(Some(value.0.cast_const().cast())) };
    text
}

/// `ReadSessionDisplayName`: empty, whitespace and `@` indirect strings read as unnamed.
pub(crate) fn read_session_display_name(control: &IAudioSessionControl) -> Option<String> {
    // SAFETY: plain COM read on a live control.
    let name = take_co_string(unsafe { control.GetDisplayName() }.ok()?)?;
    (!name.trim().is_empty() && !name.starts_with('@')).then_some(name)
}

/// `IAudioSessionControl::GetIconPath`; None when empty or unreadable.
pub(crate) fn read_icon_path(control: &IAudioSessionControl) -> Option<String> {
    // SAFETY: plain COM read on a live control.
    take_co_string(unsafe { control.GetIconPath() }.ok()?).filter(|path| !path.is_empty())
}

fn read_session_instance_id(control: &IAudioSessionControl) -> String {
    control
        .cast::<IAudioSessionControl2>()
        .ok()
        // SAFETY: plain COM read on a live control.
        .and_then(|control2| unsafe { control2.GetSessionInstanceIdentifier() }.ok())
        .and_then(take_co_string)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// Pure parts (unit-tested)

/// Cached COM values of one session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SessionValues {
    pub volume: f32,
    pub muted: bool,
    pub state: SessionState,
}

/// `AudioSessionReconciliationResult`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReconcileOutcome {
    Current,
    Expired,
    DeviceInvalidated,
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ValueChanges {
    pub volume: bool,
    pub muted: bool,
    pub state: bool,
}

/// `ReconcileWithCoreAudio` after the COM reads (`GetProcessId` first, then state, volume, mute):
/// syncs the caches and classifies the session.
pub(crate) fn reconcile_values(
    values: &mut SessionValues,
    read: windows_core::Result<(SessionState, f32, bool)>,
) -> (ReconcileOutcome, ValueChanges) {
    let mut changes = ValueChanges::default();
    let (state, volume, muted) = match read {
        Ok(read) => read,
        Err(error) if error.code() == AUDCLNT_E_DEVICE_INVALIDATED => {
            return (ReconcileOutcome::DeviceInvalidated, changes);
        }
        Err(_) => return (ReconcileOutcome::Failed, changes),
    };
    if volume.is_finite() {
        let clamped = volume.clamp(0.0, 1.0);
        if (clamped - values.volume).abs() >= VOLUME_EQUALITY_EPSILON {
            values.volume = clamped;
            changes.volume = true;
        }
    }
    if muted != values.muted {
        values.muted = muted;
        changes.muted = true;
    }
    if state != values.state {
        values.state = state;
        changes.state = true;
    }
    let outcome = if state == SessionState::Expired { ReconcileOutcome::Expired } else { ReconcileOutcome::Current };
    (outcome, changes)
}

/// `VolumeThrottle` over `AsyncThrottler`: the first write runs at once; later writes inside the
/// cooldown replace one pending value that runs when the cooldown after the previous write ends.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct VolumeThrottle {
    last_write: Option<Instant>,
    pending: Option<f32>,
}

impl VolumeThrottle {
    /// Some(level) when the write should happen now (then call [`Self::wrote`]).
    pub fn request(&mut self, level: f32, now: Instant) -> Option<f32> {
        let cooling = self.last_write.is_some_and(|at| now < at + VOLUME_WRITE_COOLDOWN);
        if self.pending.is_some() || cooling {
            self.pending = Some(level);
            None
        } else {
            Some(level)
        }
    }

    pub fn wrote(&mut self, at: Instant) {
        self.last_write = Some(at);
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.pending?;
        Some(self.last_write.map_or_else(Instant::now, |at| at + VOLUME_WRITE_COOLDOWN))
    }

    pub fn take_due(&mut self, now: Instant) -> Option<f32> {
        if self.deadline().is_some_and(|due| due <= now) { self.pending.take() } else { None }
    }
}

// ---------------------------------------------------------------------------------------------
// COM callback bridges

#[implement(IAudioSessionNotification)]
struct SessionNotificationBridge {
    device: DeviceId,
    sink: SessionSink,
    alive: Arc<AtomicBool>,
}

impl IAudioSessionNotification_Impl for SessionNotificationBridge_Impl {
    fn OnSessionCreated(&self, new_session: Ref<IAudioSessionControl>) -> windows_core::Result<()> {
        if !self.alive.load(Ordering::Acquire) {
            return Ok(());
        }
        // The caller keeps its reference; the clone (AddRef) is owned by the posted event.
        if let Some(control) = new_session.cloned() {
            (self.sink)(self.device.clone(), SessionEvent(Event::Created(AgileControl(control))));
        }
        Ok(())
    }
}

#[implement(IAudioSessionEvents)]
struct SessionEventsBridge {
    device: DeviceId,
    session: u64,
    sink: SessionSink,
    event_context: GUID,
    alive: Arc<AtomicBool>,
}

impl SessionEventsBridge {
    /// `PostIfAlive`: events stop once the session was removed or disconnected.
    fn post(&self, event: Event) {
        if self.alive.load(Ordering::Acquire) {
            (self.sink)(self.device.clone(), SessionEvent(event));
        }
    }
}

impl IAudioSessionEvents_Impl for SessionEventsBridge_Impl {
    fn OnDisplayNameChanged(&self, new_display_name: &PCWSTR, _event_context: *const GUID) -> windows_core::Result<()> {
        let name = if new_display_name.is_null() {
            String::new()
        } else {
            // SAFETY: COM passes a valid NUL-terminated string for the call's duration.
            unsafe { new_display_name.to_string() }.unwrap_or_default()
        };
        if !name.is_empty() {
            self.post(Event::DisplayNameChanged { session: self.session, name });
        }
        Ok(())
    }

    fn OnIconPathChanged(&self, _new_icon_path: &PCWSTR, _event_context: *const GUID) -> windows_core::Result<()> {
        self.post(Event::IconPathChanged { session: self.session });
        Ok(())
    }

    fn OnSimpleVolumeChanged(
        &self,
        new_volume: f32,
        new_mute: BOOL,
        event_context: *const GUID,
    ) -> windows_core::Result<()> {
        // SAFETY: COM passes a valid GUID pointer (or null) for the call's duration.
        if unsafe { event_context.as_ref() } == Some(&self.event_context) {
            return Ok(()); // Echo of our own write.
        }
        self.post(Event::SimpleVolumeChanged { session: self.session, volume: new_volume, muted: new_mute.as_bool() });
        Ok(())
    }

    fn OnChannelVolumeChanged(
        &self,
        _channel_count: u32,
        _new_channel_volume_array: *const f32,
        _changed_channel: u32,
        _event_context: *const GUID,
    ) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnGroupingParamChanged(
        &self,
        _new_grouping_param: *const GUID,
        _event_context: *const GUID,
    ) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnStateChanged(&self, new_state: AudioSessionState) -> windows_core::Result<()> {
        self.post(Event::StateChanged { session: self.session, state: session_state(new_state) });
        Ok(())
    }

    fn OnSessionDisconnected(&self, reason: AudioSessionDisconnectReason) -> windows_core::Result<()> {
        self.post(Event::Disconnected { session: self.session, reason: Some(reason.0) });
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Sessions and groups

struct Session {
    key: u64,
    control: IAudioSessionControl,
    control2: IAudioSessionControl2,
    simple: ISimpleAudioVolume,
    meter: IAudioMeterInformation,
    events: IAudioSessionEvents,
    alive: Arc<AtomicBool>,
    pid: u32,
    is_system_sounds: bool,
    instance_id: String,
    app_id: AppId,
    /// None = unresolved ("Unknown").
    display_name: Option<String>,
    icon: Option<Arc<IconPixels>>,
    values: SessionValues,
    throttle: VolumeThrottle,
    _exit_watch: Option<ExitWatch>,
}

impl Session {
    /// Stops callbacks and releases the control (`AudioSession.Dispose`).
    fn dispose(self) {
        self.alive.store(false, Ordering::Release);
        // Unregister the process watch first (waits out a running callback).
        drop(self._exit_watch);
        // SAFETY: unregisters the bridge registered in `SessionHost::create_session`.
        if let Err(error) = unsafe { self.control.UnregisterAudioSessionNotification(&self.events) } {
            log::debug!("UnregisterAudioSessionNotification (pid {}) failed: {error}", self.pid);
        }
    }

    fn read_core_audio(&self) -> windows_core::Result<(SessionState, f32, bool)> {
        // SAFETY: plain COM reads on a live control. GetProcessId first: it detects (or transfers)
        // a session invalidated by a default-device switch.
        unsafe {
            self.control2.GetProcessId()?;
            let state = session_state(self.control.GetState()?);
            let volume = self.simple.GetMasterVolume()?;
            let muted = self.simple.GetMute()?.as_bool();
            Ok((state, volume, muted))
        }
    }

    fn current_peak(&self, unified: bool, bias: i32) -> (f32, f32) {
        if self.values.state == SessionState::Active {
            read_meter_peaks(&self.meter, unified, bias)
        } else {
            (0.0, 0.0)
        }
    }
}

struct Group {
    app_id: AppId,
    /// Non-empty; `sessions[0]` is the representative.
    sessions: Vec<Session>,
    ding: DingSuppressionPeak,
}

impl Group {
    /// Active if any session is Active; Inactive if any is not Expired; else Expired.
    fn state(&self) -> SessionState {
        let mut inactive = false;
        for session in &self.sessions {
            match session.values.state {
                SessionState::Active => return SessionState::Active,
                SessionState::Inactive => inactive = true,
                SessionState::Expired => {}
            }
        }
        if inactive { SessionState::Inactive } else { SessionState::Expired }
    }
}

pub struct SessionHost {
    device_id: DeviceId,
    flow: Flow,
    manager: IAudioSessionManager2,
    notification: IAudioSessionNotification,
    notification_alive: Arc<AtomicBool>,
    config: SessionHostConfig,
    icons: Arc<IconCache>,
    sink: SessionSink,
    groups: Vec<Group>,
    instance_ids: HashSet<String>,
    next_session: u64,
    exclusive_held: bool,
}

impl SessionHost {
    /// Registers session notifications, then enumerates existing sessions (that order, so none is
    /// missed). Audio thread only.
    pub fn attach(
        device_id: DeviceId,
        flow: Flow,
        manager: IAudioSessionManager2,
        config: SessionHostConfig,
        icons: Arc<IconCache>,
        sink: SessionSink,
    ) -> windows_core::Result<Self> {
        let notification_alive = Arc::new(AtomicBool::new(true));
        let notification: IAudioSessionNotification = SessionNotificationBridge {
            device: device_id.clone(),
            sink: sink.clone(),
            alive: notification_alive.clone(),
        }
        .into();
        // SAFETY: registers our bridge; unregistered in `Drop`.
        unsafe { manager.RegisterSessionNotification(&notification) }?;
        let mut host = Self {
            device_id,
            flow,
            manager,
            notification,
            notification_alive,
            config,
            icons,
            sink,
            groups: Vec::new(),
            instance_ids: HashSet::new(),
            next_session: 0,
            exclusive_held: false,
        };
        host.enumerate_existing();
        Ok(host)
    }

    pub fn handle(&mut self, event: SessionEvent) -> HostChange {
        match event.0 {
            Event::Created(control) => {
                if self.add_session(control.0) {
                    HostChange::structural()
                } else {
                    HostChange::default()
                }
            }
            Event::DisplayNameChanged { session, name } => self.on_display_name_changed(session, name),
            Event::IconPathChanged { session } => {
                if let Some((g, s)) = self.find(session) {
                    self.request_icon(&self.groups[g].sessions[s], IconJobKind::IconPathChanged);
                }
                HostChange::default()
            }
            Event::SimpleVolumeChanged { session, volume, muted } => {
                self.on_simple_volume_changed(session, volume, muted)
            }
            Event::StateChanged { session, state } => self.on_state_changed(session, state),
            Event::Disconnected { session, reason } => {
                let Some((g, s)) = self.find(session) else { return HostChange::default() };
                if reason == Some(DisconnectReasonExclusiveModeOverride.0) {
                    // Another app initialized the endpoint exclusively and kicked shared sessions off.
                    self.exclusive_held = true;
                }
                self.remove_session(g, s);
                HostChange::structural()
            }
            Event::WriteFailed { session } => {
                let Some((g, s)) = self.find(session) else { return HostChange::default() };
                log::debug!("SessionHost: retiring invalidated session pid={}", self.groups[g].sessions[s].pid);
                self.remove_session(g, s);
                HostChange::structural()
            }
            Event::Metadata { session, icon, name, clear_icon_on_none } => {
                self.on_metadata(session, icon, name, clear_icon_on_none)
            }
        }
    }

    /// Re-enumerates sessions (`ReconcileSessions`): adds missing, drops dead.
    pub fn reconcile(&mut self) -> HostChange {
        let mut change = HostChange::default();
        let keys: Vec<u64> = self.groups.iter().flat_map(|g| g.sessions.iter().map(|s| s.key)).collect();
        for key in keys {
            let Some((g, s)) = self.find(key) else { continue };
            let group_state = self.groups[g].state();
            let session = &mut self.groups[g].sessions[s];
            let read = session.read_core_audio();
            let (outcome, changes) = reconcile_values(&mut session.values, read);
            match outcome {
                ReconcileOutcome::Expired => {
                    self.remove_session(g, s);
                    change |= HostChange::structural();
                }
                ReconcileOutcome::DeviceInvalidated => {
                    log::debug!(
                        "SessionHost.reconcile: retiring invalidated session pid={} on {}",
                        session.pid,
                        self.device_id
                    );
                    self.remove_session(g, s);
                    change |= HostChange::structural();
                }
                ReconcileOutcome::Failed => {
                    log::warn!(
                        "SessionHost.reconcile: reading session pid={} on {} failed",
                        session.pid,
                        self.device_id
                    );
                }
                ReconcileOutcome::Current => {
                    let representative = s == 0;
                    if changes.state && session.values.state == SessionState::Active {
                        self.reresolve(g, s);
                    }
                    if representative && changes.volume {
                        change |= HostChange::values();
                    }
                    if (representative && changes.muted) || self.groups[g].state() != group_state {
                        change |= HostChange::structural();
                    }
                }
            }
        }
        // Registration stays active while enumerating; the instance-id set deduplicates.
        if self.enumerate_existing() {
            change |= HostChange::structural();
        }
        change
    }

    /// Groups in insertion order.
    pub fn groups(&self) -> Vec<AppGroupSnapshot> {
        self.groups
            .iter()
            .map(|group| {
                let rep = &group.sessions[0];
                AppGroupSnapshot {
                    app_id: group.app_id.clone(),
                    display_name: rep.display_name.clone(),
                    icon: rep.icon.clone(),
                    pid: rep.pid,
                    is_system_sounds: rep.is_system_sounds,
                    state: group.state(),
                    volume: rep.values.volume,
                    muted: rep.values.muted,
                    session_count: u16::try_from(group.sessions.len()).unwrap_or(u16::MAX),
                    // `IsExclusiveControlHolder` is never detected by the .NET app either.
                    is_exclusive: false,
                }
            })
            .collect()
    }

    /// Sets the cached level at once (returned by `groups`); the COM write to every session of the
    /// group is throttled (30 ms, latest wins) and performed by [`SessionHost::flush_due`].
    /// Returns true when any session's cached level changed.
    pub fn set_group_volume(&mut self, app: &AppId, level: f32) -> bool {
        let Some(g) = self.group_index(app) else { return false };
        let clamped = if level.is_nan() { 0.0 } else { level.clamp(0.0, 1.0) };
        let now = Instant::now();
        let mut changed = false;
        for s in 0..self.groups[g].sessions.len() {
            let session = &mut self.groups[g].sessions[s];
            if (clamped - session.values.volume).abs() < VOLUME_EQUALITY_EPSILON {
                continue;
            }
            session.values.volume = clamped;
            changed = true;
            if let Some(write) = session.throttle.request(clamped, now) {
                self.write_volume(g, s, write);
            }
        }
        changed
    }

    /// Fans the mute out to every session (synchronous `SetMute`); a session caches the value only
    /// when its write succeeded. Returns true when any session changed.
    pub fn set_group_mute(&mut self, app: &AppId, muted: bool) -> bool {
        let Some(g) = self.group_index(app) else { return false };
        let mut changed = false;
        for s in 0..self.groups[g].sessions.len() {
            changed |= self.set_session_mute(g, s, muted);
        }
        changed
    }

    /// Earliest pending throttled write.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.groups.iter().flat_map(|g| g.sessions.iter()).filter_map(|s| s.throttle.deadline()).min()
    }

    /// Performs throttled writes that are due.
    pub fn flush_due(&mut self, now: Instant) {
        for g in 0..self.groups.len() {
            for s in 0..self.groups[g].sessions.len() {
                if let Some(level) = self.groups[g].sessions[s].throttle.take_due(now) {
                    self.write_volume(g, s, level);
                }
            }
        }
    }

    /// Reads peaks of the groups `visible` accepts (aggregate: max of session mins, max of
    /// session maxes; non-Active sessions read as silence) and feeds each such group's decaying
    /// ding-suppression peak. Hidden groups are not read (their meters freeze, as in the .NET app).
    pub fn sample_group_peaks(
        &mut self,
        visible: &dyn Fn(&AppId) -> bool,
        unified: bool,
        low_channel_bias: i32,
        now: Instant,
        out: &mut dyn FnMut(&AppId, (f32, f32)),
    ) {
        for group in &mut self.groups {
            if !visible(&group.app_id) {
                continue;
            }
            let mut aggregate = (0.0f32, 0.0f32);
            for session in &group.sessions {
                let (min, max) = session.current_peak(unified, low_channel_bias);
                aggregate.0 = aggregate.0.max(min);
                aggregate.1 = aggregate.1.max(max);
            }
            group.ding.observe(aggregate.1, now);
            out(&group.app_id, aggregate);
        }
    }

    /// Recent decaying peak of a group for ding suppression (None = unavailable).
    pub fn group_ding_peak(&mut self, app: &AppId, now: Instant) -> Option<f32> {
        let g = self.group_index(app)?;
        Some(self.groups[g].ding.read(now))
    }

    /// `ReadExternalSessionPeak`: the loudest current (non-unified) session peak not owned by this
    /// process, for the device ding-suppression bypass window. 0 when there is none.
    pub fn external_peak(&self) -> f32 {
        self.groups
            .iter()
            .flat_map(|g| g.sessions.iter())
            .filter(|s| s.pid != self.config.own_pid)
            .map(|s| s.current_peak(false, 0).1)
            .fold(0.0, f32::max)
    }

    pub fn group_volume(&self, app: &AppId) -> Option<f32> {
        self.group_index(app).map(|g| self.groups[g].sessions[0].values.volume)
    }

    /// The endpoint's data flow.
    pub fn flow(&self) -> Flow {
        self.flow
    }

    /// Any group Active (render endpoint meter pin rule, capture sleeping glyph).
    pub fn any_group_active(&self) -> bool {
        self.groups.iter().any(|g| g.state() == SessionState::Active)
    }

    /// A session disconnected with `DisconnectReasonExclusiveModeOverride`; cleared when a new
    /// session is added.
    pub fn exclusive_held(&self) -> bool {
        self.exclusive_held
    }

    /// Unregisters every notification and releases every session.
    pub fn detach(self) {
        drop(self);
    }

    // -----------------------------------------------------------------------------------------

    fn group_index(&self, app: &str) -> Option<usize> {
        self.groups.iter().position(|g| &*g.app_id == app)
    }

    fn find(&self, key: u64) -> Option<(usize, usize)> {
        self.groups
            .iter()
            .enumerate()
            .find_map(|(g, group)| group.sessions.iter().position(|s| s.key == key).map(|s| (g, s)))
    }

    fn enumerate_existing(&mut self) -> bool {
        // SAFETY: plain COM calls on the live session manager.
        let enumerator = match unsafe { self.manager.GetSessionEnumerator() } {
            Ok(enumerator) => enumerator,
            Err(error) => {
                log::debug!("SessionHost: GetSessionEnumerator on {} failed: {error}", self.device_id);
                return false;
            }
        };
        // SAFETY: as above.
        let count = unsafe { enumerator.GetCount() }.unwrap_or(0);
        let mut changed = false;
        for index in 0..count {
            // SAFETY: as above; best effort (the device may disconnect mid-enumeration).
            match unsafe { enumerator.GetSession(index) } {
                Ok(control) => changed |= self.add_session(control),
                Err(_) => break,
            }
        }
        changed
    }

    /// `AudioDevice.AddSession`. True when the session was published.
    fn add_session(&mut self, control: IAudioSessionControl) -> bool {
        // Cheap identity pre-check so a re-enumerated control does not spin up a wrapper.
        let preflight = read_session_instance_id(&control);
        if !preflight.is_empty() && self.instance_ids.contains(&preflight) {
            return false;
        }

        let session = match self.create_session(control) {
            Ok(session) => session,
            Err(error) => {
                // The session may already be torn down (race with OnSessionCreated).
                log::debug!("SessionHost: wrapping a session on {} failed: {error}", self.device_id);
                return false;
            }
        };
        // An enumerator can retain a control after its last stream expired; never publish it.
        if session.values.state == SessionState::Expired {
            session.dispose();
            return false;
        }
        if !session.instance_id.is_empty() && self.instance_ids.contains(&session.instance_id) {
            session.dispose();
            return false;
        }
        if !session.instance_id.is_empty() {
            self.instance_ids.insert(session.instance_id.clone());
        }

        self.request_icon(&session, IconJobKind::Initial { resolve_name: session.display_name.is_none() });
        match self.group_index(&session.app_id) {
            Some(g) => {
                self.groups[g].sessions.push(session);
                let s = self.groups[g].sessions.len() - 1;
                // A new session inherits the group's mute so a fresh child process does not unmute
                // an app the user silenced.
                let muted = self.groups[g].sessions[0].values.muted || self.groups[g].sessions[s].values.muted;
                self.set_session_mute(g, s, muted);
            }
            None => self.groups.push(Group {
                app_id: session.app_id.clone(),
                sessions: vec![session],
                ding: DingSuppressionPeak::default(),
            }),
        }
        // A new shared session implies exclusive control was released.
        self.exclusive_held = false;
        true
    }

    /// `AudioSession` constructor: synchronous identity/value reads, event registration, process
    /// exit watch.
    fn create_session(&mut self, control: IAudioSessionControl) -> windows_core::Result<Session> {
        let control2: IAudioSessionControl2 = control.cast()?;
        let simple: ISimpleAudioVolume = control.cast()?;
        let meter: IAudioMeterInformation = control.cast()?;
        // SAFETY: plain COM reads on a live control.
        let (pid, is_system_sounds) = unsafe { (control2.GetProcessId()?, control2.IsSystemSoundsSession() == S_OK) };
        // SAFETY: as above.
        let instance_id =
            unsafe { control2.GetSessionInstanceIdentifier() }.ok().and_then(take_co_string).unwrap_or_default();

        let (display_name, app_id): (Option<String>, AppId) = if is_system_sounds {
            (Some(SYSTEM_SOUNDS_NAME.to_owned()), SYSTEM_SOUNDS_APP_ID.into())
        } else {
            let app_id = match image_path(pid).filter(|path| !path.is_empty()) {
                Some(path) => path.to_lowercase(),
                None => format!("pid:{pid}"),
            };
            (read_session_display_name(&control), app_id.into())
        };

        // SAFETY: plain COM reads on a live control.
        let values = unsafe {
            SessionValues {
                volume: simple.GetMasterVolume()?,
                muted: simple.GetMute()?.as_bool(),
                state: session_state(control.GetState()?),
            }
        };

        self.next_session += 1;
        let key = self.next_session;
        let alive = Arc::new(AtomicBool::new(true));
        let events: IAudioSessionEvents = SessionEventsBridge {
            device: self.device_id.clone(),
            session: key,
            sink: self.sink.clone(),
            event_context: self.config.event_context,
            alive: alive.clone(),
        }
        .into();
        // SAFETY: registers our bridge; unregistered in `Session::dispose`.
        unsafe { control.RegisterAudioSessionNotification(&events) }?;

        // Force-killed apps do not always expire promptly; the process handle signals at once.
        // System sounds (pid 0) and processes this user cannot open are not watched.
        let mut exit_watch = None;
        if pid > 0 && pid <= i32::MAX as u32 && !is_system_sounds {
            let sink = self.sink.clone();
            let device = self.device_id.clone();
            let on_exit = Box::new(move || {
                sink(device.clone(), SessionEvent(Event::Disconnected { session: key, reason: None }));
            });
            match watch_exit(pid, on_exit) {
                ExitWatchResult::Watching(watch) => exit_watch = Some(watch),
                ExitWatchResult::Exited => {
                    (self.sink)(
                        self.device_id.clone(),
                        SessionEvent(Event::Disconnected { session: key, reason: None }),
                    );
                }
                ExitWatchResult::Unavailable => {}
            }
        }

        Ok(Session {
            key,
            control,
            control2,
            simple,
            meter,
            events,
            alive,
            pid,
            is_system_sounds,
            instance_id,
            app_id,
            display_name,
            icon: None,
            values,
            throttle: VolumeThrottle::default(),
            _exit_watch: exit_watch,
        })
    }

    fn request_icon(&self, session: &Session, kind: IconJobKind) {
        self.icons.request(IconJob {
            device: self.device_id.clone(),
            session: session.key,
            app_id: session.app_id.clone(),
            pid: session.pid,
            is_system_sounds: session.is_system_sounds,
            control: AgileControl(session.control.clone()),
            alive: session.alive.clone(),
            sink: self.sink.clone(),
            kind,
        });
    }

    /// `TryReresolveProcessMetadata` when a session goes Active with a placeholder name or no icon.
    fn reresolve(&self, g: usize, s: usize) {
        let session = &self.groups[g].sessions[s];
        if session.is_system_sounds {
            return;
        }
        let name = session.display_name.is_none();
        let icon = session.icon.is_none();
        if name || icon {
            self.request_icon(session, IconJobKind::Reresolve { name, icon });
        }
    }

    fn remove_session(&mut self, g: usize, s: usize) {
        let session = self.groups[g].sessions.remove(s);
        if !session.instance_id.is_empty() {
            self.instance_ids.remove(&session.instance_id);
        }
        session.dispose();
        if self.groups[g].sessions.is_empty() {
            self.groups.remove(g);
        }
    }

    fn write_volume(&mut self, g: usize, s: usize, level: f32) {
        let session = &mut self.groups[g].sessions[s];
        // SAFETY: plain COM write on a live control; the event context marks the echo as ours.
        let result = unsafe { session.simple.SetMasterVolume(level, &self.config.event_context) };
        session.throttle.wrote(Instant::now());
        if let Err(error) = result {
            self.on_write_failed(g, s, &error);
        }
    }

    /// Synchronous `SetMute`; caches only on success. True when the cached value changed.
    fn set_session_mute(&mut self, g: usize, s: usize, muted: bool) -> bool {
        let session = &self.groups[g].sessions[s];
        if session.values.muted == muted {
            return false;
        }
        // SAFETY: plain COM write on a live control; the event context marks the echo as ours.
        match unsafe { session.simple.SetMute(muted, &self.config.event_context) } {
            Ok(()) => {
                self.groups[g].sessions[s].values.muted = muted;
                true
            }
            Err(error) => {
                self.on_write_failed(g, s, &error);
                false
            }
        }
    }

    /// `OnControlWriteFailed`: an invalidated control (stream moved to another endpoint) retires
    /// the session through a posted event.
    fn on_write_failed(&self, g: usize, s: usize, error: &windows_core::Error) {
        let session = &self.groups[g].sessions[s];
        if error.code() == AUDCLNT_E_DEVICE_INVALIDATED {
            if session.alive.load(Ordering::Acquire) {
                (self.sink)(self.device_id.clone(), SessionEvent(Event::WriteFailed { session: session.key }));
            }
        } else {
            log::debug!("SessionHost: session write (pid {}) failed: {error}", session.pid);
        }
    }

    fn on_display_name_changed(&mut self, key: u64, name: String) -> HostChange {
        let Some((g, s)) = self.find(key) else { return HostChange::default() };
        let session = &mut self.groups[g].sessions[s];
        if session.display_name.as_deref() == Some(name.as_str()) {
            return HostChange::default();
        }
        session.display_name = Some(name);
        if s == 0 { HostChange::structural() } else { HostChange::default() }
    }

    fn on_simple_volume_changed(&mut self, key: u64, volume: f32, muted: bool) -> HostChange {
        let Some((g, s)) = self.find(key) else { return HostChange::default() };
        let session = &mut self.groups[g].sessions[s];
        let mut change = HostChange::default();
        if (volume - session.values.volume).abs() >= VOLUME_EQUALITY_EPSILON {
            session.values.volume = volume;
            if s == 0 {
                change |= HostChange::values();
            }
        }
        if muted != session.values.muted {
            session.values.muted = muted;
            if s == 0 {
                change |= HostChange::structural();
            }
        }
        change
    }

    fn on_state_changed(&mut self, key: u64, state: SessionState) -> HostChange {
        let Some((g, s)) = self.find(key) else { return HostChange::default() };
        let group_state = self.groups[g].state();
        self.groups[g].sessions[s].values.state = state;
        if state == SessionState::Expired {
            // The app stopped using the endpoint.
            self.remove_session(g, s);
            return HostChange::structural();
        }
        if state == SessionState::Active {
            self.reresolve(g, s);
        }
        if self.groups[g].state() != group_state { HostChange::structural() } else { HostChange::default() }
    }

    fn on_metadata(
        &mut self,
        key: u64,
        icon: Option<Arc<IconPixels>>,
        name: Option<String>,
        clear_icon_on_none: bool,
    ) -> HostChange {
        let Some((g, s)) = self.find(key) else { return HostChange::default() };
        let session = &mut self.groups[g].sessions[s];
        let mut changed = false;
        match icon {
            Some(icon) => {
                if !session.icon.as_ref().is_some_and(|current| Arc::ptr_eq(current, &icon)) {
                    session.icon = Some(icon);
                    changed = true;
                }
            }
            None if clear_icon_on_none && session.icon.is_some() => {
                session.icon = None;
                changed = true;
            }
            None => {}
        }
        if let Some(name) = name
            && session.display_name.as_deref() != Some(name.as_str())
        {
            session.display_name = Some(name);
            changed = true;
        }
        if changed && s == 0 { HostChange::structural() } else { HostChange::default() }
    }
}

impl Drop for SessionHost {
    fn drop(&mut self) {
        self.notification_alive.store(false, Ordering::Release);
        // SAFETY: unregisters the bridge registered in `attach`.
        if let Err(error) = unsafe { self.manager.UnregisterSessionNotification(&self.notification) } {
            log::debug!("SessionHost: UnregisterSessionNotification on {} failed: {error}", self.device_id);
        }
        for group in self.groups.drain(..) {
            for session in group.sessions {
                session.dispose();
            }
        }
        self.instance_ids.clear();
    }
}

#[cfg(test)]
mod tests {
    //! Pure parts of `AudioSessionReconciliationTests.cs` and the volume throttle.
    use super::*;

    fn values(state: SessionState, volume: f32) -> SessionValues {
        SessionValues { volume, muted: false, state }
    }

    #[test]
    fn reconcile_refreshes_state_and_volume_from_current_control() {
        let mut current = values(SessionState::Active, 1.0);
        let (outcome, changes) = reconcile_values(&mut current, Ok((SessionState::Inactive, 0.77, true)));
        assert_eq!(outcome, ReconcileOutcome::Current);
        assert_eq!(current, SessionValues { volume: 0.77, muted: true, state: SessionState::Inactive });
        assert_eq!(changes, ValueChanges { volume: true, muted: true, state: true });
    }

    #[test]
    fn reconcile_retires_control_invalidated_by_default_device_switch() {
        let mut current = values(SessionState::Active, 1.0);
        let error = windows_core::Error::from_hresult(AUDCLNT_E_DEVICE_INVALIDATED);
        let (outcome, changes) = reconcile_values(&mut current, Err(error));
        assert_eq!(outcome, ReconcileOutcome::DeviceInvalidated);
        assert_eq!(changes, ValueChanges::default());
    }

    #[test]
    fn reconcile_reports_already_expired_control() {
        let mut current = values(SessionState::Inactive, 0.5);
        let (outcome, _) = reconcile_values(&mut current, Ok((SessionState::Expired, 0.5, false)));
        assert_eq!(outcome, ReconcileOutcome::Expired);
        assert_eq!(current.state, SessionState::Expired);
    }

    #[test]
    fn reconcile_reports_other_failures_without_touching_the_cache() {
        let mut current = values(SessionState::Active, 0.3);
        let error = windows_core::Error::from_hresult(windows::Win32::Foundation::E_FAIL);
        let (outcome, _) = reconcile_values(&mut current, Err(error));
        assert_eq!(outcome, ReconcileOutcome::Failed);
        assert_eq!(current, values(SessionState::Active, 0.3));
    }

    #[test]
    fn reconcile_clamps_and_ignores_non_finite_volume_and_sub_epsilon_drift() {
        let mut current = values(SessionState::Active, 0.5);
        let (_, changes) = reconcile_values(&mut current, Ok((SessionState::Active, f32::NAN, false)));
        assert_eq!((current.volume, changes), (0.5, ValueChanges::default()));
        let (_, changes) = reconcile_values(&mut current, Ok((SessionState::Active, 0.500_000_5, false)));
        assert!(!changes.volume);
        let (_, changes) = reconcile_values(&mut current, Ok((SessionState::Active, 1.5, false)));
        assert!(changes.volume);
        assert_eq!(current.volume, 1.0);
    }

    #[test]
    fn volume_throttle_writes_first_then_latest_after_cooldown() {
        let start = Instant::now();
        let mut throttle = VolumeThrottle::default();
        assert_eq!(throttle.request(0.1, start), Some(0.1));
        throttle.wrote(start);
        assert_eq!(throttle.deadline(), None);

        // Inside the cooldown: values replace each other, latest wins.
        assert_eq!(throttle.request(0.2, start + Duration::from_millis(5)), None);
        assert_eq!(throttle.request(0.3, start + Duration::from_millis(10)), None);
        assert_eq!(throttle.deadline(), Some(start + VOLUME_WRITE_COOLDOWN));
        assert_eq!(throttle.take_due(start + Duration::from_millis(29)), None);
        assert_eq!(throttle.take_due(start + VOLUME_WRITE_COOLDOWN), Some(0.3));
        throttle.wrote(start + VOLUME_WRITE_COOLDOWN);

        // A request after the cooldown writes at once again.
        let later = start + VOLUME_WRITE_COOLDOWN * 3;
        assert_eq!(throttle.request(0.4, later), Some(0.4));

        // Inside the cooldown again: pending until the cooldown ends.
        throttle.wrote(later);
        assert_eq!(throttle.request(0.5, later), None);
        assert_eq!(throttle.deadline(), Some(later + VOLUME_WRITE_COOLDOWN));
    }
}
