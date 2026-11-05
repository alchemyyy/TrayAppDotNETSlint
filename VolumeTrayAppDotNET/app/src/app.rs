//! The running app (port of `App.cs` `VolumeAvaloniaApp` + the tray-mode parts of
//! `TrayAppDotNETAppHost`): every subscription, tray handler, palette refresh, keep-open pairing
//! and the once-only shutdown. UI thread only.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use tadn_common::hotkey::{HotkeyAction, HotkeyBinding};
use tadn_common::paths::DataDirs;
use tadn_common::settings::ContextMenuPosition;
use tadn_common::theme::{Palette, ThemeFile};
use tadn_menu::{ContextMenu, MenuCloseReason, MenuOptions, MenuPlacement};
use tadn_shell::foreground::TaskbarForegroundTracker;
use tadn_shell::hotkeys::{HotkeyRegistration, HotkeyService};
use tadn_shell::placement::Rect;
use tadn_shell::system::{SystemEvent, SystemEvents};
use tadn_shell::tray::{Modifiers, TrayEvent, TrayIcon, TrayIconConfig, TrayInputOptions};
use vtadn_audio::AudioService;
use vtadn_flyout::{Flyout, FlyoutDeps, FlyoutHost};
use vtadn_model::IDENTITY;
use vtadn_model::bus::{AudioClient, AudioEvent};
use vtadn_model::command::{AudioCommand, FeedbackTiming};
use vtadn_model::config::AudioBackendConfig;
use vtadn_model::links;
use vtadn_model::settings::{AppSettings, TrayClickAction};
use vtadn_model::settings_handle::{SettingsChange, SettingsHandle, Subscription};
use vtadn_model::snapshot::{AudioSnapshot, DeviceId};
use vtadn_model::text;
use vtadn_settings::{SettingsHost, SettingsWindow};
use windows::Win32::Foundation::HWND;

use crate::persistence::Stores;
use crate::tray::{self, TrayMenuAction, TrayRenderInput};

/// Exit code when Windows ends the session (`OnLifetimeExit`).
pub const SESSION_END_EXIT_CODE: i32 = 3;
/// `TimeConstants.WarmWindowIdleEvictionDelayMs`.
pub const WARM_WINDOW_IDLE_EVICTION_DELAY: Duration = Duration::from_millis(10_000);

thread_local! {
    /// The running app, for UI-thread hops that start on other threads (`invoke_from_event_loop`
    /// closures must be `Send`, so they look the app up here instead of capturing it).
    static CURRENT: RefCell<Weak<App>> = const { RefCell::new(Weak::new()) };
}

/// Runs `f` with the running app (UI thread only; no-op once it is gone).
pub fn with_current(f: impl FnOnce(&Rc<App>)) {
    if let Some(app) = CURRENT.with(|c| c.borrow().upgrade()) {
        f(&app);
    }
}

/// What `run()` does after the event loop returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitAction {
    Exit(i32),
    /// Re-launch the exe with these arguments after everything is released.
    Restart(Vec<std::ffi::OsString>),
}

pub struct StartupState {
    pub dirs: &'static DataDirs,
    pub settings: SettingsHandle,
    pub stores: Rc<Stores>,
    pub theme: ThemeFile,
}

/// Optimistic device volume after a tray scroll, until the next published snapshot.
#[derive(Clone)]
struct PendingVolume {
    device: DeviceId,
    volume: f32,
    rev: u64,
}

pub struct App {
    dirs: &'static DataDirs,
    settings: SettingsHandle,
    stores: Rc<Stores>,
    theme: ThemeFile,
    palette: RefCell<Palette>,
    audio_service: RefCell<Option<AudioService>>,
    audio: AudioClient,
    snapshot: RefCell<Arc<AudioSnapshot>>,
    /// Shared by the flyout's format/listen menus.
    menu: Rc<ContextMenu>,
    /// The tray menu's warm slot (`TrayAppDotNETWarmWindowSlot`): created on demand, dropped
    /// `WARM_WINDOW_IDLE_EVICTION_DELAY` after a dismiss while `KeepTrayContextMenuWarm` is off.
    tray_menu: RefCell<Option<Rc<ContextMenu>>>,
    tray_menu_eviction: slint::Timer,
    flyout: RefCell<Option<Flyout>>,
    settings_window: RefCell<Option<SettingsWindow>>,
    tray: RefCell<Option<TrayIcon>>,
    hotkeys: RefCell<Option<HotkeyService>>,
    /// Bindings passed to the last `register_all` (the hotkey handler indexes into them) and its
    /// results.
    registered: RefCell<(Vec<HotkeyBinding>, Vec<HotkeyRegistration>)>,
    system_events: RefCell<Option<SystemEvents>>,
    subscription: RefCell<Option<Subscription>>,
    last_icon: Cell<Option<TrayRenderInput>>,
    last_config: RefCell<Option<AudioBackendConfig>>,
    last_input_options: Cell<Option<TrayInputOptions>>,
    pending_volume: RefCell<Option<PendingVolume>>,
    recovery_requested_rev: Cell<Option<u64>>,
    /// `SettingsFlyoutKeepOpenCoordinator`: the flyout is paired with the settings window.
    keep_open_paired: Cell<bool>,
    shutting_down: Cell<bool>,
    exit_action: RefCell<ExitAction>,
}

