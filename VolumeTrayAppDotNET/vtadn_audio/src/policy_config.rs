//! Undocumented `IPolicyConfig` (port of `Interop/PolicyConfig.cs`) and the per-device
//! single-flight worker runner (`AudioDevice.RunPolicyConfigCall`), including
//! `ForceCycleEndpoint` and the synchronous default-change fan-out it waits on
//! (`AudioDeviceManager.DefaultDeviceChangedRaw`).

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use vtadn_model::snapshot::{DeviceId, Flow};
use windows::Win32::Media::Audio::{ERole, WAVEFORMATEX, eCommunications, eConsole, eMultimedia};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows_core::{GUID, HRESULT, HSTRING, PCWSTR};

use crate::com::MtaScope;
use crate::consts::{DEFAULT_DEVICE_ROLE_CHANGE_TIMEOUT_MS, DEVICE_VISIBILITY_TOGGLE_SETTLE_DELAY_MS};

/// `CLSID_PolicyConfigClient`.
const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

mod ffi {
    #![allow(non_snake_case)]

    use windows::Win32::Media::Audio::{ERole, WAVEFORMATEX};
    use windows_core::{HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR};

    /// mmsys.cpl's private policy interface (Win7 / Win10 RS1+ IID). Slot order is
    /// load-bearing: the unused methods only keep `SetDeviceFormat` at 3, `SetDefaultEndpoint`
    /// at 10 and `SetEndpointVisibility` at 11 after IUnknown.
    #[windows_core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    pub unsafe trait IPolicyConfig: IUnknown {
        fn GetMixFormat(&self) -> HRESULT;
        fn GetDeviceFormat(&self) -> HRESULT;
        fn ResetDeviceFormat(&self) -> HRESULT;
        pub fn SetDeviceFormat(
            &self,
            device_id: PCWSTR,
            endpoint_format: *const WAVEFORMATEX,
            mix_format: *const WAVEFORMATEX,
        ) -> HRESULT;
        fn GetProcessingPeriod(&self) -> HRESULT;
        fn SetProcessingPeriod(&self) -> HRESULT;
        fn GetShareMode(&self) -> HRESULT;
        fn SetShareMode(&self) -> HRESULT;
        fn GetPropertyValue(&self) -> HRESULT;
        fn SetPropertyValue(&self) -> HRESULT;
        pub fn SetDefaultEndpoint(&self, device_id: PCWSTR, role: ERole) -> HRESULT;
        /// Native `INT bVisible` (4-byte BOOL).
        pub fn SetEndpointVisibility(&self, device_id: PCWSTR, visible: i32) -> HRESULT;
    }
}

use ffi::IPolicyConfig;

/// Typed wrapper so job bodies never touch raw vtable calls.
pub struct PolicyClient {
    inner: IPolicyConfig,
}

impl PolicyClient {
    fn create() -> windows_core::Result<Self> {
        // SAFETY: plain COM activation on an MTA thread.
        let inner = unsafe { CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) }?;
        Ok(Self { inner })
    }

    pub fn set_default_endpoint(&self, id: &str, role: ERole) -> windows_core::Result<()> {
        let id = HSTRING::from(id);
        // SAFETY: `id` outlives the call.
        unsafe { self.inner.SetDefaultEndpoint(PCWSTR(id.as_ptr()), role) }.ok()
    }

    pub fn set_endpoint_visibility(&self, id: &str, visible: bool) -> windows_core::Result<()> {
        let id = HSTRING::from(id);
        // SAFETY: `id` outlives the call.
        unsafe { self.inner.SetEndpointVisibility(PCWSTR(id.as_ptr()), i32::from(visible)) }.ok()
    }

    /// Endpoint and mix format both get the same WAVEFORMATEXTENSIBLE (as mmsys.cpl does).
    pub fn set_device_format(&self, id: &str, blob: &[u8]) -> HRESULT {
        let id = HSTRING::from(id);
        let format = blob.as_ptr().cast::<WAVEFORMATEX>();
        // SAFETY: `blob` is a complete 40-byte WAVEFORMATEXTENSIBLE (WAVEFORMATEX is packed, so
        // any alignment works) and outlives the call.
        unsafe { self.inner.SetDeviceFormat(PCWSTR(id.as_ptr()), format, format) }
    }
}

/// One `IMMNotificationClient::OnDefaultDeviceChanged` callback.
#[derive(Clone, Debug)]
pub struct DefaultChange {
    pub flow: Flow,
    pub role: ERole,
    pub id: Option<String>,
}

