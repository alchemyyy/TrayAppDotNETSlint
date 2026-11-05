//! Equalizer APO detection and per-endpoint install/remove (ports of `Audio/EqualizerAPO.cs`,
//! `Audio/EqualizerAPOInstaller.cs`, and `AudioDevice.ToggleEqualizerAPO` /
//! `RefreshEqualizerAPOState`). Registry writes need elevation.
//!
//! The installer is a faithful port of Equalizer APO 1.4.2's `DeviceAPOInfo`: key paths, value
//! names and sentinels are verbatim.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use vtadn_model::snapshot::EqApoState;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS,
    GENERIC_ALL, HANDLE, HLOCAL, LUID, LocalFree, WAIT_OBJECT_0, WIN32_ERROR,
};
use windows::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetSecurityInfo, NO_MULTIPLE_TRUSTEE, SE_REGISTRY_KEY, SetEntriesInAclW,
    SetSecurityInfo, TRUSTEE_IS_GROUP, TRUSTEE_IS_SID, TRUSTEE_W,
};
use windows::Win32::Security::{
    ACL, AdjustTokenPrivileges, CONTAINER_INHERIT_ACE, CreateWellKnownSid, DACL_SECURITY_INFORMATION,
    LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    PSID, SE_PRIVILEGE_ENABLED, SE_TAKE_OWNERSHIP_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    WinBuiltinAdministratorsSid,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::Win32::Storage::FileSystem::{READ_CONTROL, WRITE_DAC, WRITE_OWNER};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ,
    REG_MULTI_SZ, REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW,
    RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::System::Threading::{GetCurrentProcess, INFINITE, OpenProcessToken, WaitForMultipleObjects};
use windows_core::{GUID, PCWSTR};

use crate::util::{OwnedEvent, from_wide, guid_braced_lower, wide};

pub const DOWNLOAD_URL: &str = "https://sourceforge.net/projects/equalizerapo/";

/// `EqualizerAPOMonitor.LatestInstallerURL`: the pinned x64 installer the not-available dialog
/// opens.
pub const LATEST_INSTALLER_URL: &str =
    "https://sourceforge.net/projects/equalizerapo/files/1.4.2/EqualizerAPO-x64-1.4.2.exe/download";

/// Default 64-bit install location (`EqualizerAPOMonitor.DefaultInstallDir`).
pub const DEFAULT_INSTALL_DIR: &str = r"C:\Program Files\EqualizerAPO";
const EDITOR_EXE_NAME: &str = "Editor.exe";
const EQUALIZER_APO_DLL_NAME: &str = "EqualizerAPO.dll";

/// What a toggle did; the caller restarts (force-cycles) the endpoint after a change.
/// The C# app force-cycles after `Installed` and `Removed` only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EqApoToggle {
    Installed,
    Removed,
    EnhancementsEnabled,
    Unchanged,
}

// Pre-mix / post-mix CLSIDs registered by EqualizerAPO.dll (EAPO helpers/RegistryHelper.h).
const PRE_MIX_GUID: GUID = GUID::from_u128(0xeacd2258_fcac_4ff4_b36d_419e924a6d79);
const POST_MIX_GUID: GUID = GUID::from_u128(0xec1cc9ce_faed_4822_828a_82a81a6f018f);

/// Backup sentinel: FxProperties did not exist before install (uninstall deletes the key).
const APOGUID_NOKEY: &str = "!KEY";
/// Backup sentinel: FxProperties existed but this slot had no value (uninstall deletes it).
const APOGUID_NOVALUE: &str = "!VALUE";

const LFX: usize = 0;
const GFX: usize = 1;
const SFX: usize = 2;
const MFX: usize = 3;
const EFX: usize = 4;
const FX_SLOT_COUNT: usize = 5;

const FX_SLOT_VALUE_NAMES: [&str; FX_SLOT_COUNT] = [
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},1", // LFX (pre-mix, pre-Win8.1)
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},2", // GFX (post-mix, pre-Win8.1)
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},5", // SFX (pre-mix)
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},6", // MFX (post-mix, software pipe)
    "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},7", // EFX (post-mix, endpoint pipe)
];

const MULTI_SFX_VALUE_NAME: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},13";
const MULTI_MFX_VALUE_NAME: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},14";
const MULTI_EFX_VALUE_NAME: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},15";

const SFX_PROCESSING_MODES_VALUE_NAME: &str = "{d3993a3f-99c2-4402-b5ec-a92a0367664b},5";
const MFX_PROCESSING_MODES_VALUE_NAME: &str = "{d3993a3f-99c2-4402-b5ec-a92a0367664b},6";
const EFX_PROCESSING_MODES_VALUE_NAME: &str = "{d3993a3f-99c2-4402-b5ec-a92a0367664b},7";
const DEFAULT_PROCESSING_MODE_VALUE: &str = "{c18e2f7e-933d-4965-b7d1-1eef228d2af3}";

/// PKEY_AudioEndpoint_Disable_SysFx as materialized on FxProperties.
const DISABLE_ENHANCEMENTS_VALUE_NAME: &str = "{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5";
/// PKEY_AudioEndpoint_CombinedDevice (Win11 combined Bluetooth endpoints; EFX cannot load).
const COMBINED_DEVICE_VALUE_NAME: &str = "{b3f8fa53-0004-438e-9003-51a46e139bfc},41";
const FX_TITLE_VALUE_NAME: &str = "{b725f130-47ef-101a-a5f1-02608c9eebac},10";

const PRE_MIX_CHILD_GUID_VALUE_NAME: &str = "PreMixChild";
const POST_MIX_CHILD_GUID_VALUE_NAME: &str = "PostMixChild";
const ALLOW_SILENT_BUFFER_VALUE_NAME: &str = "AllowSilentBufferModification";
const DISABLE_AUTO_ADJUST_VALUE_NAME: &str = "DisableAutomaticAdjustment";
const VERSION_VALUE_NAME: &str = "Version";
const INSTALL_VERSION: &str = "2";

