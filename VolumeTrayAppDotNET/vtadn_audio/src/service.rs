//! The audio thread (port of `AudioDeviceManager`): owns the enumerator, every endpoint and
//! session host, handles commands and notifications in batches, drives every timer from one
//! deadline-driven loop, and publishes snapshots and meters.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Instant;

use vtadn_model::IDENTITY;
use vtadn_model::bus::{AudioEvent, AudioPublisher};
use vtadn_model::command::{AudioCommand, FeedbackTiming, FormatQuery};
use vtadn_model::config::AudioBackendConfig;
use vtadn_model::policy::ding::should_suppress;
use vtadn_model::snapshot::{
    AppId, AudioSnapshot, Defaults, DeviceId, DeviceSnapshot, DuckingMode, EqApoState, Flow, Format,
};
use vtadn_native::bluetooth::{BluetoothCoordinator, BtEvent, BtSink};
use vtadn_native::ducking::{self, DuckingWatcher};
use vtadn_native::eqapo::{self, AvailabilityWatcher, EqApoToggle};
use vtadn_native::feedback::FeedbackPlayer;
use vtadn_sessions::{CaptureMeterActivation, IconCache, SessionEvent, SessionHostConfig};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    DEVICE_STATE, DEVICE_STATEMASK_ALL, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator, IMMEndpoint,
    IMMNotificationClient, MMDeviceEnumerator, eCapture, eCommunications, eConsole, eMultimedia, eRender,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_core::{HSTRING, Interface};

use crate::classify::{Change, classify};
use crate::com::MtaScope;
use crate::consts::{
    DEFAULTS_REFRESH_COALESCE_DWELL_MS, DEVICE_LIST_REFRESH_AFTER_MISSING_DEFAULT_MS,
    DEVICE_LIST_REFRESH_AFTER_RESUME_MS, VOLUME_EQUALITY_EPSILON, VOLUME_FEEDBACK_DING_DELAY_MS,
    VOLUME_FEEDBACK_DING_DWELL_POLL_SLICE_MS, VOLUME_FEEDBACK_DING_METER_BYPASS_GRACE_MS,
};
use crate::defaults::{self, EndpointRef, Role, RoleIds, SLOTS};
use crate::endpoint::{self, ActivationEnv, Endpoint};
use crate::format::{self, build_format_blob, container_bits, parse_format_blob};
use crate::notify::{NotificationClient, flow_from};
use crate::policy_config::{CycleRoles, DefaultChange, DefaultChangeHub, PolicyRunner};
use crate::power::PowerRegistration;
use crate::props::WatchedProperty;
use crate::throttle::{Debounce, Ticker, sample_interval};

/// Everything the audio thread reacts to.
pub enum Msg {
    Command(AudioCommand),
    DeviceStateChanged {
        id: String,
        state: u32,
    },
    DeviceAdded(String),
    DeviceRemoved(String),
    DefaultChanged(DefaultChange),
    PropertyChanged {
        id: String,
        property: WatchedProperty,
    },
    /// Foreign endpoint volume/mute change: re-read.
    EndpointVolume(DeviceId),
    /// Routed back to the host of `device` if it is still the host of `generation`.
    Session {
        device: DeviceId,
        generation: u64,
        event: SessionEvent,
    },
    Bt(BtEvent),
    EqApoAvailabilityChanged,
    /// Re-probe one endpoint after its EAPO toggle ran on a worker.
    EqApoRefresh(DeviceId),
    DuckingChanged,
    Resumed,
}

/// Feedback throttler keys: `device:<id>` and `app`.
#[derive(Clone, Debug, PartialEq)]
enum FeedbackTarget {
    Device(DeviceId),
    App { device: DeviceId, app: AppId, level: f32 },
}

impl FeedbackTarget {
    fn same_key(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Device(a), Self::Device(b)) => a == b,
            (Self::App { .. }, Self::App { .. }) => true,
            _ => false,
        }
    }
}

/// A pending ding: suppression is re-checked every poll slice until `deadline`, then it plays.
struct FeedbackJob {
    target: FeedbackTarget,
    deadline: Instant,
    next_check: Instant,
}

fn edata_flow(flow: Flow) -> EDataFlow {
    match flow {
        Flow::Render => eRender,
        Flow::Capture => eCapture,
    }
}

fn erole(role: Role) -> ERole {
    match role {
        Role::Multimedia => eMultimedia,
        Role::Communications => eCommunications,
    }
}

/// `is_elevated`: TokenElevation of the process token.
fn process_is_elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: querying this process's own token; the handle is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    // SAFETY: the buffer is a TOKEN_ELEVATION of the size passed.
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::from_mut(&mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    // SAFETY: closing the token opened above.
    if let Err(error) = unsafe { CloseHandle(token) } {
        log::warn!("CloseHandle(process token) failed: {error}");
    }
    queried.is_ok() && elevation.TokenIsElevated != 0
}

fn create_enumerator() -> windows_core::Result<IMMDeviceEnumerator> {
    // SAFETY: plain COM activation on the audio (MTA) thread.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

/// Displayed name: the Bluetooth custom name from devices.xml when set, else the Windows name.
fn display_name(endpoint: &Endpoint, is_bluetooth: bool, custom_names: &HashMap<String, String>) -> String {
    if is_bluetooth && let Some(custom) = custom_names.get(&*endpoint.id).filter(|name| !name.trim().is_empty()) {
        return custom.clone();
    }
    endpoint.system_name.clone()
}

fn non_negative(value: i32) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(0)
}

pub struct Service {
    tx: Sender<Msg>,
    publisher: AudioPublisher,
    config: AudioBackendConfig,
    /// Persisted `LastKnownDefault*DeviceID`; owned by the service after start (it is the only
    /// writer; the UI persists `AudioEvent::LastKnownDefaults`).
    last_known: RoleIds,
    /// `_notified*DeviceID`.
    notified: RoleIds,
    /// `_renderSessionReconciliationPending` / `_captureSessionReconciliationPending`.
    reconcile_pending: [bool; 2],
    enumerator: Option<IMMDeviceEnumerator>,
    notification_client: IMMNotificationClient,
    policy: PolicyRunner,
    endpoints: Vec<Endpoint>,
    defaults: Defaults,
    icons: Arc<IconCache>,
    host_config: SessionHostConfig,
    next_host_generation: u64,
    bluetooth: Option<BluetoothCoordinator>,
    capture_activation: Option<CaptureMeterActivation>,
    capture_requested: Vec<DeviceId>,
    feedback: FeedbackPlayer,
    feedback_jobs: Vec<FeedbackJob>,
    eq_apo_available: bool,
    eq_apo_watcher: Option<AvailabilityWatcher>,
    ducking: DuckingMode,
    ducking_watcher: Option<DuckingWatcher>,
    flyout_active: bool,
    meter_ticker: Ticker,
    visible_groups: HashMap<DeviceId, HashSet<AppId>>,
    scratch_devices: Vec<(DeviceId, (f32, f32))>,
    scratch_groups: Vec<((DeviceId, AppId), (f32, f32))>,
    prune_meters: bool,
    defaults_refresh: Debounce,
    missing_default_refresh: Debounce,
    resume_refresh: Debounce,
    missing_default_recovery_armed: bool,
    missing_default_reason: &'static str,
    is_elevated: bool,
    ready: bool,
    dirty: bool,
    last_snapshot: Arc<AudioSnapshot>,
    power: Option<PowerRegistration>,
}

