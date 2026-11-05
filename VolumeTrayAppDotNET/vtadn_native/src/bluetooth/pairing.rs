//! Separates the Bluetooth audio endpoints of a paired device's current pairing from those
//! Windows keeps after an unpairing or a pairing through another radio (port of
//! `Audio/BluetoothPairingResolver.cs`). The endpoint's devnode lineage decides: the current
//! pairing descends from a radio adapter that is still present.

use std::collections::HashSet;

use windows::Win32::Devices::DeviceQuery::{
    DEV_OBJECT, DEVPROP_FILTER_EXPRESSION, DEVPROP_OPERATOR, DEVPROP_OPERATOR_AND_CLOSE, DEVPROP_OPERATOR_AND_OPEN,
    DEVPROP_OPERATOR_EQUALS, DEVPROP_OPERATOR_OR_CLOSE, DEVPROP_OPERATOR_OR_OPEN, DevFreeObjects, DevGetObjects,
    DevObjectTypeAEP,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_Device_ClassGuid, DEVPKEY_Device_Parent, DEVPROP_STORE_SYSTEM, DEVPROP_TRUE, DEVPROP_TYPE_BOOLEAN,
    DEVPROP_TYPE_GUID, DEVPROPCOMPKEY, DEVPROPERTY, DEVPROPTYPE,
};
use windows::Win32::Foundation::DEVPROPKEY;
use windows_core::{GUID, PCWSTR};

use super::cfgmgr;

const SOFTWARE_ENDPOINT_INSTANCE_PREFIX: &str = r"SWD\MMDEVAPI\";
const RECORDED_PATH_SEPARATOR: &str = "}.";
const MAX_LINEAGE_DEPTH: usize = 16;

pub(crate) const BLUETOOTH_CLASS_GUID: GUID = GUID::from_u128(0xe0cbf06c_cd8b_4647_bb8a_263b43f0f974);
const BLUETOOTH_CLASSIC_PROTOCOL_ID: GUID = GUID::from_u128(0xe0cbf06c_cd8b_4647_bb8a_263b43f0f974);
const BLUETOOTH_LOW_ENERGY_PROTOCOL_ID: GUID = GUID::from_u128(0xbb7bb05e_5972_42b5_94fc_76eaa7084d49);

/// `PKEY_Devices_Aep_ProtocolId`.
const PKEY_DEVICES_AEP_PROTOCOL_ID: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0x3b2ce006_5e61_4fde_bab8_9b8aac9b26df), pid: 5 };
/// `PKEY_Devices_Aep_IsPaired`.
const PKEY_DEVICES_AEP_IS_PAIRED: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0xa35996ab_11cf_4935_8b61_a6761081ecdf), pid: 16 };
/// `PKEY_Devices_Aep_ContainerId`.
const PKEY_DEVICES_AEP_CONTAINER_ID: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0xe7c3fb29_caa7_4f47_8c8b_be59b330d4c5), pid: 2 };

/// Case-insensitive instance-id key (`StringComparer.OrdinalIgnoreCase`).
pub(crate) fn instance_key(instance_id: &str) -> String {
    instance_id.to_uppercase()
}

/// Where the lineage walk starts: the adapter devnode Windows recorded on the endpoint, else
/// the endpoint's own SWD devnode (whose parent is that same adapter devnode).
pub fn resolve_lineage_start(endpoint_id: &str, recorded_adapter_path: Option<&str>) -> String {
    let adapter = parse_recorded_adapter_instance_id(recorded_adapter_path);
    if adapter.is_empty() { format!("{SOFTWARE_ENDPOINT_INSTANCE_PREFIX}{endpoint_id}") } else { adapter.to_owned() }
}

/// Strips the `{N}.` prefix Windows puts in front of the recorded adapter instance id.
pub fn parse_recorded_adapter_instance_id(recorded: Option<&str>) -> &str {
    let Some(path) = recorded.filter(|p| !p.is_empty()) else { return "" };
    if !path.starts_with('{') {
        return path;
    }
    path.find(RECORDED_PATH_SEPARATOR).map_or("", |index| &path[index + RECORDED_PATH_SEPARATOR.len()..])
}

