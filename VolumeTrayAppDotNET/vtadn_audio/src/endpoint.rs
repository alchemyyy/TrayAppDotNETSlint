//! One audio endpoint (the endpoint half of `Audio/AudioDevice.cs`): property-store state,
//! endpoint volume/meter proxies, the volume callback and the device's `SessionHost`.
//!
//! Every COM object here is created and used on the audio thread only. `ComProxyGate` semantics
//! (no call on a released proxy, none after retirement) hold structurally: the proxies live in
//! `Option<Proxies>`, every call goes through it, and `deactivate` drops the pending throttled
//! write before taking (retiring) the proxies.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Instant;

use vtadn_model::IDENTITY;
use vtadn_model::policy::ding::DingSuppressionPeak;
use vtadn_model::policy::meter::should_pin_endpoint_meter_to_silence;
use vtadn_model::snapshot::{AppGroupSnapshot, DeviceId, DeviceState, EqApoState, Flow, Format};
use vtadn_model::text::UNKNOWN_DEVICE_NAME;
use vtadn_native::bluetooth::BtEndpointInfo;
use vtadn_sessions::{IconCache, SessionEvent, SessionHost, SessionHostConfig, SessionSink};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioMeterInformation,
};
use windows::Win32::Media::Audio::{IAudioSessionManager2, IMMDevice};
use windows::Win32::System::Com::{CLSCTX_ALL, CoTaskMemFree};
use windows_core::GUID;

use crate::consts::{VOLUME_EQUALITY_EPSILON, VOLUME_WRITE_RATE_DEFAULT_MS};
use crate::format::parse_format_blob;
use crate::notify::VolumeCallback;
use crate::props::{self, PropVariant};
use crate::service::Msg;
use crate::throttle::WriteThrottle;

/// What activation needs from the service.
pub struct ActivationEnv<'a> {
    pub tx: &'a Sender<Msg>,
    pub host_config: SessionHostConfig,
    pub icons: &'a Arc<IconCache>,
}

struct Proxies {
    volume: IAudioEndpointVolume,
    meter: IAudioMeterInformation,
    callback: IAudioEndpointVolumeCallback,
    callback_pending: Arc<AtomicBool>,
    host: SessionHost,
    host_generation: u64,
}

pub struct Endpoint {
    pub id: DeviceId,
    pub flow: Flow,
    device: IMMDevice,
    pub state: DeviceState,
    /// Windows FriendlyName (the displayed name unless a Bluetooth custom name applies).
    pub system_name: String,
    pub description: String,
    pub interface_name: String,
    pub enumerator: String,
    pub container_id: Option<GUID>,
    pub adapter_instance_path: Option<String>,
    pub format: Option<Format>,
    pub listen_enabled: bool,
    pub listen_target: Option<DeviceId>,
    pub exclusive_allowed: bool,
    pub eq_apo: EqApoState,
    pub is_default: bool,
    pub is_default_comms: bool,
    pub volume: f32,
    pub muted: bool,
    pub capture_sleeping: bool,
    pub exclusive_held: bool,
    /// `_pinEndpointMeterToSilence`.
    pub pinned: bool,
    /// Cached `SessionHost::groups` (refreshed when the host reports a change).
    pub groups: Vec<AppGroupSnapshot>,
    pub ding: DingSuppressionPeak,
    pub ding_bypass_until: Option<Instant>,
    pub volume_write: WriteThrottle<f32>,
    proxies: Option<Proxies>,
}

/// `IMMDevice::GetId`.
pub fn device_id_of(device: &IMMDevice) -> windows_core::Result<String> {
    // SAFETY: plain COM call; the returned CoTaskMem string is freed below exactly once.
    let raw = unsafe { device.GetId() }?;
    // SAFETY: GetId returned a valid NUL-terminated string.
    let text = unsafe { raw.to_string() };
    // SAFETY: the string was allocated by the callee with CoTaskMemAlloc.
    unsafe { CoTaskMemFree(Some(raw.0.cast_const().cast())) };
    text.map_err(|_| windows_core::Error::from_hresult(windows::Win32::Foundation::E_UNEXPECTED))
}

pub fn device_state(device: &IMMDevice) -> windows_core::Result<DeviceState> {
    // SAFETY: plain COM call.
    unsafe { device.GetState() }.map(|state| DeviceState(state.0))
}