const MMDEVICES_RENDER_SUB_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render";
const MMDEVICES_CAPTURE_SUB_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture";
const APP_REG_SUB_KEY: &str = r"SOFTWARE\EqualizerAPO";
const CHILD_APO_SUB_KEY: &str = r"SOFTWARE\EqualizerAPO\Child APOs";
const APO_REGISTRATION_SUB_KEY: &str = r"AudioEngine\AudioProcessingObjects";
const CLSID_SUB_KEY: &str = "CLSID";

/// MMDevice DeviceState NotPresent bit as published in the registry.
const DEVICE_STATE_NOTPRESENT: u32 = 0x0000_0004;

/// FX-slot pairing the EAPO chain installs into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstallMode {
    /// Pre-Win8.1 driver shim; also used when the driver only published LFX/GFX.
    LfxGfx,
    /// Win11 combined Bluetooth endpoints where EFX will not load.
    SfxMfx,
    /// Default Win8.1+ pairing.
    SfxEfx,
}

/// Snapshot of one endpoint's EAPO install state (`DeviceAPOInfo`).
#[derive(Clone, Debug)]
struct DeviceApoInfo {
    is_installed: bool,
    enhancements_disabled: bool,
    current_install_mode: InstallMode,
}

/// Registry failure with the operation that hit it.
#[derive(Debug)]
struct RegError {
    code: WIN32_ERROR,
    context: String,
}

impl std::fmt::Display for RegError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = windows_core::Error::from(self.code).message();
        write!(f, "{}: {} (error {})", self.context, message, self.code.0)
    }
}

type RegResult<T> = Result<T, RegError>;

fn reg_error(code: WIN32_ERROR, context: impl Into<String>) -> RegError {
    RegError { code, context: context.into() }
}

/// Registry value as `Microsoft.Win32.RegistryKey.GetValue` classifies it.
#[derive(Debug, Clone, PartialEq)]
enum RegValue {
    Dword(u32),
    String(String),
    Other,
}

/// Owned registry key handle opened in the 64-bit view.
struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: the key was opened/created by this module and is closed exactly once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

impl RegKey {
    /// `OpenSubKey`: None when the key does not exist; other failures are errors.
    fn open(root: HKEY, path: &str, access: REG_SAM_FLAGS) -> RegResult<Option<Self>> {
        let wide_path = wide(path);
        let mut key = HKEY::default();
        // SAFETY: valid NUL-terminated path; ownership of `key` moves into `RegKey`.
        let status =
            unsafe { RegOpenKeyExW(root, PCWSTR(wide_path.as_ptr()), None, access | KEY_WOW64_64KEY, &mut key) };
        match status {
            ERROR_SUCCESS => Ok(Some(Self(key))),
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
            code => Err(reg_error(code, format!("open {path}"))),
        }
    }

    fn open_read(root: HKEY, path: &str) -> RegResult<Option<Self>> {
        Self::open(root, path, KEY_READ)
    }

    /// `OpenSubKey(path, writable: true)`.
    fn open_write(root: HKEY, path: &str) -> RegResult<Option<Self>> {
        Self::open(root, path, KEY_READ | KEY_WRITE)
    }

    /// `CreateSubKey(path, writable: true)`: opens or creates.
    fn create(root: HKEY, path: &str) -> RegResult<Self> {
        let wide_path = wide(path);
        let mut key = HKEY::default();
        // SAFETY: valid NUL-terminated path; ownership of `key` moves into `RegKey`.
        let status = unsafe {
            RegCreateKeyExW(
                root,
                PCWSTR(wide_path.as_ptr()),
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
                None,
                &mut key,
                None,
            )
        };
        if status == ERROR_SUCCESS { Ok(Self(key)) } else { Err(reg_error(status, format!("create {path}"))) }
    }

    fn value(&self, name: &str) -> Option<RegValue> {
        let wide_name = wide(name);
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        // SAFETY: size query with no data buffer.
        let status = unsafe {
            RegQueryValueExW(self.0, PCWSTR(wide_name.as_ptr()), None, Some(&mut kind), None, Some(&mut size))
        };
        if status != ERROR_SUCCESS && status != ERROR_MORE_DATA {
            return None;
        }
        let mut buffer = vec![0u8; size as usize + 2];
        let mut size = buffer.len() as u32;
        // SAFETY: `buffer` is `size` bytes long.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(wide_name.as_ptr()),
                None,
                Some(&mut kind),
                Some(buffer.as_mut_ptr()),
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        buffer.truncate(size as usize);
        Some(match kind {
            REG_DWORD if buffer.len() >= 4 => {
                RegValue::Dword(u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]))
            }
            REG_SZ | REG_EXPAND_SZ => {
                let units: Vec<u16> = buffer.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                RegValue::String(from_wide(&units))
            }
            _ => RegValue::Other,
        })
    }

    fn string(&self, name: &str) -> Option<String> {
        match self.value(name) {
            Some(RegValue::String(text)) => Some(text),
            _ => None,
        }
    }

    fn has_value(&self, name: &str) -> bool {
        self.value(name).is_some()
    }

    fn set_raw(&self, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> RegResult<()> {
        let wide_name = wide(name);
        // SAFETY: writes `data` from a live buffer.
        let status = unsafe { RegSetValueExW(self.0, PCWSTR(wide_name.as_ptr()), None, kind, Some(data)) };
        if status == ERROR_SUCCESS { Ok(()) } else { Err(reg_error(status, format!("set value {name}"))) }
    }

    fn set_string(&self, name: &str, value: &str) -> RegResult<()> {
        let bytes: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
        self.set_raw(name, REG_SZ, &bytes)
    }

    fn set_multi_string(&self, name: &str, values: &[&str]) -> RegResult<()> {
        let mut units: Vec<u16> = Vec::new();
        for value in values {
            units.extend(value.encode_utf16());
            units.push(0);
        }
        units.push(0);
        let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
        self.set_raw(name, REG_MULTI_SZ, &bytes)
    }

    /// `DeleteValue(name, throwOnMissingValue: false)`.
    fn delete_value(&self, name: &str) -> RegResult<()> {
        let wide_name = wide(name);
        // SAFETY: valid NUL-terminated value name.
        match unsafe { RegDeleteValueW(self.0, PCWSTR(wide_name.as_ptr())) } {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            code => Err(reg_error(code, format!("delete value {name}"))),
        }
    }

    /// `DeleteValueIfPresent`.
    fn delete_value_if_present(&self, name: &str) -> RegResult<()> {
        if self.has_value(name) { self.delete_value(name) } else { Ok(()) }
    }

    /// `DeleteSubKeyTree(sub, throwOnMissingSubKey: false)`.
    fn delete_tree(&self, sub_key: &str) -> RegResult<()> {
        let wide_sub = wide(sub_key);
        // SAFETY: valid NUL-terminated subkey name relative to this open key.
        match unsafe { RegDeleteTreeW(self.0, PCWSTR(wide_sub.as_ptr())) } {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(()),
            code => Err(reg_error(code, format!("delete tree {sub_key}"))),
        }
    }

    /// (`SubKeyCount`, `ValueCount`).
    fn counts(&self) -> RegResult<(u32, u32)> {
        let mut sub_keys = 0u32;
        let mut values = 0u32;
        // SAFETY: only the two count out-parameters are requested.
        let status = unsafe {
            RegQueryInfoKeyW(
                self.0,
                None,
                None,
                None,
                Some(&mut sub_keys),
                None,
                None,
                Some(&mut values),
                None,
                None,
                None,
                None,
            )
        };
        if status == ERROR_SUCCESS { Ok((sub_keys, values)) } else { Err(reg_error(status, "query key info")) }
    }
}

