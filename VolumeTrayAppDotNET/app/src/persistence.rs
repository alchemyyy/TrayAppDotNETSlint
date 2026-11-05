//! `settings.xml` / `devices.xml` stores behind [`SettingsPersistence`] (port of the
//! `AppSettingsCommon` persistence lifecycle and `DeviceSettings` saving).
//!
//! - Implicit saves (`request_save`) happen only while `Autosave` is on, debounced by the store.
//! - Explicit saves (`save_now`, settings window edits) always write.
//! - devices.xml is written at once on every edit, no Autosave gating.
//! - Exit: `SaveOnExit` writes settings only when `Autosave`, then both stores are flushed.

use std::path::Path;

use tadn_common::xml::XmlFileStore;
use vtadn_model::device_settings::DeviceSettings;
use vtadn_model::settings::AppSettings;
use vtadn_model::settings_handle::SettingsPersistence;

pub const SETTINGS_FILE_NAME: &str = "settings.xml";
pub const DEVICES_FILE_NAME: &str = "devices.xml";
pub const THEME_FILE_NAME: &str = "theme.xml";

pub struct Stores {
    settings: XmlFileStore<AppSettings>,
    devices: XmlFileStore<DeviceSettings>,
}

pub struct Loaded {
    pub stores: Stores,
    pub settings: AppSettings,
    pub devices: DeviceSettings,
}

impl Stores {
    /// Loads both files with the store rules (a missing file is created at once by the store); hotkeys are normalized and saved once when that changed them.
    pub fn load(dir: &Path) -> Loaded {
        let (settings_store, mut settings, settings_state) =
            XmlFileStore::<AppSettings>::open(dir.join(SETTINGS_FILE_NAME));
        log::info!("settings.xml: {settings_state:?}");
        settings.clamp_all();
        let normalized = tadn_common::hotkey::normalize(&mut settings.common.hotkeys);
        if normalized && let Err(error) = settings_store.save_now(&settings) {
            log::warn!("settings.xml: initial save failed: {error}");
        }

        // A missing file is created by the store itself (`TrayXmlFileStore` `Missing → SaveNow`).
        let (devices_store, devices, devices_state) = XmlFileStore::<DeviceSettings>::open(dir.join(DEVICES_FILE_NAME));
        log::info!("devices.xml: {devices_state:?}");
        Loaded { stores: Stores { settings: settings_store, devices: devices_store }, settings, devices }
    }

    /// `SaveOnExit` + `Flush` for both stores.
    pub fn save_on_exit(&self, settings: &AppSettings) {
        if settings.common.autosave
            && let Err(error) = self.settings.save_now(settings)
        {
            log::warn!("settings.xml: save on exit failed: {error}");
        }
        self.settings.flush();
        self.devices.flush();
    }
}

impl SettingsPersistence for Stores {
    fn request_save(&self, settings: &AppSettings) {
        if settings.common.autosave {
            self.settings.schedule_save(settings);
        }
    }

    fn save_now(&self, settings: &AppSettings) {
        if let Err(error) = self.settings.save_now(settings) {
            log::warn!("settings.xml: save failed: {error}");
        }
    }

    fn save_devices(&self, devices: &DeviceSettings) {
        if let Err(error) = self.devices.save_now(devices) {
            log::warn!("devices.xml: save failed: {error}");
        }
    }
}