/// `DefaultDeviceChangedRaw`: raised synchronously on the COM notification thread, before the
/// audio-thread hop, so a worker blocked in `ForceCycleEndpoint` can be woken without deadlock.
#[derive(Default)]
pub struct DefaultChangeHub {
    subscribers: Mutex<Vec<(u64, Sender<DefaultChange>)>>,
    next_token: Mutex<u64>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl DefaultChangeHub {
    pub fn raise(&self, change: &DefaultChange) {
        for (_, subscriber) in lock(&self.subscribers).iter() {
            // A subscriber that already finished waiting dropped its receiver.
            let _ = subscriber.send(change.clone());
        }
    }

    fn subscribe(self: &Arc<Self>) -> Subscription {
        let (tx, rx) = mpsc::channel();
        let token = {
            let mut next = lock(&self.next_token);
            *next += 1;
            *next
        };
        lock(&self.subscribers).push((token, tx));
        Subscription { hub: self.clone(), token, rx }
    }
}

/// Unsubscribes on drop (the C# `finally { DefaultDeviceChangedRaw -= handler }`).
struct Subscription {
    hub: Arc<DefaultChangeHub>,
    token: u64,
    rx: Receiver<DefaultChange>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        lock(&self.hub.subscribers).retain(|(token, _)| *token != self.token);
    }
}

impl Subscription {
    /// Waits until `expected` changes matching `relevant` arrived or `timeout` elapsed.
    fn wait(&self, expected: usize, timeout: Duration, relevant: impl Fn(&DefaultChange) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        let mut seen = 0;
        while seen < expected {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(remaining) {
                Ok(change) if relevant(&change) => seen += 1,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return false,
            }
        }
        true
    }
}

/// What a job may do besides IPolicyConfig calls.
pub struct JobContext {
    pub client: PolicyClient,
    hub: Arc<DefaultChangeHub>,
}

/// Role membership snapshot for `ForceCycleEndpoint` (fallback flags included, as in C#).
#[derive(Clone, Copy, Debug)]
pub struct CycleRoles {
    pub flow: Flow,
    pub was_default: bool,
    pub was_default_comms: bool,
}

/// Default-change predicate of `ForceCycleEndpoint`: same flow, a role we held, and either
/// "now someone else" (`promoted_to_us == false`) or "now us" (`true`). Ids compare
/// case-insensitively; a missing id counts as "someone else".
pub fn is_relevant_role_change(change: &DefaultChange, id: &str, roles: CycleRoles, promoted_to_us: bool) -> bool {
    if change.flow != roles.flow {
        return false;
    }
    let is_ours = change.id.as_deref().is_some_and(|new_id| new_id.eq_ignore_ascii_case(id));
    if is_ours != promoted_to_us {
        return false;
    }
    (roles.was_default && (change.role == eConsole || change.role == eMultimedia))
        || (roles.was_default_comms && change.role == eCommunications)
}

impl JobContext {
    /// `ForceCycleEndpoint`: hide + show the endpoint so the engine reloads its effects chain,
    /// preserving the default roles it held.
    pub fn force_cycle(&self, id: &str, roles: CycleRoles, description: &str) -> windows_core::Result<()> {
        if !roles.was_default && !roles.was_default_comms {
            self.client.set_endpoint_visibility(id, false)?;
            std::thread::sleep(DEVICE_VISIBILITY_TOGGLE_SETTLE_DELAY_MS);
            return self.client.set_endpoint_visibility(id, true);
        }

        let expected = if roles.was_default { 2 } else { 0 } + usize::from(roles.was_default_comms);

        // Phase 1: subscribe before disabling so the demotion fan-out cannot slip past.
        {
            let demotions = self.hub.subscribe();
            self.client.set_endpoint_visibility(id, false)?;
            if !demotions
                .wait(expected, DEFAULT_DEVICE_ROLE_CHANGE_TIMEOUT_MS, |c| is_relevant_role_change(c, id, roles, false))
            {
                log::info!("{description}: timed out waiting for default-promotion fanout");
            }
        }

        // Phase 2: re-enable.
        self.client.set_endpoint_visibility(id, true)?;

        // Phase 3: restore every role we held and wait for the confirmations.
        let restores = self.hub.subscribe();
        if roles.was_default {
            self.client.set_default_endpoint(id, eConsole)?;
            self.client.set_default_endpoint(id, eMultimedia)?;
        }
        if roles.was_default_comms {
            self.client.set_default_endpoint(id, eCommunications)?;
        }
        if !restores
            .wait(expected, DEFAULT_DEVICE_ROLE_CHANGE_TIMEOUT_MS, |c| is_relevant_role_change(c, id, roles, true))
        {
            log::info!("{description}: timed out waiting for default-restore confirmation");
        }
        Ok(())
    }
}