/// `ResolveDeviceNames`: (FriendlyName, DeviceDesc, DeviceInterface FriendlyName); the latter two
/// fall back to the first; any store failure makes all three "Unknown device".
fn read_names(device: &IMMDevice) -> (String, String, String) {
    let Ok(store) = props::open_read(device) else {
        return (UNKNOWN_DEVICE_NAME.to_owned(), UNKNOWN_DEVICE_NAME.to_owned(), UNKNOWN_DEVICE_NAME.to_owned());
    };
    let friendly =
        props::read_string(&store, &props::PKEY_DEVICE_FRIENDLY_NAME).unwrap_or_else(|| UNKNOWN_DEVICE_NAME.to_owned());
    let description = props::read_string(&store, &props::PKEY_DEVICE_DEVICE_DESC).unwrap_or_else(|| friendly.clone());
    let interface =
        props::read_string(&store, &props::PKEY_DEVICE_INTERFACE_FRIENDLY_NAME).unwrap_or_else(|| friendly.clone());
    (friendly, description, interface)
}

/// `ReadCurrentFormatBlob`.
pub fn read_format_blob(device: &IMMDevice) -> Option<Vec<u8>> {
    let store = props::open_read(device).ok()?;
    props::get(&store, &props::PKEY_AUDIO_ENGINE_DEVICE_FORMAT).ok()?.blob()
}

/// `ReadListenStateFromStore`: VT_BOOL / VT_UI4 / VT_I4 enable flag, VT_LPWSTR target.
fn read_listen(device: &IMMDevice) -> (bool, Option<DeviceId>) {
    let Ok(store) = props::open_read(device) else { return (false, None) };
    let enabled = match props::get(&store, &props::PKEY_AUDIO_ENDPOINT_LISTEN_TO_THIS_DEVICE) {
        Ok(value) => {
            value.as_bool().or_else(|| value.as_u32().map(|v| v != 0)).or_else(|| value.as_i32().map(|v| v != 0))
        }
        Err(_) => return (false, None),
    }
    .unwrap_or(false);
    let target = match props::get(&store, &props::PKEY_AUDIO_ENDPOINT_LISTEN_TARGET_DEVICE_ID) {
        Ok(value) => value.string().map(Arc::from),
        Err(_) => return (false, None),
    };
    (enabled, target)
}

/// `ReadAllowExclusiveControlFromStore`: VT_EMPTY, unknown types and errors read as allowed.
fn read_allow_exclusive(device: &IMMDevice) -> bool {
    let value: Option<PropVariant> = props::open_read(device)
        .ok()
        .and_then(|store| props::get(&store, &props::PKEY_AUDIO_ENDPOINT_ALLOW_EXCLUSIVE_CONTROL).ok());
    let Some(value) = value else { return true };
    if value.is_empty() {
        return true;
    }
    value
        .as_u32()
        .map(|v| v != 0)
        .or_else(|| value.as_i32().map(|v| v != 0))
        .or_else(|| value.as_bool())
        .unwrap_or(true)
}

impl Endpoint {
    /// The `AudioDevice` constructor minus activation (the caller activates Active endpoints
    /// once the wrapper is published) and minus the EAPO probe (needs service state).
    pub fn read(device: IMMDevice, flow: Flow) -> windows_core::Result<Self> {
        let id: DeviceId = Arc::from(device_id_of(&device)?);
        let (system_name, description, interface_name) = read_names(&device);
        let format = read_format_blob(&device).as_deref().and_then(parse_format_blob);
        let (enumerator, container_id, adapter_instance_path) = match props::open_read(&device) {
            Ok(store) => (
                props::read_string(&store, &props::PKEY_DEVICE_ENUMERATOR_NAME).unwrap_or_default(),
                props::get(&store, &props::PKEY_DEVICE_CONTAINER_ID).ok().and_then(|v| v.guid()),
                props::read_string(&store, &props::PKEY_ENDPOINT_ADAPTER_INSTANCE_PATH),
            ),
            Err(_) => (String::new(), None, None),
        };
        let state = device_state(&device)?;
        let (listen_enabled, listen_target) = if flow == Flow::Capture { read_listen(&device) } else { (false, None) };
        let exclusive_allowed = read_allow_exclusive(&device);
        Ok(Self {
            id,
            flow,
            device,
            state,
            system_name,
            description,
            interface_name,
            enumerator,
            container_id,
            adapter_instance_path,
            format,
            listen_enabled,
            listen_target,
            exclusive_allowed,
            eq_apo: EqApoState::NotAvailable,
            is_default: false,
            is_default_comms: false,
            volume: 0.0,
            muted: false,
            capture_sleeping: false,
            exclusive_held: false,
            pinned: false,
            groups: Vec::new(),
            ding: DingSuppressionPeak::default(),
            ding_bypass_until: None,
            volume_write: WriteThrottle::new(VOLUME_WRITE_RATE_DEFAULT_MS),
            proxies: None,
        })
    }

