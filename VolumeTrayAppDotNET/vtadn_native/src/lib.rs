//! Bluetooth, Equalizer APO, the volume-change ding and communications ducking (ports of
//! `Audio/Bluetooth*.cs`, `Interop/BluetoothApis.cs`, `Interop/RadioManager.cs`,
//! `Audio/EqualizerAPO*.cs`, `Audio/EndpointSoundPlayback.cs`, `Audio/WAVTemplate.cs`,
//! `Audio/CommunicationsDucking.cs`). The audio service orchestrates these; nothing here owns
//! audio endpoint state.

pub mod bluetooth;
pub mod ducking;
pub mod eqapo;
pub mod feedback;
mod util;
