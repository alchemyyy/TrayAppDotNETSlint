//! VolumeTrayAppDotNET shared model: the contract between the audio service (`vtadn_audio`), the
//! flyout (`vtadn_flyout`), the settings window (`vtadn_settings`) and the app (`app`).
//!
//! - [`snapshot`]: immutable audio model the service publishes.
//! - [`command`]: what the UI asks the service to do.
//! - [`bus`]: snapshot/event delivery with UI wake coalescing; [`meters`]: shared peak samples.
//! - [`settings`], [`device_settings`], [`settings_handle`], [`config`]: settings model, the
//!   UI-thread owner with change notification, and the service's view of it.
//! - [`policy`]: pure ordering/visibility/scale/meter/ding rules.
//! - [`links`], [`text`]: shell links and user-visible strings.

pub mod bus;
pub mod command;
pub mod config;
pub mod device_settings;
pub mod links;
pub mod meters;
pub mod policy;
pub mod settings;
pub mod settings_handle;
mod settings_xml;
pub mod snapshot;
pub mod text;

use tadn_common::identity::AppIdentity;
use windows_core::GUID;

/// VolumeTrayAppDotNET (Slint build) identity. The tray and event-context GUIDs differ from the
/// .NET app's `1ac1ef49-bb6a-4a21-8480-24766db1f35e` so both can run side by side.
pub const IDENTITY: AppIdentity = AppIdentity {
    app_name: "VolumeTrayAppDotNET",
    instance_name: "VolumeTrayAppDotNETSlint",
    tray_icon_guid: GUID::from_u128(0xe7c5d03c_9c5f_4f28_999d_f91ec7897b7a),
    app_guid: GUID::from_u128(0xb8c65b82_deb2_4a57_895a_995145e8a4a1),
};
