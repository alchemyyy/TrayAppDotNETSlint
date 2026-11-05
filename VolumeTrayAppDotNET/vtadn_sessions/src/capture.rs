//! Keeps shared-mode capture streams open on requested recording endpoints so their (software)
//! peak meters report while the flyout is visible (port of
//! `Audio/CaptureMeterActivationService.cs`). Own MTA worker; drains every 20 ms; retries every
//! 2 s. Every COM object and capture-buffer operation stays on the worker thread.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use vtadn_model::snapshot::DeviceId;
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_NOPERSIST, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows_core::HSTRING;

/// `TimeConstants.CaptureMeterActivationDrainIntervalMs`.
const DRAIN_INTERVAL: Duration = Duration::from_millis(20);
/// `TimeConstants.CaptureMeterActivationRetryIntervalMs`.
const RETRY_INTERVAL: Duration = Duration::from_millis(2000);
/// `TimeConstants.CaptureMeterActivationWorkerJoinTimeoutMs`.
const WORKER_JOIN_TIMEOUT: Duration = Duration::from_millis(2000);
/// `MaxPacketsPerDrain`.
const MAX_PACKETS_PER_DRAIN: u32 = 64;
const THREAD_NAME: &str = "vtadn-capture-meter-activation";

#[derive(Default)]
struct Request {
    ids: HashSet<DeviceId>,
    version: u64,
    disposed: bool,
    /// `AutoResetEvent` state.
    woken: bool,
}

