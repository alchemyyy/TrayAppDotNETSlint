//! Immutable audio model published by the audio service (port of the state of `AudioDevice`,
//! `AudioAppGroup`, `AudioDeviceManager` defaults, the Bluetooth radio and communications ducking).
//!
//! The service publishes a fresh [`AudioSnapshot`] after every change. `structure_rev` changes
//! only for changes that need a flyout rebuild (device list, states, defaults, names, formats,
//! Bluetooth flags, group membership/state/icon/name/mute, radio, ducking); value-only changes
//! (volume, device mute, capture sleeping, listen, exclusive, EAPO, BT countdown seconds) bump
//! only `rev` so the UI can patch rows in place.

use std::sync::Arc;

/// IMMDevice id, verbatim (`{0.0.F.00000000}.{guid}`, F = 0 render, 1 capture).
pub type DeviceId = Arc<str>;
/// App group key: lower-cased process image path, `pid:N` when the path is unknown, or the
/// system-sounds session id. Groups are per device.
pub type AppId = Arc<str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Flow {
    Render,
    Capture,
}

/// `DEVICE_STATE_*` flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DeviceState(pub u32);

impl DeviceState {
    pub const ACTIVE: u32 = 0x1;
    pub const DISABLED: u32 = 0x2;
    pub const NOT_PRESENT: u32 = 0x4;
    pub const UNPLUGGED: u32 = 0x8;

    pub const fn is_active(self) -> bool {
        self.0 & Self::ACTIVE != 0
    }

    pub const fn is_disabled(self) -> bool {
        self.0 & Self::DISABLED != 0
    }

    pub const fn is_not_present(self) -> bool {
        self.0 & Self::NOT_PRESENT != 0
    }

    pub const fn is_unplugged(self) -> bool {
        self.0 & Self::UNPLUGGED != 0
    }

    /// `IsDisconnected`: unplugged or not present.
    pub const fn is_disconnected(self) -> bool {
        self.0 & (Self::UNPLUGGED | Self::NOT_PRESENT) != 0
    }
}

/// Endpoint default format (`PKEY_AudioEngine_DeviceFormat`); `bits` is wValidBitsPerSample for
/// WAVEFORMATEXTENSIBLE.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Format {
    pub channels: u16,
    pub bits: u16,
    pub rate: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SessionState {
    #[default]
    Inactive,
    Active,
    Expired,
}

/// Equalizer APO on one endpoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EqApoState {
    /// Equalizer APO is not installed on this PC (button shows a badge; click explains).
    #[default]
    NotAvailable,
    /// Installed on the PC but not on this endpoint.
    NotInstalled,
    /// Installed on this endpoint but its enhancements are disabled.
    EnhancementsOff,
    /// Running on this endpoint.
    Running,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RadioState {
    On,
    Off,
    Unavailable,
    #[default]
    Unknown,
}

/// `UserDuckingPreference` (`HKCU\Software\Microsoft\Multimedia\Audio`); a missing value is
/// `Reduce80`, the OS default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DuckingMode {
    MuteAll,
    #[default]
    Reduce80,
    Reduce50,
    DoNothing,
}

impl DuckingMode {
    /// Ducking counts as on for every mode except `DoNothing`.
    pub const fn is_active(self) -> bool {
        !matches!(self, Self::DoNothing)
    }
}

