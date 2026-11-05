//! Endpoint property-store keys (C# `Interop/AudioInterop.cs` `PropertyKeys`) and PROPVARIANT
//! read/write helpers. Audio thread only.

use windows::Win32::Foundation::{PROPERTYKEY, VARIANT_BOOL};
use windows::Win32::Media::Audio::IMMDevice;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
use windows::Win32::System::Com::{STGM, STGM_READ, STGM_WRITE};
use windows::Win32::System::Variant::{VARENUM, VT_BLOB, VT_BOOL, VT_CLSID, VT_EMPTY, VT_I4, VT_LPWSTR, VT_UI4};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows_core::{GUID, PWSTR};

const fn key(fmtid: u128, pid: u32) -> PROPERTYKEY {
    PROPERTYKEY { fmtid: GUID::from_u128(fmtid), pid }
}

const DEVICE_FMTID: u128 = 0xa45c254e_df1c_4efd_8020_67d146a850e0;
const AUDIO_ENDPOINT_FMTID: u128 = 0xb3f8fa53_0004_438e_9003_51a46e139bfc;
const LISTEN_FMTID: u128 = 0x24dbb0fc_9311_4b3d_9cf0_18ff155639d4;

/// "Speakers (Realtek(R) Audio)".
pub const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = key(DEVICE_FMTID, 14);
/// "Speakers".
pub const PKEY_DEVICE_DEVICE_DESC: PROPERTYKEY = key(DEVICE_FMTID, 2);
/// "BTHENUM", "HDAUDIO", "USB".
pub const PKEY_DEVICE_ENUMERATOR_NAME: PROPERTYKEY = key(DEVICE_FMTID, 24);
/// "Realtek(R) Audio".
pub const PKEY_DEVICE_INTERFACE_FRIENDLY_NAME: PROPERTYKEY = key(0x026e516e_b814_414b_83cd_856d6fef4822, 2);
pub const PKEY_DEVICE_CONTAINER_ID: PROPERTYKEY = key(0x8c7ed206_3f8a_4827_b3ab_ae9e1faefc6c, 2);
pub const PKEY_ENDPOINT_ADAPTER_INSTANCE_PATH: PROPERTYKEY = key(AUDIO_ENDPOINT_FMTID, 2);
/// "Allow applications to take exclusive control".
pub const PKEY_AUDIO_ENDPOINT_ALLOW_EXCLUSIVE_CONTROL: PROPERTYKEY = key(AUDIO_ENDPOINT_FMTID, 3);
/// "Give exclusive mode applications priority".
pub const PKEY_AUDIO_ENDPOINT_EXCLUSIVE_MODE_APPS_PRIORITY: PROPERTYKEY = key(AUDIO_ENDPOINT_FMTID, 4);
pub const PKEY_AUDIO_ENDPOINT_LISTEN_TARGET_DEVICE_ID: PROPERTYKEY = key(LISTEN_FMTID, 0);
pub const PKEY_AUDIO_ENDPOINT_LISTEN_TO_THIS_DEVICE: PROPERTYKEY = key(LISTEN_FMTID, 1);
/// "Disable all enhancements" (VT_UI4).
pub const PKEY_AUDIO_ENDPOINT_DISABLE_SYS_FX: PROPERTYKEY = key(0x1da5d803_d492_4edd_8c23_e0c0ffee7f0e, 5);
/// WAVEFORMATEX(TENSIBLE) blob of the engine's default format.
pub const PKEY_AUDIO_ENGINE_DEVICE_FORMAT: PROPERTYKEY = key(0xf19f064d_082c_4e27_bc73_6882a1bb8e4c, 0);

/// `IMMNotificationClient::OnPropertyValueChanged` keys the service reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchedProperty {
    FriendlyName,
    Listen,
    DeviceFormat,
    AllowExclusive,
}

