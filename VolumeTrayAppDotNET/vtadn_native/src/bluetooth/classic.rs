//! Classic Bluetooth device APIs (ports of `Interop/BluetoothApis.cs`,
//! `Audio/BluetoothDeviceDisconnector.cs` and the connected-address query of
//! `Audio/BluetoothBatteryMonitor.cs`).

use std::collections::HashSet;

use windows::Win32::Devices::Bluetooth::{
    BLUETOOTH_DEVICE_INFO, BLUETOOTH_DEVICE_SEARCH_PARAMS, BLUETOOTH_FIND_RADIO_PARAMS, BluetoothFindDeviceClose,
    BluetoothFindFirstDevice, BluetoothFindFirstRadio, BluetoothFindNextDevice, BluetoothFindNextRadio,
    BluetoothFindRadioClose,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_ITEMS, ERROR_NOT_FOUND, HANDLE};
use windows::Win32::System::IO::DeviceIoControl;
use windows_core::GUID;

/// `CTL_CODE(FILE_DEVICE_BLUETOOTH, 3, METHOD_BUFFERED, FILE_ANY_ACCESS)` (bthioctl.h).
const IOCTL_BTH_DISCONNECT_DEVICE: u32 = 0x0041_000C;
const BLUETOOTH_ADDRESS_HEX_LENGTH: usize = 12;

/// Remote address embedded in a BTHENUM/BTHLE devnode id after `DEV_` (12 hex digits, not
/// followed by another hex digit, non-zero).
pub fn try_parse_address(device_instance_id: Option<&str>) -> Option<u64> {
    let id = device_instance_id.filter(|id| !id.is_empty())?;
    let upper = id.to_ascii_uppercase();
    let start = upper.find("DEV_")? + 4;
    let end = start + BLUETOOTH_ADDRESS_HEX_LENGTH;
    let bytes = id.as_bytes();
    if end > bytes.len() {
        return None;
    }
    if end < bytes.len() && bytes[end].is_ascii_hexdigit() {
        return None;
    }
    let mut parsed = 0u64;
    for &byte in &bytes[start..end] {
        let digit = char::from(byte).to_digit(16)?;
        parsed = (parsed << 4) | u64::from(digit);
    }
    (parsed != 0).then_some(parsed)
}

/// Containers whose devnode address is in `connected` (`ResolveConnectedContainers`).
pub(crate) fn resolve_connected_containers<'a>(
    id_to_container: impl IntoIterator<Item = (&'a String, &'a GUID)>,
    connected: &HashSet<u64>,
) -> HashSet<GUID> {
    id_to_container
        .into_iter()
        .filter(|(id, _)| try_parse_address(Some(id)).is_some_and(|address| connected.contains(&address)))
        .map(|(_, container)| *container)
        .collect()
}

/// Disconnects a paired Classic device (keeps the pairing) through the first local radio that
/// accepts `IOCTL_BTH_DISCONNECT_DEVICE`.
pub(crate) fn try_disconnect(address: u64) -> bool {
    if address == 0 {
        return false;
    }
    let params = BLUETOOTH_FIND_RADIO_PARAMS { dwSize: size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32 };
    let mut radio = HANDLE::default();
    // SAFETY: valid params/out-parameter; the find handle is closed below.
    let find = match unsafe { BluetoothFindFirstRadio(&params, &mut radio) } {
        Ok(find) => find,
        Err(error) => {
            log::warn!("BluetoothDeviceDisconnector: no local radio found; error={}", error.code().0);
            return false;
        }
    };
    let mut last_error = 0i32;
    let mut disconnected = false;
    while !radio.is_invalid() && !radio.0.is_null() {
        let mut returned = 0u32;
        // SAFETY: synchronous IOCTL on a live radio handle with an 8-byte input buffer.
        let result = unsafe {
            DeviceIoControl(
                radio,
                IOCTL_BTH_DISCONNECT_DEVICE,
                Some(std::ptr::from_ref(&address).cast()),
                size_of::<u64>() as u32,
                None,
                0,
                Some(&mut returned),
                None,
            )
        };
        // SAFETY: each radio handle returned by the find API is closed exactly once.
        let _ = unsafe { CloseHandle(radio) };
        match result {
            Ok(()) => {
                disconnected = true;
                break;
            }
            Err(error) => last_error = error.code().0,
        }
        radio = HANDLE::default();
        // SAFETY: valid find handle and out-parameter.
        if unsafe { BluetoothFindNextRadio(find, &mut radio) }.is_err() {
            break;
        }
    }
    // SAFETY: closes the find handle opened above.
    let _ = unsafe { BluetoothFindRadioClose(find) };
    if disconnected {
        log::debug!("BluetoothDeviceDisconnector: disconnected {address:012X}");
    } else {
        log::warn!("BluetoothDeviceDisconnector: disconnect failed for {address:012X}; error={last_error}");
    }
    disconnected
}

