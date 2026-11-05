//! Process-wide identity constants (port of `TrayAppDotNETConstants` + each app's `Constants`).
//!
//! The Slint builds run side by side with the .NET apps, so every machine-global name (data root,
//! mutexes, events, startup shortcut, tray GUID, window classes) differs from the .NET build while
//! the exe name and the on-disk file formats stay the same.

use windows_core::GUID;

/// Folder under `%LOCALAPPDATA%` holding every Slint app's data folder.
pub const DATA_ROOT_FOLDER: &str = "TrayAppDotNETSlint";
/// Folder under `%LOCALAPPDATA%` used by the .NET apps; first-run import reads from here.
pub const LEGACY_DATA_ROOT_FOLDER: &str = "TrayAppDotNET";

pub const PUBLISHER: &str = "alchemyyy";
pub const REPOSITORY_URL: &str = "https://github.com/alchemyyy/TrayAppDotNET";
pub const WEBSITE_URL: &str = "https://trayapp.net";

/// Window class of the primary taskbar.
pub const TASKBAR_WINDOW_CLASS: &str = "Shell_TrayWnd";

#[derive(Clone, Copy, Debug)]
pub struct AppIdentity {
    /// Exe name and per-app data folder name, e.g. `VolumeTrayAppDotNET`.
    pub app_name: &'static str,
    /// Prefix of every machine-global name of this build (mutex, exit event, startup shortcut,
    /// window classes), e.g. `VolumeTrayAppDotNETSlint`.
    pub instance_name: &'static str,
    /// `NOTIFYICONDATAW::guidItem`. Must differ from the .NET app's GUID: the shell binds a GUID
    /// to one exe path.
    pub tray_icon_guid: GUID,
    /// Event-context GUID passed to Core Audio writes so the app recognises its own echoes.
    pub app_guid: GUID,
}

impl AppIdentity {
    /// Mutex held for the lifetime of the running instance.
    pub fn instance_mutex_name(&self) -> String {
        format!("Local\\{}-Instance-{:?}", self.instance_name, self.app_guid)
    }

    /// Named event a newer instance signals to ask the process `pid` to exit.
    pub fn exit_event_name(&self, pid: u32) -> String {
        format!("Local\\{}-Exit-{pid}", self.instance_name)
    }

    /// File name of the Startup-folder shortcut, e.g. `VolumeTrayAppDotNETSlint.lnk`.
    pub fn startup_shortcut_file_name(&self) -> String {
        format!("{}.lnk", self.instance_name)
    }
}