fn key_exists(root: HKEY, path: &str) -> bool {
    matches!(RegKey::open_read(root, path), Ok(Some(_)))
}

fn device_sub_key(device_guid: &str, is_capture: bool) -> String {
    let parent = if is_capture { MMDEVICES_CAPTURE_SUB_KEY } else { MMDEVICES_RENDER_SUB_KEY };
    format!(r"{parent}\{device_guid}")
}

/// `AudioDevice.TryExtractEndpointGuid`: the `{guid}` after `}.` in an IMMDevice id.
pub fn endpoint_guid(endpoint_id: &str) -> Option<&str> {
    endpoint_id.find("}.{").map(|separator| &endpoint_id[separator + 2..])
}

/// `EqualizerAPOMonitor.ResolveInstallDir`.
pub fn install_dir() -> PathBuf {
    PathBuf::from(DEFAULT_INSTALL_DIR)
}

/// Equalizer APO is installed on this PC: `EqualizerAPO.dll` exists in the install folder and
/// the EAPO COM server is registered (both re-probed on every call).
pub fn is_available() -> bool {
    install_dir().join(EQUALIZER_APO_DLL_NAME).is_file() && is_apo_registered()
}

/// `IsAPORegistered`: both APOs are published under AudioEngine\AudioProcessingObjects and CLSID.
fn is_apo_registered() -> bool {
    let pre = guid_braced_lower(&PRE_MIX_GUID);
    let post = guid_braced_lower(&POST_MIX_GUID);
    key_exists(HKEY_CLASSES_ROOT, &format!(r"{APO_REGISTRATION_SUB_KEY}\{pre}"))
        && key_exists(HKEY_CLASSES_ROOT, &format!(r"{APO_REGISTRATION_SUB_KEY}\{post}"))
        && key_exists(HKEY_CLASSES_ROOT, &format!(r"{CLSID_SUB_KEY}\{pre}"))
        && key_exists(HKEY_CLASSES_ROOT, &format!(r"{CLSID_SUB_KEY}\{post}"))
}

/// `AudioDevice.RefreshEqualizerAPOState` for one endpoint (full IMMDevice id).
pub fn probe(endpoint_id: &str, capture: bool) -> EqApoState {
    if !is_available() {
        return EqApoState::NotAvailable;
    }
    let Some(device_guid) = endpoint_guid(endpoint_id) else {
        return EqApoState::NotAvailable;
    };
    match probe_device(device_guid, capture) {
        Some(DeviceApoInfo { is_installed: true, enhancements_disabled: true, .. }) => EqApoState::EnhancementsOff,
        Some(DeviceApoInfo { is_installed: true, .. }) => EqApoState::Running,
        _ => EqApoState::NotInstalled,
    }
}

/// `AudioDevice.ToggleEqualizerAPO`: Running → uninstall; EnhancementsOff → reinstall (force-
/// clears the disable-sysfx value); NotInstalled → install; NotAvailable → nothing. Runs the
/// registry I/O synchronously on the calling thread (run it on a worker). Errors (typically
/// "admin required") are logged and returned; the caller re-probes either way.
pub fn toggle(endpoint_id: &str, capture: bool) -> Result<EqApoToggle, String> {
    let Some(device_guid) = endpoint_guid(endpoint_id) else {
        log::warn!("ToggleEqualizerAPO: no endpoint GUID in '{endpoint_id}'");
        return Ok(EqApoToggle::Unchanged);
    };
    let before = probe(endpoint_id, capture);
    log::info!("ToggleEqualizerAPO: begin state={before:?} guid={device_guid} capture={capture}");
    let result = match before {
        EqApoState::Running => uninstall(device_guid, capture).map(|()| EqApoToggle::Removed),
        EqApoState::EnhancementsOff => reinstall(device_guid, capture).map(|()| EqApoToggle::EnhancementsEnabled),
        EqApoState::NotInstalled => install(device_guid, capture).map(|()| EqApoToggle::Installed),
        EqApoState::NotAvailable => return Ok(EqApoToggle::Unchanged),
    };
    match result {
        Ok(done) => {
            log::info!("ToggleEqualizerAPO: end state={:?}", probe(endpoint_id, capture));
            Ok(done)
        }
        Err(error) => {
            let message = if error.code == ERROR_ACCESS_DENIED {
                format!("admin required - {error}")
            } else {
                format!("state={before:?}: {error}")
            };
            log::warn!("ToggleEqualizerAPO({device_guid}): {message}");
            Err(message)
        }
    }
}

