//! One-shot connect/disconnect requests to the Bluetooth audio profile drivers behind Core
//! Audio endpoints, as the Windows Settings Connect/Disconnect buttons send them (port of
//! `Audio/BluetoothAudioConnector.cs` with the `KSTopologyInterop` parts it uses). Each profile
//! has its own driver (A2DP behind the stereo endpoint, HFP behind the hands-free pair); a
//! request is asynchronous: acceptance only means the driver started an attempt.

use std::collections::HashSet;

use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, PROPERTYKEY};
use windows::Win32::Media::Audio::{IDeviceTopology, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::Media::KernelStreaming::{
    DEVPKEY_KsAudio_Controller_DeviceInterface_Path, IKsControl, IOCTL_KS_PROPERTY, KSIDENTIFIER, KSIDENTIFIER_0,
    KSIDENTIFIER_0_0, KSPROPERTY_ONESHOT_DISCONNECT, KSPROPERTY_ONESHOT_RECONNECT, KSPROPERTY_TYPE_GET,
    KSPROPSETID_BtAudio,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Variant::VT_LPWSTR;
use windows_core::{HSTRING, PCWSTR};

use crate::util::wide;

/// Sends one request to each distinct filter behind `endpoint_ids`, never stopping at the first
/// driver that accepts. True when any driver accepted. Needs COM (MTA) on the calling worker.
pub(crate) fn request_connection(endpoint_ids: &[String], connect: bool) -> bool {
    let property_id = if connect { KSPROPERTY_ONESHOT_RECONNECT.0 } else { KSPROPERTY_ONESHOT_DISCONNECT.0 } as u32;
    let request_name = if connect { "reconnect" } else { "disconnect" };
    // SAFETY: COM is initialized on this worker thread.
    let enumerator: IMMDeviceEnumerator = match unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) } {
        Ok(enumerator) => enumerator,
        Err(error) => {
            log::warn!("BluetoothAudioConnector: {error}");
            return false;
        }
    };
    let mut requested_filters = HashSet::new();
    let mut any_accepted = false;
    for endpoint_id in endpoint_ids {
        if endpoint_id.trim().is_empty() {
            continue;
        }
        // The hands-free playback and recording endpoints share one filter: one request.
        let Some(filter_id) = resolve_filter_id(&enumerator, endpoint_id) else { continue };
        if !requested_filters.insert(filter_id.to_uppercase()) {
            continue;
        }
        if !send_request(&enumerator, &filter_id, property_id, request_name) {
            continue;
        }
        any_accepted = true;
        log::debug!("BluetoothAudioConnector: {request_name} requested for '{endpoint_id}' through '{filter_id}'");
    }
    any_accepted
}

/// Device id of the KS filter on the far side of the endpoint's connector 0 (the filter that
/// implements KSPROPSETID_BtAudio).
fn resolve_filter_id(enumerator: &IMMDeviceEnumerator, endpoint_id: &str) -> Option<String> {
    // SAFETY: COM objects are used on the thread that created them; the PWSTR returned by
    // GetDeviceIdConnectedTo is freed with CoTaskMemFree.
    unsafe {
        let endpoint = match enumerator.GetDevice(&HSTRING::from(endpoint_id)) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                log::warn!("BluetoothAudioConnector: filter lookup failed for '{endpoint_id}': {error}");
                return None;
            }
        };
        let topology: IDeviceTopology = match endpoint.Activate(CLSCTX_ALL, None) {
            Ok(topology) => topology,
            Err(error) => {
                log::warn!(
                    "BluetoothAudioConnector: Activate(IDeviceTopology) failed for '{endpoint_id}', hr=0x{:08X}",
                    error.code().0
                );
                return None;
            }
        };
        let connector = match topology.GetConnector(0) {
            Ok(connector) => connector,
            Err(error) => {
                log::warn!(
                    "BluetoothAudioConnector: IDeviceTopology.GetConnector(0) failed for '{endpoint_id}', hr=0x{:08X}",
                    error.code().0
                );
                return None;
            }
        };
        // IKsControl lives on the filter across connector 0, not on the endpoint itself.
        let filter_id = match connector.GetDeviceIdConnectedTo() {
            Ok(id) if !id.is_null() => id,
            Ok(_) => return None,
            Err(error) => {
                log::warn!(
                    "BluetoothAudioConnector: IConnector.GetDeviceIdConnectedTo failed for '{endpoint_id}', hr=0x{:08X}",
                    error.code().0
                );
                return None;
            }
        };
        let text = filter_id.to_string().ok();
        CoTaskMemFree(Some(filter_id.0.cast()));
        let text = text.filter(|t| !t.trim().is_empty())?;
        log::debug!("BluetoothAudioConnector: endpoint '{endpoint_id}' resolved to filter '{text}'");
        Some(text)
    }
}

/// The Settings route: a filter that names a controller interface (A2DP filters do) gets the
/// request there, without an IKsControl fallback; every other filter gets it via IKsControl.
fn send_request(enumerator: &IMMDeviceEnumerator, filter_id: &str, property_id: u32, request_name: &str) -> bool {
    // SAFETY: COM object used on its creating thread.
    let filter = match unsafe { enumerator.GetDevice(&HSTRING::from(filter_id)) } {
        Ok(filter) => filter,
        Err(error) => {
            log::warn!("BluetoothAudioConnector: one-shot {request_name} failed for '{filter_id}': {error}");
            return false;
        }
    };
    match read_controller_interface_path(&filter) {
        Some(path) => send_through_controller_interface(&path, property_id, request_name),
        None => send_through_ks_control(&filter, filter_id, property_id, request_name),
    }
}

