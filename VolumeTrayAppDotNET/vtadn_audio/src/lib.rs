//! The audio service (port of `Audio/AudioDeviceManager.cs`, `Audio/AudioDevice.cs` endpoint
//! parts, `Interop/PolicyConfig.cs`, `Audio/VolumeThrottle.cs`, `Audio/ComProxyGate.cs`, the meter
//! timers and the feedback policy of `AppVolumeFeedbackPlayer`).
//!
//! One MTA thread owns the enumerator, every endpoint and every `SessionHost`; COM callbacks and
//! workers post into its channel. It publishes [`vtadn_model::snapshot::AudioSnapshot`]s through
//! the bus and writes meters into the shared board.

mod classify;
mod com;
mod consts;
mod defaults;
mod endpoint;
mod format;
mod notify;
mod policy_config;
mod power;
mod props;
mod service;
mod throttle;

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;

use vtadn_model::bus::{AudioClient, bus};
use vtadn_model::command::AudioCommand;
use vtadn_model::config::AudioBackendConfig;

use crate::consts::SERVICE_JOIN_TIMEOUT_MS;
use crate::service::Msg;

pub struct AudioService {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    /// Signalled (or disconnected) when the audio thread finished.
    done: Receiver<()>,
}

impl AudioService {
    /// Starts the service thread. `wake` must schedule a UI-thread call ending in
    /// `AudioClient::take_update` (see `vtadn_model::bus`).
    pub fn start(config: AudioBackendConfig, wake: impl Fn() + Send + Sync + 'static) -> (Self, AudioClient) {
        let (tx, rx) = mpsc::channel();
        let command_tx = tx.clone();
        let (client, publisher) = bus(
            move |command| {
                // A closed channel means the service already stopped.
                let _ = command_tx.send(Msg::Command(command));
            },
            wake,
        );
        let (done_tx, done) = mpsc::channel();
        let thread_tx = tx.clone();
        let thread = std::thread::Builder::new()
            .name("vtadn-audio".into())
            .spawn(move || {
                service::run(config, publisher, thread_tx, rx);
                let _ = done_tx.send(());
            })
            .inspect_err(|error| log::error!("AudioService: spawning the audio thread failed: {error}"))
            .ok();
        (Self { tx, thread, done }, client)
    }

    /// Stops the thread and releases every COM object (bounded wait).
    pub fn shutdown(mut self) {
        let _ = self.tx.send(Msg::Command(AudioCommand::Shutdown));
        let Some(thread) = self.thread.take() else { return };
        match self.done.recv_timeout(SERVICE_JOIN_TIMEOUT_MS) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                if thread.join().is_err() {
                    log::error!("AudioService: the audio thread panicked");
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                log::warn!(
                    "AudioService: the audio thread did not stop within {SERVICE_JOIN_TIMEOUT_MS:?}; detaching it"
                );
            }
        }
    }
}

impl Drop for AudioService {
    /// Dropped without `shutdown`: still ask the thread to stop (without waiting).
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.tx.send(Msg::Command(AudioCommand::Shutdown));
        }
    }
}