/// `DeviceAPOInfo::load` minus channel/rate readout. None when the endpoint is not in the
/// registry or is a NotPresent ghost.
fn probe_device(device_guid: &str, is_capture: bool) -> Option<DeviceApoInfo> {
    let sub_key = device_sub_key(device_guid, is_capture);
    let device_key = match RegKey::open_read(HKEY_LOCAL_MACHINE, &sub_key) {
        Ok(Some(key)) => key,
        Ok(None) => return None,
        Err(error) => {
            log::warn!("EqualizerAPOInstaller.Probe({device_guid}, capture={is_capture}): {error}");
            return None;
        }
    };
    let state = match device_key.value("DeviceState") {
        Some(RegValue::Dword(state)) => state,
        _ => 0,
    };
    if state & DEVICE_STATE_NOTPRESENT != 0 {
        return None;
    }

    let mut enhancements_disabled = false;
    let mut found_at = [false; FX_SLOT_COUNT];
    let mut installed = false;
    let pre = guid_braced_lower(&PRE_MIX_GUID);
    let post = guid_braced_lower(&POST_MIX_GUID);
    if let Ok(Some(fx_props)) = RegKey::open_read(HKEY_LOCAL_MACHINE, &format!(r"{sub_key}\FxProperties")) {
        enhancements_disabled =
            matches!(fx_props.value(DISABLE_ENHANCEMENTS_VALUE_NAME), Some(RegValue::Dword(d)) if d != 0);
        for (slot, name) in FX_SLOT_VALUE_NAMES.iter().enumerate() {
            if let Some(raw) = fx_props.string(name)
                && (raw.eq_ignore_ascii_case(&pre) || raw.eq_ignore_ascii_case(&post))
            {
                found_at[slot] = true;
                installed = true;
            }
        }
    }

    let current_install_mode = infer_install_mode(&sub_key, &found_at, installed);
    Some(DeviceApoInfo { is_installed: installed, enhancements_disabled, current_install_mode })
}

/// EAPO's `load()` install-mode selection (see the C# `InferInstallMode`).
fn infer_install_mode(sub_key: &str, found_at: &[bool; FX_SLOT_COUNT], installed: bool) -> InstallMode {
    if installed {
        if found_at[LFX] || found_at[GFX] {
            return InstallMode::LfxGfx;
        }
        if found_at[EFX] {
            return InstallMode::SfxEfx;
        }
        if found_at[SFX] || found_at[MFX] {
            return InstallMode::SfxMfx;
        }
    }
    if !is_windows_version_at_least(6, 3) {
        return InstallMode::LfxGfx;
    }
    if let Ok(Some(fx_props)) = RegKey::open_read(HKEY_LOCAL_MACHINE, &format!(r"{sub_key}\FxProperties")) {
        let has = |name: &str| fx_props.has_value(name);
        let legacy_only = (has(FX_SLOT_VALUE_NAMES[LFX]) || has(FX_SLOT_VALUE_NAMES[GFX]))
            && !has(FX_SLOT_VALUE_NAMES[SFX])
            && !has(FX_SLOT_VALUE_NAMES[MFX])
            && !has(FX_SLOT_VALUE_NAMES[EFX])
            && !has(MULTI_SFX_VALUE_NAME)
            && !has(MULTI_MFX_VALUE_NAME)
            && !has(MULTI_EFX_VALUE_NAME);
        if legacy_only {
            return InstallMode::LfxGfx;
        }
    }
    if let Ok(Some(props)) = RegKey::open_read(HKEY_LOCAL_MACHINE, &format!(r"{sub_key}\Properties"))
        && props.has_value(COMBINED_DEVICE_VALUE_NAME)
    {
        return InstallMode::SfxMfx;
    }
    InstallMode::SfxEfx
}

/// EAPO `RegistryHelper::isWindowsVersionAtLeast`, read from the registry so an application
/// manifest cannot lie about it (Windows 10+ publishes `CurrentMajorVersionNumber`).
fn is_windows_version_at_least(major: u32, minor: u32) -> bool {
    let Ok(Some(key)) = RegKey::open_read(HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion") else {
        return true;
    };
    let (actual_major, actual_minor) =
        match (key.value("CurrentMajorVersionNumber"), key.value("CurrentMinorVersionNumber")) {
            (Some(RegValue::Dword(a)), Some(RegValue::Dword(b))) => (a, b),
            _ => {
                let Some(text) = key.string("CurrentVersion") else { return true };
                let mut parts = text.split('.').map(|p| p.trim().parse::<u32>().unwrap_or(0));
                (parts.next().unwrap_or(0), parts.next().unwrap_or(0))
            }
        };
    if actual_major != major { actual_major > major } else { actual_minor >= minor }
}