/// Thread entry: joins the MTA, runs until `AudioCommand::Shutdown`, then tears down.
pub fn run(config: AudioBackendConfig, publisher: AudioPublisher, tx: Sender<Msg>, rx: Receiver<Msg>) {
    let Some(_apartment) = MtaScope::enter("AudioService") else { return };
    let mut service = Service::new(config, publisher, tx);
    service.start();
    service.run_loop(&rx);
    service.shutdown();
}

impl Service {
    fn new(config: AudioBackendConfig, publisher: AudioPublisher, tx: Sender<Msg>) -> Self {
        let hub = Arc::new(DefaultChangeHub::default());
        let notification_client = NotificationClient::create(tx.clone(), hub.clone());
        let enumerator = match create_enumerator() {
            Ok(enumerator) => {
                // SAFETY: registering our callback object on our enumerator.
                if let Err(error) = unsafe { enumerator.RegisterEndpointNotificationCallback(&notification_client) } {
                    log::error!("AudioDeviceManager: RegisterEndpointNotificationCallback failed: {error}");
                }
                Some(enumerator)
            }
            Err(error) => {
                log::error!("AudioDeviceManager: creating the device enumerator failed: {error}");
                None
            }
        };

        let bt_tx = tx.clone();
        let bt_sink: BtSink = Arc::new(move |event| {
            let _ = bt_tx.send(Msg::Bt(event));
        });
        let eq_tx = tx.clone();
        let eq_apo_watcher = AvailabilityWatcher::start(Arc::new(move || {
            let _ = eq_tx.send(Msg::EqApoAvailabilityChanged);
        }));
        let last_known = RoleIds::new(
            config.last_known_default_playback.clone(),
            config.last_known_default_comms_playback.clone(),
            config.last_known_default_recording.clone(),
            config.last_known_default_comms_recording.clone(),
        );

        Self {
            policy: PolicyRunner::new(hub),
            notification_client,
            enumerator,
            icons: IconCache::new(non_negative(config.icon_retry_interval_ms), non_negative(config.icon_lru_limit)),
            host_config: SessionHostConfig { event_context: IDENTITY.app_guid, own_pid: std::process::id() },
            bluetooth: Some(BluetoothCoordinator::new(bt_sink, config.show_device_codec_text)),
            capture_activation: Some(CaptureMeterActivation::new()),
            feedback: FeedbackPlayer::new(),
            eq_apo_available: eqapo::is_available(),
            eq_apo_watcher: Some(eq_apo_watcher),
            ducking: ducking::read_mode(),
            power: PowerRegistration::register(tx.clone()),
            is_elevated: process_is_elevated(),
            last_known,
            config,
            publisher,
            tx,
            notified: RoleIds::default(),
            reconcile_pending: [false; 2],
            endpoints: Vec::new(),
            defaults: Defaults::default(),
            next_host_generation: 0,
            capture_requested: Vec::new(),
            feedback_jobs: Vec::new(),
            ducking_watcher: None,
            flyout_active: false,
            meter_ticker: Ticker::default(),
            visible_groups: HashMap::new(),
            scratch_devices: Vec::new(),
            scratch_groups: Vec::new(),
            prune_meters: false,
            defaults_refresh: Debounce::default(),
            missing_default_refresh: Debounce::default(),
            resume_refresh: Debounce::default(),
            missing_default_recovery_armed: false,
            missing_default_reason: "",
            ready: false,
            dirty: true,
            last_snapshot: Arc::new(AudioSnapshot::default()),
        }
    }

    fn start(&mut self) {
        if !self.rebuild_device_list() {
            log::error!("AudioDeviceManager: initial device enumeration failed");
        }
        self.ready = true;
        self.dirty = true;
        self.publish_if_dirty();
    }

