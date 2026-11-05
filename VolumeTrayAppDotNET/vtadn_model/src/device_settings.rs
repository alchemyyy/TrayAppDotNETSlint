//! Per-device persisted state, `devices.xml` (port of `Models/DeviceSettings.cs`):
//! `<Devices><Device Id="..." IsAppDrawerExpanded="true" CustomFriendlyName="" /></Devices>`.
//! Saved immediately on every edit (no Autosave gating, no debounce).

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceEntry {
    /// IMMDevice id, matched ordinally.
    pub id: String,
    pub is_app_drawer_expanded: bool,
    /// Bluetooth-only custom name; empty = none.
    pub custom_friendly_name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceSettings {
    pub devices: Vec<DeviceEntry>,
}

pub const DEVICES_ROOT_ELEMENT: &str = "Devices";

impl DeviceSettings {
    pub fn find(&self, id: &str) -> Option<&DeviceEntry> {
        self.devices.iter().find(|d| d.id == id)
    }

    /// Adds a row on first edit. A new row starts with `default_expanded` and no custom name.
    pub fn get_or_create(&mut self, id: &str, default_expanded: bool) -> &mut DeviceEntry {
        if let Some(index) = self.devices.iter().position(|d| d.id == id) {
            return &mut self.devices[index];
        }
        self.devices.push(DeviceEntry {
            id: id.to_owned(),
            is_app_drawer_expanded: default_expanded,
            custom_friendly_name: String::new(),
        });
        self.devices.last_mut().expect("just pushed")
    }

    /// The drawer state for `id`: the per-device override, else `default_expanded`.
    pub fn is_drawer_expanded(&self, id: &str, default_expanded: bool) -> bool {
        self.find(id).map_or(default_expanded, |d| d.is_app_drawer_expanded)
    }

    /// Non-blank custom name for `id`.
    pub fn custom_name(&self, id: &str) -> Option<&str> {
        self.find(id).map(|d| d.custom_friendly_name.as_str()).filter(|n| !n.trim().is_empty())
    }
}