/// The app's face towards the flyout and the settings window (weak: no reference cycle).
struct Bridge(Weak<App>);

impl App {
    /// Composition root (`OnFrameworkInitializationCompleted` steps 5–12). The Slint backend is
    /// initialized by the caller.
    pub fn start(state: StartupState) -> Result<Rc<Self>, String> {
        let StartupState { dirs, settings, stores, theme } = state;
        let palette = tadn_common::theme::resolve_palette(&theme, &settings.get().common);

        let config = AudioBackendConfig::new(&settings.get(), &settings.devices());
        let (audio_service, audio) = AudioService::start(config.clone(), || {
            if let Err(error) = slint::invoke_from_event_loop(|| with_current(|app| app.on_audio_wake())) {
                log::warn!("audio wake: {error}");
            }
        });

        let menu = Rc::new(ContextMenu::new().map_err(|e| format!("menu window: {e}"))?);
        menu.apply_palette(&palette);
        let foreground = Rc::new(TaskbarForegroundTracker::install());

        let app = Rc::new(Self {
            dirs,
            settings: settings.clone(),
            stores,
            theme,
            palette: RefCell::new(palette),
            audio_service: RefCell::new(Some(audio_service)),
            audio: audio.clone(),
            snapshot: RefCell::new(audio.latest()),
            menu: menu.clone(),
            tray_menu: RefCell::new(None),
            tray_menu_eviction: slint::Timer::default(),
            flyout: RefCell::new(None),
            settings_window: RefCell::new(None),
            tray: RefCell::new(None),
            hotkeys: RefCell::new(None),
            registered: RefCell::new((Vec::new(), Vec::new())),
            system_events: RefCell::new(None),
            subscription: RefCell::new(None),
            last_icon: Cell::new(None),
            last_config: RefCell::new(Some(config)),
            last_input_options: Cell::new(None),
            pending_volume: RefCell::new(None),
            recovery_requested_rev: Cell::new(None),
            keep_open_paired: Cell::new(false),
            shutting_down: Cell::new(false),
            exit_action: RefCell::new(ExitAction::Exit(0)),
        });
        CURRENT.with(|c| *c.borrow_mut() = Rc::downgrade(&app));

        // Settings changes: palette, tray input, hotkeys, audio configuration, tray refresh.
        let weak = Rc::downgrade(&app);
        *app.subscription.borrow_mut() = Some(settings.subscribe(move |change| {
            if let Some(app) = weak.upgrade() {
                app.on_settings_changed(change);
            }
        }));

        // Flyout (primes itself off-screen with KeepFlyoutWarm).
        let flyout = Flyout::new(FlyoutDeps {
            settings: settings.clone(),
            audio,
            menu,
            foreground,
            host: Rc::new(Bridge(Rc::downgrade(&app))),
        })
        .map_err(|e| format!("flyout: {e}"))?;
        flyout.apply_palette(&app.palette.borrow());
        *app.flyout.borrow_mut() = Some(flyout);

        // Hotkeys (`StartHotkeyService`): a failure leaves the app without hotkeys.
        let weak = Rc::downgrade(&app);
        match HotkeyService::new(move |index| {
            if let Some(app) = weak.upgrade() {
                app.on_hotkey(index);
            }
        }) {
            Ok(service) => *app.hotkeys.borrow_mut() = Some(service),
            Err(error) => log::error!("hotkey service: {error}"),
        }
        app.apply_hotkeys(false);

        // Tray icon (`CreateTrayIcon`).
        let input = app.tray_input_options();
        let weak = Rc::downgrade(&app);
        let tray = TrayIcon::new(
            TrayIconConfig { guid: IDENTITY.tray_icon_guid, class_prefix: IDENTITY.instance_name, input },
            move |event| {
                if let Some(app) = weak.upgrade() {
                    app.on_tray_event(event);
                }
            },
        )
        .map_err(|e| format!("tray icon: {e}"))?;
        app.last_input_options.set(Some(input));
        *app.tray.borrow_mut() = Some(tray);

        let weak = Rc::downgrade(&app);
        match SystemEvents::new(move |event| {
            if let Some(app) = weak.upgrade() {
                app.on_system_event(event);
            }
        }) {
            Ok(events) => *app.system_events.borrow_mut() = Some(events),
            Err(error) => log::error!("system events: {error}"),
        }

        app.refresh_tray(false);
        if let Some(flyout) = app.flyout.borrow().as_ref() {
            flyout.restore_on_startup();
        }
        log::info!("startup complete");
        Ok(app)
    }