    fn run_loop(&mut self, rx: &Receiver<Msg>) {
        loop {
            let first = match self.next_deadline() {
                Some(deadline) => match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(msg) => Some(msg),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => return,
                },
            };
            if let Some(msg) = first
                && !self.handle(msg)
            {
                return;
            }
            while let Ok(msg) = rx.try_recv() {
                if !self.handle(msg) {
                    return;
                }
            }
            self.run_due(Instant::now());
            self.publish_if_dirty();
        }
    }

    /// Manager `Dispose` order (power, meters, capture activation, EAPO watcher, notification
    /// client, endpoints with their callbacks and hosts, enumerator), except that the Bluetooth
    /// coordinator goes last: its codec monitor may need seconds to retire its ETW session, and
    /// the COM teardown should finish within `AudioService::shutdown`'s bounded wait first.
    fn shutdown(mut self) {
        self.power = None;
        self.meter_ticker.stop();
        self.ducking_watcher = None;
        if let Some(capture) = self.capture_activation.take() {
            capture.shutdown();
        }
        self.eq_apo_watcher = None;
        if let Some(enumerator) = &self.enumerator {
            // SAFETY: unregistering the callback registered on this enumerator.
            if let Err(error) = unsafe { enumerator.UnregisterEndpointNotificationCallback(&self.notification_client) }
            {
                log::warn!("AudioDeviceManager.Dispose: notification unregister failed: {error}");
            }
        }
        for endpoint in &mut self.endpoints {
            endpoint.deactivate();
        }
        self.endpoints.clear();
        self.enumerator = None;
        if let Some(bluetooth) = self.bluetooth.take() {
            bluetooth.shutdown();
        }
    }

    // ---------------------------------------------------------------- scheduling

    fn next_deadline(&self) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        let mut consider = |candidate: Option<Instant>| {
            if let Some(candidate) = candidate {
                next = Some(next.map_or(candidate, |current| current.min(candidate)));
            }
        };
        for endpoint in &self.endpoints {
            consider(endpoint.volume_write.deadline());
            consider(endpoint.host().and_then(vtadn_sessions::SessionHost::next_deadline));
        }
        consider(self.defaults_refresh.deadline());
        consider(self.missing_default_refresh.deadline());
        consider(self.resume_refresh.deadline());
        consider(self.bluetooth.as_ref().and_then(BluetoothCoordinator::next_deadline));
        consider(self.meter_ticker.deadline());
        for job in &self.feedback_jobs {
            consider(Some(job.next_check));
        }
        next
    }

    fn run_due(&mut self, now: Instant) {
        for endpoint in &mut self.endpoints {
            if let Some(volume) = endpoint.volume_write.take_due(now) {
                endpoint.write_volume(volume);
            }
            if let Some(host) = endpoint.host_mut()
                && host.next_deadline().is_some_and(|deadline| deadline <= now)
            {
                host.flush_due(now);
            }
        }
        if self.defaults_refresh.take_due(now) {
            self.update_all_defaults();
            self.reconcile_pending_sessions();
        }
        if self.missing_default_refresh.take_due(now) {
            self.run_device_list_refresh(self.missing_default_reason);
        }
        if self.resume_refresh.take_due(now) {
            self.run_device_list_refresh("power-resume");
        }
        if let Some(bluetooth) = &mut self.bluetooth
            && bluetooth.next_deadline().is_some_and(|deadline| deadline <= now)
            && bluetooth.tick(now)
        {
            self.dirty = true;
        }
        if self.meter_ticker.take_due(now) {
            self.sample_meters(now);
        }
        if !self.feedback_jobs.is_empty() {
            self.process_feedback(now);
        }
    }

    // ---------------------------------------------------------------- messages

    /// Returns false on shutdown.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Command(command) => return self.handle_command(command),
            Msg::DeviceStateChanged { id, state } => self.on_device_state_changed(&id, state),
            Msg::DeviceAdded(id) => self.add_device(&id),
            Msg::DeviceRemoved(id) => self.remove_device(&id),
            Msg::DefaultChanged(change) => self.on_default_changed(change),
            Msg::PropertyChanged { id, property } => self.on_property_changed(&id, property),
            Msg::EndpointVolume(id) => {
                if let Some(endpoint) = self.endpoint_mut(&id)
                    && endpoint.refresh_volume(false)
                {
                    self.dirty = true;
                }
            }
            Msg::Session { device, generation, event } => self.on_session_event(&device, generation, event),
            Msg::Bt(event) => {
                if let Some(bluetooth) = &mut self.bluetooth
                    && bluetooth.handle(event)
                {
                    self.dirty = true;
                }
            }
            Msg::EqApoAvailabilityChanged => {
                self.eq_apo_available = eqapo::is_available();
                for index in 0..self.endpoints.len() {
                    self.refresh_eq_apo(index);
                }
                self.dirty = true;
            }
            Msg::EqApoRefresh(id) => {
                if let Some(index) = self.index_of(&id) {
                    self.refresh_eq_apo(index);
                    log::info!(
                        "AudioDevice.ToggleEqualizerAPO({}): end state={:?}",
                        self.endpoint_label(index),
                        self.endpoints[index].eq_apo
                    );
                    self.dirty = true;
                }
            }
            Msg::DuckingChanged => {
                self.ducking = ducking::read_mode();
                self.dirty = true;
            }
            Msg::Resumed => {
                self.missing_default_recovery_armed = false;
                self.resume_refresh.schedule(Instant::now(), DEVICE_LIST_REFRESH_AFTER_RESUME_MS);
            }
        }
        true
    }

    fn handle_command(&mut self, command: AudioCommand) -> bool {
        match command {
            AudioCommand::Configure(config) => self.configure(config),
            AudioCommand::SetFlyoutActive(active) => self.set_flyout_active(active),
            AudioCommand::SetVisibleGroupMeters(groups) => {
                self.visible_groups.clear();
                for (device, app) in groups {
                    self.visible_groups.entry(device).or_default().insert(app);
                }
            }
            AudioCommand::ReconcileSessions => self.reconcile_sessions(None),
            AudioCommand::SetDeviceVolume { id, volume } => self.set_device_volume(&id, volume),
            AudioCommand::SetDeviceMute { id, muted } => {
                if let Some(endpoint) = self.endpoint_mut(&id)
                    && endpoint.muted != muted
                    && endpoint.set_mute(muted)
                {
                    self.dirty = true;
                }
            }
            AudioCommand::SetDefault { id } => self.set_default(&id),
            AudioCommand::SetDefaultComms { id } => self.set_default_comms(&id),
            AudioCommand::SetEnabled { id, enabled } => self.set_enabled(&id, enabled),
            AudioCommand::Rename { id, name } => self.rename(&id, name.as_deref()),
            AudioCommand::QueryFormats { id, reply } => self.query_formats(&id, reply),
            AudioCommand::SetFormat { id, format } => self.set_format(&id, format),
            AudioCommand::SetListenEnabled { id, enabled } => {
                if let Some(endpoint) = self.endpoint_mut(&id) {
                    endpoint.write_listen_enabled(enabled);
                    self.dirty = true;
                }
            }
            AudioCommand::SetListenTarget { id, target } => {
                if let Some(endpoint) = self.endpoint_mut(&id) {
                    endpoint.write_listen_target(target.as_deref());
                    self.dirty = true;
                }
            }
            AudioCommand::ToggleExclusiveAllowed { id } => {
                if let Some(endpoint) = self.endpoint_mut(&id) {
                    endpoint.toggle_exclusive_allowed();
                    self.dirty = true;
                }
            }
            AudioCommand::ToggleEqualizerApo { id } => self.toggle_equalizer_apo(&id),
            AudioCommand::OpenEqualizerApoEditor { id } => eqapo::open_configuration_editor(&id),
            AudioCommand::ConnectBluetooth { id } => {
                if let Some(bluetooth) = &mut self.bluetooth {
                    bluetooth.connect(&id);
                    self.dirty = true;
                }
            }
            AudioCommand::DisconnectBluetooth { id } => {
                if let Some(bluetooth) = &mut self.bluetooth {
                    bluetooth.disconnect(&id);
                    self.dirty = true;
                }
            }
            AudioCommand::ToggleBluetoothRadio => {
                if let Some(bluetooth) = &mut self.bluetooth {
                    bluetooth.toggle_radio();
                    self.dirty = true;
                }
            }
            AudioCommand::SetDucking(mode) => {
                if let Err(error) = ducking::write_mode(mode) {
                    log::warn!("CommunicationsDucking.SetMode({mode:?}): {error}");
                }
                self.ducking = ducking::read_mode();
                self.dirty = true;
            }
            AudioCommand::SetGroupVolume { device, app, volume } => {
                if let Some(index) = self.index_of(&device)
                    && let Some(host) = self.endpoints[index].host_mut()
                    && host.set_group_volume(&app, volume)
                {
                    self.after_groups_changed(index);
                }
            }
            AudioCommand::SetGroupMute { device, app, muted } => {
                if let Some(index) = self.index_of(&device)
                    && let Some(host) = self.endpoints[index].host_mut()
                    && host.set_group_mute(&app, muted)
                {
                    self.after_groups_changed(index);
                }
            }
            AudioCommand::PlayDeviceFeedback { id, timing } => self.queue_device_feedback(id, timing),
            AudioCommand::PlayAppFeedback { device, app, timing } => self.queue_app_feedback(device, app, timing),
            AudioCommand::RequestDefaultRecovery => self.request_missing_default_recovery("tray-null-default"),
            AudioCommand::Shutdown => return false,
        }
        true
    }

    fn configure(&mut self, config: AudioBackendConfig) {
        let old = std::mem::replace(&mut self.config, config);
        let new = &self.config;
        if old.meter_peak_sample_rate != new.meter_peak_sample_rate && self.meter_ticker.is_running() {
            // C# retunes the timer under a fresh generation.
            self.meter_ticker.start(Instant::now(), sample_interval(new.meter_peak_sample_rate));
        }
        if old.show_device_codec_text != new.show_device_codec_text
            && let Some(bluetooth) = &mut self.bluetooth
        {
            bluetooth.set_show_codec(new.show_device_codec_text);
            self.dirty = true;
        }
        if old.icon_retry_interval_ms != new.icon_retry_interval_ms || old.icon_lru_limit != new.icon_lru_limit {
            self.icons.configure(non_negative(new.icon_retry_interval_ms), non_negative(new.icon_lru_limit));
        }
        let names_changed = old.custom_names != new.custom_names;
        if old.activate_recording_devices_for_peak_meters != new.activate_recording_devices_for_peak_meters {
            self.reconcile_capture_activation();
        }
        if names_changed {
            self.dirty = true;
        }
    }

    fn set_flyout_active(&mut self, active: bool) {
        self.flyout_active = active;
        if active {
            // `StartMetering`: stop, then re-arm.
            self.meter_ticker.start(Instant::now(), sample_interval(self.config.meter_peak_sample_rate));
            let tx = self.tx.clone();
            self.ducking_watcher = match DuckingWatcher::start(Arc::new(move || {
                let _ = tx.send(Msg::DuckingChanged);
            })) {
                Ok(watcher) => Some(watcher),
                Err(error) => {
                    log::warn!("CommunicationsDucking.Start: {error}");
                    None
                }
            };
            let mode = ducking::read_mode();
            if mode != self.ducking {
                self.ducking = mode;
                self.dirty = true;
            }
        } else {
            self.meter_ticker.stop();
            self.ducking_watcher = None;
        }
        self.reconcile_capture_activation();
        if let Some(bluetooth) = &mut self.bluetooth {
            bluetooth.set_flyout_active(active);
            self.dirty = true;
        }
    }

    // ---------------------------------------------------------------- lookup helpers

    fn index_of(&self, id: &str) -> Option<usize> {
        self.endpoints.iter().position(|e| &*e.id == id)
    }

    fn endpoint_mut(&mut self, id: &str) -> Option<&mut Endpoint> {
        self.endpoints.iter_mut().find(|e| &*e.id == id)
    }

    fn endpoint_refs(&self) -> Vec<EndpointRef<'_>> {
        self.endpoints.iter().map(|e| EndpointRef { id: &e.id, flow: e.flow, active: e.state.is_active() }).collect()
    }

    fn is_bluetooth(&self, id: &DeviceId) -> bool {
        self.bluetooth.as_ref().is_some_and(|bt| bt.state(id).is_some())
    }

    fn endpoint_label(&self, index: usize) -> String {
        let endpoint = &self.endpoints[index];
        display_name(endpoint, self.is_bluetooth(&endpoint.id), &self.config.custom_names)
    }

    // ---------------------------------------------------------------- endpoints

    fn activate_endpoint(&mut self, index: usize) {
        self.next_host_generation += 1;
        let generation = self.next_host_generation;
        let env = ActivationEnv { tx: &self.tx, host_config: self.host_config, icons: &self.icons };
        let pin_on = self.endpoints[index].activate(&env, generation);
        if pin_on {
            self.write_silence(index);
        }
    }

    fn deactivate_endpoint(&mut self, index: usize) {
        self.endpoints[index].deactivate();
        self.write_silence(index);
        self.prune_meters = true;
    }

    /// `RefreshEqualizerAPOState`.
    fn refresh_eq_apo(&mut self, index: usize) {
        let endpoint = &mut self.endpoints[index];
        endpoint.eq_apo = if self.eq_apo_available {
            eqapo::probe(&endpoint.id, endpoint.flow == Flow::Capture)
        } else {
            EqApoState::NotAvailable
        };
    }

    /// Group membership/state changed on a host: refresh the cached groups and the meter pin.
    fn after_groups_changed(&mut self, index: usize) {
        if self.endpoints[index].refresh_groups() {
            self.write_silence(index);
        }
        self.prune_meters = true;
        self.dirty = true;
    }

    /// Pin-to-silence fast path (`PinEndpointMeterToSilenceNow`).
    fn write_silence(&self, index: usize) {
        let id = &self.endpoints[index].id;
        self.publisher.meters().write(|frame| match frame.devices.get_mut(id) {
            Some(slot) => *slot = (0.0, 0.0),
            None => {
                frame.devices.insert(id.clone(), (0.0, 0.0));
            }
        });
    }

    fn update_bluetooth_endpoints(&mut self) {
        let infos: Vec<_> = self.endpoints.iter().map(Endpoint::bt_info).collect();
        if let Some(bluetooth) = &mut self.bluetooth
            && bluetooth.set_endpoints(&infos)
        {
            self.dirty = true;
        }
    }

    /// Shared tail of add/remove/state handlers.
    fn after_topology_change(&mut self) {
        self.reconcile_capture_activation();
        // `NoteExternalDeviceTopologyChanged`.
        self.missing_default_recovery_armed = false;
        self.schedule_update_all_defaults();
        self.update_bluetooth_endpoints();
        self.prune_meters = true;
        self.dirty = true;
    }

    /// `AddDeviceByID`: runtime additions are appended (manager order).
    fn add_device(&mut self, id: &str) {
        if id.is_empty() || self.index_of(id).is_some() {
            return;
        }
        let Some(enumerator) = &self.enumerator else { return };
        // SAFETY: plain COM call; the HSTRING outlives it.
        let device = match unsafe { enumerator.GetDevice(&HSTRING::from(id)) } {
            Ok(device) => device,
            Err(error) => {
                log::info!("AudioDeviceManager.AddDeviceByID({id}): {error}");
                return;
            }
        };
        // SAFETY: plain COM calls (QueryInterface + GetDataFlow).
        let flow = device.cast::<IMMEndpoint>().and_then(|endpoint| unsafe { endpoint.GetDataFlow() });
        let Some(flow) = flow.ok().and_then(flow_from) else { return };
        let endpoint = match Endpoint::read(device, flow) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                log::info!("AudioDeviceManager.AddDeviceByID({id}): {error}");
                return;
            }
        };
        self.endpoints.push(endpoint);
        let index = self.endpoints.len() - 1;
        self.refresh_eq_apo(index);
        if self.endpoints[index].state.is_active() {
            self.activate_endpoint(index);
        }
        self.after_topology_change();
    }

    /// `RemoveDeviceByID`.
    fn remove_device(&mut self, id: &str) {
        let Some(index) = self.index_of(id) else { return };
        let mut endpoint = self.endpoints.remove(index);
        endpoint.deactivate();
        self.feedback_jobs.retain(|job| match &job.target {
            FeedbackTarget::Device(device) | FeedbackTarget::App { device, .. } => **device != *id,
        });
        self.after_topology_change();
    }

    /// `HandleDeviceStateChanged`.
    fn on_device_state_changed(&mut self, id: &str, state: u32) {
        let Some(index) = self.index_of(id) else {
            self.add_device(id);
            return;
        };
        let state = vtadn_model::snapshot::DeviceState(state);
        self.endpoints[index].state = state;
        if state.is_active() {
            self.activate_endpoint(index);
        } else {
            self.deactivate_endpoint(index);
        }
        self.after_topology_change();
    }

    /// `HandleDefaultDeviceChanged`.
    fn on_default_changed(&mut self, change: DefaultChange) {
        self.missing_default_recovery_armed = false;
        if let Some(bluetooth) = &mut self.bluetooth {
            bluetooth.on_default_device_changed();
            self.dirty = true;
        }
        let role = if change.role == eMultimedia {
            Some(Role::Multimedia)
        } else if change.role == eCommunications {
            Some(Role::Communications)
        } else {
            // eConsole is ignored.
            None
        };
        if let Some(role) = role {
            self.notified.set(change.flow, role, change.id);
        }
        self.reconcile_pending[match change.flow {
            Flow::Render => 0,
            Flow::Capture => 1,
        }] = true;
        self.schedule_update_all_defaults();
    }

    fn on_property_changed(&mut self, id: &str, property: WatchedProperty) {
        let Some(endpoint) = self.endpoint_mut(id) else { return };
        match property {
            WatchedProperty::FriendlyName => endpoint.refresh_names(),
            WatchedProperty::Listen => endpoint.refresh_listen(),
            WatchedProperty::DeviceFormat => endpoint.refresh_format(),
            WatchedProperty::AllowExclusive => endpoint.refresh_exclusive_allowed(),
        }
        if property == WatchedProperty::FriendlyName {
            // Names feed Bluetooth detection.
            self.update_bluetooth_endpoints();
        }
        self.dirty = true;
    }

    fn on_session_event(&mut self, device: &str, generation: u64, event: SessionEvent) {
        let Some(index) = self.index_of(device) else { return };
        if self.endpoints[index].host_generation() != Some(generation) {
            // Late event from a detached host.
            return;
        }
        let Some(host) = self.endpoints[index].host_mut() else { return };
        if host.handle(event).any() {
            self.after_groups_changed(index);
        }
    }

    /// `RebuildDeviceList`.
    fn rebuild_device_list(&mut self) -> bool {
        let built = match self.try_build_device_list() {
            Ok(list) => list,
            Err(error) => {
                if !self.try_recreate_enumerator("rebuild-enumeration-failed", Some(&error)) {
                    return false;
                }
                match self.try_build_device_list() {
                    Ok(list) => list,
                    Err(_) => return false,
                }
            }
        };
        let built = if !self.endpoints.is_empty() && built.is_empty() {
            log::info!("AudioDeviceManager.RebuildDeviceList: empty runtime snapshot; recreating enumerator");
            if !self.try_recreate_enumerator("empty-runtime-snapshot", None) {
                return false;
            }
            match self.try_build_device_list() {
                Ok(list) if !list.is_empty() => list,
                _ => return false,
            }
        } else {
            built
        };

        self.notified.clear();
        for endpoint in &mut self.endpoints {
            endpoint.deactivate();
        }
        self.endpoints = built;
        for index in 0..self.endpoints.len() {
            self.refresh_eq_apo(index);
            if self.endpoints[index].state.is_active() {
                self.activate_endpoint(index);
            }
        }
        // The coordinator keeps pending Bluetooth deadlines by id across the rebuild.
        self.update_bluetooth_endpoints();
        self.update_all_defaults();
        self.reconcile_capture_activation();
        self.prune_meters = true;
        self.dirty = true;
        true
    }

    fn try_build_device_list(&self) -> windows_core::Result<Vec<Endpoint>> {
        let Some(enumerator) = &self.enumerator else {
            return Err(windows_core::Error::from_hresult(windows::Win32::Foundation::E_POINTER));
        };
        let mut list = Vec::new();
        for flow in [Flow::Render, Flow::Capture] {
            // SAFETY: plain COM calls on the audio thread's enumerator.
            let collection =
                unsafe { enumerator.EnumAudioEndpoints(edata_flow(flow), DEVICE_STATE(DEVICE_STATEMASK_ALL)) }
                    .inspect_err(|e| {
                        log::warn!("AudioDeviceManager.RebuildDeviceList: enumerate {flow:?} failed: {e}")
                    })?;
            // SAFETY: as above.
            let count = unsafe { collection.GetCount() }?;
            for index in 0..count {
                // SAFETY: as above; `index < count`.
                let device: IMMDevice = unsafe { collection.Item(index) }?;
                match Endpoint::read(device, flow) {
                    Ok(endpoint) => list.push(endpoint),
                    Err(error) => {
                        log::info!("AudioDeviceManager.RebuildDeviceList: wrapping a {flow:?} endpoint failed: {error}")
                    }
                }
            }
        }
        Ok(list)
    }

    /// `TryRecreateEnumerator`: register on the new enumerator, then unregister the old one.
    fn try_recreate_enumerator(&mut self, reason: &str, cause: Option<&windows_core::Error>) -> bool {
        let replacement = match create_enumerator() {
            Ok(replacement) => replacement,
            Err(error) => {
                log::warn!("AudioDeviceManager.TryRecreateEnumerator({reason}): {error}");
                return false;
            }
        };
        // SAFETY: registering our callback object on the new enumerator.
        if let Err(error) = unsafe { replacement.RegisterEndpointNotificationCallback(&self.notification_client) } {
            log::warn!("AudioDeviceManager.TryRecreateEnumerator({reason}): {error}");
            return false;
        }
        if let Some(old) = self.enumerator.take() {
            // SAFETY: unregistering from the enumerator it was registered on.
            if let Err(error) = unsafe { old.UnregisterEndpointNotificationCallback(&self.notification_client) } {
                log::warn!(
                    "AudioDeviceManager.TryRecreateEnumerator({reason}): unregistering the old enumerator failed: {error}"
                );
            }
        }
        self.enumerator = Some(replacement);
        let cause = cause.map_or_else(|| "no exception".to_owned(), ToString::to_string);
        log::info!("AudioDeviceManager.TryRecreateEnumerator({reason}): recreated after {cause}");
        true
    }

    // ---------------------------------------------------------------- defaults & recovery

    fn schedule_update_all_defaults(&mut self) {
        self.defaults_refresh.schedule(Instant::now(), DEFAULTS_REFRESH_COALESCE_DWELL_MS);
    }

    /// `LookupDefault`: remembered notification first, then `GetDefaultAudioEndpoint` with the
    /// state refreshed from the default endpoint (valid only while Active).
    fn lookup_default(&mut self, flow: Flow, role: Role) -> Option<usize> {
        let os_id = {
            let refs = self.endpoint_refs();
            if let Some(index) = defaults::notified_default(&refs, flow, self.notified.get(flow, role)) {
                return Some(index);
            }
            drop(refs);
            let enumerator = self.enumerator.as_ref()?;
            // SAFETY: plain COM call; E_NOTFOUND (no endpoint of that flow) means "no default".
            let device = unsafe { enumerator.GetDefaultAudioEndpoint(edata_flow(flow), erole(role)) }.ok()?;
            let id = endpoint::device_id_of(&device).ok()?;
            (device, id)
        };
        let (device, id) = os_id;
        let index = defaults::os_default_candidate(&self.endpoint_refs(), flow, &id)?;

        // `RefreshDeviceStateFromDefaultEndpoint`.
        let state = match endpoint::device_state(&device) {
            Ok(state) => state,
            Err(error) => {
                log::warn!(
                    "AudioDeviceManager.LookupDefault({}): GetState failed: {error}",
                    self.endpoint_label(index)
                );
                return None;
            }
        };
        if self.endpoints[index].state != state {
            self.endpoints[index].state = state;
            if state.is_active() {
                self.activate_endpoint(index);
            } else {
                self.deactivate_endpoint(index);
            }
            self.update_bluetooth_endpoints();
            self.reconcile_capture_activation();
        }
        state.is_active().then_some(index)
    }

    /// `UpdateAllDefaults`.
    fn update_all_defaults(&mut self) {
        let mut results = [None; 4];
        for (slot, (flow, role)) in SLOTS.into_iter().enumerate() {
            results[slot] = self.lookup_default(flow, role);
        }
        let ids: [Option<DeviceId>; 4] = results.map(|r| r.map(|index| self.endpoints[index].id.clone()));

        if defaults::persist_last_known(&mut self.last_known, [0, 1, 2, 3].map(|slot| ids[slot].as_deref())) {
            let [playback, comms_playback, recording, comms_recording] = self.last_known.clone().into_parts();
            self.publisher.push_event(AudioEvent::LastKnownDefaults {
                playback,
                comms_playback,
                recording,
                comms_recording,
            });
        }

        let refs = self.endpoint_refs();
        let effective = [0, 1, 2, 3].map(|slot| {
            let (flow, role) = SLOTS[slot];
            defaults::effective_default(&refs, results[slot], self.last_known.get(flow, role))
        });
        let flags = defaults::default_flags(&refs, effective);
        drop(refs);
        for (endpoint, (is_default, is_default_comms)) in self.endpoints.iter_mut().zip(flags) {
            endpoint.is_default = is_default;
            endpoint.is_default_comms = is_default_comms;
        }

        let [render, render_comms, capture, capture_comms] = ids;
        let has_render_default = render.is_some();
        self.defaults = Defaults { render, render_comms, capture, capture_comms };
        // `UpdateMissingDefaultRecovery`.
        if has_render_default {
            self.missing_default_recovery_armed = false;
        } else {
            self.request_missing_default_recovery("missing-render-default");
        }
        self.dirty = true;
    }

    /// `RequestMissingDefaultRecovery`: armed once; rebuild after the dwell if still no default.
    fn request_missing_default_recovery(&mut self, reason: &'static str) {
        if self.missing_default_recovery_armed {
            return;
        }
        self.missing_default_recovery_armed = true;
        self.missing_default_reason = reason;
        self.missing_default_refresh.schedule(Instant::now(), DEVICE_LIST_REFRESH_AFTER_MISSING_DEFAULT_MS);
    }

    /// `ScheduleDeviceListRefresh` payload.
    fn run_device_list_refresh(&mut self, reason: &str) {
        if self.defaults.render.is_some() {
            self.missing_default_recovery_armed = false;
            return;
        }
        log::info!("AudioDeviceManager.RebuildDeviceList: reason={reason}");
        if !self.rebuild_device_list() {
            self.missing_default_recovery_armed = false;
        }
    }

    fn reconcile_pending_sessions(&mut self) {
        let [render, capture] = std::mem::take(&mut self.reconcile_pending);
        if render {
            self.reconcile_sessions(Some(Flow::Render));
        }
        if capture {
            self.reconcile_sessions(Some(Flow::Capture));
        }
    }

    /// `ReconcileSessions` for Active endpoints of `flow` (both flows when None).
    fn reconcile_sessions(&mut self, flow: Option<Flow>) {
        for index in 0..self.endpoints.len() {
            let endpoint = &mut self.endpoints[index];
            if flow.is_some_and(|f| f != endpoint.flow) || !endpoint.state.is_active() {
                continue;
            }
            if let Some(host) = endpoint.host_mut()
                && host.reconcile().any()
            {
                self.after_groups_changed(index);
            }
        }
    }

    fn reconcile_capture_activation(&mut self) {
        let requested: Vec<DeviceId> = if self.config.activate_recording_devices_for_peak_meters && self.flyout_active {
            self.endpoints
                .iter()
                .filter(|e| e.flow == Flow::Capture && e.state.is_active())
                .map(|e| e.id.clone())
                .collect()
        } else {
            Vec::new()
        };
        if requested != self.capture_requested {
            if let Some(capture) = &self.capture_activation {
                capture.set_requested(requested.clone());
            }
            self.capture_requested = requested;
        }
    }

    // ---------------------------------------------------------------- commands

    /// `AudioDevice.Volume = v`.
    fn set_device_volume(&mut self, id: &str, volume: f32) {
        if !volume.is_finite() {
            return;
        }
        let Some(endpoint) = self.endpoint_mut(id) else { return };
        let volume = volume.clamp(0.0, 1.0);
        if (volume - endpoint.volume).abs() < VOLUME_EQUALITY_EPSILON || !endpoint.has_proxies() {
            return;
        }
        endpoint.volume = volume;
        endpoint.volume_write.request(volume, Instant::now());
        self.dirty = true;
    }

    fn set_default(&mut self, id: &DeviceId) {
        let Some(index) = self.index_of(id) else { return };
        let label = self.endpoint_label(index);
        let mirror_comms = self.config.set_default_comms_to_default;
        let target = id.clone();
        self.policy.run(id, format!("AudioDevice.SetAsDefault({label})"), move |job| {
            job.client.set_default_endpoint(&target, eConsole)?;
            job.client.set_default_endpoint(&target, eMultimedia)?;
            if mirror_comms {
                job.client.set_default_endpoint(&target, eCommunications)?;
            }
            Ok(())
        });
    }

    fn set_default_comms(&mut self, id: &DeviceId) {
        let Some(index) = self.index_of(id) else { return };
        let label = self.endpoint_label(index);
        let target = id.clone();
        self.policy.run(id, format!("AudioDevice.SetAsDefaultCommunications({label})"), move |job| {
            job.client.set_default_endpoint(&target, eCommunications)
        });
    }

    fn set_enabled(&mut self, id: &DeviceId, enabled: bool) {
        let Some(index) = self.index_of(id) else { return };
        let label = self.endpoint_label(index);
        let target = id.clone();
        self.policy.run(id, format!("AudioDevice.SetEnabled({label}, {enabled})"), move |job| {
            job.client.set_endpoint_visibility(&target, enabled)
        });
    }

    fn set_format(&mut self, id: &DeviceId, format: Format) {
        let Some(index) = self.index_of(id) else { return };
        let description = format!(
            "AudioDevice.SetDeviceFormat({}, {}ch/{}-bit/{}Hz)",
            self.endpoint_label(index),
            format.channels,
            format.bits,
            format.rate
        );
        let blob = build_format_blob(format.channels, format.bits, container_bits(format.bits), format.rate);
        let target = id.clone();
        let log_description = description.clone();
        self.policy.run(id, description, move |job| {
            let hr = job.client.set_device_format(&target, &blob);
            if hr.is_err() {
                log::warn!("{log_description}: hr=0x{:08X}", hr.0);
            }
            Ok(())
        });
    }

    /// `SetCustomFriendlyName`.
    fn rename(&mut self, id: &DeviceId, name: Option<&str>) {
        let Some(index) = self.index_of(id) else { return };
        let trimmed = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
        let is_bluetooth = self.is_bluetooth(id);
        let displayed = display_name(&self.endpoints[index], is_bluetooth, &self.config.custom_names);
        if trimmed.as_deref().is_some_and(|t| t == displayed) {
            return;
        }
        if is_bluetooth {
            let persisted = trimmed.clone().unwrap_or_default();
            let current = self.config.custom_names.get(&**id).cloned().unwrap_or_default();
            if current != persisted {
                if persisted.is_empty() {
                    self.config.custom_names.remove(&**id);
                } else {
                    self.config.custom_names.insert(id.to_string(), persisted);
                }
                self.publisher.push_event(AudioEvent::CustomNameChanged { id: id.clone(), name: trimmed });
            }
        } else {
            self.endpoints[index].write_friendly_name(trimmed.as_deref());
            self.update_bluetooth_endpoints();
        }
        self.dirty = true;
    }

    /// `EnumerateSupportedFormats` + `GetCurrentFormat`, off the audio thread (up to ~170 KS
    /// probes) on a worker that re-acquires the device by id.
    fn query_formats(&mut self, id: &DeviceId, reply: Box<dyn FnOnce(FormatQuery) + Send>) {
        let label = self.index_of(id).map_or_else(|| id.to_string(), |index| self.endpoint_label(index));
        let id = id.clone();
        let spawned = std::thread::Builder::new().name("vtadn-formats".into()).spawn(move || {
            let query = match MtaScope::enter("QueryFormats") {
                Some(_apartment) => probe_formats(&id, &label),
                None => FormatQuery::default(),
            };
            reply(query);
        });
        if let Err(error) = spawned {
            log::error!("QueryFormats worker spawn failed: {error}");
        }
    }

    /// `ToggleEqualizerAPO`: registry toggle + force-cycle on the device's single-flight worker,
    /// re-probed on this thread afterwards. NotAvailable is a no-op (the UI explains instead).
    fn toggle_equalizer_apo(&mut self, id: &DeviceId) {
        let Some(index) = self.index_of(id) else { return };
        let label = self.endpoint_label(index);
        let endpoint = &self.endpoints[index];
        let before = endpoint.eq_apo;
        if before == EqApoState::NotAvailable {
            return;
        }
        let roles = CycleRoles {
            flow: endpoint.flow,
            was_default: endpoint.is_default,
            was_default_comms: endpoint.is_default_comms,
        };
        let capture = endpoint.flow == Flow::Capture;
        log::info!("AudioDevice.ToggleEqualizerAPO({label}): begin state={before:?} capture={capture}");
        let target = id.clone();
        let tx = self.tx.clone();
        let description = format!("AudioDevice.ToggleEqualizerAPO({label})");
        let job_description = description.clone();
        self.policy.run(id, description, move |job| {
            let outcome = eqapo::toggle(&target, capture);
            let _ = tx.send(Msg::EqApoRefresh(target.clone()));
            match outcome {
                Ok(toggle @ (EqApoToggle::Installed | EqApoToggle::Removed)) => {
                    let after = if toggle == EqApoToggle::Installed { "install" } else { "uninstall" };
                    job.force_cycle(
                        &target,
                        roles,
                        &format!("AudioDevice.ForceCycleEndpoint({label}, after EAPO {after})"),
                    )
                }
                Ok(_) => Ok(()),
                Err(message) => {
                    log::warn!("{job_description}: {message}");
                    Ok(())
                }
            }
        });
    }

    // ---------------------------------------------------------------- meters

    /// One sample tick (`SamplePeaks` / `UpdatePeakValueBackground`): endpoint peaks (pinned →
    /// silence), ding observation, visible group peaks, then one in-place board write.
    fn sample_meters(&mut self, now: Instant) {
        let unified = self.config.unified_peak_meter;
        let bias = self.config.unified_meter_low_channel_bias_multiplier;
        let scratch_devices = &mut self.scratch_devices;
        let scratch_groups = &mut self.scratch_groups;
        scratch_devices.clear();
        scratch_groups.clear();
        for endpoint in &mut self.endpoints {
            if !endpoint.has_proxies() {
                continue;
            }
            let bypass = endpoint.ding_bypassed(now);
            let peaks =
                if endpoint.pinned { (0.0, 0.0) } else { endpoint.read_peaks(unified, bias).unwrap_or((0.0, 0.0)) };
            let observed = if bypass { endpoint.external_peak() } else { peaks.1 };
            endpoint.ding.observe(observed, now);
            scratch_devices.push((endpoint.id.clone(), peaks));

            let id = endpoint.id.clone();
            let visible = self.visible_groups.get(&id);
            if let Some(host) = endpoint.host_mut() {
                host.sample_group_peaks(
                    &|app: &AppId| visible.is_some_and(|set| set.contains(app)),
                    unified,
                    bias,
                    now,
                    &mut |app: &AppId, peaks: (f32, f32)| scratch_groups.push(((id.clone(), app.clone()), peaks)),
                );
            }
        }
        let (devices, groups) = (&self.scratch_devices, &self.scratch_groups);
        self.publisher.meters().write(|frame| {
            for (id, peaks) in devices {
                match frame.devices.get_mut(id) {
                    Some(slot) => *slot = *peaks,
                    None => {
                        frame.devices.insert(id.clone(), *peaks);
                    }
                }
            }
            for (key, peaks) in groups {
                match frame.groups.get_mut(key) {
                    Some(slot) => *slot = *peaks,
                    None => {
                        frame.groups.insert(key.clone(), *peaks);
                    }
                }
            }
        });
    }

    /// Drops board entries of removed endpoints and groups.
    fn prune_meter_board(&mut self) {
        let endpoints = &self.endpoints;
        self.publisher.meters().write(|frame| {
            frame.devices.retain(|id, _| endpoints.iter().any(|e| e.id == *id));
            frame.groups.retain(|(device, app), _| {
                endpoints.iter().any(|e| e.id == *device && e.groups.iter().any(|g| g.app_id == *app))
            });
        });
        self.prune_meters = false;
    }

    // ---------------------------------------------------------------- feedback

    fn queue_feedback(&mut self, target: FeedbackTarget, timing: FeedbackTiming) {
        let now = Instant::now();
        let deadline = match timing {
            FeedbackTiming::Immediate => now,
            FeedbackTiming::Debounced => now + VOLUME_FEEDBACK_DING_DELAY_MS,
        };
        // Latest wins per key: a newer request replaces the pending one.
        self.feedback_jobs.retain(|job| !job.target.same_key(&target));
        self.feedback_jobs.push(FeedbackJob { target, deadline, next_check: now });
    }

    /// `AppVolumeFeedbackPlayer.PlayForDevice` (render devices only).
    fn queue_device_feedback(&mut self, id: DeviceId, timing: FeedbackTiming) {
        if !self.config.play_device_volume_change_sound {
            return;
        }
        let Some(index) = self.index_of(&id) else { return };
        if self.endpoints[index].flow == Flow::Capture {
            return;
        }
        self.queue_feedback(FeedbackTarget::Device(id), timing);
    }

    /// `AppVolumeFeedbackPlayer.PlayForApp`: the level is captured now.
    fn queue_app_feedback(&mut self, device: DeviceId, app: AppId, timing: FeedbackTiming) {
        if !self.config.play_app_volume_change_sound {
            return;
        }
        let Some(level) =
            self.index_of(&device).and_then(|i| self.endpoints[i].host()).and_then(|h| h.group_volume(&app))
        else {
            return;
        };
        self.queue_feedback(FeedbackTarget::App { device, app, level }, timing);
    }

    fn process_feedback(&mut self, now: Instant) {
        let jobs = std::mem::take(&mut self.feedback_jobs);
        for mut job in jobs {
            if now < job.next_check {
                self.feedback_jobs.push(job);
                continue;
            }
            if self.feedback_suppressed(&job.target, now) {
                continue;
            }
            if now >= job.deadline {
                self.play_feedback(&job.target, now);
                continue;
            }
            job.next_check = (now + VOLUME_FEEDBACK_DING_DWELL_POLL_SLICE_MS).min(job.deadline);
            self.feedback_jobs.push(job);
        }
    }

    /// `ShouldSuppressDeviceDing` / `ShouldSuppressAppDing`.
    fn feedback_suppressed(&mut self, target: &FeedbackTarget, now: Instant) -> bool {
        if !self.config.suppress_device_volume_change_sound_when_audio_playing {
            return false;
        }
        let threshold = self.config.ding_suppression_peak_threshold_percent;
        match target {
            FeedbackTarget::Device(id) => match self.endpoint_mut(id) {
                Some(endpoint) => {
                    let peak = endpoint.read_ding_peak(now);
                    should_suppress(Some(peak), threshold, endpoint.volume)
                }
                // Gone: an unavailable peak fails closed.
                None => true,
            },
            FeedbackTarget::App { device, app, .. } => {
                let host = self.endpoint_mut(device).and_then(Endpoint::host_mut);
                let (peak, volume) = match host {
                    Some(host) => (host.group_ding_peak(app, now), host.group_volume(app).unwrap_or(0.0)),
                    None => (None, 0.0),
                };
                should_suppress(peak, threshold, volume)
            }
        }
    }

    fn play_feedback(&mut self, target: &FeedbackTarget, now: Instant) {
        let Some(duration) = self.feedback.duration() else {
            log::debug!("AppVolumeFeedbackPlayer: feedback WAV unavailable");
            return;
        };
        match target {
            FeedbackTarget::Device(id) => {
                let Some(index) = self.index_of(id) else { return };
                let endpoint = &mut self.endpoints[index];
                if !endpoint.state.is_active() {
                    return;
                }
                endpoint.begin_ding_bypass(now + duration + VOLUME_FEEDBACK_DING_METER_BYPASS_GRACE_MS);
                self.feedback.play_on_endpoint(id);
            }
            FeedbackTarget::App { level, .. } => self.feedback.play_in_own_session(*level),
        }
    }

    // ---------------------------------------------------------------- snapshots

    fn build_snapshot(&self) -> AudioSnapshot {
        let devices = self
            .endpoints
            .iter()
            .enumerate()
            .map(|(order, e)| {
                let bt = self.bluetooth.as_ref().and_then(|b| b.state(&e.id));
                DeviceSnapshot {
                    id: e.id.clone(),
                    flow: e.flow,
                    order: u32::try_from(order).unwrap_or(u32::MAX),
                    state: e.state,
                    name: display_name(e, bt.is_some(), &self.config.custom_names),
                    description: e.description.clone(),
                    interface_name: e.interface_name.clone(),
                    enumerator: e.enumerator.clone(),
                    is_default: e.is_default,
                    is_default_comms: e.is_default_comms,
                    volume: e.volume,
                    muted: e.muted,
                    capture_sleeping: e.capture_sleeping,
                    format: e.format,
                    listen_enabled: e.listen_enabled,
                    listen_target: e.listen_target.clone(),
                    exclusive_allowed: e.exclusive_allowed,
                    exclusive_held: e.exclusive_held,
                    eq_apo: e.eq_apo,
                    bt,
                    groups: e.groups.clone(),
                }
            })
            .collect();
        AudioSnapshot {
            rev: 0,
            structure_rev: 0,
            devices,
            defaults: self.defaults.clone(),
            radio: self.bluetooth.as_ref().map(BluetoothCoordinator::radio).unwrap_or_default(),
            ducking: self.ducking,
            eq_apo_available: self.eq_apo_available,
            is_elevated: self.is_elevated,
            ready: self.ready,
        }
    }

    /// Builds a snapshot after a dirty batch; publishes it (one coalesced UI wake) when it
    /// differs, bumping `rev` always and `structure_rev` for rebuild-class changes.
    fn publish_if_dirty(&mut self) {
        if self.prune_meters {
            self.prune_meter_board();
        }
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let mut next = self.build_snapshot();
        let previous = &self.last_snapshot;
        let (rev, structure_rev) = match classify(previous, &next) {
            Change::None => return,
            Change::Values => (previous.rev + 1, previous.structure_rev),
            Change::Structure => (previous.rev + 1, previous.structure_rev + 1),
        };
        next.rev = rev;
        next.structure_rev = structure_rev;
        let next = Arc::new(next);
        self.publisher.publish(next.clone());
        self.last_snapshot = next;
    }
}

/// Worker body of `QueryFormats`: re-acquire the device by id, read the current format, probe
/// when Active.
fn probe_formats(id: &str, label: &str) -> FormatQuery {
    let device = create_enumerator().and_then(|enumerator| {
        // SAFETY: plain COM call; the HSTRING outlives it.
        unsafe { enumerator.GetDevice(&HSTRING::from(id)) }
    });
    let device = match device {
        Ok(device) => device,
        Err(error) => {
            log::info!("AudioDevice.EnumerateSupportedFormats({label}): {error}");
            return FormatQuery::default();
        }
    };
    let blob = endpoint::read_format_blob(&device);
    let current = blob.as_deref().and_then(parse_format_blob);
    let active = endpoint::device_state(&device).is_ok_and(|state| state.is_active());
    let supported = if active {
        format::query_supported_formats(&device, format::current_channels(blob.as_deref()), label).unwrap_or_default()
    } else {
        Vec::new()
    };
    FormatQuery { supported, current }
}
