//! UI-thread owner of the live [`AppSettings`] and [`DeviceSettings`] with change notification and
//! the persistence rules of `AppSettingsCommon` (`SetField` / `SetState` / explicit `Save`) and
//! `DeviceSettings` (immediate save).

use std::cell::{Cell, Ref, RefCell};
use std::rc::{Rc, Weak};

use crate::device_settings::DeviceSettings;
use crate::settings::AppSettings;

/// Where saves go (implemented by the app over `tadn_common::xml::XmlFileStore`).
pub trait SettingsPersistence {
    /// Implicit save: skipped when `Autosave` is off; debounced.
    fn request_save(&self, settings: &AppSettings);
    /// Explicit save (settings window edits): always written.
    fn save_now(&self, settings: &AppSettings);
    /// devices.xml: always written at once.
    fn save_devices(&self, devices: &DeviceSettings);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsChange {
    /// `settings.xml` values changed (never raised for runtime state).
    Settings,
    /// `devices.xml` changed.
    Devices,
}

type Observer = Rc<dyn Fn(SettingsChange)>;

struct Inner {
    settings: RefCell<AppSettings>,
    devices: RefCell<DeviceSettings>,
    observers: RefCell<Vec<(u64, Observer)>>,
    next_id: Cell<u64>,
    persistence: RefCell<Option<Rc<dyn SettingsPersistence>>>,
}

/// Cheap to clone; UI thread only.
#[derive(Clone)]
pub struct SettingsHandle {
    inner: Rc<Inner>,
}

/// Unsubscribes when dropped.
pub struct Subscription {
    inner: Weak<Inner>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.observers.borrow_mut().retain(|(id, _)| *id != self.id);
        }
    }
}

impl SettingsHandle {
    pub fn new(mut settings: AppSettings, devices: DeviceSettings) -> Self {
        settings.clamp_all();
        Self {
            inner: Rc::new(Inner {
                settings: RefCell::new(settings),
                devices: RefCell::new(devices),
                observers: RefCell::new(Vec::new()),
                next_id: Cell::new(1),
                persistence: RefCell::new(None),
            }),
        }
    }

    pub fn set_persistence(&self, persistence: Rc<dyn SettingsPersistence>) {
        *self.inner.persistence.borrow_mut() = Some(persistence);
    }

    pub fn get(&self) -> Ref<'_, AppSettings> {
        self.inner.settings.borrow()
    }

    pub fn devices(&self) -> Ref<'_, DeviceSettings> {
        self.inner.devices.borrow()
    }

    /// A user change made outside the settings window (flyout eye button, ...): clamp, notify when
    /// anything changed, implicit save.
    pub fn update(&self, f: impl FnOnce(&mut AppSettings)) {
        if self.apply(f) {
            if let Some(p) = self.persistence() {
                p.request_save(&self.inner.settings.borrow());
            }
            self.notify(SettingsChange::Settings);
        }
    }

    /// A change made in the settings window: clamp, notify when anything changed, explicit save.
    pub fn update_explicit(&self, f: impl FnOnce(&mut AppSettings)) {
        if self.apply(f) {
            if let Some(p) = self.persistence() {
                p.save_now(&self.inner.settings.borrow());
            }
            self.notify(SettingsChange::Settings);
        }
    }

    /// Transient live preview (the color picker's `Temporary*Color`): clamp and notify when
    /// anything changed, never save. The caller ends the preview with `update_explicit` (commit)
    /// or another `preview` back to the original value.
    pub fn preview(&self, f: impl FnOnce(&mut AppSettings)) {
        if self.apply(f) {
            self.notify(SettingsChange::Settings);
        }
    }

    /// Runtime state (`SetState`: dock position, last-known defaults): implicit save, no
    /// notification.
    pub fn set_state(&self, f: impl FnOnce(&mut AppSettings)) {
        if self.apply(f)
            && let Some(p) = self.persistence()
        {
            p.request_save(&self.inner.settings.borrow());
        }
    }

    /// devices.xml edit: immediate save and a `Devices` notification when anything changed.
    pub fn update_devices(&self, f: impl FnOnce(&mut DeviceSettings)) {
        let changed = {
            let mut devices = self.inner.devices.borrow_mut();
            let before = devices.clone();
            f(&mut devices);
            *devices != before
        };
        if changed {
            if let Some(p) = self.persistence() {
                p.save_devices(&self.inner.devices.borrow());
            }
            self.notify(SettingsChange::Devices);
        }
    }

    /// Registers an observer; it runs after each change with no borrows held, so it may read the
    /// settings (and even update them).
    pub fn subscribe(&self, f: impl Fn(SettingsChange) + 'static) -> Subscription {
        let id = self.inner.next_id.get();
        self.inner.next_id.set(id + 1);
        self.inner.observers.borrow_mut().push((id, Rc::new(f)));
        Subscription { inner: Rc::downgrade(&self.inner), id }
    }

    fn apply(&self, f: impl FnOnce(&mut AppSettings)) -> bool {
        let mut settings = self.inner.settings.borrow_mut();
        let before = settings.clone();
        f(&mut settings);
        settings.clamp_all();
        *settings != before
    }

    fn persistence(&self) -> Option<Rc<dyn SettingsPersistence>> {
        self.inner.persistence.borrow().clone()
    }

    fn notify(&self, change: SettingsChange) {
        let observers: Vec<Observer> = self.inner.observers.borrow().iter().map(|(_, o)| o.clone()).collect();
        for observer in observers {
            observer(change);
        }
    }
}
