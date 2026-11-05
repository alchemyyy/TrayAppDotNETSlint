//! Resume-from-sleep notification (C# `SystemEvents.PowerModeChanged` → `PowerModes.Resume`),
//! registered as a callback so the audio thread needs no window.

use std::ffi::c_void;
use std::sync::mpsc::Sender;

use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY, PowerRegisterSuspendResumeNotification,
    PowerUnregisterSuspendResumeNotification,
};
use windows::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND};

use crate::service::Msg;

/// Keeps the registration and its callback context alive; unregisters on drop.
pub struct PowerRegistration {
    handle: HPOWERNOTIFY,
    // Boxed so the addresses handed to the OS stay stable until unregistration.
    _params: Box<DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS>,
    _context: Box<Sender<Msg>>,
}

unsafe extern "system" fn on_power(context: *const c_void, kind: u32, _setting: *const c_void) -> u32 {
    if kind == PBT_APMRESUMEAUTOMATIC || kind == PBT_APMRESUMESUSPEND {
        log::info!("AudioDeviceManager.PowerModeChanged: resume");
        // SAFETY: `context` is the boxed Sender owned by the live `PowerRegistration`; the OS stops
        // calling before `PowerUnregisterSuspendResumeNotification` returns in `drop`.
        let tx = unsafe { &*context.cast::<Sender<Msg>>() };
        let _ = tx.send(Msg::Resumed);
    }
    ERROR_SUCCESS.0
}

impl PowerRegistration {
    pub fn register(tx: Sender<Msg>) -> Option<Self> {
        let context = Box::new(tx);
        let params = Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(on_power),
            Context: std::ptr::from_ref::<Sender<Msg>>(&context).cast_mut().cast(),
        });
        let mut handle: *mut c_void = std::ptr::null_mut();
        // SAFETY: with DEVICE_NOTIFY_CALLBACK the recipient is a pointer to the subscribe
        // parameters, which (and their context) stay alive in the returned registration.
        let status = unsafe {
            PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                HANDLE(std::ptr::from_ref::<DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS>(&params).cast_mut().cast()),
                &mut handle,
            )
        };
        if status != ERROR_SUCCESS {
            log::warn!("PowerRegisterSuspendResumeNotification failed: {}", status.0);
            return None;
        }
        Some(Self { handle: HPOWERNOTIFY(handle as isize), _params: params, _context: context })
    }
}

impl Drop for PowerRegistration {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful registration and is unregistered once.
        let status = unsafe { PowerUnregisterSuspendResumeNotification(self.handle) };
        if status != ERROR_SUCCESS {
            log::warn!("PowerUnregisterSuspendResumeNotification failed: {}", status.0);
        }
    }
}