/// C# `NotificationBridge.OnPropertyValueChanged` key filter; any other key is ignored.
pub fn classify_property(key: &PROPERTYKEY) -> Option<WatchedProperty> {
    let same = |k: &PROPERTYKEY| k.fmtid == key.fmtid && k.pid == key.pid;
    if same(&PKEY_DEVICE_FRIENDLY_NAME) {
        Some(WatchedProperty::FriendlyName)
    } else if key.fmtid == PKEY_AUDIO_ENDPOINT_LISTEN_TO_THIS_DEVICE.fmtid
        && (key.pid == PKEY_AUDIO_ENDPOINT_LISTEN_TO_THIS_DEVICE.pid
            || key.pid == PKEY_AUDIO_ENDPOINT_LISTEN_TARGET_DEVICE_ID.pid)
    {
        Some(WatchedProperty::Listen)
    } else if same(&PKEY_AUDIO_ENGINE_DEVICE_FORMAT) {
        Some(WatchedProperty::DeviceFormat)
    } else if same(&PKEY_AUDIO_ENDPOINT_ALLOW_EXCLUSIVE_CONTROL) {
        Some(WatchedProperty::AllowExclusive)
    } else {
        None
    }
}

/// Owned PROPVARIANT, cleared on drop.
pub struct PropVariant(PROPVARIANT);

impl Drop for PropVariant {
    fn drop(&mut self) {
        // SAFETY: the variant was filled by `IPropertyStore::GetValue` and is owned here; clearing
        // frees whatever it references exactly once.
        if let Err(error) = unsafe { PropVariantClear(&mut self.0) } {
            log::warn!("PropVariantClear failed: {error}");
        }
    }
}

impl PropVariant {
    fn vt(&self) -> VARENUM {
        // SAFETY: every PROPVARIANT starts with the `vt` tag; reading it is always valid.
        unsafe { self.0.Anonymous.Anonymous.vt }
    }

    /// VT_LPWSTR text (C# `PROPVARIANT.GetString`); None for any other type or a null pointer.
    pub fn string(&self) -> Option<String> {
        if self.vt() != VT_LPWSTR {
            return None;
        }
        // SAFETY: the tag says the union holds `pwszVal`.
        let text: PWSTR = unsafe { self.0.Anonymous.Anonymous.Anonymous.pwszVal };
        if text.is_null() {
            return None;
        }
        // SAFETY: a non-null VT_LPWSTR is a NUL-terminated UTF-16 string owned by the variant.
        unsafe { text.to_string() }.ok()
    }

    pub fn as_u32(&self) -> Option<u32> {
        // SAFETY: the tag says the union holds `ulVal`.
        (self.vt() == VT_UI4).then(|| unsafe { self.0.Anonymous.Anonymous.Anonymous.ulVal })
    }

    pub fn as_i32(&self) -> Option<i32> {
        // SAFETY: the tag says the union holds `lVal`.
        (self.vt() == VT_I4).then(|| unsafe { self.0.Anonymous.Anonymous.Anonymous.lVal })
    }

    pub fn as_bool(&self) -> Option<bool> {
        // SAFETY: the tag says the union holds `boolVal`.
        (self.vt() == VT_BOOL).then(|| unsafe { self.0.Anonymous.Anonymous.Anonymous.boolVal.0 } != 0)
    }

    pub fn is_empty(&self) -> bool {
        self.vt() == VT_EMPTY
    }

    /// VT_CLSID (C# `GetGuid`).
    pub fn guid(&self) -> Option<GUID> {
        if self.vt() != VT_CLSID {
            return None;
        }
        // SAFETY: the tag says the union holds `puuid`.
        let guid = unsafe { self.0.Anonymous.Anonymous.Anonymous.puuid };
        // SAFETY: a non-null VT_CLSID pointer references a GUID owned by the variant.
        (!guid.is_null()).then(|| unsafe { *guid })
    }

