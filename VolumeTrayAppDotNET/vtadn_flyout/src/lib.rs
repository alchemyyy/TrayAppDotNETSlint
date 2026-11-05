//! The volume flyout (port of `UI/Flyout/*` and the Common flyout infrastructure it uses:
//! `FlyoutWindowCommon`, `FlyoutTrayPlacement`, `FlyoutDockingController`,
//! `FlyoutChromeDragController`, `FlyoutUndockButtonController`, `FlyoutRebuildScheduler`,
//! `FlyoutSlider`, `WheelStepAccumulator`, `SettingsFlyoutKeepOpenCoordinator` (flyout side)).
//!
//! Threading: everything here runs on the Slint UI thread. Audio snapshots arrive through
//! [`Flyout::on_audio_update`]; commands go out through `AudioClient::send`; meters are read from the
//! shared `MeterBoard` by a render timer that only runs while the flyout is visible.

mod controller;
pub mod docking;
mod echo;
pub mod layout;
pub mod metrics;
pub mod placement;
pub mod present;
pub mod rebuild;
pub mod slider;
pub mod view;

use std::rc::Rc;
use std::sync::Arc;

use tadn_common::theme::Palette;
use tadn_menu::ContextMenu;
use tadn_shell::foreground::TaskbarForegroundTracker;
use tadn_shell::placement::Rect;
use vtadn_model::bus::AudioClient;
use vtadn_model::settings_handle::SettingsHandle;
use vtadn_model::snapshot::AudioSnapshot;
use windows::Win32::Foundation::HWND;

/// Slint-generated components and globals.
pub mod ui {
    slint::include_modules!();
}

/// What the flyout needs from the app.
pub trait FlyoutHost {
    /// Header gear button.
    fn open_settings(&self);
    /// `Shell_NotifyIconGetRect` of the tray icon (docked placement anchor).
    fn tray_icon_rect(&self) -> Option<Rect>;
    /// Current palette (also pushed through [`Flyout::apply_palette`]).
    fn palette(&self) -> Palette;
    /// The settings window while it is open: while the flyout is held open for it, a deactivation
    /// towards the settings window (or a window it owns) keeps the flyout open.
    fn settings_hwnd(&self) -> Option<HWND> {
        None
    }
}

pub struct FlyoutDeps {
    pub settings: SettingsHandle,
    pub audio: AudioClient,
    /// Shared menu window for the sample-rate and listen-target menus.
    pub menu: Rc<ContextMenu>,
    pub foreground: Rc<TaskbarForegroundTracker>,
    pub host: Rc<dyn FlyoutHost>,
}

pub struct Flyout {
    inner: Rc<controller::Inner>,
}

impl Flyout {
    /// Creates the controller (and, with `KeepFlyoutWarm`, creates the window ahead of the first show).
    pub fn new(deps: FlyoutDeps) -> Result<Self, slint::PlatformError> {
        Ok(Self { inner: controller::Inner::new(deps)? })
    }

    /// Newest snapshot from the audio bus (structure_rev change → coalesced rebuild, held while the
    /// user interacts; otherwise in-place value updates).
    pub fn on_audio_update(&self, snapshot: &Arc<AudioSnapshot>) {
        self.inner.on_audio_update(snapshot);
    }

    pub fn apply_palette(&self, palette: &Palette) {
        self.inner.apply_palette(palette);
    }

    /// Tray left click (after modifier actions): `TrayWindowToggle` decision → show docked, hide,
    /// or activate in place.
    pub fn toggle_from_tray(&self) {
        self.inner.toggle_from_tray();
    }

    /// Redock and show (tray Enter/Space, hotkey, double-click with action Nothing).
    pub fn show_docked(&self, activate: bool) {
        self.inner.show_docked(activate);
    }

    pub fn hide(&self) {
        self.inner.hide();
    }

    pub fn is_visible(&self) -> bool {
        self.inner.is_visible()
    }

    /// The flyout window is the foreground window.
    pub fn is_active(&self) -> bool {
        self.inner.is_active()
    }

    /// At startup: show undocked at the saved position when `RestoreFlyoutUndockedOnStartup`,
    /// `AllowFlyoutUndock`, `FlyoutUndocked` and `FlyoutHasSavedPosition` all hold.
    pub fn restore_on_startup(&self) {
        self.inner.restore_on_startup();
    }

    /// Keep-open coordination with the settings window: while held, deactivation does not hide.
    pub fn set_hold_open(&self, hold: bool) {
        self.inner.set_hold_open(hold);
    }

    /// `HideFromCoordinator`: releases the hold, then hides unless the one-shot suppression is armed
    /// or the flyout cannot auto-hide (undocked, menu or dialog open).
    pub fn hide_from_coordinator(&self) {
        self.inner.hide_from_coordinator();
    }

    pub fn hwnd(&self) -> Option<HWND> {
        self.inner.hwnd()
    }
}
