//! The channel between the audio service thread and the UI thread.
//!
//! - UI → service: [`AudioClient::send`] (a closure installed by the service, so the service's
//!   internal message type stays private).
//! - Service → UI: [`AudioPublisher::publish`] stores the newest snapshot and calls `wake` only when
//!   no wake is outstanding; the UI's wake handler calls [`AudioClient::take_update`], which clears
//!   the flag. Bursts collapse into one UI hop carrying the latest state (backpressure, no timers).
//! - Meters: [`MeterBoard`] is shared memory written by the sampler and read by the UI render tick.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::command::AudioCommand;
use crate::meters::MeterBoard;
use crate::snapshot::{AudioSnapshot, DeviceId};

/// Service → UI notifications that are not part of the snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioEvent {
    /// Persist as runtime state (no settings-changed cascade).
    LastKnownDefaults {
        playback: Option<String>,
        comms_playback: Option<String>,
        recording: Option<String>,
        comms_recording: Option<String>,
    },
    /// A Bluetooth endpoint's app-side name changed: persist in devices.xml (None/blank clears).
    CustomNameChanged { id: DeviceId, name: Option<String> },
}

struct BusInner {
    send: Box<dyn Fn(AudioCommand) + Send + Sync>,
    wake: Box<dyn Fn() + Send + Sync>,
    latest: Mutex<Arc<AudioSnapshot>>,
    events: Mutex<Vec<AudioEvent>>,
    wake_pending: AtomicBool,
    meters: MeterBoard,
}

/// UI-side handle. Cheap to clone; `Send + Sync`.
#[derive(Clone)]
pub struct AudioClient {
    inner: Arc<BusInner>,
}

/// Service-side handle.
#[derive(Clone)]
pub struct AudioPublisher {
    inner: Arc<BusInner>,
}

/// Creates a connected client/publisher pair. `send` forwards commands into the service; `wake`
/// must schedule a UI-thread call that ends in [`AudioClient::take_update`].
pub fn bus(
    send: impl Fn(AudioCommand) + Send + Sync + 'static,
    wake: impl Fn() + Send + Sync + 'static,
) -> (AudioClient, AudioPublisher) {
    let inner = Arc::new(BusInner {
        send: Box::new(send),
        wake: Box::new(wake),
        latest: Mutex::new(Arc::new(AudioSnapshot::default())),
        events: Mutex::new(Vec::new()),
        wake_pending: AtomicBool::new(false),
        meters: MeterBoard::default(),
    });
    (AudioClient { inner: inner.clone() }, AudioPublisher { inner })
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl AudioClient {
    pub fn send(&self, command: AudioCommand) {
        (self.inner.send)(command);
    }

    /// Newest published snapshot (does not clear the wake flag).
    pub fn latest(&self) -> Arc<AudioSnapshot> {
        lock(&self.inner.latest).clone()
    }

    /// Clears the wake flag and returns the newest snapshot plus queued events. Call from the wake
    /// handler on the UI thread.
    pub fn take_update(&self) -> (Arc<AudioSnapshot>, Vec<AudioEvent>) {
        self.inner.wake_pending.store(false, Ordering::Release);
        let snapshot = lock(&self.inner.latest).clone();
        let events = std::mem::take(&mut *lock(&self.inner.events));
        (snapshot, events)
    }

    pub fn meters(&self) -> &MeterBoard {
        &self.inner.meters
    }
}

impl AudioPublisher {
    pub fn publish(&self, snapshot: Arc<AudioSnapshot>) {
        *lock(&self.inner.latest) = snapshot;
        self.wake_once();
    }

    pub fn push_event(&self, event: AudioEvent) {
        lock(&self.inner.events).push(event);
        self.wake_once();
    }

    pub fn latest(&self) -> Arc<AudioSnapshot> {
        lock(&self.inner.latest).clone()
    }

    pub fn meters(&self) -> &MeterBoard {
        &self.inner.meters
    }

    fn wake_once(&self) {
        if !self.inner.wake_pending.swap(true, Ordering::AcqRel) {
            (self.inner.wake)();
        }
    }
}
