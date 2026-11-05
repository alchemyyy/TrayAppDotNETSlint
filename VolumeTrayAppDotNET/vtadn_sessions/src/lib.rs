//! Per-endpoint audio session tracking (port of `Audio/AudioSession.cs`, `Audio/AudioAppGroup.cs`,
//! the session parts of `Audio/AudioDevice.cs`, `Audio/AppIconResolver.cs`,
//! `Audio/CaptureMeterActivationService.cs`, `Audio/ProcessHelper.cs`, `Audio/MeterReader.cs`).
//!
//! Everything here is driven by the audio service thread (MTA). COM callbacks and background
//! workers never touch host state directly: they post [`SessionEvent`]s through the
//! [`SessionSink`] the service supplies, and the service hands each event back to the
//! [`SessionHost`] of that device.

pub mod capture;
pub mod host;
pub mod icons;
mod meter;
mod process;

pub use capture::CaptureMeterActivation;
pub use host::{HostChange, SessionEvent, SessionHost, SessionHostConfig, SessionSink};
pub use icons::IconCache;
pub use meter::read_meter_peaks;