/// `DeviceAPOInfo::install` (see the C# `Install`). Capture endpoints get only the pre-mix.
fn install(device_guid: &str, is_capture: bool) -> RegResult<()> {
    let existing = probe_device(device_guid, is_capture);
    let mode = existing.map_or(InstallMode::SfxEfx, |info| info.current_install_mode);
    let install_post_mix = !is_capture;

    let _app_root = RegKey::create(HKEY_LOCAL_MACHINE, APP_REG_SUB_KEY)?;
    let _child_apos = RegKey::create(HKEY_LOCAL_MACHINE, CHILD_APO_SUB_KEY)?;
    let child_device = RegKey::create(HKEY_LOCAL_MACHINE, &format!(r"{CHILD_APO_SUB_KEY}\{device_guid}"))?;
    let device_key_path = device_sub_key(device_guid, is_capture);
    let fx_props_path = format!(r"{device_key_path}\FxProperties");

    let fx_existed_before = matches!(RegKey::open_read(HKEY_LOCAL_MACHINE, &fx_props_path), Ok(Some(_)));
    let fx_props = if fx_existed_before {
        open_writable_with_fallback(HKEY_LOCAL_MACHINE, &fx_props_path)
            .ok_or_else(|| reg_error(ERROR_ACCESS_DENIED, format!("cannot open {fx_props_path}")))?
    } else {
        RegKey::create(HKEY_LOCAL_MACHINE, &fx_props_path)?
    };

    if fx_existed_before {
        // Snapshot every slot: real GUIDs become the restore value, missing values NOVALUE.
        for name in FX_SLOT_VALUE_NAMES {
            let existing_slot = fx_props.string(name);
            child_device.set_string(name, existing_slot.as_deref().unwrap_or(APOGUID_NOVALUE))?;
        }
    } else {
        fx_props.set_string(FX_TITLE_VALUE_NAME, "Equalizer APO")?;
        for name in FX_SLOT_VALUE_NAMES {
            child_device.set_string(name, APOGUID_NOKEY)?;
        }
    }

    child_device.set_string(PRE_MIX_CHILD_GUID_VALUE_NAME, "")?;
    child_device.set_string(POST_MIX_CHILD_GUID_VALUE_NAME, "")?;
    child_device.set_string(ALLOW_SILENT_BUFFER_VALUE_NAME, "false")?;
    if child_device.has_value(DISABLE_AUTO_ADJUST_VALUE_NAME) {
        child_device.delete_value(DISABLE_AUTO_ADJUST_VALUE_NAME)?;
    }
    child_device.set_string(VERSION_VALUE_NAME, INSTALL_VERSION)?;

    let pre = guid_braced_lower(&PRE_MIX_GUID);
    let post = guid_braced_lower(&POST_MIX_GUID);
    let set_with_mode = |slot: usize, guid: &str, modes_name: &str| -> RegResult<()> {
        fx_props.set_string(FX_SLOT_VALUE_NAMES[slot], guid)?;
        if !fx_props.has_value(modes_name) {
            fx_props.set_multi_string(modes_name, &[DEFAULT_PROCESSING_MODE_VALUE])?;
        }
        Ok(())
    };
    match mode {
        InstallMode::LfxGfx => {
            fx_props.set_string(FX_SLOT_VALUE_NAMES[LFX], &pre)?;
            if install_post_mix {
                fx_props.set_string(FX_SLOT_VALUE_NAMES[GFX], &post)?;
            }
            fx_props.delete_value_if_present(FX_SLOT_VALUE_NAMES[SFX])?;
            fx_props.delete_value_if_present(FX_SLOT_VALUE_NAMES[MFX])?;
            fx_props.delete_value_if_present(FX_SLOT_VALUE_NAMES[EFX])?;
        }
        InstallMode::SfxMfx => {
            fx_props.delete_value_if_present(FX_SLOT_VALUE_NAMES[LFX])?;
            fx_props.delete_value_if_present(FX_SLOT_VALUE_NAMES[GFX])?;
            set_with_mode(SFX, &pre, SFX_PROCESSING_MODES_VALUE_NAME)?;
            if install_post_mix {
                set_with_mode(MFX, &post, MFX_PROCESSING_MODES_VALUE_NAME)?;
            }
            // EFX deliberately left alone ("don't change efx").
        }
        InstallMode::SfxEfx => {
            fx_props.delete_value_if_present(FX_SLOT_VALUE_NAMES[LFX])?;
            fx_props.delete_value_if_present(FX_SLOT_VALUE_NAMES[GFX])?;
            set_with_mode(SFX, &pre, SFX_PROCESSING_MODES_VALUE_NAME)?;
            if install_post_mix {
                set_with_mode(EFX, &post, EFX_PROCESSING_MODES_VALUE_NAME)?;
            }
            // MFX deliberately left alone.
        }
    }
    // Force-enable enhancements; a stale 1 makes the engine bypass every APO.
    fx_props.delete_value_if_present(DISABLE_ENHANCEMENTS_VALUE_NAME)
}

/// `DeviceAPOInfo::uninstall` (see the C# `Uninstall`).
fn uninstall(device_guid: &str, is_capture: bool) -> RegResult<()> {
    let device_key_path = device_sub_key(device_guid, is_capture);
    let fx_props_path = format!(r"{device_key_path}\FxProperties");
    let child_device_path = format!(r"{CHILD_APO_SUB_KEY}\{device_guid}");

    let mut original: [String; FX_SLOT_COUNT] = Default::default();
    let mut child_backup_found = false;
    if let Some(child_device) = RegKey::open_read(HKEY_LOCAL_MACHINE, &child_device_path)? {
        child_backup_found = true;
        for (slot, name) in FX_SLOT_VALUE_NAMES.iter().enumerate() {
            if let Some(value) = child_device.string(name) {
                original[slot] = value;
            }
        }
    }
    log::info!(
        "EqualizerAPOInstaller.Uninstall({device_guid}): childBackup={child_backup_found} slots=[{}]",
        original.join(",")
    );

    if original[0] == APOGUID_NOKEY {
        match open_writable_with_fallback(HKEY_LOCAL_MACHINE, &device_key_path) {
            None => log::warn!(
                "EqualizerAPOInstaller.Uninstall({device_guid}): could not open {device_key_path} for write (key missing or access denied)"
            ),
            Some(device_key) => {
                device_key.delete_tree("FxProperties")?;
                log::info!("EqualizerAPOInstaller.Uninstall({device_guid}): deleted synthetic FxProperties key");
            }
        }
    } else {
        match open_writable_with_fallback(HKEY_LOCAL_MACHINE, &fx_props_path) {
            None => log::warn!(
                "EqualizerAPOInstaller.Uninstall({device_guid}): could not open {fx_props_path} for write (key missing or access denied)"
            ),
            Some(fx_props) => {
                let pre = guid_braced_lower(&PRE_MIX_GUID);
                let post = guid_braced_lower(&POST_MIX_GUID);
                for (slot, name) in FX_SLOT_VALUE_NAMES.iter().enumerate() {
                    let backup = &original[slot];
                    if backup == APOGUID_NOVALUE {
                        fx_props.delete_value_if_present(name)?;
                    } else if !backup.is_empty() {
                        fx_props.set_string(name, backup)?;
                    } else if let Some(current) = fx_props.string(name)
                        && (current.eq_ignore_ascii_case(&pre) || current.eq_ignore_ascii_case(&post))
                    {
                        // No backup for this slot: clear the orphaned EAPO GUID.
                        fx_props.delete_value(name)?;
                        log::info!(
                            "EqualizerAPOInstaller.Uninstall({device_guid}): cleared orphaned EAPO GUID from slot {slot} (no Child APOs backup)"
                        );
                    }
                }
                log::info!("EqualizerAPOInstaller.Uninstall({device_guid}): replayed slots from backup");
            }
        }
    }

    let Some(child_parent) = RegKey::open_write(HKEY_LOCAL_MACHINE, CHILD_APO_SUB_KEY)? else {
        return Ok(());
    };
    if let Err(error) = child_parent.delete_value(device_guid) {
        log::warn!("EqualizerAPOInstaller.Uninstall({device_guid}): backup cleanup failed: {error}");
    }
    child_parent.delete_tree(device_guid)?;
    let (sub_keys, values) = child_parent.counts()?;
    if sub_keys != 0 || values != 0 {
        return Ok(());
    }
    drop(child_parent);
    if let Some(app_root) = RegKey::open_write(HKEY_LOCAL_MACHINE, APP_REG_SUB_KEY)? {
        app_root.delete_tree("Child APOs")?;
    }
    Ok(())
}