#[derive(Default)]
struct Shared {
    request: Mutex<Request>,
    wake: Condvar,
    done: Mutex<bool>,
    done_signal: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct CaptureMeterActivation {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Default for CaptureMeterActivation {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureMeterActivation {
    pub fn new() -> Self {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        let thread = match std::thread::Builder::new().name(THREAD_NAME.to_owned()).spawn(move || {
            run(&worker);
            *lock(&worker.done) = true;
            worker.done_signal.notify_all();
        }) {
            Ok(thread) => Some(thread),
            Err(error) => {
                log::error!("CaptureMeterActivation: could not start the worker: {error}");
                None
            }
        };
        Self { shared, thread }
    }

    /// The endpoints that should be activated now; an empty list releases everything.
    pub fn set_requested(&self, devices: Vec<DeviceId>) {
        let ids: HashSet<DeviceId> = devices.into_iter().filter(|id| !id.is_empty()).collect();
        let mut request = lock(&self.shared.request);
        if request.disposed || request.ids == ids {
            return;
        }
        request.ids = ids;
        request.version += 1;
        request.woken = true;
        self.shared.wake.notify_one();
    }

    /// Stops the worker (joins with a 2 s timeout).
    pub fn shutdown(self) {
        drop(self);
    }

    fn stop(&mut self) {
        {
            let mut request = lock(&self.shared.request);
            if request.disposed {
                return;
            }
            request.disposed = true;
            request.ids.clear();
            request.woken = true;
            self.shared.wake.notify_one();
        }
        let Some(thread) = self.thread.take() else { return };
        let done = lock(&self.shared.done);
        let (done, _) = self
            .shared
            .done_signal
            .wait_timeout_while(done, WORKER_JOIN_TIMEOUT, |done| !*done)
            .unwrap_or_else(PoisonError::into_inner);
        if *done {
            drop(done);
            if thread.join().is_err() {
                log::error!("CaptureMeterActivation: worker panicked");
            }
        } else {
            log::warn!("CaptureMeterActivation: worker did not stop before shutdown timeout");
        }
    }
}

impl Drop for CaptureMeterActivation {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(shared: &Shared) {
    // SAFETY: a fresh thread has no apartment yet; balanced by CoUninitialize below.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if com.is_err() {
        log::error!("CaptureMeterActivation: CoInitializeEx failed: {com:?}");
        return;
    }
    {
        let mut worker = Worker::default();
        let mut observed_version = None;
        loop {
            let wait = {
                let request = lock(&shared.request);
                if request.disposed {
                    break;
                }
                if observed_version != Some(request.version) {
                    worker.requested.clone_from(&request.ids);
                    observed_version = Some(request.version);
                }
                drop(request);
                worker.reconcile();
                worker.drain();
                if worker.requested.is_empty() { None } else { Some(DRAIN_INTERVAL) }
            };

            let mut request = lock(&shared.request);
            match wait {
                None => {
                    while !request.woken {
                        request = shared.wake.wait(request).unwrap_or_else(PoisonError::into_inner);
                    }
                }
                Some(timeout) => {
                    if !request.woken {
                        request = shared
                            .wake
                            .wait_timeout_while(request, timeout, |r| !r.woken)
                            .unwrap_or_else(PoisonError::into_inner)
                            .0;
                    }
                }
            }
            request.woken = false;
        }
        // Streams stop and release here, before COM is uninitialized.
    }
    // SAFETY: matches the successful CoInitializeEx above.
    unsafe { CoUninitialize() };
}

#[derive(Default)]
struct Worker {
    requested: HashSet<DeviceId>,
    active: HashMap<DeviceId, CaptureStream>,
    retry_after: HashMap<DeviceId, Instant>,
    last_failures: HashMap<DeviceId, String>,
}

impl Worker {
    fn reconcile(&mut self) {
        let requested = &self.requested;
        self.active.retain(|id, _| requested.contains(id));
        self.retry_after.retain(|id, _| requested.contains(id));
        self.last_failures.retain(|id, _| requested.contains(id));

        let now = Instant::now();
        for id in &self.requested {
            if self.active.contains_key(id) || self.retry_after.get(id).is_some_and(|at| now < *at) {
                continue;
            }
            match CaptureStream::create(id) {
                Ok(stream) => {
                    self.active.insert(id.clone(), stream);
                    self.retry_after.remove(id);
                    self.last_failures.remove(id);
                    log::debug!("CaptureMeterActivation: activated recording endpoint '{id}'");
                }
                Err(failure) => {
                    self.retry_after.insert(id.clone(), now + RETRY_INTERVAL);
                    log_failure_if_changed(&mut self.last_failures, id, failure);
                }
            }
        }
    }

    fn drain(&mut self) {
        let failed: Vec<(DeviceId, String)> = self
            .active
            .iter()
            .filter_map(|(id, stream)| stream.drain().err().map(|error| (id.clone(), format!("drain {error}"))))
            .collect();
        for (id, failure) in failed {
            self.active.remove(&id);
            self.retry_after.insert(id.clone(), Instant::now() + RETRY_INTERVAL);
            log_failure_if_changed(&mut self.last_failures, &id, failure);
        }
    }
}

fn log_failure_if_changed(last_failures: &mut HashMap<DeviceId, String>, id: &DeviceId, failure: String) {
    if last_failures.get(id) == Some(&failure) {
        return;
    }
    log::debug!("CaptureMeterActivation: recording endpoint '{id}' unavailable: {failure}");
    last_failures.insert(id.clone(), failure);
}

struct CaptureStream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
}

impl CaptureStream {
    /// New enumerator -> device -> `IAudioClient` -> shared, non-persistent stream on the mix
    /// format -> `IAudioCaptureClient` -> `Start`.
    fn create(id: &str) -> Result<Self, String> {
        fn step(name: &'static str) -> impl Fn(windows_core::Error) -> String {
            move |error| format!("{name} {error}")
        }
        // SAFETY: plain COM activation and calls on this MTA thread; the mix format is freed with
        // CoTaskMemFree after Initialize copied it.
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(step("MMDeviceEnumerator"))?;
            let device = enumerator.GetDevice(&HSTRING::from(id)).map_err(step("GetDevice"))?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(step("IAudioClient activation"))?;
            let mix = client.GetMixFormat().map_err(step("GetMixFormat"))?;
            if mix.is_null() {
                return Err("GetMixFormat returned no format".to_owned());
            }
            let initialized =
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_NOPERSIST, 0, 0, mix, None);
            CoTaskMemFree(Some(mix.cast_const().cast()));
            initialized.map_err(step("Initialize"))?;
            let capture: IAudioCaptureClient = client.GetService().map_err(step("IAudioCaptureClient service"))?;
            client.Start().map_err(step("Start"))?;
            Ok(Self { client, capture })
        }
    }

    /// Releases up to 64 pending packets without reading them.
    fn drain(&self) -> windows_core::Result<()> {
        for _ in 0..MAX_PACKETS_PER_DRAIN {
            // SAFETY: plain COM calls on this thread's stream; out pointers are locals.
            unsafe {
                if self.capture.GetNextPacketSize()? == 0 {
                    return Ok(());
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                if frames == 0 {
                    return Ok(());
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
        Ok(())
    }
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        // SAFETY: stopping a started stream; the endpoint may already be gone (ignored).
        let _ = unsafe { self.client.Stop() };
    }
}