    pub fn has_proxies(&self) -> bool {
        self.proxies.is_some()
    }

    pub fn host_mut(&mut self) -> Option<&mut SessionHost> {
        self.proxies.as_mut().map(|p| &mut p.host)
    }

    pub fn host(&self) -> Option<&SessionHost> {
        self.proxies.as_ref().map(|p| &p.host)
    }

    pub fn host_generation(&self) -> Option<u64> {
        self.proxies.as_ref().map(|p| p.host_generation)
    }

    /// `TryActivateProxies` (`UpgradeFromActiveState`). Already active: refresh volume/mute.
    /// Returns true when the meter pin just turned on.
    pub fn activate(&mut self, env: &ActivationEnv<'_>, generation: u64) -> bool {
        if self.proxies.is_some() {
            self.refresh_volume(false);
            return false;
        }
        match self.create_proxies(env, generation) {
            Ok(proxies) => {
                self.proxies = Some(proxies);
                self.refresh_volume(true);
                self.refresh_groups()
            }
            Err(error) => {
                log::warn!("AudioDevice.TryActivateProxies({}): {error}", self.system_name);
                false
            }
        }
    }

    fn create_proxies(&self, env: &ActivationEnv<'_>, generation: u64) -> windows_core::Result<Proxies> {
        // SAFETY: COM activation on the audio thread's own device.
        let volume: IAudioEndpointVolume = unsafe { self.device.Activate(CLSCTX_ALL, None) }?;
        // SAFETY: as above.
        let meter: IAudioMeterInformation = unsafe { self.device.Activate(CLSCTX_ALL, None) }?;
        // SAFETY: as above.
        let manager: IAudioSessionManager2 = unsafe { self.device.Activate(CLSCTX_ALL, None) }?;

        let callback_pending = Arc::new(AtomicBool::new(false));
        let callback = VolumeCallback::create(self.id.clone(), env.tx.clone(), callback_pending.clone());
        // SAFETY: the callback stays referenced by `Proxies` until it is unregistered.
        unsafe { volume.RegisterControlChangeNotify(&callback) }?;

        let tx = env.tx.clone();
        let sink: SessionSink = Arc::new(move |device: DeviceId, event: SessionEvent| {
            let _ = tx.send(Msg::Session { device, generation, event });
        });
        let host =
            match SessionHost::attach(self.id.clone(), self.flow, manager, env.host_config, env.icons.clone(), sink) {
                Ok(host) => host,
                Err(error) => {
                    // SAFETY: undoing the registration above on the same interface.
                    if let Err(unregister) = unsafe { volume.UnregisterControlChangeNotify(&callback) } {
                        log::warn!("AudioDevice.ReleaseEndpointProxies: volume callback cleanup failed: {unregister}");
                    }
                    return Err(error);
                }
            };
        Ok(Proxies { volume, meter, callback, callback_pending, host, host_generation: generation })
    }

    /// `DowngradeFromActiveState` / `ReleaseEndpointProxies`: drop the pending write, retire the
    /// proxies, unregister both callbacks, detach the session host.
    pub fn deactivate(&mut self) {
        self.volume_write.drop_pending();
        if let Some(proxies) = self.proxies.take() {
            // SAFETY: unregistering the callback registered in `create_proxies`.
            if let Err(error) = unsafe { proxies.volume.UnregisterControlChangeNotify(&proxies.callback) } {
                log::warn!("AudioDevice.ReleaseEndpointProxies: volume callback cleanup failed: {error}");
            }
            proxies.host.detach();
        }
        self.groups.clear();
        self.exclusive_held = false;
        self.capture_sleeping = false;
        self.pinned = true;
    }

