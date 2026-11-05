//! COM callbacks (C# `NotificationBridge`, `EndpointVolumeBridge`). They run on MTA worker
//! threads, log, and only post messages into the audio thread's channel; the default-change
//! callback additionally raises the synchronous `DefaultDeviceChangedRaw` fan-out first.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use vtadn_model::IDENTITY;
use vtadn_model::snapshot::{DeviceId, Flow};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, EDataFlow, ERole, IMMNotificationClient, IMMNotificationClient_Impl,
    eCapture, eRender,
};
use windows_core::{PCWSTR, implement};

use crate::policy_config::{DefaultChange, DefaultChangeHub};
use crate::props::classify_property;
use crate::service::Msg;

pub fn flow_from(flow: EDataFlow) -> Option<Flow> {
    if flow == eRender {
        Some(Flow::Render)
    } else if flow == eCapture {
        Some(Flow::Capture)
    } else {
        None
    }
}

fn text(value: &PCWSTR) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: a non-null callback string is a NUL-terminated UTF-16 string valid for the call.
    unsafe { value.to_string() }.ok()
}

#[implement(IMMNotificationClient)]
pub struct NotificationClient {
    tx: Sender<Msg>,
    hub: Arc<DefaultChangeHub>,
}

impl NotificationClient {
    pub fn create(tx: Sender<Msg>, hub: Arc<DefaultChangeHub>) -> IMMNotificationClient {
        Self { tx, hub }.into()
    }

    /// `PostIfAlive`: a closed channel means the service is gone.
    fn post(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }
}

impl IMMNotificationClient_Impl for NotificationClient_Impl {
    fn OnDeviceStateChanged(&self, device_id: &PCWSTR, new_state: DEVICE_STATE) -> windows_core::Result<()> {
        if let Some(id) = text(device_id) {
            log::info!("AudioDeviceManager.OnDeviceStateChanged: id={id} newState=0x{:X}", new_state.0);
            self.post(Msg::DeviceStateChanged { id, state: new_state.0 });
        }
        Ok(())
    }

    fn OnDeviceAdded(&self, device_id: &PCWSTR) -> windows_core::Result<()> {
        if let Some(id) = text(device_id) {
            log::info!("AudioDeviceManager.OnDeviceAdded: id={id}");
            self.post(Msg::DeviceAdded(id));
        }
        Ok(())
    }

    fn OnDeviceRemoved(&self, device_id: &PCWSTR) -> windows_core::Result<()> {
        if let Some(id) = text(device_id) {
            log::info!("AudioDeviceManager.OnDeviceRemoved: id={id}");
            self.post(Msg::DeviceRemoved(id));
        }
        Ok(())
    }

    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, default_id: &PCWSTR) -> windows_core::Result<()> {
        let id = text(default_id);
        log::info!(
            "AudioDeviceManager.OnDefaultDeviceChanged: flow={} role={} id={}",
            flow.0,
            role.0,
            id.as_deref().unwrap_or("<null>")
        );
        if let Some(flow) = flow_from(flow) {
            let change = DefaultChange { flow, role, id };
            // Synchronous pass-through before the audio-thread hop (ForceCycleEndpoint waits on it).
            self.hub.raise(&change);
            self.post(Msg::DefaultChanged(change));
        }
        Ok(())
    }

    fn OnPropertyValueChanged(&self, device_id: &PCWSTR, key: &PROPERTYKEY) -> windows_core::Result<()> {
        if let Some(property) = classify_property(key)
            && let Some(id) = text(device_id)
        {
            self.post(Msg::PropertyChanged { id, property });
        }
        Ok(())
    }
}

/// Endpoint volume/mute callback. The payload is ignored (a stale payload could poison the cache
/// with 0); a foreign change only asks the audio thread to re-read the endpoint. `pending`
/// collapses bursts into one re-read.
#[implement(IAudioEndpointVolumeCallback)]
pub struct VolumeCallback {
    device: DeviceId,
    tx: Sender<Msg>,
    pending: Arc<AtomicBool>,
}

impl VolumeCallback {
    pub fn create(device: DeviceId, tx: Sender<Msg>, pending: Arc<AtomicBool>) -> IAudioEndpointVolumeCallback {
        Self { device, tx, pending }.into()
    }
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, notify: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows_core::Result<()> {
        if notify.is_null() {
            return Ok(());
        }
        // SAFETY: the engine passes a valid notification block for the duration of the call; the
        // field is read unaligned-safe because the struct is caller-allocated.
        let context = unsafe { std::ptr::addr_of!((*notify).guidEventContext).read_unaligned() };
        // Echo of our own write.
        if context == IDENTITY.app_guid {
            return Ok(());
        }
        if !self.pending.swap(true, Ordering::AcqRel) {
            let _ = self.tx.send(Msg::EndpointVolume(self.device.clone()));
        }
        Ok(())
    }
}