/// `DeviceAPOInfo::reinstall`.
fn reinstall(device_guid: &str, is_capture: bool) -> RegResult<()> {
    uninstall(device_guid, is_capture)?;
    install(device_guid, is_capture)
}

/// Opens `path` writable, falling back to take-ownership + FullControl grant on access denied.
/// None when the key is missing or access could not be granted (logged).
fn open_writable_with_fallback(root: HKEY, path: &str) -> Option<RegKey> {
    match RegKey::open_write(root, path) {
        Ok(key) => key,
        Err(error) if error.code == ERROR_ACCESS_DENIED => {
            log::warn!(
                "EqualizerAPOInstaller.OpenWritableWithFallback({path}): access denied, attempting take-ownership fallback"
            );
            try_grant_write_access(path);
            match RegKey::open_write(root, path) {
                Ok(key) => key,
                Err(error) => {
                    log::warn!(
                        "EqualizerAPOInstaller.OpenWritableWithFallback({path}): retry after fallback failed - {error}"
                    );
                    None
                }
            }
        }
        Err(error) => {
            log::warn!("EqualizerAPOInstaller.OpenWritableWithFallback({path}): {error}");
            None
        }
    }
}

/// Port of EAPO `RegistryHelper::takeOwnership` + `makeWritable`: enable SeTakeOwnership, make
/// BUILTIN\Administrators the owner, then add a FullControl (container+object inherit) ACE.
fn try_grant_write_access(path: &str) {
    if !try_enable_take_ownership_privilege() {
        log::warn!(
            "EqualizerAPOInstaller.TryGrantWriteAccess({path}): SeTakeOwnership privilege unavailable - process needs admin"
        );
        return;
    }
    let mut sid_buffer = [0u8; 68]; // SECURITY_MAX_SID_SIZE
    let mut sid_size = sid_buffer.len() as u32;
    let admins = PSID(sid_buffer.as_mut_ptr().cast());
    // SAFETY: `sid_buffer` is SECURITY_MAX_SID_SIZE bytes, enough for any well-known SID.
    if let Err(error) = unsafe { CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(admins), &mut sid_size) } {
        log::warn!("EqualizerAPOInstaller.TryGrantWriteAccess({path}): CreateWellKnownSid failed - {error}");
        return;
    }

    // Phase 1: take ownership.
    let owner_key = match RegKey::open(HKEY_LOCAL_MACHINE, path, REG_SAM_FLAGS(WRITE_OWNER.0 | READ_CONTROL.0)) {
        Ok(Some(key)) => key,
        Ok(None) => {
            log::warn!("EqualizerAPOInstaller.TryGrantWriteAccess({path}): could not open with TakeOwnership");
            return;
        }
        Err(error) => {
            log::warn!("EqualizerAPOInstaller.TryGrantWriteAccess({path}): take-ownership failed - {error}");
            return;
        }
    };
    // SAFETY: the key handle is valid and `admins` points at a well-formed SID in `sid_buffer`.
    let status = unsafe {
        SetSecurityInfo(
            HANDLE(owner_key.0.0),
            SE_REGISTRY_KEY,
            OWNER_SECURITY_INFORMATION,
            Some(admins),
            None,
            None,
            None,
        )
    };
    drop(owner_key);
    if status != ERROR_SUCCESS {
        log::warn!(
            "EqualizerAPOInstaller.TryGrantWriteAccess({path}): take-ownership failed - {}",
            reg_error(status, "SetSecurityInfo(owner)")
        );
        return;
    }

    // Phase 2: grant FullControl to Administrators in the DACL.
    let dacl_key = match RegKey::open(HKEY_LOCAL_MACHINE, path, REG_SAM_FLAGS(WRITE_DAC.0 | READ_CONTROL.0)) {
        Ok(Some(key)) => key,
        _ => {
            log::warn!(
                "EqualizerAPOInstaller.TryGrantWriteAccess({path}): could not open with ChangePermissions after ownership"
            );
            return;
        }
    };
    let mut old_dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: requests the DACL of a valid key handle; `descriptor` is freed with LocalFree.
    let status = unsafe {
        GetSecurityInfo(
            HANDLE(dacl_key.0.0),
            SE_REGISTRY_KEY,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut old_dacl),
            None,
            Some(&mut descriptor),
        )
    };
    if status != ERROR_SUCCESS {
        log::warn!(
            "EqualizerAPOInstaller.TryGrantWriteAccess({path}): DACL grant failed - {}",
            reg_error(status, "GetSecurityInfo(dacl)")
        );
        return;
    }
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL.0,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_GROUP,
            ptstrName: windows_core::PWSTR(admins.0.cast()),
        },
    };
    let mut new_dacl: *mut ACL = std::ptr::null_mut();
    // SAFETY: `old_dacl` points into `descriptor` (still alive); `new_dacl` is freed below.
    let status = unsafe { SetEntriesInAclW(Some(&[access]), Some(old_dacl), &mut new_dacl) };
    if status == ERROR_SUCCESS {
        // SAFETY: valid key handle and a DACL produced by SetEntriesInAclW.
        let status = unsafe {
            SetSecurityInfo(
                HANDLE(dacl_key.0.0),
                SE_REGISTRY_KEY,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(new_dacl),
                None,
            )
        };
        if status == ERROR_SUCCESS {
            log::info!("EqualizerAPOInstaller.TryGrantWriteAccess({path}): admins granted FullControl");
        } else {
            log::warn!(
                "EqualizerAPOInstaller.TryGrantWriteAccess({path}): DACL grant failed - {}",
                reg_error(status, "SetSecurityInfo(dacl)")
            );
        }
        // SAFETY: `new_dacl` was allocated by SetEntriesInAclW with LocalAlloc.
        let _ = unsafe { LocalFree(Some(HLOCAL(new_dacl.cast()))) };
    } else {
        log::warn!(
            "EqualizerAPOInstaller.TryGrantWriteAccess({path}): DACL grant failed - {}",
            reg_error(status, "SetEntriesInAclW")
        );
    }
    // SAFETY: `descriptor` was allocated by GetSecurityInfo with LocalAlloc.
    let _ = unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
}