    pub fn exit_action(&self) -> ExitAction {
        self.exit_action.borrow().clone()
    }

    // ---- audio -------------------------------------------------------------------------------

    fn on_audio_wake(self: &Rc<Self>) {
        if self.shutting_down.get() {
            return;
        }
        let (snapshot, events) = self.audio.take_update();
        for event in events {
            self.on_audio_event(event);
        }
        let changed = !Arc::ptr_eq(&snapshot, &self.snapshot.borrow());
        if !changed {
            return;
        }
        *self.snapshot.borrow_mut() = snapshot.clone();
        // A newer snapshot carries the service's view of any volume we just wrote.
        let stale = self.pending_volume.borrow().as_ref().is_some_and(|p| p.rev != snapshot.rev);
        if stale {
            *self.pending_volume.borrow_mut() = None;
        }
        if let Some(flyout) = self.flyout.borrow().as_ref() {
            flyout.on_audio_update(&snapshot);
        }
        self.refresh_tray(false);
    }

    fn on_audio_event(&self, event: AudioEvent) {
        match event {
            AudioEvent::LastKnownDefaults { playback, comms_playback, recording, comms_recording } => {
                self.settings.set_state(|s| {
                    s.last_known_default_playback_device_id = playback;
                    s.last_known_default_comms_playback_device_id = comms_playback;
                    s.last_known_default_recording_device_id = recording;
                    s.last_known_default_comms_recording_device_id = comms_recording;
                });
            }
            AudioEvent::CustomNameChanged { id, name } => {
                let default_expanded = self.settings.get().default_app_drawer_expanded;
                let name = name.filter(|n| !n.trim().is_empty());
                self.settings.update_devices(|devices| match name {
                    Some(name) => devices.get_or_create(&id, default_expanded).custom_friendly_name = name,
                    None => {
                        if let Some(entry) = devices.devices.iter_mut().find(|d| *d.id == *id) {
                            entry.custom_friendly_name.clear();
                        }
                    }
                });
            }
        }
    }

    /// Sends `Configure` when the settings-derived configuration changed.
    fn configure_audio(&self) {
        let config = AudioBackendConfig::new(&self.settings.get(), &self.settings.devices());
        if self.last_config.borrow().as_ref() == Some(&config) {
            return;
        }
        *self.last_config.borrow_mut() = Some(config.clone());
        self.audio.send(AudioCommand::Configure(config));
    }

    // ---- settings / palette ------------------------------------------------------------------

    fn on_settings_changed(self: &Rc<Self>, change: SettingsChange) {
        if self.shutting_down.get() {
            return;
        }
        match change {
            SettingsChange::Settings => {
                self.refresh_palette();
                let input = self.tray_input_options();
                if self.last_input_options.get() != Some(input) {
                    self.last_input_options.set(Some(input));
                    if let Some(tray) = self.tray.borrow().as_ref() {
                        tray.set_input_options(input);
                    }
                }
                self.apply_hotkeys(false);
                let menu_open = self.tray_menu.borrow().as_ref().is_some_and(|m| m.is_open());
                if !menu_open {
                    self.schedule_tray_menu_eviction();
                }
                self.configure_audio();
                self.refresh_tray(false);
            }
            SettingsChange::Devices => self.configure_audio(),
        }
    }