/// Non-empty controller interface path the filter publishes; an unreadable store reads as
/// missing (Settings then uses IKsControl).
fn read_controller_interface_path(filter: &IMMDevice) -> Option<String> {
    let key = PROPERTYKEY {
        fmtid: DEVPKEY_KsAudio_Controller_DeviceInterface_Path.fmtid,
        pid: DEVPKEY_KsAudio_Controller_DeviceInterface_Path.pid,
    };
    // SAFETY: the property store and PROPVARIANT are used on this thread and the variant is
    // cleared before returning; the string is only read while VT_LPWSTR.
    unsafe {
        let store = match filter.OpenPropertyStore(STGM_READ) {
            Ok(store) => store,
            Err(error) => {
                log::debug!("BluetoothAudioConnector: controller interface lookup failed: {error}");
                return None;
            }
        };
        let mut value = match store.GetValue(&key) {
            Ok(value) => value,
            Err(error) => {
                log::debug!("BluetoothAudioConnector: controller interface lookup failed: {error}");
                return None;
            }
        };
        let inner = &value.Anonymous.Anonymous;
        let path = if inner.vt == VT_LPWSTR && !inner.Anonymous.pwszVal.is_null() {
            inner.Anonymous.pwszVal.to_string().ok()
        } else {
            None
        };
        let _ = PropVariantClear(&mut value);
        path.filter(|p| !p.is_empty())
    }
}

/// KSPROPERTY header for a GET of `property_id` in KSPROPSETID_BtAudio.
fn create_request(property_id: u32) -> KSIDENTIFIER {
    KSIDENTIFIER {
        Anonymous: KSIDENTIFIER_0 {
            Anonymous: KSIDENTIFIER_0_0 { Set: KSPROPSETID_BtAudio, Id: property_id, Flags: KSPROPERTY_TYPE_GET },
        },
    }
}

fn send_through_ks_control(filter: &IMMDevice, filter_id: &str, property_id: u32, request_name: &str) -> bool {
    // SAFETY: COM objects used on their creating thread; the request struct outlives the call.
    unsafe {
        let control: IKsControl = match filter.Activate(CLSCTX_ALL, None) {
            Ok(control) => control,
            Err(error) => {
                log::warn!(
                    "BluetoothAudioConnector: Activate(IKsControl) failed for '{filter_id}', hr=0x{:08X}",
                    error.code().0
                );
                return false;
            }
        };
        let request = create_request(property_id);
        let mut returned = 0u32;
        match control.KsProperty(&request, size_of::<KSIDENTIFIER>() as u32, std::ptr::null_mut(), 0, &mut returned) {
            Ok(()) => true,
            Err(error) => {
                log::warn!(
                    "BluetoothAudioConnector: one-shot {request_name} failed for '{filter_id}', hr=0x{:08X}",
                    error.code().0
                );
                false
            }
        }
    }
}

/// Opens the controller interface by its published path (`\??\` prefix included, as Settings
/// passes it) and submits the property as IOCTL_KS_PROPERTY. Settings opens the handle
/// overlapped without passing an OVERLAPPED; a synchronous handle avoids that undefined case.
fn send_through_controller_interface(path: &str, property_id: u32, request_name: &str) -> bool {
    let wide_path = wide(path);
    // SAFETY: valid NUL-terminated path; the handle is closed below.
    let handle = match unsafe {
        CreateFileW(
            PCWSTR(wide_path.as_ptr()),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    } {
        Ok(handle) if !handle.is_invalid() => handle,
        Ok(_) => return false,
        Err(error) => {
            log::warn!("BluetoothAudioConnector: CreateFile failed for controller '{path}', error={}", error.code().0);
            return false;
        }
    };
    let request = create_request(property_id);
    let mut returned = 0u32;
    // SAFETY: synchronous IOCTL on a valid handle with a live input buffer and no output.
    let result = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_KS_PROPERTY,
            Some(std::ptr::from_ref(&request).cast()),
            size_of::<KSIDENTIFIER>() as u32,
            None,
            0,
            Some(&mut returned),
            None,
        )
    };
    // SAFETY: closes the handle opened above.
    let _ = unsafe { CloseHandle(handle) };
    match result {
        Ok(()) => true,
        Err(error) => {
            log::warn!(
                "BluetoothAudioConnector: one-shot {request_name} failed for controller '{path}', error={}",
                error.code().0
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ks_property_header_matches_native_layout() {
        assert_eq!(size_of::<KSIDENTIFIER>(), 24);
        let request = create_request(KSPROPERTY_ONESHOT_RECONNECT.0 as u32);
        // SAFETY: reading the field that was written.
        let inner = unsafe { request.Anonymous.Anonymous };
        assert_eq!(inner.Set, KSPROPSETID_BtAudio);
        assert_eq!(inner.Id, 0);
        assert_eq!(inner.Flags, KSPROPERTY_TYPE_GET);
        assert_eq!(IOCTL_KS_PROPERTY, 0x002F_0003);
    }
}