/// One devnode of a lineage walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineageNode {
    pub instance_id: String,
    pub is_bluetooth_class: bool,
}

impl LineageNode {
    pub fn new(instance_id: &str, is_bluetooth_class: bool) -> Self {
        Self { instance_id: instance_id.to_owned(), is_bluetooth_class }
    }
}

/// Walks `DEVPKEY_Device_Parent` upward from a devnode, phantoms included, recording whether
/// each devnode is Bluetooth class. Stops at the root, at a devnode Windows no longer has, or
/// after [`MAX_LINEAGE_DEPTH`] nodes.
pub(crate) fn resolve_lineage(start: &str) -> Vec<LineageNode> {
    let mut lineage = Vec::new();
    let mut instance_id = Some(start.to_owned());
    while lineage.len() < MAX_LINEAGE_DEPTH {
        let Some(id) = instance_id.take().filter(|id| !id.is_empty()) else { break };
        let Some(devinst) = cfgmgr::locate(&id, true) else { break };
        let class_guid = cfgmgr::read_guid_property(devinst, &DEVPKEY_Device_ClassGuid);
        lineage.push(LineageNode { instance_id: id, is_bluetooth_class: class_guid == Some(BLUETOOTH_CLASS_GUID) });
        instance_id = cfgmgr::read_string_property(devinst, &DEVPKEY_Device_Parent);
    }
    lineage
}

/// Index of the radio adapter: the top of the first run of Bluetooth-class devnodes.
pub fn find_radio_adapter_index(lineage: &[LineageNode]) -> Option<usize> {
    let mut index = lineage.iter().position(|node| node.is_bluetooth_class)?;
    while index + 1 < lineage.len() && lineage[index + 1].is_bluetooth_class {
        index += 1;
    }
    Some(index)
}

/// True when the first present devnode in the lineage is the radio adapter itself: the shape
/// of a radio that is off (the adapter stays present but tears down its stack). `present`
/// holds [`instance_key`]s.
pub fn is_radio_off_pairing(lineage: &[LineageNode], present: &HashSet<String>) -> bool {
    let Some(adapter) = find_radio_adapter_index(lineage) else { return false };
    lineage
        .iter()
        .position(|node| present.contains(&instance_key(&node.instance_id)))
        .is_some_and(|first_present| first_present == adapter)
}

/// Tracked lineage starts (as [`instance_key`]s) whose lineage is a current pairing with its
/// radio off.
pub(crate) fn resolve_radio_off_lineages(lineage_starts: &[String], present: &HashSet<String>) -> HashSet<String> {
    lineage_starts
        .iter()
        .filter(|start| is_radio_off_pairing(&resolve_lineage(start), present))
        .map(|start| instance_key(start))
        .collect()
}

fn equals_expression(
    key: DEVPROPKEY,
    kind: DEVPROPTYPE,
    value: *mut core::ffi::c_void,
    size: u32,
) -> DEVPROP_FILTER_EXPRESSION {
    DEVPROP_FILTER_EXPRESSION {
        Operator: DEVPROP_OPERATOR_EQUALS,
        Property: DEVPROPERTY {
            CompKey: DEVPROPCOMPKEY { Key: key, Store: DEVPROP_STORE_SYSTEM, LocaleName: PCWSTR::null() },
            Type: kind,
            BufferSize: size,
            Buffer: value,
        },
    }
}

fn operator_expression(operator: DEVPROP_OPERATOR) -> DEVPROP_FILTER_EXPRESSION {
    DEVPROP_FILTER_EXPRESSION { Operator: operator, Property: DEVPROPERTY::default() }
}

fn same_key(left: &DEVPROPKEY, right: &DEVPROPKEY) -> bool {
    left.fmtid == right.fmtid && left.pid == right.pid
}

