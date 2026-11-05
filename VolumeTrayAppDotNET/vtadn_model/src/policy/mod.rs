//! Pure policies shared by the audio service, flyout, tray menu and tests (ports of
//! `DeviceVisibility`, `FlyoutDeviceOrdering`, `AppDrawerVisibilityPolicy`, `AppVolumeScale`,
//! `MeterReader`/`MeterLerp`, `DingSuppressionPeak`). No COM, no UI, fully unit-tested.

pub mod ding;
pub mod drawer;
pub mod meter;
pub mod ordering;
pub mod scale;
pub mod visibility;