/// Straight-alpha RGBA8 app icon pixels. The UI converts each distinct `Arc` once into a
/// `slint::Image` (which is not `Send`).
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct IconPixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Bluetooth state of an endpoint; present iff the endpoint is (or was promoted to) Bluetooth.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BtState {
    /// `PKEY_Device_ContainerId`, braced uppercase GUID text.
    pub container_id: Option<Arc<str>>,
    /// Physical device link is up (independent of the endpoint being Active).
    pub connected: bool,
    /// Paired, and the pairing is classified radio-off (kept listed while the radio is off).
    pub radio_off_pairing: bool,
    /// Reconnect observation window deadline in `GetTickCount64` milliseconds; None = not pending.
    pub pending_deadline_ms: Option<u64>,
    /// Whole seconds left in the reconnect window (ceil).
    pub seconds_remaining: u32,
    /// 0..100 while known.
    pub battery: Option<u8>,
    /// Last known level; survives disconnects for the session.
    pub last_known_battery: Option<u8>,
    /// A2DP codec name ("SBC", "AAC", "aptX", "LDAC", ...), render endpoints, admin only.
    pub codec: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppGroupSnapshot {
    pub app_id: AppId,
    /// None while unresolved (shown as "Unknown").
    pub display_name: Option<String>,
    pub icon: Option<Arc<IconPixels>>,
    /// Representative session's process id.
    pub pid: u32,
    pub is_system_sounds: bool,
    /// Active if any session is active; Inactive if any is not expired; else Expired.
    pub state: SessionState,
    /// Linear session level 0..1 (Windows' stored value).
    pub volume: f32,
    pub muted: bool,
    pub session_count: u16,
    /// Capture only: this app currently holds the endpoint in exclusive mode.
    pub is_exclusive: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceSnapshot {
    pub id: DeviceId,
    pub flow: Flow,
    /// Manager list index ("Windows order": initial enumeration render then capture, runtime
    /// additions appended).
    pub order: u32,
    pub state: DeviceState,
    /// Displayed name: system FriendlyName, or the Bluetooth custom name from devices.xml.
    pub name: String,
    /// `PKEY_Device_DeviceDesc` ("Speakers").
    pub description: String,
    /// `PKEY_DeviceInterface_FriendlyName` ("Realtek(R) Audio").
    pub interface_name: String,
    /// `PKEY_Device_EnumeratorName` ("BTHENUM", "HDAUDIO", "USB", "").
    pub enumerator: String,
    /// Multimedia default for its flow, with the last-known fallback when the OS reports none.
    pub is_default: bool,
    /// Communications default, same fallback.
    pub is_default_comms: bool,
    /// Endpoint scalar 0..1 (audio-tapered by Windows).
    pub volume: f32,
    pub muted: bool,
    /// Capture with no Active group (sleeping mic glyph).
    pub capture_sleeping: bool,
    pub format: Option<Format>,
    /// Capture only: "Listen to this device".
    pub listen_enabled: bool,
    /// Capture only: listen target; None = default playback device.
    pub listen_target: Option<DeviceId>,
    /// "Allow applications to take exclusive control"; unreadable reads as true.
    pub exclusive_allowed: bool,
    /// An app currently holds the endpoint exclusively.
    pub exclusive_held: bool,
    pub eq_apo: EqApoState,
    pub bt: Option<BtState>,
    /// Insertion order.
    pub groups: Vec<AppGroupSnapshot>,
}

impl DeviceSnapshot {
    pub fn is_capture(&self) -> bool {
        self.flow == Flow::Capture
    }

    pub fn is_bluetooth(&self) -> bool {
        self.bt.is_some()
    }

    /// Bluetooth endpoint in the Unplugged state: the only state that accepts reconnect.
    pub fn bt_unplugged(&self) -> bool {
        self.is_bluetooth() && self.state.is_unplugged()
    }

    /// Link up, endpoint still unplugged ("Connected - audio waiting").
    pub fn bt_audio_waiting(&self) -> bool {
        self.bt.as_ref().is_some_and(|b| b.connected) && self.bt_unplugged()
    }

    /// Left behind by an unpairing or an older pairing: never listed.
    pub fn bt_orphaned(&self) -> bool {
        self.bt.as_ref().is_some_and(|b| !b.radio_off_pairing) && self.state.is_not_present()
    }

    pub fn bt_pending(&self) -> bool {
        self.bt.as_ref().is_some_and(|b| b.pending_deadline_ms.is_some())
    }
}

/// Strictly Active defaults (the manager's `DefaultDevice` and friends).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Defaults {
    pub render: Option<DeviceId>,
    pub render_comms: Option<DeviceId>,
    pub capture: Option<DeviceId>,
    pub capture_comms: Option<DeviceId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioSnapshot {
    /// Bumped on every publish.
    pub rev: u64,
    /// Bumped only for rebuild-class changes (see module docs).
    pub structure_rev: u64,
    /// Manager order.
    pub devices: Vec<DeviceSnapshot>,
    pub defaults: Defaults,
    pub radio: RadioState,
    pub ducking: DuckingMode,
    /// Equalizer APO is installed on this PC.
    pub eq_apo_available: bool,
    /// The process runs elevated (EQ APO install/remove and codec monitoring need it).
    pub is_elevated: bool,
    /// False until the first enumeration finished.
    pub ready: bool,
}

impl AudioSnapshot {
    pub fn device(&self, id: &str) -> Option<&DeviceSnapshot> {
        self.devices.iter().find(|d| &*d.id == id)
    }

    /// The active default render device (tray icon, wheel).
    pub fn default_render(&self) -> Option<&DeviceSnapshot> {
        self.defaults.render.as_deref().and_then(|id| self.device(id))
    }
}