    /// VT_BLOB bytes (C# `GetBlobBytes`); None for another type or an empty payload.
    pub fn blob(&self) -> Option<Vec<u8>> {
        if self.vt() != VT_BLOB {
            return None;
        }
        // SAFETY: the tag says the union holds `blob`.
        let blob = unsafe { self.0.Anonymous.Anonymous.Anonymous.blob };
        if blob.cbSize == 0 || blob.pBlobData.is_null() {
            return None;
        }
        // SAFETY: `pBlobData` points at `cbSize` bytes owned by the variant.
        Some(unsafe { std::slice::from_raw_parts(blob.pBlobData, blob.cbSize as usize) }.to_vec())
    }
}

pub fn open_store(device: &IMMDevice, access: STGM) -> windows_core::Result<IPropertyStore> {
    // SAFETY: plain COM call on a live device owned by the audio thread.
    unsafe { device.OpenPropertyStore(access) }
}

pub fn open_read(device: &IMMDevice) -> windows_core::Result<IPropertyStore> {
    open_store(device, STGM_READ)
}

pub fn open_write(device: &IMMDevice) -> windows_core::Result<IPropertyStore> {
    open_store(device, STGM_WRITE)
}

pub fn get(store: &IPropertyStore, key: &PROPERTYKEY) -> windows_core::Result<PropVariant> {
    // SAFETY: plain COM call; the returned variant is owned and cleared by `PropVariant`.
    unsafe { store.GetValue(key) }.map(PropVariant)
}

/// C# `ReadStringProperty`: VT_LPWSTR or None (also on errors).
pub fn read_string(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<String> {
    get(store, key).ok()?.string()
}

fn set(store: &IPropertyStore, key: &PROPERTYKEY, value: &PROPVARIANT) -> windows_core::Result<()> {
    // SAFETY: `value` is a valid PROPVARIANT that outlives the call; SetValue copies it.
    unsafe { store.SetValue(key, value) }
}

pub fn commit(store: &IPropertyStore) -> windows_core::Result<()> {
    // SAFETY: plain COM call.
    unsafe { store.Commit() }
}

fn variant_with(vt: VARENUM, fill: impl FnOnce(&mut PROPVARIANT)) -> PROPVARIANT {
    let mut value = PROPVARIANT::default();
    // SAFETY: writing the tag of a zeroed, caller-owned variant.
    unsafe {
        (*value.Anonymous.Anonymous).vt = vt;
    }
    fill(&mut value);
    value
}

/// VT_UI4 (`Disable_SysFx`, allow-exclusive pids 3/4).
pub fn set_u32(store: &IPropertyStore, key: &PROPERTYKEY, number: u32) -> windows_core::Result<()> {
    let value = variant_with(VT_UI4, |v| {
        // SAFETY: the tag was set to VT_UI4, so `ulVal` is the active member.
        unsafe { (*v.Anonymous.Anonymous).Anonymous.ulVal = number };
    });
    set(store, key, &value)
}

/// VT_BOOL (0xFFFF / 0), listen pid 1.
pub fn set_bool(store: &IPropertyStore, key: &PROPERTYKEY, flag: bool) -> windows_core::Result<()> {
    let value = variant_with(VT_BOOL, |v| {
        // SAFETY: the tag was set to VT_BOOL, so `boolVal` is the active member.
        unsafe { (*v.Anonymous.Anonymous).Anonymous.boolVal = VARIANT_BOOL(if flag { -1 } else { 0 }) };
    });
    set(store, key, &value)
}

/// VT_LPWSTR text, or VT_EMPTY for None (deletes the value).
pub fn set_text(store: &IPropertyStore, key: &PROPERTYKEY, text: Option<&str>) -> windows_core::Result<()> {
    match text {
        None => set(store, key, &variant_with(VT_EMPTY, |_| {})),
        Some(text) => {
            let mut wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let value = variant_with(VT_LPWSTR, |v| {
                // SAFETY: the tag was set to VT_LPWSTR; the pointer references `wide`, which
                // outlives the SetValue call below (SetValue copies the string).
                unsafe { (*v.Anonymous.Anonymous).Anonymous.pwszVal = PWSTR(wide.as_mut_ptr()) };
            });
            // Not wrapped in `PropVariant`: the buffer is ours, not CoTaskMem.
            set(store, key, &value)
        }
    }
}
