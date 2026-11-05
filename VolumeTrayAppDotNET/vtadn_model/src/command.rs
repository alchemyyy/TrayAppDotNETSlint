//! UI → audio service commands. Every command is fire-and-forget; results show up in the next
//! published snapshot. Queries carry a reply closure that the service calls on its own thread
//! (wrap UI work in `slint::invoke_from_event_loop`).

use crate::config::AudioBackendConfig;
use crate::snapshot::{AppId, DeviceId, DuckingMode, Format};

/// `QueryFormats` answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormatQuery {
    /// Formats the endpoint accepts (probed like `AudioDevice.EnumerateSupportedFormats`), sorted
    /// for display. Empty when the device would not list any: the UI then offers the 8000..192000
    /// fallback rates at the current channel count and bit depth.
    pub supported: Vec<Format>,
    /// `GetCurrentFormat`.
    pub current: Option<Format>,
}

/// Why a volume changed, for the change "ding" (`AppVolumeFeedbackPlayer`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedbackTiming {
    /// Drag release or percent commit: play now unless superseded.
    Immediate,
    /// Wheel/keyboard/tray scroll: trailing debounce (350 ms after the last change).
    Debounced,
}

pub enum AudioCommand {
    /// New settings-derived configuration (sent at startup and on every settings change).
    Configure(AudioBackendConfig),
    /// Flyout visible: sample meters (device + listed groups), poll BT radio/battery, watch ducking,
    /// open capture streams when configured. `false` stops all of it.
    SetFlyoutActive(bool),
    /// Groups whose meters the flyout currently draws.
    SetVisibleGroupMeters(Vec<(DeviceId, AppId)>),
    /// Flyout opening: re-enumerate sessions of every device (`ReconcileSessions`).
    ReconcileSessions,

    /// Endpoint scalar 0..1. Applied to the published model at once; the COM write is throttled
    /// (30 ms, latest wins).
    SetDeviceVolume {
        id: DeviceId,
        volume: f32,
    },
    SetDeviceMute {
        id: DeviceId,
        muted: bool,
    },
    /// Multimedia + console default; also communications when `SetDefaultCommsToDefault`.
    SetDefault {
        id: DeviceId,
    },
    SetDefaultComms {
        id: DeviceId,
    },
    /// Enable/disable the endpoint in Windows (IPolicyConfig::SetEndpointVisibility).
    SetEnabled {
        id: DeviceId,
        enabled: bool,
    },
    /// Rename; None/blank restores the Windows name. Bluetooth names are kept app-side
    /// (`AudioEvent::CustomNameChanged`), others are written to Windows.
    Rename {
        id: DeviceId,
        name: Option<String>,
    },
    QueryFormats {
        id: DeviceId,
        reply: Box<dyn FnOnce(FormatQuery) + Send>,
    },
    SetFormat {
        id: DeviceId,
        format: Format,
    },
    /// Capture only. Turning Listen on also re-enables the device's enhancements.
    SetListenEnabled {
        id: DeviceId,
        enabled: bool,
    },
    /// Capture only; None = default playback device. Also enables Listen.
    SetListenTarget {
        id: DeviceId,
        target: Option<DeviceId>,
    },
    ToggleExclusiveAllowed {
        id: DeviceId,
    },
    /// Install/remove Equalizer APO on the endpoint, or re-enable enhancements; restarts the
    /// endpoint. Needs elevation.
    ToggleEqualizerApo {
        id: DeviceId,
    },
    OpenEqualizerApoEditor {
        id: DeviceId,
    },

    ConnectBluetooth {
        id: DeviceId,
    },
    DisconnectBluetooth {
        id: DeviceId,
    },
    /// Turn the Bluetooth radio on/off.
    ToggleBluetoothRadio,
    SetDucking(DuckingMode),

    /// Linear session level 0..1 for every session of the group (throttled like device volume).
    SetGroupVolume {
        device: DeviceId,
        app: AppId,
        volume: f32,
    },
    SetGroupMute {
        device: DeviceId,
        app: AppId,
        muted: bool,
    },

    /// Volume-change ding through the endpoint (render devices only); suppression and settings
    /// checks happen in the service.
    PlayDeviceFeedback {
        id: DeviceId,
        timing: FeedbackTiming,
    },
    /// Volume-change ding at the group's level, through this process's own session.
    PlayAppFeedback {
        device: DeviceId,
        app: AppId,
        timing: FeedbackTiming,
    },

    /// The tray found no default device: re-resolve defaults and rebuild the device list if needed.
    RequestDefaultRecovery,
    /// Stop the service thread (sent by `AudioService::shutdown`).
    Shutdown,
}