/// Addresses of remembered/authenticated Classic devices whose cached record has `fConnected`.
/// None when the query failed (callers keep the previous state); an empty enumeration is data.
pub(crate) fn query_connected_classic_addresses() -> Option<HashSet<u64>> {
    let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
        dwSize: size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
        fReturnAuthenticated: true.into(),
        fReturnRemembered: true.into(),
        fReturnUnknown: true.into(),
        fReturnConnected: true.into(),
        fIssueInquiry: false.into(),
        cTimeoutMultiplier: 0,
        hRadio: HANDLE::default(),
    };
    let new_info = || BLUETOOTH_DEVICE_INFO { dwSize: size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
    let mut info = new_info();
    let mut connected = HashSet::new();
    // SAFETY: valid search params and a correctly sized device-info record.
    let find = match unsafe { BluetoothFindFirstDevice(&params, &mut info) } {
        Ok(find) => find,
        Err(error) => {
            let code = error.code();
            let empty =
                code.is_ok() || code == ERROR_NO_MORE_ITEMS.to_hresult() || code == ERROR_NOT_FOUND.to_hresult();
            if !empty {
                log::debug!("BluetoothBatteryMonitor: BluetoothFindFirstDevice failed; error={}", code.0);
            }
            return empty.then_some(connected);
        }
    };
    let mut ok = true;
    loop {
        // SAFETY: the union's u64 view is always initialized by the API.
        let address = unsafe { info.Address.Anonymous.ullLong };
        if address != 0 && info.fConnected.as_bool() {
            connected.insert(address);
        }
        info = new_info();
        // SAFETY: valid find handle and record.
        if let Err(error) = unsafe { BluetoothFindNextDevice(find, &mut info) } {
            let code = error.code();
            if code.is_err() && code != ERROR_NO_MORE_ITEMS.to_hresult() {
                log::debug!("BluetoothBatteryMonitor: BluetoothFindNextDevice failed; error={}", code.0);
                ok = false;
            }
            break;
        }
    }
    // SAFETY: closes the find handle opened above.
    let _ = unsafe { BluetoothFindDeviceClose(find) };
    ok.then_some(connected)
}

/// Remote address of a container: prefers a `BTHENUM\DEV_` (Classic) devnode, then any devnode
/// with an embedded address (`TryGetBluetoothAddress`).
pub(crate) fn address_for_container(id_to_container: &[(String, GUID)], container: &GUID) -> Option<u64> {
    const CLASSIC_DEVICE_PREFIX: &str = r"BTHENUM\DEV_";
    let is_classic = |id: &str| {
        id.len() >= CLASSIC_DEVICE_PREFIX.len()
            && id.is_char_boundary(CLASSIC_DEVICE_PREFIX.len())
            && id[..CLASSIC_DEVICE_PREFIX.len()].eq_ignore_ascii_case(CLASSIC_DEVICE_PREFIX)
    };
    let matching = || id_to_container.iter().filter(move |(_, c)| c == container);
    matching()
        .filter(|(id, _)| is_classic(id))
        .find_map(|(id, _)| try_parse_address(Some(id)))
        .or_else(|| matching().find_map(|(id, _)| try_parse_address(Some(id))))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn parses_bluetooth_devnode_ids() {
        let cases = [
            (r"BTHENUM\DEV_001122AABBCC\7&123456&0&BLUETOOTHDEVICE_001122AABBCC", 0x0011_22AA_BBCC_u64),
            (r"BTHLE\dev_a1b2c3d4e5f6\8&ABCDEF&0&a1b2c3d4e5f6", 0xA1B2_C3D4_E5F6),
            ("BTHENUM\\DEV_ABCDEF123456", 0xABCD_EF12_3456),
        ];
        for (id, expected) in cases {
            assert_eq!(try_parse_address(Some(id)), Some(expected), "{id}");
        }
    }

    #[test]
    fn rejects_missing_or_malformed_addresses() {
        for id in [
            None,
            Some(""),
            Some(r"BTHENUM\{0000110B-0000-1000-8000-00805F9B34FB}"),
            Some(r"BTHENUM\DEV_001122AABBC"),
            Some(r"BTHENUM\DEV_001122AABBCG"),
            Some(r"BTHENUM\DEV_001122AABBCCD"),
            Some(r"BTHENUM\DEV_000000000000"),
        ] {
            assert_eq!(try_parse_address(id), None, "{id:?}");
        }
    }

    #[test]
    fn remembered_bluetooth_devnode_is_not_treated_as_connected() {
        let container = GUID::from_u128(1);
        let map: HashMap<String, GUID> =
            [(r"BTHENUM\DEV_8099E75CA52C\7&123456&0&BLUETOOTHDEVICE_8099E75CA52C".to_owned(), container)].into();
        assert!(resolve_connected_containers(&map, &HashSet::new()).is_empty());
    }

    #[test]
    fn connected_bluetooth_address_maps_to_its_pnp_container() {
        let connected_container = GUID::from_u128(1);
        let disconnected_container = GUID::from_u128(2);
        let map: HashMap<String, GUID> = [
            (r"BTHENUM\DEV_8099E75CA52C".to_owned(), connected_container),
            (r"BTHENUM\DEV_8099E7B6BC69".to_owned(), disconnected_container),
        ]
        .into();
        let result = resolve_connected_containers(&map, &HashSet::from([0x8099_E75C_A52C_u64]));
        assert!(result.contains(&connected_container));
        assert!(!result.contains(&disconnected_container));
    }

    #[test]
    fn address_lookup_prefers_classic_devnodes() {
        let container = GUID::from_u128(7);
        let pairs = vec![
            (r"BTHLE\DEV_111111111111\x".to_owned(), container),
            (r"BTHENUM\DEV_222222222222\y".to_owned(), container),
            (r"BTHENUM\DEV_333333333333\z".to_owned(), GUID::from_u128(8)),
        ];
        assert_eq!(address_for_container(&pairs, &container), Some(0x2222_2222_2222));
        assert_eq!(address_for_container(&pairs[..1], &container), Some(0x1111_1111_1111));
        assert_eq!(address_for_container(&pairs, &GUID::from_u128(9)), None);
    }

    #[test]
    fn classic_bluetooth_structures_match_native_x64_layouts() {
        assert_eq!(size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>(), 40);
        assert_eq!(size_of::<BLUETOOTH_DEVICE_INFO>(), 560);
    }
}