    /// Re-reads endpoint volume and mute; true when either changed (ε for volume) or `force`.
    pub fn refresh_volume(&mut self, force: bool) -> bool {
        let Some(proxies) = &self.proxies else { return false };
        proxies.callback_pending.store(false, Ordering::Release);
        // SAFETY: plain COM calls on live proxies.
        let read =
            unsafe { proxies.volume.GetMasterVolumeLevelScalar().and_then(|v| Ok((v, proxies.volume.GetMute()?))) };
        let Ok((level, mute)) = read else { return false };
        if !level.is_finite() {
            return false;
        }
        let level = level.clamp(0.0, 1.0);
        let muted = mute.as_bool();
        let changed = force || (level - self.volume).abs() >= VOLUME_EQUALITY_EPSILON || muted != self.muted;
        self.volume = level;
        self.muted = muted;
        changed
    }

    /// Re-reads the host's groups and recomputes `IsCaptureSleeping`, `IsExclusiveControlHeld`
    /// and the meter pin (`RecomputeEndpointMeterSilenceState`). Returns true when the pin just
    /// turned on.
    pub fn refresh_groups(&mut self) -> bool {
        let Some(proxies) = &self.proxies else { return false };
        self.groups = proxies.host.groups();
        let any_active = proxies.host.any_group_active();
        self.exclusive_held = proxies.host.exclusive_held();
        self.capture_sleeping = self.flow == Flow::Capture && !any_active;
        let pin = should_pin_endpoint_meter_to_silence(self.flow, any_active, self.exclusive_held);
        let turned_on = pin && !self.pinned;
        self.pinned = pin;
        turned_on
    }

    /// `RefreshFriendlyNameFromStore`.
    pub fn refresh_names(&mut self) {
        (self.system_name, self.description, self.interface_name) = read_names(&self.device);
    }

    /// `RefreshDefaultFormatFromStore`.
    pub fn refresh_format(&mut self) {
        self.format = read_format_blob(&self.device).as_deref().and_then(parse_format_blob);
    }

    /// `RefreshListenStateFromStore` (capture only).
    pub fn refresh_listen(&mut self) {
        if self.flow == Flow::Capture {
            (self.listen_enabled, self.listen_target) = read_listen(&self.device);
        }
    }

    /// `RefreshAllowExclusiveControlFromStore`.
    pub fn refresh_exclusive_allowed(&mut self) {
        self.exclusive_allowed = read_allow_exclusive(&self.device);
    }

    /// `MeterReader.ReadStereoPeaks` on the endpoint meter; None without proxies.
    pub fn read_peaks(&self, unified: bool, low_channel_bias: i32) -> Option<(f32, f32)> {
        let proxies = self.proxies.as_ref()?;
        Some(vtadn_sessions::read_meter_peaks(&proxies.meter, unified, low_channel_bias))
    }

    /// Throttled write payload: `SetMasterVolumeLevelScalar(v, &AppGUID)`.
    pub fn write_volume(&self, volume: f32) {
        let Some(proxies) = &self.proxies else { return };
        // SAFETY: plain COM call on live proxies; the GUID outlives the call.
        if let Err(error) = unsafe { proxies.volume.SetMasterVolumeLevelScalar(volume, &IDENTITY.app_guid) } {
            log::warn!("AudioDevice.WriteEndpointVolume({}): {error}", self.system_name);
        }
    }

    /// `IsMuted = b`: written synchronously; cached only when the write succeeded.
    pub fn set_mute(&mut self, muted: bool) -> bool {
        let Some(proxies) = &self.proxies else { return false };
        // SAFETY: plain COM call on live proxies; the GUID outlives the call.
        match unsafe { proxies.volume.SetMute(muted, &IDENTITY.app_guid) } {
            Ok(()) => {
                self.muted = muted;
                true
            }
            Err(error) => {
                log::warn!("AudioDevice.IsMuted({}): {error}", self.system_name);
                false
            }
        }
    }

    fn write_store(
        &self,
        what: &str,
        write: impl FnOnce(&windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore) -> windows_core::Result<()>,
    ) -> bool {
        let result = props::open_write(&self.device).and_then(|store| {
            write(&store)?;
            props::commit(&store)
        });
        if let Err(error) = &result {
            log::warn!("AudioDevice.{what}({}): {error}", self.system_name);
        }
        result.is_ok()
    }

