//! Bluetooth radio power through RadioMgr.h (ports of `Interop/RadioManager.cs` and
//! `BluetoothRadioPower` in `Audio/BluetoothRadioController.cs`). The Bluetooth radio manager
//! CLSID activates the Bluetooth-specific `IMediaRadioManager`, so its instances never include
//! Wi-Fi or WWAN radios.

use vtadn_model::snapshot::RadioState;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows_core::{GUID, HRESULT};

use self::ffi::IMediaRadioManager;

/// `BluetoothRadioStateChangeTimeoutSeconds`: RadioMgr's synchronous change timeout
/// (Microsoft recommends one to five seconds).
const BLUETOOTH_RADIO_STATE_CHANGE_TIMEOUT_SECONDS: u32 = 3;

/// Bluetooth Radio Media Manager (BthRadioMedia.dll).
const CLSID_BLUETOOTH_RADIO_MANAGER: GUID = GUID::from_u128(0xafd198ac_5f30_4e89_a789_5ddf60a69366);

/// `DEVICE_RADIO_STATE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeviceRadioState {
    RadioOn,
    SoftwareRadioOff,
    HardwareRadioOff,
    SoftwareAndHardwareRadioOff,
    HardwareRadioOnUncontrollable,
    Invalid,
    HardwareRadioOffUncontrollable,
}

impl DeviceRadioState {
    pub(crate) fn from_raw(raw: i32) -> Option<Self> {
        Some(match raw {
            0 => Self::RadioOn,
            1 => Self::SoftwareRadioOff,
            2 => Self::HardwareRadioOff,
            3 => Self::SoftwareAndHardwareRadioOff,
            4 => Self::HardwareRadioOnUncontrollable,
            5 => Self::Invalid,
            6 => Self::HardwareRadioOffUncontrollable,
            _ => return None,
        })
    }

    fn raw(self) -> i32 {
        match self {
            Self::RadioOn => 0,
            Self::SoftwareRadioOff => 1,
            Self::HardwareRadioOff => 2,
            Self::SoftwareAndHardwareRadioOff => 3,
            Self::HardwareRadioOnUncontrollable => 4,
            Self::Invalid => 5,
            Self::HardwareRadioOffUncontrollable => 6,
        }
    }
}

/// RadioMgr.h desktop interfaces (Win32_Devices_PortableDevices is not a workspace feature).
#[allow(non_snake_case)]
mod ffi {
    use windows_core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, interface};

    #[interface("6cfdcab5-fc47-42a5-9241-074b58830e73")]
    pub(super) unsafe trait IMediaRadioManager: IUnknown {
        pub(super) fn GetRadioInstances(&self, collection: *mut Option<IRadioInstanceCollection>) -> HRESULT;
        pub(super) fn OnSystemRadioStateChange(&self, state: i32, timeout_seconds: u32) -> HRESULT;
    }

    #[interface("e5791fae-5665-4e0c-95be-5fde31644185")]
    pub(super) unsafe trait IRadioInstanceCollection: IUnknown {
        pub(super) fn GetCount(&self, count: *mut u32) -> HRESULT;
        pub(super) fn GetAt(&self, index: u32, instance: *mut Option<IRadioInstance>) -> HRESULT;
    }

    /// Slots before `GetRadioState`/`SetRadioState` are kept for vtable layout and never called.
    #[interface("70aa1c9e-f2b4-4c61-86d3-6b9fb75fd1a2")]
    pub(super) unsafe trait IRadioInstance: IUnknown {
        pub(super) fn GetRadioManagerSignature(&self, signature: *mut GUID) -> HRESULT;
        pub(super) fn GetInstanceSignature(&self, signature: *mut *mut core::ffi::c_void) -> HRESULT;
        pub(super) fn GetFriendlyName(&self, lcid: u32, name: *mut *mut core::ffi::c_void) -> HRESULT;
        pub(super) fn GetRadioState(&self, state: *mut i32) -> HRESULT;
        pub(super) fn SetRadioState(&self, state: i32, timeout_seconds: u32) -> HRESULT;
    }
}

/// Aggregate state of every Bluetooth radio: On when any radio is on; Off when none is on but
/// one is off; Unavailable without a valid radio.
pub(crate) fn resolve_state(states: &[DeviceRadioState]) -> RadioState {
    let mut found_off = false;
    for state in states {
        match state {
            DeviceRadioState::RadioOn | DeviceRadioState::HardwareRadioOnUncontrollable => return RadioState::On,
            DeviceRadioState::SoftwareRadioOff
            | DeviceRadioState::HardwareRadioOff
            | DeviceRadioState::SoftwareAndHardwareRadioOff
            | DeviceRadioState::HardwareRadioOffUncontrollable => found_off = true,
            DeviceRadioState::Invalid => {}
        }
    }
    if found_off { RadioState::Off } else { RadioState::Unavailable }
}

/// Whether a radio in `state` must be changed to reach `enabled`.
pub(crate) fn needs_state_change(state: DeviceRadioState, enabled: bool) -> bool {
    match state {
        DeviceRadioState::RadioOn => !enabled,
        DeviceRadioState::SoftwareRadioOff => enabled,
        DeviceRadioState::HardwareRadioOff => !enabled,
        DeviceRadioState::SoftwareAndHardwareRadioOff => enabled,
        DeviceRadioState::HardwareRadioOnUncontrollable
        | DeviceRadioState::Invalid
        | DeviceRadioState::HardwareRadioOffUncontrollable => false,
    }
}