    /// Recomputes the palette (light/dark, overrides, animations) and pushes it everywhere.
    fn refresh_palette(&self) {
        let palette = tadn_common::theme::resolve_palette(&self.theme, &self.settings.get().common);
        if *self.palette.borrow() == palette {
            return;
        }
        self.menu.apply_palette(&palette);
        if let Some(menu) = self.tray_menu.borrow().as_ref() {
            menu.apply_palette(&palette);
        }
        if let Some(flyout) = self.flyout.borrow().as_ref() {
            flyout.apply_palette(&palette);
        }
        if let Some(window) = self.settings_window.borrow().as_ref() {
            window.apply_palette(&palette);
        }
        *self.palette.borrow_mut() = palette;
    }

    fn tray_input_options(&self) -> TrayInputOptions {
        let s = self.settings.get();
        TrayInputOptions {
            wheel: s.tray_scroll_enabled,
            touchpad: s.precision_touchpad_scroll_enabled,
            touchpad_units_per_step: u32::try_from(s.precision_touchpad_units_per_scroll_step.clamp(1, 1000))
                .unwrap_or(90),
        }
    }

    fn on_system_event(self: &Rc<Self>, event: SystemEvent) {
        if self.shutting_down.get() {
            return;
        }
        match event {
            SystemEvent::ThemeChanged | SystemEvent::ParametersChanged => {
                self.refresh_palette();
                self.refresh_tray(false);
            }
            SystemEvent::DisplayChanged => self.refresh_tray(false),
            SystemEvent::SessionEnding => {
                log::info!("session ending");
                *self.exit_action.borrow_mut() = ExitAction::Exit(SESSION_END_EXIT_CODE);
                self.shutdown();
            }
            SystemEvent::Resumed => {}
        }
    }

    // ---- hotkeys -----------------------------------------------------------------------------

    /// Registers `settings.common.hotkeys` when they differ from the registered set (or `force`).
    fn apply_hotkeys(&self, force: bool) -> Vec<HotkeyRegistration> {
        let bindings = self.settings.get().common.hotkeys.clone();
        {
            let registered = self.registered.borrow();
            if !force && registered.0 == bindings && !registered.1.is_empty() {
                return registered.1.clone();
            }
        }
        let results = match self.hotkeys.borrow().as_ref() {
            Some(service) => service.register_all(&bindings),
            None => {
                bindings.iter().map(|_| HotkeyRegistration::Failed("hotkey service unavailable".to_owned())).collect()
            }
        };
        for (binding, result) in bindings.iter().zip(&results) {
            if let HotkeyRegistration::Failed(reason) = result {
                log::warn!(
                    "Hotkey {:?}({}) #{} not registered: {reason}",
                    binding.action,
                    binding.parameter,
                    binding.binding_id
                );
            }
        }
        *self.registered.borrow_mut() = (bindings, results.clone());
        results
    }

    fn on_hotkey(self: &Rc<Self>, index: usize) {
        if self.shutting_down.get() {
            return;
        }
        let action = self.registered.borrow().0.get(index).map(|b| b.action);
        match action {
            Some(HotkeyAction::OpenSettings) => self.open_settings(),
            Some(HotkeyAction::OpenFlyout) => self.show_volume_flyout(true, true),
            None => log::warn!("hotkey #{index} fired with no binding"),
        }
    }

    // ---- tray --------------------------------------------------------------------------------

    fn on_tray_event(self: &Rc<Self>, event: TrayEvent) {
        if self.shutting_down.get() {
            return;
        }
        match event {
            TrayEvent::LeftClick { modifiers, .. } => {
                let action = {
                    let s = self.settings.get();
                    modifiers.resolve(
                        s.tray_ctrl_left_click_action,
                        s.tray_alt_left_click_action,
                        TrayClickAction::Nothing,
                        TrayClickAction::Nothing,
                    )
                };
                if action == TrayClickAction::OpenSettings {
                    self.open_settings();
                    return;
                }
                if let Some(flyout) = self.flyout.borrow().as_ref() {
                    flyout.toggle_from_tray();
                }
                if self.flyout_visible() {
                    self.hold_open();
                }
            }
            TrayEvent::DoubleClick { modifiers, .. } => {
                let action = {
                    let s = self.settings.get();
                    modifiers.resolve(
                        s.tray_ctrl_double_left_click_action,
                        s.tray_alt_double_left_click_action,
                        s.tray_double_click_action,
                        TrayClickAction::Nothing,
                    )
                };
                if action == TrayClickAction::OpenSettings {
                    self.open_settings();
                } else {
                    self.show_volume_flyout(true, true);
                }
            }
            TrayEvent::KeySelect => self.show_volume_flyout(true, true),
            TrayEvent::RightClick { x, y, modifiers } => {
                let action = {
                    let s = self.settings.get();
                    modifiers.resolve(
                        s.tray_ctrl_right_click_action,
                        s.tray_alt_right_click_action,
                        TrayClickAction::Nothing,
                        TrayClickAction::Nothing,
                    )
                };
                if action == TrayClickAction::OpenSettings {
                    self.open_settings();
                } else {
                    self.show_tray_menu(x, y);
                }
            }
            TrayEvent::Wheel { notches, modifiers } => self.on_tray_wheel(notches, modifiers),
            TrayEvent::TouchpadScroll { steps } => self.adjust_tracked_device_volume(steps, 1),
            TrayEvent::Recreated | TrayEvent::RefreshNeeded => {
                self.last_icon.set(None);
                self.refresh_tray(false);
            }
        }
    }