/// Runs IPolicyConfig work off the audio thread, at most one job per device at a time; a job for
/// a busy device is dropped (`RunPolicyConfigCall`).
#[derive(Clone)]
pub struct PolicyRunner {
    in_flight: Arc<Mutex<HashSet<DeviceId>>>,
    hub: Arc<DefaultChangeHub>,
}

/// Clears the in-flight flag even when the job panics (the C# `finally`).
struct InFlightGuard {
    in_flight: Arc<Mutex<HashSet<DeviceId>>>,
    id: DeviceId,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        lock(&self.in_flight).remove(&self.id);
    }
}

impl PolicyRunner {
    pub fn new(hub: Arc<DefaultChangeHub>) -> Self {
        Self { in_flight: Arc::default(), hub }
    }

    pub fn run(
        &self,
        id: &DeviceId,
        description: String,
        job: impl FnOnce(&JobContext) -> windows_core::Result<()> + Send + 'static,
    ) {
        if !lock(&self.in_flight).insert(id.clone()) {
            log::info!("{description}: dropped (prior IPolicyConfig call still in flight)");
            return;
        }
        let guard = InFlightGuard { in_flight: self.in_flight.clone(), id: id.clone() };
        let hub = self.hub.clone();
        let spawned = std::thread::Builder::new().name("vtadn-policy".into()).spawn(move || {
            let _guard = guard;
            let Some(_apartment) = MtaScope::enter(&description) else { return };
            // Every COM object of the job (the client included) is released inside this
            // expression, before the apartment scope ends.
            let result = PolicyClient::create().and_then(|client| job(&JobContext { client, hub }));
            if let Err(error) = result {
                log::warn!("{description}: {error}");
            }
        });
        if let Err(error) = spawned {
            log::error!("IPolicyConfig worker spawn failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "{0.0.0.00000000}.{AAAA}";

    fn change(flow: Flow, role: ERole, id: Option<&str>) -> DefaultChange {
        DefaultChange { flow, role, id: id.map(str::to_owned) }
    }

    #[test]
    fn demotion_predicate() {
        let roles = CycleRoles { flow: Flow::Render, was_default: true, was_default_comms: false };
        assert!(is_relevant_role_change(&change(Flow::Render, eConsole, Some("{other}")), ID, roles, false));
        assert!(is_relevant_role_change(&change(Flow::Render, eMultimedia, None), ID, roles, false));
        // Held only console/multimedia: comms changes are not ours to wait on.
        assert!(!is_relevant_role_change(&change(Flow::Render, eCommunications, None), ID, roles, false));
        // Other flow, or still us.
        assert!(!is_relevant_role_change(&change(Flow::Capture, eConsole, None), ID, roles, false));
        assert!(!is_relevant_role_change(&change(Flow::Render, eConsole, Some(ID)), ID, roles, false));
    }

    #[test]
    fn promotion_predicate_ignores_case() {
        let roles = CycleRoles { flow: Flow::Capture, was_default: false, was_default_comms: true };
        let lower = ID.to_ascii_lowercase();
        assert!(is_relevant_role_change(&change(Flow::Capture, eCommunications, Some(&lower)), ID, roles, true));
        assert!(!is_relevant_role_change(&change(Flow::Capture, eMultimedia, Some(ID)), ID, roles, true));
        assert!(!is_relevant_role_change(&change(Flow::Capture, eCommunications, Some("{x}")), ID, roles, true));
    }

    #[test]
    fn subscription_counts_relevant_changes_and_unsubscribes() {
        let hub = Arc::new(DefaultChangeHub::default());
        let roles = CycleRoles { flow: Flow::Render, was_default: true, was_default_comms: true };
        let subscription = hub.subscribe();
        hub.raise(&change(Flow::Render, eConsole, None));
        hub.raise(&change(Flow::Capture, eConsole, None));
        hub.raise(&change(Flow::Render, eMultimedia, Some("{x}")));
        hub.raise(&change(Flow::Render, eCommunications, Some("{y}")));
        assert!(subscription.wait(3, Duration::from_millis(1), |c| is_relevant_role_change(c, ID, roles, false)));
        assert!(!subscription.wait(1, Duration::from_millis(1), |_| true));
        drop(subscription);
        assert!(lock(&hub.subscribers).is_empty());
    }

    #[test]
    fn single_flight_drops_second_job() {
        let runner = PolicyRunner::new(Arc::default());
        let id: DeviceId = Arc::from(ID);
        lock(&runner.in_flight).insert(id.clone());
        let (tx, rx) = mpsc::channel::<()>();
        runner.run(&id, "test".into(), move |_| {
            let _ = tx.send(());
            Ok(())
        });
        // The job closure (and its sender) was dropped without running.
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
    }
}