fn try_enable_take_ownership_privilege() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: the current-process pseudo handle is always valid; the token is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut luid = LUID::default();
    // SAFETY: valid privilege name constant and out-parameter.
    let enabled = unsafe { LookupPrivilegeValueW(PCWSTR::null(), SE_TAKE_OWNERSHIP_NAME, &mut luid) }.is_ok() && {
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
        };
        // SAFETY: valid token with TOKEN_ADJUST_PRIVILEGES and a one-entry privilege list.
        let adjusted = unsafe { AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None) };
        // AdjustTokenPrivileges succeeds with ERROR_NOT_ALL_ASSIGNED for non-admins.
        adjusted.is_ok() && windows_core::Error::from_thread().code().is_ok()
    };
    // SAFETY: closes the token opened above.
    let _ = unsafe { CloseHandle(token) };
    enabled
}

/// Launches the Configuration Editor, scoped to the endpoint's `config\{guid}\config.txt` when
/// one exists (only after a successful install); otherwise with the default config.
pub fn open_configuration_editor(endpoint_id: &str) {
    let dir = install_dir();
    let editor = dir.join(EDITOR_EXE_NAME);
    if !editor.is_file() {
        log::warn!("OpenConfigurationEditor({endpoint_id}): {} not found", editor.display());
        return;
    }
    let mut command = std::process::Command::new(&editor);
    command.current_dir(&dir);
    if let Some(device_guid) = endpoint_guid(endpoint_id) {
        let per_device = dir.join("config").join(device_guid).join("config.txt");
        if per_device.is_file() {
            use std::os::windows::process::CommandExt;
            command.raw_arg(format!("\"{}\"", per_device.display()));
        }
    }
    if let Err(error) = command.spawn() {
        log::warn!("OpenConfigurationEditor({endpoint_id}): {error}");
    }
}

/// Watches the install folder's parent for the `EqualizerAPO` entry being created, deleted or
/// renamed (C# `FileSystemWatcher` with Filter = "EqualizerAPO"); `on_change` runs on a worker
/// thread and listeners re-read [`is_available`] and [`probe`]. Stops when dropped.
pub struct AvailabilityWatcher {
    stop: Option<Arc<OwnedEvent>>,
    done: Option<mpsc::Receiver<()>>,
}

/// How long dropping the watcher waits for its thread to notice the stop request.
const WATCHER_STOP_TIMEOUT: Duration = Duration::from_millis(500);

impl AvailabilityWatcher {
    pub fn start(on_change: Arc<dyn Fn() + Send + Sync>) -> Self {
        let inert = Self { stop: None, done: None };
        let install = install_dir();
        let (Some(parent), Some(filter)) = (install.parent().map(Path::to_path_buf), install.file_name()) else {
            return inert;
        };
        if !parent.is_dir() {
            return inert;
        }
        let filter = filter.to_string_lossy().into_owned();
        let stop = match OwnedEvent::new(true) {
            Ok(event) => Arc::new(event),
            Err(error) => {
                log::warn!("EqualizerAPOMonitor.TryStartDirWatcher: {error}");
                return inert;
            }
        };
        let thread_stop = Arc::clone(&stop);
        let (done_tx, done) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("vtadn-eqapo-watcher".into()).spawn(move || {
            if let Err(error) = watch_directory(&parent, &filter, &thread_stop, on_change.as_ref()) {
                log::warn!("EqualizerAPOMonitor.TryStartDirWatcher: {error}");
            }
            let _ = done_tx.send(());
        });
        if let Err(error) = spawned {
            log::warn!("EqualizerAPOMonitor.TryStartDirWatcher: {error}");
            return inert;
        }
        Self { stop: Some(stop), done: Some(done) }
    }
}

impl Drop for AvailabilityWatcher {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.set();
        }
        if let Some(done) = self.done.take()
            && done.recv_timeout(WATCHER_STOP_TIMEOUT).is_err()
        {
            log::warn!("EqualizerAPOMonitor.Dispose: watcher thread did not stop in time");
        }
    }
}