    fn on_tray_wheel(self: &Rc<Self>, notches: i32, modifiers: Modifiers) {
        let step = tray::wheel_step_percent(&self.settings.get(), modifiers);
        // `Nothing` → no adjustment and no tooltip.
        if let Some(step) = step {
            self.adjust_tracked_device_volume(notches, step);
        }
    }

    /// `AdjustTrackedDeviceVolume`.
    fn adjust_tracked_device_volume(&self, steps: i32, step_percent: i32) {
        let snapshot = self.snapshot.borrow().clone();
        let Some(device) = snapshot.default_render() else { return };
        if steps == 0 {
            return;
        }
        let pending = self.pending_volume.borrow().clone();
        let current =
            pending.filter(|p| p.device == device.id && p.rev == snapshot.rev).map_or(device.volume, |p| p.volume);
        let mut shown = device.clone();
        shown.volume = current;
        if let Some(next) = tray::adjusted_volume(current, steps, step_percent) {
            self.audio.send(AudioCommand::SetDeviceVolume { id: device.id.clone(), volume: next });
            *self.pending_volume.borrow_mut() =
                Some(PendingVolume { device: device.id.clone(), volume: next, rev: snapshot.rev });
            shown.volume = next;
            let ding = {
                let s = self.settings.get();
                s.play_tray_scroll_volume_change_sound && s.play_device_volume_change_sound
            };
            if ding && !device.is_capture() {
                self.audio.send(AudioCommand::PlayDeviceFeedback {
                    id: device.id.clone(),
                    timing: FeedbackTiming::Debounced,
                });
            }
        }
        // Always pop the tooltip, even when clamped at a bound.
        self.push_tray_icon(Some(&shown));
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.set_tooltip(&text::tray_tooltip(Some(&shown)), true);
        }
    }

    /// `RequestTrayRefresh`: tooltip + (deduped) icon for the default render device.
    fn refresh_tray(&self, show_now: bool) {
        let snapshot = self.snapshot.borrow().clone();
        let device = snapshot.default_render();
        if device.is_none() && snapshot.ready && self.recovery_requested_rev.get() != Some(snapshot.structure_rev) {
            self.recovery_requested_rev.set(Some(snapshot.structure_rev));
            self.audio.send(AudioCommand::RequestDefaultRecovery);
        }
        let tooltip = text::tray_tooltip(device);
        if let Some(tray) = self.tray.borrow().as_ref() {
            tray.set_tooltip(&tooltip, show_now);
        }
        self.push_tray_icon(device);
    }

    fn push_tray_icon(&self, device: Option<&vtadn_model::snapshot::DeviceSnapshot>) {
        let tray_ref = self.tray.borrow();
        let Some(tray) = tray_ref.as_ref() else { return };
        let palette = self.palette.borrow();
        let input = TrayRenderInput::new(device, palette.tray_icon, tray.icon_size_px());
        if self.last_icon.get() == Some(input) {
            return;
        }
        match tadn_shell::raster::render_glyph_icon(&input.layers(palette.icon_font), input.size_px) {
            Ok(image) => {
                tray.set_icon(&image);
                self.last_icon.set(Some(input));
            }
            Err(error) => {
                // Fallback app icon; the next refresh retries the glyph.
                log::warn!("tray icon render failed: {error}");
                if let Some(image) = tadn_shell::raster::app_icon_image(input.size_px) {
                    tray.set_icon(&image);
                }
                self.last_icon.set(None);
            }
        }
    }

    fn tray_rect(&self) -> Option<Rect> {
        self.tray.borrow().as_ref().and_then(TrayIcon::rect)
    }

    fn show_tray_menu(self: &Rc<Self>, x: i32, y: i32) {
        let (items, actions, placement, options) = {
            let settings = self.settings.get();
            let snapshot = self.snapshot.borrow();
            let (items, actions) = tray::build_tray_menu(&snapshot, &settings);
            let placement = match settings.common.context_menu_position {
                ContextMenuPosition::Modern => MenuPlacement::TrayIcon { x, y, icon: self.tray_rect() },
                ContextMenuPosition::Classic => MenuPlacement::AtPoint { x, y },
            };
            let options = MenuOptions {
                font_size: settings.common.context_menu_font_size,
                scroll_to_bottom: true,
                ..MenuOptions::default()
            };
            (items, actions, placement, options)
        };
        let Some(menu) = self.take_or_create_tray_menu() else { return };
        let weak = Rc::downgrade(self);
        let weak_closed = weak.clone();
        menu.show(
            items,
            placement,
            options,
            move |id| {
                let action = usize::try_from(id).ok().and_then(|i| actions.get(i).cloned());
                if let (Some(app), Some(action)) = (weak.upgrade(), action) {
                    app.run_tray_menu_action(action);
                }
            },
            move |reason| {
                let Some(app) = weak_closed.upgrade() else { return };
                // Escape returns keyboard focus to the icon, as a shell menu does.
                if reason == MenuCloseReason::Dismissed
                    && let Some(tray) = app.tray.borrow().as_ref()
                {
                    tray.restore_focus();
                }
                app.schedule_tray_menu_eviction();
            },
        );
    }

    /// `TakeOrCreate`: the cached tray menu window, or a new one.
    fn take_or_create_tray_menu(&self) -> Option<Rc<ContextMenu>> {
        self.tray_menu_eviction.stop();
        if let Some(menu) = self.tray_menu.borrow().as_ref() {
            return Some(menu.clone());
        }
        match ContextMenu::new() {
            Ok(menu) => {
                menu.apply_palette(&self.palette.borrow());
                let menu = Rc::new(menu);
                *self.tray_menu.borrow_mut() = Some(menu.clone());
                Some(menu)
            }
            Err(error) => {
                log::error!("tray menu window: {error}");
                None
            }
        }
    }

    /// With `KeepTrayContextMenuWarm` off, a dismissed tray menu is dropped after the idle delay
    /// (never while open); with it on the window stays cached.
    fn schedule_tray_menu_eviction(self: &Rc<Self>) {
        let keep_warm = self.settings.get().common.keep_tray_context_menu_warm;
        if keep_warm || self.tray_menu.borrow().is_none() {
            self.tray_menu_eviction.stop();
            return;
        }
        let weak = Rc::downgrade(self);
        self.tray_menu_eviction.start(slint::TimerMode::SingleShot, WARM_WINDOW_IDLE_EVICTION_DELAY, move || {
            let Some(app) = weak.upgrade() else { return };
            let open = app.tray_menu.borrow().as_ref().is_some_and(|m| m.is_open());
            if tray::should_evict_tray_menu(app.settings.get().common.keep_tray_context_menu_warm, open) {
                app.tray_menu.borrow_mut().take();
            }
        });
    }

    fn run_tray_menu_action(self: &Rc<Self>, action: TrayMenuAction) {
        match action {
            TrayMenuAction::DeviceProperties(id) => links::open_device_properties(&id),
            TrayMenuAction::SoundDevices => links::open_playback_tab(),
            TrayMenuAction::Recording => links::open_recording_tab(),
            TrayMenuAction::Sounds => links::open_sounds_tab(),
            TrayMenuAction::Communications => links::open_communications_tab(),
            TrayMenuAction::Bluetooth => links::open_bluetooth_quick_settings(),
            TrayMenuAction::Settings => self.open_settings(),
            TrayMenuAction::Exit => {
                // Leave the menu's event handler before tearing windows down.
                let weak = Rc::downgrade(self);
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if let Some(app) = weak.upgrade() {
                        app.exit();
                    }
                });
            }
        }
    }

    // ---- flyout / settings window / keep-open --------------------------------------------------

    fn flyout_visible(&self) -> bool {
        self.flyout.borrow().as_ref().is_some_and(Flyout::is_visible)
    }

    fn settings_window_visible(&self) -> bool {
        self.settings_window.borrow().as_ref().is_some_and(SettingsWindow::is_visible)
    }

    /// `ShowVolumeFlyout(activate, holdOpenForSettings)`: always docked from the tray.
    fn show_volume_flyout(&self, activate: bool, hold: bool) {
        if let Some(flyout) = self.flyout.borrow().as_ref() {
            flyout.show_docked(activate);
        }
        if hold {
            self.hold_open();
        }
    }

    /// `OpenSettings`: create lazily, show, pair the flyout.
    pub fn open_settings(self: &Rc<Self>) {
        if self.shutting_down.get() {
            return;
        }
        if self.settings_window.borrow().is_none() {
            match SettingsWindow::new(self.settings.clone(), Rc::new(Bridge(Rc::downgrade(self)))) {
                Ok(window) => {
                    window.apply_palette(&self.palette.borrow());
                    *self.settings_window.borrow_mut() = Some(window);
                }
                Err(error) => {
                    log::error!("settings window: {error}");
                    return;
                }
            }
        }
        if let Some(window) = self.settings_window.borrow().as_ref() {
            window.show(None);
        }
        self.hold_open();
    }

    /// `SettingsFlyoutKeepOpenCoordinator.HoldOpen`.
    fn hold_open(&self) {
        if !self.settings_window_visible() {
            return;
        }
        let flyout_ref = self.flyout.borrow();
        let Some(flyout) = flyout_ref.as_ref() else { return };
        if flyout.is_visible() {
            self.keep_open_paired.set(true);
            flyout.set_hold_open(true);
            return;
        }
        // Restore only a flyout that was already paired with settings.
        if self.keep_open_paired.get() {
            flyout.show_docked(false);
            if flyout.is_visible() {
                flyout.set_hold_open(true);
                return;
            }
        }
        self.keep_open_paired.set(false);
        flyout.set_hold_open(false);
    }

    /// Settings window closed (flyout stays) or minimized (flyout hides).
    fn release_hold(&self) {
        let minimized = self.settings_window_visible();
        if let Some(flyout) = self.flyout.borrow().as_ref() {
            if minimized && self.keep_open_paired.get() {
                flyout.hide_from_coordinator();
            } else {
                flyout.set_hold_open(false);
            }
        }
        self.keep_open_paired.set(false);
    }

    /// The settings window lost activation: once focus settles, hide a paired flyout when neither
    /// window (nor a window owned by settings) is foreground.
    fn on_settings_deactivated(self: &Rc<Self>) {
        if !self.keep_open_paired.get() {
            return;
        }
        let weak = Rc::downgrade(self);
        slint::Timer::single_shot(Duration::ZERO, move || {
            let Some(app) = weak.upgrade() else { return };
            if app.shutting_down.get() || !app.keep_open_paired.get() {
                return;
            }
            let settings_hwnd = app.settings_window.borrow().as_ref().and_then(SettingsWindow::hwnd);
            let flyout_hwnd = app.flyout.borrow().as_ref().and_then(Flyout::hwnd);
            if foreground_in_group(&[settings_hwnd, flyout_hwnd]) {
                return;
            }
            app.keep_open_paired.set(false);
            if let Some(flyout) = app.flyout.borrow().as_ref() {
                flyout.hide_from_coordinator();
            }
        });
    }

    // ---- lifetime ----------------------------------------------------------------------------

    /// Tray Exit / exit request: orderly shutdown, exit code 0.
    pub fn exit(&self) {
        self.shutdown();
    }

    /// Restart (rendering backend / language change): shut down, then re-launch with the same
    /// `--settings-dir`.
    fn restart(&self) {
        let mut args = Vec::new();
        if let Some(dir) = &self.dirs.settings_dir_override {
            args.push(std::ffi::OsString::from(crate::args::SETTINGS_DIR_ARGUMENT));
            args.push(dir.as_os_str().to_owned());
        }
        *self.exit_action.borrow_mut() = ExitAction::Restart(args);
        self.shutdown();
    }

    /// The host shutdown core + `App.ShutdownServices`, exactly once.
    pub fn shutdown(&self) {
        if self.shutting_down.replace(true) {
            return;
        }
        log::info!("shutdown");
        self.subscription.borrow_mut().take();
        // Hotkeys first (host order), then windows, tray, audio, settings.
        if let Some(service) = self.hotkeys.borrow_mut().take() {
            service.unregister_all();
        }
        self.menu.close();
        self.tray_menu_eviction.stop();
        if let Some(menu) = self.tray_menu.borrow_mut().take() {
            menu.close();
        }
        if let Some(flyout) = self.flyout.borrow().as_ref() {
            flyout.set_hold_open(false);
            flyout.hide();
        }
        if let Some(window) = self.settings_window.borrow().as_ref() {
            window.hide();
        }
        self.tray.borrow_mut().take();
        self.system_events.borrow_mut().take();
        if let Some(service) = self.audio_service.borrow_mut().take() {
            service.shutdown();
        }
        self.stores.save_on_exit(&self.settings.get());
        log::info!("shutdown complete");
        tadn_common::log::flush();
        if let Err(error) = slint::quit_event_loop() {
            log::warn!("quit_event_loop: {error}");
        }
    }
}