/// Container ids of every paired Bluetooth Classic or LE device in the Windows pairing store
/// (the list Settings shows). None when the query fails so callers keep the last answer.
pub(crate) fn query_paired_containers() -> Option<HashSet<GUID>> {
    let mut classic = BLUETOOTH_CLASSIC_PROTOCOL_ID;
    let mut low_energy = BLUETOOTH_LOW_ENERGY_PROTOCOL_ID;
    let mut paired = DEVPROP_TRUE.0;
    let guid_size = size_of::<GUID>() as u32;
    // (ProtocolId == Classic OR ProtocolId == LE) AND IsPaired, all inside one AND group:
    // DevGetObjects rejects a group beside another top-level expression with E_INVALIDARG.
    let filter = [
        operator_expression(DEVPROP_OPERATOR_AND_OPEN),
        operator_expression(DEVPROP_OPERATOR_OR_OPEN),
        equals_expression(
            PKEY_DEVICES_AEP_PROTOCOL_ID,
            DEVPROP_TYPE_GUID,
            std::ptr::from_mut(&mut classic).cast(),
            guid_size,
        ),
        equals_expression(
            PKEY_DEVICES_AEP_PROTOCOL_ID,
            DEVPROP_TYPE_GUID,
            std::ptr::from_mut(&mut low_energy).cast(),
            guid_size,
        ),
        operator_expression(DEVPROP_OPERATOR_OR_CLOSE),
        equals_expression(PKEY_DEVICES_AEP_IS_PAIRED, DEVPROP_TYPE_BOOLEAN, std::ptr::from_mut(&mut paired).cast(), 1),
        operator_expression(DEVPROP_OPERATOR_AND_CLOSE),
    ];
    let requested = [DEVPROPCOMPKEY {
        Key: PKEY_DEVICES_AEP_CONTAINER_ID,
        Store: DEVPROP_STORE_SYSTEM,
        LocaleName: PCWSTR::null(),
    }];
    let mut count = 0u32;
    let mut objects: *mut DEV_OBJECT = std::ptr::null_mut();
    // SAFETY: the filter values point at locals that outlive the synchronous call; the result
    // array is freed with DevFreeObjects below.
    if let Err(error) =
        unsafe { DevGetObjects(DevObjectTypeAEP, 0, Some(&requested), Some(&filter), &mut count, &mut objects) }
    {
        log::debug!("BluetoothPairingResolver: DevGetObjects failed, hr=0x{:08X}", error.code().0);
        return None;
    }
    let mut containers = HashSet::new();
    if objects.is_null() {
        return Some(containers);
    }
    // SAFETY: DevGetObjects returned `count` objects at `objects`, each with
    // `cPropertyCount` properties; all stay valid until DevFreeObjects.
    unsafe {
        let records = std::slice::from_raw_parts(objects, count as usize);
        for record in records {
            if record.pProperties.is_null() {
                continue;
            }
            let properties = std::slice::from_raw_parts(record.pProperties, record.cPropertyCount as usize);
            for property in properties {
                if !same_key(&property.CompKey.Key, &PKEY_DEVICES_AEP_CONTAINER_ID)
                    || property.Type != DEVPROP_TYPE_GUID
                    || property.BufferSize != guid_size
                    || property.Buffer.is_null()
                {
                    continue;
                }
                containers.insert(property.Buffer.cast::<GUID>().read_unaligned());
            }
        }
        DevFreeObjects(records);
    }
    Some(containers)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Lineages mirror a real machine: WH-1000XM4 pairings through a removed Intel radio and the
    // current pairings through a MediaTek radio. DEVPKEY_Device_Parent reports lowercase hashes
    // while the present-devnode list reports uppercase ones.
    const MEDIATEK_RADIO: &str = r"USB\VID_13D3&PID_3602&MI_00\9&2434504c&0&0000";
    const MEDIATEK_RADIO_AS_LISTED: &str = r"USB\VID_13D3&PID_3602&MI_00\9&2434504C&0&0000";
    const MEDIATEK_COMPOSITE: &str = r"USB\VID_13D3&PID_3602\000000000";
    const MEDIATEK_ROOT_HUB: &str = r"USB\ROOT_HUB30\7&137d32ad&0&0";
    const MEDIATEK_STACK: &str = r"BTH\MS_BTHBRB\a&352eeb07&0&1";
    const MEDIATEK_STEREO_PROFILE: &str =
        r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0d58\b&63b6b4e&0&8099E75CA52C_C00000000";
    const MEDIATEK_HANDS_FREE_SERVICE: &str =
        r"BTHENUM\{0000111e-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0d58\b&63b6b4e&0&8099E75CA52C_C00000000";
    const MEDIATEK_HANDS_FREE_AUDIO: &str = r"BTHHFENUM\BthHFPAudio\c&8128029&0&97";
    const INTEL_RADIO: &str = r"USB\VID_8087&PID_0029\8&384b90af&0&6";
    const INTEL_ROOT_HUB: &str = r"USB\ROOT_HUB30\7&30304c2c&0&0";
    const INTEL_STACK: &str = r"BTH\MS_BTHBRB\9&2c6d7227&0&1";
    const INTEL_STEREO_PROFILE: &str =
        r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0002054c_PID&0d58\a&18ca57&0&8099E75CA52C_C00000000";
    const PCI_ROOT: &str = r"ACPI\PNP0A08\0";

    fn current_stereo() -> Vec<LineageNode> {
        vec![
            LineageNode::new(MEDIATEK_STEREO_PROFILE, false),
            LineageNode::new(MEDIATEK_STACK, true),
            LineageNode::new(MEDIATEK_RADIO, true),
            LineageNode::new(MEDIATEK_COMPOSITE, false),
            LineageNode::new(MEDIATEK_ROOT_HUB, false),
            LineageNode::new(PCI_ROOT, false),
        ]
    }

    fn current_hands_free() -> Vec<LineageNode> {
        vec![
            LineageNode::new(MEDIATEK_HANDS_FREE_AUDIO, false),
            LineageNode::new(MEDIATEK_HANDS_FREE_SERVICE, false),
            LineageNode::new(MEDIATEK_STACK, true),
            LineageNode::new(MEDIATEK_RADIO, true),
            LineageNode::new(MEDIATEK_COMPOSITE, false),
            LineageNode::new(MEDIATEK_ROOT_HUB, false),
            LineageNode::new(PCI_ROOT, false),
        ]
    }

    fn older_stereo() -> Vec<LineageNode> {
        vec![
            LineageNode::new(INTEL_STEREO_PROFILE, false),
            LineageNode::new(INTEL_STACK, true),
            LineageNode::new(INTEL_RADIO, true),
            LineageNode::new(INTEL_ROOT_HUB, false),
            LineageNode::new(PCI_ROOT, false),
        ]
    }

    fn present(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| instance_key(id)).collect()
    }

    #[test]
    fn current_pairing_with_radio_off_is_radio_off_pairing() {
        let present =
            present(&[MEDIATEK_RADIO_AS_LISTED, MEDIATEK_COMPOSITE, MEDIATEK_ROOT_HUB, INTEL_ROOT_HUB, PCI_ROOT]);
        assert!(is_radio_off_pairing(&current_stereo(), &present));
        assert!(is_radio_off_pairing(&current_hands_free(), &present));
    }

    #[test]
    fn current_pairing_with_radio_on_is_not_radio_off_pairing() {
        let present = present(&[
            MEDIATEK_STEREO_PROFILE,
            MEDIATEK_HANDS_FREE_SERVICE,
            MEDIATEK_HANDS_FREE_AUDIO,
            MEDIATEK_STACK,
            MEDIATEK_RADIO_AS_LISTED,
            MEDIATEK_COMPOSITE,
            MEDIATEK_ROOT_HUB,
            PCI_ROOT,
        ]);
        assert!(!is_radio_off_pairing(&current_stereo(), &present));
        assert!(!is_radio_off_pairing(&current_hands_free(), &present));
    }

    #[test]
    fn pairing_through_removed_radio_is_never_radio_off_pairing() {
        for radio_on in [true, false] {
            let present = if radio_on {
                present(&[
                    MEDIATEK_STACK,
                    MEDIATEK_RADIO_AS_LISTED,
                    MEDIATEK_COMPOSITE,
                    MEDIATEK_ROOT_HUB,
                    INTEL_ROOT_HUB,
                    PCI_ROOT,
                ])
            } else {
                present(&[MEDIATEK_RADIO_AS_LISTED, MEDIATEK_COMPOSITE, MEDIATEK_ROOT_HUB, INTEL_ROOT_HUB, PCI_ROOT])
            };
            assert!(!is_radio_off_pairing(&older_stereo(), &present));
        }
    }

    #[test]
    fn missing_profile_while_radio_on_is_not_radio_off_pairing() {
        let present =
            present(&[MEDIATEK_STACK, MEDIATEK_RADIO_AS_LISTED, MEDIATEK_COMPOSITE, MEDIATEK_ROOT_HUB, PCI_ROOT]);
        assert!(!is_radio_off_pairing(&current_hands_free(), &present));
    }

    #[test]
    fn lineage_without_radio_adapter_is_never_radio_off_pairing() {
        let wired = vec![
            LineageNode::new(r"HDAUDIO\FUNC_01&VEN_10EC&DEV_0897\5&1b9c8b3&0&0001", false),
            LineageNode::new(PCI_ROOT, false),
        ];
        let present = present(&[PCI_ROOT]);
        assert!(!is_radio_off_pairing(&wired, &present));
        assert!(!is_radio_off_pairing(&[], &present));
    }

    #[test]
    fn radio_adapter_is_top_of_first_bluetooth_run() {
        assert_eq!(find_radio_adapter_index(&current_stereo()), Some(2));
        assert_eq!(find_radio_adapter_index(&current_hands_free()), Some(3));
        assert_eq!(find_radio_adapter_index(&older_stereo()), Some(2));
        assert_eq!(find_radio_adapter_index(&[]), None);
    }

    #[test]
    fn recorded_adapter_path_loses_its_prefix() {
        let cases: [(Option<&str>, &str); 5] = [
            (
                Some(
                    r"{1}.BTHENUM\{0000110B-0000-1000-8000-00805F9B34FB}_VID&0002054C_PID&0D58\B&63B6B4E&0&8099E75CA52C_C00000000",
                ),
                r"BTHENUM\{0000110B-0000-1000-8000-00805F9B34FB}_VID&0002054C_PID&0D58\B&63B6B4E&0&8099E75CA52C_C00000000",
            ),
            (Some(r"BTHHFENUM\BthHFPAudio\c&8128029&0&97"), r"BTHHFENUM\BthHFPAudio\c&8128029&0&97"),
            (Some(r"{1}BTHENUM\missing-separator"), ""),
            (Some(""), ""),
            (None, ""),
        ];
        for (recorded, expected) in cases {
            assert_eq!(parse_recorded_adapter_instance_id(recorded), expected);
        }
    }

    #[test]
    fn endpoint_without_recorded_adapter_starts_at_its_software_devnode() {
        const ENDPOINT: &str = "{0.0.0.00000000}.{031f2cba-f397-4334-84c2-312cba509f6e}";
        assert_eq!(
            resolve_lineage_start(ENDPOINT, None),
            r"SWD\MMDEVAPI\{0.0.0.00000000}.{031f2cba-f397-4334-84c2-312cba509f6e}"
        );
        let recorded = format!("{{1}}.{MEDIATEK_STEREO_PROFILE}");
        assert_eq!(resolve_lineage_start(ENDPOINT, Some(&recorded)), MEDIATEK_STEREO_PROFILE);
    }

    #[test]
    fn device_query_structures_match_native_x64_layouts() {
        assert_eq!(size_of::<DEVPROPCOMPKEY>(), 32);
        assert_eq!(size_of::<DEVPROPERTY>(), 48);
        assert_eq!(size_of::<DEVPROP_FILTER_EXPRESSION>(), 56);
        assert_eq!(size_of::<DEV_OBJECT>(), 32);
    }
}