/// Directory handle closed on drop.
struct DirHandle(HANDLE);

impl Drop for DirHandle {
    fn drop(&mut self) {
        // SAFETY: opened by CreateFileW below and closed exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn watch_directory(
    parent: &Path,
    filter: &str,
    stop: &OwnedEvent,
    on_change: &(dyn Fn() + Send + Sync),
) -> windows_core::Result<()> {
    let wide_parent = wide(&parent.to_string_lossy());
    // SAFETY: valid NUL-terminated path; the handle is owned by `DirHandle`.
    let directory = DirHandle(unsafe {
        CreateFileW(
            PCWSTR(wide_parent.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            None,
        )?
    });
    let io_event = OwnedEvent::new(true)?;
    // DWORD-aligned notification buffer.
    let mut buffer = vec![0u32; 2048];
    let byte_len = (buffer.len() * size_of::<u32>()) as u32;
    loop {
        let mut overlapped = OVERLAPPED { hEvent: io_event.handle(), ..Default::default() };
        // SAFETY: `buffer` and `overlapped` outlive the I/O: we wait for completion (or cancel and
        // wait) before either is reused or dropped.
        unsafe {
            ReadDirectoryChangesW(
                directory.0,
                buffer.as_mut_ptr().cast(),
                byte_len,
                false,
                FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_FILE_NAME,
                None,
                Some(&mut overlapped),
                None,
            )?;
        }
        let waits = [stop.handle(), io_event.handle()];
        // SAFETY: both handles are live events.
        let result = unsafe { WaitForMultipleObjects(&waits, false, INFINITE) };
        if result.0 != WAIT_OBJECT_0.0 + 1 {
            // Stop requested (or wait failure): cancel the read and wait for it to unwind.
            // SAFETY: cancels our own pending I/O, then waits for its completion so the buffer
            // and OVERLAPPED are no longer referenced.
            unsafe {
                let _ = CancelIoEx(directory.0, Some(&overlapped));
                let mut ignored = 0u32;
                let _ = GetOverlappedResult(directory.0, &overlapped, &mut ignored, true);
            }
            return Ok(());
        }
        let mut transferred = 0u32;
        // SAFETY: the I/O completed (event signalled).
        unsafe { GetOverlappedResult(directory.0, &overlapped, &mut transferred, false)? };
        // A zero-length result means the buffer overflowed: treat it as "may have changed".
        if transferred == 0 || notification_names_match(&buffer, transferred as usize, filter) {
            on_change();
        }
    }
}

/// Walks `FILE_NOTIFY_INFORMATION` records; true when an added/removed/renamed entry's name is
/// `filter` (case-insensitive, like `FileSystemWatcher.Filter`).
fn notification_names_match(buffer: &[u32], len: usize, filter: &str) -> bool {
    let base = buffer.as_ptr().cast::<u8>();
    let mut offset = 0usize;
    let header = std::mem::offset_of!(FILE_NOTIFY_INFORMATION, FileName);
    while offset + header <= len {
        // SAFETY: `offset + header <= len <= buffer bytes`; records are DWORD-aligned.
        let record = unsafe { &*base.add(offset).cast::<FILE_NOTIFY_INFORMATION>() };
        let name_len = record.FileNameLength as usize / 2;
        if offset + header + name_len * 2 > len {
            break;
        }
        // SAFETY: the name lies within the transferred bytes (checked above).
        let name = unsafe { std::slice::from_raw_parts(base.add(offset + header).cast::<u16>(), name_len) };
        let name = String::from_utf16_lossy(name);
        let relevant = matches!(
            record.Action,
            FILE_ACTION_ADDED | FILE_ACTION_REMOVED | FILE_ACTION_RENAMED_OLD_NAME | FILE_ACTION_RENAMED_NEW_NAME
        );
        if relevant && name.eq_ignore_ascii_case(filter) {
            return true;
        }
        if record.NextEntryOffset == 0 {
            break;
        }
        offset += record.NextEntryOffset as usize;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_guid_follows_the_separator() {
        assert_eq!(
            endpoint_guid("{0.0.0.00000000}.{031f2cba-f397-4334-84c2-312cba509f6e}"),
            Some("{031f2cba-f397-4334-84c2-312cba509f6e}")
        );
        assert_eq!(endpoint_guid("synthetic"), None);
        assert_eq!(endpoint_guid(""), None);
    }

    #[test]
    fn apo_guids_format_like_eapo() {
        assert_eq!(guid_braced_lower(&PRE_MIX_GUID), "{eacd2258-fcac-4ff4-b36d-419e924a6d79}");
        assert_eq!(guid_braced_lower(&POST_MIX_GUID), "{ec1cc9ce-faed-4822-828a-82a81a6f018f}");
    }

    #[test]
    fn windows_10_passes_the_win81_gate() {
        assert!(is_windows_version_at_least(6, 3));
    }

    #[test]
    fn notification_filter_matches_only_the_install_folder() {
        fn record(action: u32, name: &str, next: u32) -> Vec<u8> {
            let units: Vec<u16> = name.encode_utf16().collect();
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&next.to_le_bytes());
            bytes.extend_from_slice(&action.to_le_bytes());
            bytes.extend_from_slice(&((units.len() * 2) as u32).to_le_bytes());
            for unit in units {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            while bytes.len() % 4 != 0 {
                bytes.push(0);
            }
            bytes
        }
        let first = record(FILE_ACTION_ADDED.0, "Other", 0);
        let mut both = record(FILE_ACTION_ADDED.0, "Other", first.len() as u32);
        both.extend(record(FILE_ACTION_RENAMED_NEW_NAME.0, "equalizerapo", 0));
        let to_words = |bytes: &[u8]| -> Vec<u32> {
            bytes.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
        };
        assert!(!notification_names_match(&to_words(&first), first.len(), "EqualizerAPO"));
        assert!(notification_names_match(&to_words(&both), both.len(), "EqualizerAPO"));
    }
}