/// True when the foreground window (or its root owner) is one of `hwnds`.
fn foreground_in_group(hwnds: &[Option<HWND>]) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GA_ROOTOWNER, GetAncestor, GetForegroundWindow};
    // SAFETY: plain user32 queries.
    let (foreground, root) = unsafe {
        let foreground = GetForegroundWindow();
        (foreground, GetAncestor(foreground, GA_ROOTOWNER))
    };
    if foreground.is_invalid() {
        return false;
    }
    hwnds.iter().flatten().any(|&h| h == foreground || h == root)
}

impl FlyoutHost for Bridge {
    fn open_settings(&self) {
        if let Some(app) = self.0.upgrade() {
            // Leave the flyout's event handler first.
            slint::Timer::single_shot(Duration::ZERO, move || app.open_settings());
        }
    }

    fn tray_icon_rect(&self) -> Option<Rect> {
        self.0.upgrade().and_then(|app| app.tray_rect())
    }

    fn settings_hwnd(&self) -> Option<HWND> {
        let app = self.0.upgrade()?;
        let window = app.settings_window.borrow();
        window.as_ref().filter(|w| w.is_visible()).and_then(SettingsWindow::hwnd)
    }

    fn palette(&self) -> Palette {
        self.0.upgrade().map_or_else(
            || tadn_common::theme::resolve_palette(&ThemeFile::default(), &AppSettings::default().common),
            |app| app.palette.borrow().clone(),
        )
    }
}