    /// `WriteDisableSysFx(false)`: re-enable enhancements so the listen monitor is audible.
    fn clear_disable_sys_fx(&self) {
        self.write_store("WriteDisableSysFx", |store| {
            props::set_u32(store, &props::PKEY_AUDIO_ENDPOINT_DISABLE_SYS_FX, 0)
        });
    }

    /// `SetListenEnabled` (capture only): pid 1 only, target untouched.
    pub fn write_listen_enabled(&mut self, enabled: bool) {
        if self.flow != Flow::Capture {
            return;
        }
        if enabled {
            self.clear_disable_sys_fx();
        }
        self.write_store("WriteListenBool", |store| {
            props::set_bool(store, &props::PKEY_AUDIO_ENDPOINT_LISTEN_TO_THIS_DEVICE, enabled)
        });
        self.refresh_listen();
    }

    /// `SetListenTarget(target, enable: true)` (capture only): pid 0 and pid 1 in one commit.
    pub fn write_listen_target(&mut self, target: Option<&str>) {
        if self.flow != Flow::Capture {
            return;
        }
        self.clear_disable_sys_fx();
        let written = self.write_store("SetListenTarget", |store| {
            props::set_text(
                store,
                &props::PKEY_AUDIO_ENDPOINT_LISTEN_TARGET_DEVICE_ID,
                target.filter(|t| !t.is_empty()),
            )?;
            props::set_bool(store, &props::PKEY_AUDIO_ENDPOINT_LISTEN_TO_THIS_DEVICE, true)
        });
        if written {
            self.refresh_listen();
        }
    }

    /// `ToggleAllowExclusiveControl`: pid 3 and pid 4 as VT_UI4, then re-read.
    pub fn toggle_exclusive_allowed(&mut self) {
        let allowed = u32::from(!self.exclusive_allowed);
        self.write_store("WriteAllowExclusiveControl", |store| {
            props::set_u32(store, &props::PKEY_AUDIO_ENDPOINT_ALLOW_EXCLUSIVE_CONTROL, allowed)?;
            props::set_u32(store, &props::PKEY_AUDIO_ENDPOINT_EXCLUSIVE_MODE_APPS_PRIORITY, allowed)
        });
        self.refresh_exclusive_allowed();
    }

    /// Non-Bluetooth rename: `PKEY_Device_FriendlyName` = VT_LPWSTR or VT_EMPTY, then re-read.
    pub fn write_friendly_name(&mut self, name: Option<&str>) {
        if self.write_store("SetCustomFriendlyName", |store| {
            props::set_text(store, &props::PKEY_DEVICE_FRIENDLY_NAME, name)
        }) {
            self.refresh_names();
        }
    }

    pub fn bt_info(&self) -> BtEndpointInfo {
        BtEndpointInfo {
            id: self.id.clone(),
            flow: self.flow,
            state: self.state,
            enumerator: self.enumerator.clone(),
            name: self.system_name.clone(),
            description: self.description.clone(),
            interface_name: self.interface_name.clone(),
            container_id: self.container_id,
            adapter_instance_path: self.adapter_instance_path.clone(),
        }
    }

    /// Endpoint ding bypass active (`IsDingSuppressionPeakBypassed`).
    pub fn ding_bypassed(&self, now: Instant) -> bool {
        self.ding_bypass_until.is_some_and(|until| now < until)
    }

    /// `BeginDingSuppressionPeakBypass`: monotonic max of the bypass end.
    pub fn begin_ding_bypass(&mut self, until: Instant) {
        if self.ding_bypass_until.is_none_or(|current| current < until) {
            self.ding_bypass_until = Some(until);
        }
    }

    /// Max non-unified peak of every session not owned by this process (bypass window source).
    pub fn external_peak(&self) -> f32 {
        self.host().map_or(0.0, SessionHost::external_peak)
    }

    /// `TryReadDingSuppressionPeak`: the decaying endpoint peak used to suppress the ding.
    pub fn read_ding_peak(&mut self, now: Instant) -> f32 {
        if self.ding_bypassed(now) {
            let external = self.external_peak();
            return self.ding.observe(external, now);
        }
        if self.pinned {
            return self.ding.observe(0.0, now);
        }
        match self.read_peaks(false, 0) {
            None => self.ding.read(now),
            Some((_, max)) => self.ding.observe(max, now),
        }
    }
}