/// Current aggregate state. Needs COM on the calling (worker) thread.
pub(crate) fn query() -> RadioState {
    execute(None)
}

/// Changes every controllable radio and returns the resulting state. Needs COM on the calling
/// (worker) thread; blocks up to the RadioMgr timeout per radio.
pub(crate) fn set_enabled(enabled: bool) -> RadioState {
    execute(Some(enabled))
}

fn log_failure(requested: Option<bool>, operation: &str, hr: HRESULT) {
    if requested.is_some() {
        log::warn!("Bluetooth radio {operation} failed: HRESULT=0x{:08X}", hr.0);
    }
}

fn execute(requested: Option<bool>) -> RadioState {
    // SAFETY: COM is initialized on this worker thread; every interface obtained here is
    // released before returning.
    unsafe {
        let manager: IMediaRadioManager =
            match CoCreateInstance(&CLSID_BLUETOOTH_RADIO_MANAGER, None, CLSCTX_INPROC_SERVER) {
                Ok(manager) => manager,
                Err(error) => {
                    if requested.is_some() {
                        log::warn!("Bluetooth radio toggle failed: {error}");
                    }
                    return RadioState::Unavailable;
                }
            };
        let mut collection = None;
        let hr = manager.GetRadioInstances(&mut collection);
        let Some(collection) = collection.filter(|_| hr.is_ok()) else {
            log_failure(requested, "enumeration", hr);
            return RadioState::Unavailable;
        };
        let mut count = 0u32;
        let hr = collection.GetCount(&mut count);
        if hr.is_err() {
            log_failure(requested, "count query", hr);
            return RadioState::Unavailable;
        }
        let mut states = Vec::with_capacity(count as usize);
        for index in 0..count {
            let mut radio = None;
            let hr = collection.GetAt(index, &mut radio);
            let Some(radio) = radio.filter(|_| hr.is_ok()) else {
                log_failure(requested, &format!("radio {index} lookup"), hr);
                continue;
            };
            let mut raw = 0i32;
            let hr = radio.GetRadioState(&mut raw);
            if hr.is_err() {
                log_failure(requested, &format!("radio {index} state query"), hr);
                continue;
            }
            let Some(mut state) = DeviceRadioState::from_raw(raw) else {
                log::warn!("Bluetooth radio {index}: unknown DEVICE_RADIO_STATE {raw}");
                continue;
            };
            if let Some(enabled) = requested
                && needs_state_change(state, enabled)
            {
                let target = if enabled { DeviceRadioState::RadioOn } else { DeviceRadioState::SoftwareRadioOff };
                let hr = radio.SetRadioState(target.raw(), BLUETOOTH_RADIO_STATE_CHANGE_TIMEOUT_SECONDS);
                if hr.is_err() {
                    log_failure(requested, &format!("radio {index} state change"), hr);
                } else {
                    let mut refreshed = 0i32;
                    let hr = radio.GetRadioState(&mut refreshed);
                    match DeviceRadioState::from_raw(refreshed).filter(|_| hr.is_ok()) {
                        Some(refreshed) => state = refreshed,
                        None => log_failure(requested, &format!("radio {index} refresh"), hr),
                    }
                }
            }
            states.push(state);
        }
        resolve_state(&states)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_state_uses_on_when_any_bluetooth_radio_is_on() {
        assert_eq!(resolve_state(&[DeviceRadioState::SoftwareRadioOff, DeviceRadioState::RadioOn]), RadioState::On);
    }

    #[test]
    fn resolve_state_treats_every_off_variant_as_off() {
        for state in [
            DeviceRadioState::SoftwareRadioOff,
            DeviceRadioState::HardwareRadioOff,
            DeviceRadioState::SoftwareAndHardwareRadioOff,
            DeviceRadioState::HardwareRadioOffUncontrollable,
        ] {
            assert_eq!(resolve_state(&[state]), RadioState::Off);
        }
    }

    #[test]
    fn resolve_state_uses_unavailable_without_a_valid_radio() {
        assert_eq!(resolve_state(&[DeviceRadioState::Invalid]), RadioState::Unavailable);
        assert_eq!(resolve_state(&[]), RadioState::Unavailable);
    }

    #[test]
    fn needs_state_change_respects_software_and_hardware_state() {
        use DeviceRadioState::*;
        let cases = [
            (RadioOn, false, true),
            (RadioOn, true, false),
            (SoftwareRadioOff, true, true),
            (SoftwareRadioOff, false, false),
            (HardwareRadioOff, false, true),
            (HardwareRadioOff, true, false),
            (SoftwareAndHardwareRadioOff, true, true),
            (HardwareRadioOnUncontrollable, false, false),
            (HardwareRadioOffUncontrollable, true, false),
        ];
        for (state, enabled, expected) in cases {
            assert_eq!(needs_state_change(state, enabled), expected, "{state:?} enabled={enabled}");
        }
    }

    #[test]
    fn raw_values_round_trip() {
        for raw in 0..=6 {
            let state = DeviceRadioState::from_raw(raw).expect("known value");
            assert_eq!(state.raw(), raw);
        }
        assert_eq!(DeviceRadioState::from_raw(7), None);
    }
}