impl SettingsHost for Bridge {
    fn palette(&self) -> Palette {
        FlyoutHost::palette(self)
    }

    fn startup_enabled(&self) -> bool {
        tadn_shell::startup::is_enabled(&IDENTITY)
    }

    fn set_startup_enabled(&self, enabled: bool) -> Result<(), String> {
        let override_dir = self.0.upgrade().and_then(|app| app.dirs.settings_dir_override.clone());
        tadn_shell::startup::set_enabled(&IDENTITY, enabled, override_dir.as_deref()).map_err(|e| e.message())
    }

    fn startup_target(&self) -> Option<String> {
        tadn_shell::startup::shortcut_target(&IDENTITY).map(|p| p.display().to_string())
    }

    fn register_hotkeys(&self) -> Vec<HotkeyRegistration> {
        self.0.upgrade().map(|app| app.apply_hotkeys(true)).unwrap_or_default()
    }

    fn restart(&self) {
        if let Some(app) = self.0.upgrade() {
            slint::Timer::single_shot(Duration::ZERO, move || app.restart());
        }
    }

    fn data_folder(&self) -> PathBuf {
        self.0.upgrade().map(|app| app.dirs.app_dir.clone()).unwrap_or_default()
    }

    fn settings_activated(&self) {
        if let Some(app) = self.0.upgrade() {
            app.hold_open();
        }
    }

    fn settings_closed(&self) {
        if let Some(app) = self.0.upgrade() {
            app.release_hold();
        }
    }

    fn settings_deactivated(&self) {
        if let Some(app) = self.0.upgrade() {
            app.on_settings_deactivated();
        }
    }

    fn theme(&self) -> ThemeFile {
        self.0.upgrade().map(|app| app.theme.clone()).unwrap_or_default()
    }

    fn hotkey_results(&self) -> Vec<HotkeyRegistration> {
        self.0.upgrade().map(|app| app.apply_hotkeys(false)).unwrap_or_default()
    }
}
