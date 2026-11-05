//! Configuration Manager helpers (the parts of `Interop/CfgMgr32.cs` the Bluetooth code needs).

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GETIDLIST_FILTER_PRESENT, CM_Get_DevNode_PropertyW, CM_Get_Device_ID_List_SizeW, CM_Get_Device_ID_ListW,
    CM_LOCATE_DEVNODE_FLAGS, CM_Locate_DevNodeW, CR_BUFFER_SMALL, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{DEVPROP_TYPE_BYTE, DEVPROP_TYPE_GUID, DEVPROP_TYPE_STRING, DEVPROPTYPE};
use windows::Win32::Foundation::DEVPROPKEY;
use windows_core::{GUID, PCWSTR};

use crate::util::{from_wide, wide};

/// `DEVPKEY_Bluetooth_Battery`: {104ea319-6ee2-4701-bd47-8ddbf425bbe5} pid 2, byte 0-100.
pub(crate) const DEVPKEY_BLUETOOTH_BATTERY: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5), pid: 2 };

/// Locates a devnode by instance id. `phantom` includes devnodes that are not present.
pub(crate) fn locate(instance_id: &str, phantom: bool) -> Option<u32> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::{CM_LOCATE_DEVNODE_NORMAL, CM_LOCATE_DEVNODE_PHANTOM};
    let id = wide(instance_id);
    let flags: CM_LOCATE_DEVNODE_FLAGS = if phantom { CM_LOCATE_DEVNODE_PHANTOM } else { CM_LOCATE_DEVNODE_NORMAL };
    let mut devinst = 0u32;
    // SAFETY: valid NUL-terminated instance id and out-parameter.
    let cr = unsafe { CM_Locate_DevNodeW(&mut devinst, PCWSTR(id.as_ptr()), flags) };
    (cr == CR_SUCCESS).then_some(devinst)
}

/// Raw property bytes when the stored type is `expected`.
fn read_property(devinst: u32, key: &DEVPROPKEY, expected: DEVPROPTYPE) -> Option<Vec<u8>> {
    let mut kind = DEVPROPTYPE::default();
    let mut size = 0u32;
    // SAFETY: size query with no buffer.
    let cr = unsafe { CM_Get_DevNode_PropertyW(devinst, key, &mut kind, None, &mut size, 0) };
    if cr != CR_BUFFER_SMALL && cr != CR_SUCCESS {
        return None;
    }
    if kind != expected || size == 0 {
        return None;
    }
    let mut buffer = vec![0u8; size as usize];
    // SAFETY: `buffer` is `size` bytes long.
    let cr = unsafe { CM_Get_DevNode_PropertyW(devinst, key, &mut kind, Some(buffer.as_mut_ptr()), &mut size, 0) };
    if cr != CR_SUCCESS {
        return None;
    }
    buffer.truncate(size as usize);
    Some(buffer)
}

/// `TryReadByteProperty`: a byte 0..=100 (battery percentage).
pub(crate) fn read_byte_property(devinst: u32, key: &DEVPROPKEY) -> Option<u8> {
    read_property(devinst, key, DEVPROP_TYPE_BYTE).and_then(|bytes| bytes.first().copied()).filter(|&v| v <= 100)
}

/// `TryReadGuidProperty`: exactly 16 bytes.
pub(crate) fn read_guid_property(devinst: u32, key: &DEVPROPKEY) -> Option<GUID> {
    let bytes = read_property(devinst, key, DEVPROP_TYPE_GUID)?;
    let bytes: [u8; 16] = bytes.as_slice().try_into().ok()?;
    Some(guid_from_bytes(&bytes))
}

/// `TryReadStringProperty`: None for a missing, mistyped or empty value.
pub(crate) fn read_string_property(devinst: u32, key: &DEVPROPKEY) -> Option<String> {
    let bytes = read_property(devinst, key, DEVPROP_TYPE_STRING)?;
    if bytes.len() < 2 {
        return None;
    }
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let value = from_wide(&units);
    (!value.is_empty()).then_some(value)
}

/// GUID from its in-memory (little-endian fields) byte layout, like `new Guid(byte[])`.
pub(crate) fn guid_from_bytes(bytes: &[u8; 16]) -> GUID {
    GUID {
        data1: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        data2: u16::from_le_bytes([bytes[4], bytes[5]]),
        data3: u16::from_le_bytes([bytes[6], bytes[7]]),
        data4: [bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]],
    }
}

/// `CM_Get_Device_ID_List(NULL, PRESENT)`: every present devnode instance id.
pub(crate) fn enumerate_present_devnode_ids() -> Vec<String> {
    let mut size = 0u32;
    // SAFETY: size query for the present-devnode list with no filter.
    let cr = unsafe { CM_Get_Device_ID_List_SizeW(&mut size, PCWSTR::null(), CM_GETIDLIST_FILTER_PRESENT) };
    if cr != CR_SUCCESS || size == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; size as usize];
    // SAFETY: `buffer` holds `size` UTF-16 units.
    let cr = unsafe { CM_Get_Device_ID_ListW(PCWSTR::null(), &mut buffer, CM_GETIDLIST_FILTER_PRESENT) };
    if cr != CR_SUCCESS {
        return Vec::new();
    }
    parse_multi_string(&buffer)
}

/// Splits a double-NUL-terminated multi-string.
pub(crate) fn parse_multi_string(buffer: &[u16]) -> Vec<String> {
    let mut ids = Vec::with_capacity(512);
    for part in buffer.split(|&c| c == 0) {
        if part.is_empty() {
            break;
        }
        ids.push(String::from_utf16_lossy(part));
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_string_stops_at_the_double_nul() {
        let mut buffer: Vec<u16> = "A\\1".encode_utf16().collect();
        buffer.push(0);
        buffer.extend("B\\2".encode_utf16());
        buffer.extend([0, 0]);
        buffer.extend("junk".encode_utf16());
        assert_eq!(parse_multi_string(&buffer), vec!["A\\1".to_owned(), "B\\2".to_owned()]);
        assert!(parse_multi_string(&[0, 0]).is_empty());
    }

    #[test]
    fn guid_bytes_use_the_native_layout() {
        let guid = GUID::from_u128(0xe0cbf06c_cd8b_4647_bb8a_263b43f0f974);
        let bytes = [0x6c, 0xf0, 0xcb, 0xe0, 0x8b, 0xcd, 0x47, 0x46, 0xbb, 0x8a, 0x26, 0x3b, 0x43, 0xf0, 0xf9, 0x74];
        assert_eq!(guid_from_bytes(&bytes), guid);
    }
}
