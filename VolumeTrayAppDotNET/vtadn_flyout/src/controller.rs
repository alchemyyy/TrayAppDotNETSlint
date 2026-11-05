//! The flyout window controller: `FlyoutWindowCommon` lifecycle (show/hide, auto-hide, keep-open,
//! placement, docking, drags, warm window) and `VolumeFlyoutWindow` content (generations, in-place
//! updates, meters, row and header actions, menus, editors, dialog).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel};
use tadn_common::theme::Palette;
use tadn_menu::{ContextMenu, MenuCloseReason, MenuItem, MenuOptions, MenuPlacement};
use tadn_shell::backend::{self, WindowKind};
use tadn_shell::foreground::{TaskbarForegroundTracker, TrayToggleAction};
use tadn_shell::placement::{self as shell_placement, Rect};
use tadn_shell::window as shell_window;
use vtadn_model::bus::AudioClient;
use vtadn_model::command::{AudioCommand, FeedbackTiming, FormatQuery};
use vtadn_model::links;
use vtadn_model::policy::meter::MeterLerp;
use vtadn_model::policy::scale;
use vtadn_model::settings::{
    AppSettings, FlyoutDeviceLayout, FlyoutDeviceSort, FlyoutDeviceTitlePosition, SliderThumbShape,
};
use vtadn_model::settings_handle::{SettingsChange, SettingsHandle, Subscription};
use vtadn_model::snapshot::{AppId, AudioSnapshot, DeviceId, Flow, IconPixels, RadioState};
use windows::Win32::Foundation::HWND;

use crate::docking::{DockChange, DockHost, DockSettings, DockingController};
use crate::echo::VolumeSlot;
use crate::metrics;
use crate::placement;
use crate::present::{self, Modifiers};
use crate::rebuild::{InteractionState, RebuildHooks, RebuildScheduler};
use crate::slider;
use crate::ui;
use crate::view::{self, DeviceStructure};
use crate::{FlyoutDeps, FlyoutHost};

/// `TimeConstants.WarmWindowIdleEvictionDelayMs`: a non-warm flyout is released this long after
/// it was dismissed.
const WARM_WINDOW_IDLE_EVICTION_DELAY_MS: u64 = 10_000;
/// `TimeConstants.BluetoothRadioStateChangeTimeoutSeconds`: a radio toggle counts as in flight
/// until the radio state changes or this timeout passes (`BluetoothRadioController.ToggleAsync`).
const BLUETOOTH_RADIO_STATE_CHANGE_TIMEOUT: Duration = Duration::from_secs(3);
/// Countdown pie frame interval with animations on (the render loop's frame cadence).
const COUNTDOWN_ANIMATION_FRAME: Duration = Duration::from_millis(16);

thread_local! {
    /// The live flyout of this (UI) thread, for replies posted from service threads.
    static CURRENT: RefCell<Weak<Inner>> = const { RefCell::new(Weak::new()) };
}

fn with_current(f: impl FnOnce(&Rc<Inner>)) {
    if let Some(inner) = CURRENT.with(|c| c.borrow().upgrade()) {
        f(&inner);
    }
}

/// Whose meter a slot draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MeterOwner {
    Device(usize),
    App(usize, usize),
}

struct MeterSlot {
    owner: MeterOwner,
    device: DeviceId,
    app: Option<AppId>,
    lerp: MeterLerp,
    shown: (f32, f32),
}

struct GenApp {
    id: AppId,
    level: VolumeSlot,
}

struct GenDevice {
    id: DeviceId,
    volume: VolumeSlot,
    apps: Vec<GenApp>,
    apps_model: Rc<VecModel<ui::AppRow>>,
    positions: Vec<(f64, f64)>,
}

/// One built content generation (`VolumeFlyoutContentGeneration`).
struct Generation {
    structure: Vec<DeviceStructure>,
    structure_rev: u64,
    settings_rev: u64,
    defaults: vtadn_model::snapshot::Defaults,
    devices: Vec<GenDevice>,
    meters: Vec<MeterSlot>,
}

struct UndockGesture {
    dragged: bool,
}

#[derive(Default)]
struct State {
    snapshot: Arc<AudioSnapshot>,
    generation: Option<Generation>,
    settings_rev: u64,
    visible: bool,
    visibility_generation: u64,
    docking: DockingController,
    interaction: InteractionState,
    hold_open: bool,
    suppress_next_auto_hide: bool,
    deactivated_since_armed: bool,
    focus_evaluation_queued: bool,
    undock_gesture: Option<UndockGesture>,
    chrome_drag: bool,
    hovered_default_buttons: u32,
    ordering_rebuild_pending: bool,
    radio_toggle: Option<(RadioState, Instant)>,
    last_meter_seq: u64,
    meters_active: bool,
    percent_editor: Option<(usize, Option<usize>)>,
    rename_device: Option<usize>,
    last_percent_press: Option<(usize, i32, i32, Instant)>,
    visible_group_meters: Vec<(DeviceId, AppId)>,
    animations: bool,
}

pub(crate) struct Inner {
    this: Weak<Inner>,
    settings: SettingsHandle,
    audio: AudioClient,
    menu: Rc<ContextMenu>,
    foreground: Rc<TaskbarForegroundTracker>,
    host: Rc<dyn FlyoutHost>,
    window: RefCell<Option<ui::FlyoutWindow>>,
    devices_model: Rc<VecModel<ui::DeviceRow>>,
    meters_model: Rc<VecModel<ui::MeterValue>>,
    scheduler: Rc<RebuildScheduler>,
    state: RefCell<State>,
    meter_timer: Timer,
    countdown_timer: Timer,
    eviction_timer: Timer,
    dialog: RefCell<Option<ui::ConfirmDialog>>,
    icons: RefCell<HashMap<usize, (std::sync::Weak<IconPixels>, Image)>>,
    palette: RefCell<Option<Palette>>,
    decimal_separator: String,
    own_pid: u32,
    window_created: Cell<bool>,
    subscription: RefCell<Option<Subscription>>,
}

impl Inner {
    pub(crate) fn new(deps: FlyoutDeps) -> Result<Rc<Self>, slint::PlatformError> {
        let dock_settings = dock_settings_of(&deps.settings.get());
        let inner = Rc::new_cyclic(|this: &Weak<Inner>| {
            let hooks: Weak<dyn RebuildHooks> = this.clone();
            let scheduler = RebuildScheduler::new(hooks, Box::new(|job| Timer::single_shot(Duration::ZERO, job)));
            Inner {
                this: this.clone(),
                settings: deps.settings.clone(),
                audio: deps.audio.clone(),
                menu: deps.menu.clone(),
                foreground: deps.foreground.clone(),
                host: deps.host.clone(),
                window: RefCell::new(None),
                devices_model: Rc::new(VecModel::default()),
                meters_model: Rc::new(VecModel::default()),
                scheduler,
                state: RefCell::new(State {
                    snapshot: deps.audio.latest(),
                    docking: DockingController::new(&dock_settings),
                    ..State::default()
                }),
                meter_timer: Timer::default(),
                countdown_timer: Timer::default(),
                eviction_timer: Timer::default(),
                dialog: RefCell::new(None),
                icons: RefCell::new(HashMap::new()),
                palette: RefCell::new(None),
                decimal_separator: slider::user_decimal_separator(),
                own_pid: std::process::id(),
                window_created: Cell::new(false),
                subscription: RefCell::new(None),
            }
        });
        CURRENT.with(|c| *c.borrow_mut() = inner.this.clone());
        let weak = inner.this.clone();
        let subscription = inner.settings.subscribe(move |change| {
            let weak = weak.clone();
            // `OnSettingsChanged` posts; the settings handle notifies synchronously.
            Timer::single_shot(Duration::ZERO, move || {
                if let Some(inner) = weak.upgrade() {
                    inner.on_settings_changed(change);
                }
            });
        });
        *inner.subscription.borrow_mut() = Some(subscription);
        let palette = inner.host.palette();
        inner.state.borrow_mut().animations = palette.animations;
        *inner.palette.borrow_mut() = Some(palette);
        // Warm slot: with KeepFlyoutWarm the window exists before the first show.
        if inner.settings.get().common.keep_flyout_warm {
            inner.ensure_window()?;
        }
        Ok(inner)
    }

    // ---------------------------------------------------------------- window

    fn window(&self) -> Option<ui::FlyoutWindow> {
        self.window.borrow().as_ref().map(ComponentHandle::clone_strong)
    }

    fn ensure_window(&self) -> Result<ui::FlyoutWindow, slint::PlatformError> {
        if let Some(window) = self.window() {
            return Ok(window);
        }
        let window = backend::with_window_kind(WindowKind::Flyout, ui::FlyoutWindow::new)?;
        window.set_devices(ModelRc::from(self.devices_model.clone()));
        let state = window.global::<ui::FlyoutState>();
        state.set_meters(ModelRc::from(self.meters_model.clone()));
        if let Some(palette) = self.palette.borrow().as_ref() {
            tadn_common::apply_palette!(window.global::<ui::Theme>(), palette);
        }
        self.wire_window(&window);
        self.wire_actions(&window);
        let weak = self.this.clone();
        shell_window::on_focus_changed(window.window(), move |focused| {
            if let Some(inner) = weak.upgrade() {
                inner.on_focus_changed(focused);
            }
        });
        *self.window.borrow_mut() = Some(window.clone_strong());
        self.window_created.set(false);
        {
            let mut s = self.state.borrow_mut();
            s.generation = None;
        }
        self.apply_settings_properties(&window);
        Ok(window)
    }

    fn wire_window(&self, window: &ui::FlyoutWindow) {
        let weak = self.this.clone();
        let w = move |f: fn(&Inner)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&inner);
                }
            }
        };
        window.on_settings_clicked(w(Inner::on_settings_button));
        window.on_sound_settings_clicked(w(|inner| {
            links::open_sound_settings(inner.settings.get().sound_settings_target)
        }));
        window.on_disabled_devices_clicked(w(Inner::toggle_disabled_devices));
        window.on_undock_activated(w(Inner::toggle_undocked));
        window.on_escape_pressed(w(Inner::hide));
        let weak = self.this.clone();
        window.on_bluetooth_radio_clicked(move |mods| {
            if let Some(inner) = weak.upgrade() {
                inner.on_bluetooth_radio_button(modifiers(mods));
            }
        });
        let weak = self.this.clone();
        window.on_communications_clicked(move |mods| {
            if let Some(inner) = weak.upgrade() {
                let current = inner.state.borrow().snapshot.ducking;
                inner
                    .audio
                    .send(AudioCommand::SetDucking(present::communications_ducking_mode(modifiers(mods), current)));
            }
        });
        let weak = self.this.clone();
        window.on_undock_pointer(move |kind, inside| {
            if let Some(inner) = weak.upgrade() {
                inner.on_undock_pointer(kind, inside);
            }
        });
        let weak = self.this.clone();
        window.on_chrome_pointer(move |kind| {
            if let Some(inner) = weak.upgrade() {
                inner.on_chrome_pointer(kind);
            }
        });
    }

    fn wire_actions(&self, window: &ui::FlyoutWindow) {
        let actions = window.global::<ui::FlyoutActions>();
        macro_rules! on {
            ($setter:ident, |$inner:ident $(, $arg:ident)*| $body:expr) => {{
                let weak = self.this.clone();
                actions.$setter(move |$($arg),*| {
                    if let Some($inner) = weak.upgrade() {
                        $body
                    }
                });
            }};
        }
        on!(on_any_press, |inner| inner.on_any_press());
        on!(on_device_mute, |inner, dev| inner.on_device_mute(dev));
        on!(on_device_slider_edited, |inner, dev, value| inner.on_device_slider_edited(dev, value));
        on!(on_device_slider_drag, |inner, dev, started| inner.on_device_slider_drag(dev, started));
        on!(on_app_mute, |inner, dev, app| inner.on_app_mute(dev, app));
        on!(on_app_slider_edited, |inner, dev, app, value| inner.on_app_slider_edited(dev, app, value));
        on!(on_app_slider_drag, |inner, dev, app, started| inner.on_app_slider_drag(dev, app, started));
        let weak = self.this.clone();
        actions.on_percent_pressed(move |dev, app, button, double| {
            weak.upgrade().is_some_and(|inner| inner.on_percent_pressed(dev, app, button, double))
        });
        on!(on_percent_activated, |inner, dev, app| {
            inner.open_percent_editor(dev, app);
        });
        on!(on_percent_commit, |inner, text| inner.commit_percent_editor(text.as_str()));
        on!(on_percent_cancel, |inner| inner.close_percent_editor());
        on!(on_name_double_clicked, |inner, dev| inner.begin_rename(dev));
        on!(on_name_right_clicked, |inner, dev, x, y, w, h| inner.show_format_menu(dev, (x, y, w, h)));
        on!(on_rename_commit, |inner, text| inner.commit_rename(Some(text.as_str())));
        on!(on_rename_cancel, |inner| inner.commit_rename(None));
        on!(on_bt_clicked, |inner, dev, c, a, s| inner.on_bt_button(dev, Modifiers { control: c, alt: a, shift: s }));
        on!(on_lock_clicked, |inner, dev| inner.send_for_device(dev, |id| AudioCommand::ToggleExclusiveAllowed { id }));
        on!(on_eq_clicked, |inner, dev, c, a, s| inner.on_eq_button(dev, Modifiers { control: c, alt: a, shift: s }));
        on!(on_eq_right_clicked, |inner, dev| inner
            .send_for_device(dev, |id| AudioCommand::OpenEqualizerApoEditor { id }));
        on!(on_listen_clicked, |inner, dev, c, a, s| inner
            .on_listen_button(dev, Modifiers { control: c, alt: a, shift: s }));
        on!(on_listen_right_clicked, |inner, dev, x, y, w, h| inner.show_listen_menu(dev, (x, y, w, h)));
        on!(on_default_clicked, |inner, dev, c, a, s| inner
            .on_default_button(dev, Modifiers { control: c, alt: a, shift: s }));
        on!(on_default_right_clicked, |inner, dev| {
            if let Some(id) = inner.device_id(dev) {
                links::open_device_properties(&id);
            }
        });
        on!(on_default_hover, |inner, dev, hovered| {
            let _ = dev;
            inner.on_default_hover(hovered);
        });
        on!(on_chevron_clicked, |inner, dev| inner.toggle_drawer(dev));
    }

    pub(crate) fn hwnd(&self) -> Option<HWND> {
        self.window().and_then(|w| shell_window::hwnd(w.window()))
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.state.borrow().visible
    }

    pub(crate) fn is_active(&self) -> bool {
        self.hwnd().is_some_and(shell_window::is_foreground)
    }

    fn scale(&self) -> f64 {
        self.window().map_or(1.0, |w| f64::from(w.window().scale_factor())).max(0.01)
    }

    // ---------------------------------------------------------------- palette & settings

    pub(crate) fn apply_palette(&self, palette: &Palette) {
        *self.palette.borrow_mut() = Some(palette.clone());
        self.state.borrow_mut().animations = palette.animations;
        if let Some(window) = self.window() {
            tadn_common::apply_palette!(window.global::<ui::Theme>(), palette);
            if let Some(hwnd) = shell_window::hwnd(window.window()) {
                shell_window::set_rounded_corners(hwnd, palette.rounded_corners);
            }
        }
        if let Some(dialog) = self.dialog.borrow().as_ref() {
            tadn_common::apply_palette!(dialog.global::<ui::Theme>(), palette);
        }
        self.update_countdown_timer();
    }

    fn on_settings_changed(&self, change: SettingsChange) {
        if change == SettingsChange::Settings {
            let redocked = {
                let mut s = self.state.borrow_mut();
                s.settings_rev += 1;
                s.docking.redock_if_undocking_disabled(self)
            };
            if let Some(change) = redocked {
                self.on_dock_change(change);
                return;
            }
        }
        if let Some(window) = self.window() {
            self.apply_settings_properties(&window);
        }
        self.scheduler.queue();
        // Changing MeterPeakFps re-arms the render timer.
        if self.state.borrow().meters_active {
            self.start_meter_timer();
        }
    }

    /// Settings-derived window and generation-wide properties.
    fn apply_settings_properties(&self, window: &ui::FlyoutWindow) {
        let settings = self.settings.get();
        window.set_header_at_bottom(settings.flyout_header_at_bottom);
        window.set_show_bluetooth_radio(settings.show_bluetooth_radio_button_in_flyout_header);
        window.set_eye_glyph(
            if present::shows_disabled_devices(&settings) { present::glyphs::VIEW } else { present::glyphs::HIDE }
                .into(),
        );
        window.set_undock_visible(settings.allow_flyout_undock);
        window.set_undocked(self.state.borrow().docking.is_undocked());
        let state = window.global::<ui::FlyoutState>();
        state.set_apps_bottom(settings.flyout_device_layout == FlyoutDeviceLayout::AppsBelowDevice);
        state.set_title_above(settings.flyout_device_title_position == FlyoutDeviceTitlePosition::AboveSlider);
        state.set_header_at_bottom(settings.flyout_header_at_bottom);
        let thumb = &settings.slider_thumb;
        state.set_thumb(ui::SliderThumb {
            shape: match thumb.shape {
                SliderThumbShape::Capsule => ui::SliderThumbShape::Capsule,
                SliderThumbShape::Glyph => ui::SliderThumbShape::Glyph,
            },
            glyph: thumb.glyph.as_str().into(),
            font_family: thumb.font_family.as_str().into(),
            font_size: thumb.font_size as f32,
            width: thumb.width as f32,
            height: thumb.height as f32,
            x_scale: thumb.x_scale as f32,
        });
        state.set_wheel_step(settings.wheel_volume_step_percent as f32);
        state.set_meter_peak_color(slint::Color::from_argb_encoded(settings.effective_meter_peak_color().0));
        state.set_meter_peak_stereo_color(slint::Color::from_argb_encoded(
            settings.effective_meter_peak_stereo_color().0,
        ));
        let icon_scale = f64::from(settings.app_drawer_icon_scale_percent) / 100.0;
        state.set_app_icon_image_size((metrics::APP_ICON_IMAGE_SIZE * icon_scale) as f32);
        state.set_app_icon_glyph_size((metrics::APP_ICON_GLYPH_SIZE * icon_scale) as f32);
        drop(settings);
        self.apply_header_from_snapshot(window);
    }

    fn apply_header_from_snapshot(&self, window: &ui::FlyoutWindow) {
        let settings = self.settings.get();
        let snapshot = self.state.borrow().snapshot.clone();
        window.set_bluetooth_radio_on(snapshot.radio == RadioState::On);
        window.set_bluetooth_radio_tooltip(
            present::bluetooth_radio_tooltip(snapshot.radio, settings.flyout_bluetooth_radio_button_click_gesture)
                .into(),
        );
        window.set_show_communications(present::show_communications_button(&settings, snapshot.ducking));
        window.set_communications_active(snapshot.ducking.is_active());
    }

    // ---------------------------------------------------------------- audio updates

    pub(crate) fn on_audio_update(&self, snapshot: &Arc<AudioSnapshot>) {
        {
            let mut s = self.state.borrow_mut();
            s.snapshot = snapshot.clone();
            if let Some((from, since)) = s.radio_toggle
                && (snapshot.radio != from || since.elapsed() >= BLUETOOTH_RADIO_STATE_CHANGE_TIMEOUT)
            {
                s.radio_toggle = None;
            }
        }
        let Some(window) = self.window() else { return };
        self.apply_header_from_snapshot(&window);
        self.check_structure();
        self.patch_all();
        self.update_countdown_timer();
    }

    /// Structure (device list, order, visible groups) changed since the generation was built?
    fn check_structure(&self) {
        let (needs, gated) = {
            let s = self.state.borrow();
            let Some(generation) = s.generation.as_ref() else {
                drop(s);
                self.scheduler.queue();
                return;
            };
            if generation.structure_rev == s.snapshot.structure_rev && generation.settings_rev == s.settings_rev {
                return;
            }
            let settings = self.settings.get();
            let structure = view::build_structure(&s.snapshot, &settings, &self.settings.devices(), self.own_pid);
            if structure == generation.structure {
                (false, false)
            } else {
                // Hover-gated re-sort (`QueueDeviceOrderingRebuild`): an order-only change caused by
                // a default flag waits while the pointer is over a default-device button.
                let same_set = structure.len() == generation.structure.len()
                    && structure.iter().all(|d| generation.structure.iter().any(|g| g == d || g.id == d.id));
                let defaults_same = generation.defaults.render == s.snapshot.defaults.render
                    && generation.defaults.capture == s.snapshot.defaults.capture;
                let gated = same_set
                    && defaults_same
                    && settings.flyout_device_sort == FlyoutDeviceSort::StateGrouped
                    && s.hovered_default_buttons > 0;
                (true, gated)
            }
        };
        if !needs {
            return;
        }
        if gated {
            self.state.borrow_mut().ordering_rebuild_pending = true;
        } else {
            self.scheduler.queue();
        }
    }

    fn on_default_hover(&self, hovered: bool) {
        let flush = {
            let mut s = self.state.borrow_mut();
            if hovered {
                s.hovered_default_buttons += 1;
                false
            } else {
                s.hovered_default_buttons = s.hovered_default_buttons.saturating_sub(1);
                s.hovered_default_buttons == 0 && std::mem::take(&mut s.ordering_rebuild_pending)
            }
        };
        if flush {
            self.scheduler.queue();
        }
    }

    /// In-place updates of every row of the current generation from the latest snapshot.
    fn patch_all(&self) {
        let count = self.state.borrow().generation.as_ref().map_or(0, |g| g.devices.len());
        for dev in 0..count {
            self.patch_device(dev, true);
        }
    }

    /// Recomputes one device row (and optionally its app rows) from the snapshot and pushes the
    /// rows that changed.
    fn patch_device(&self, dev: usize, with_apps: bool) {
        let settings = self.settings.get();
        let now = tick_count_ms();
        let mut s = self.state.borrow_mut();
        let snapshot = s.snapshot.clone();
        let Some(generation) = s.generation.as_mut() else { return };
        let Some(gd) = generation.devices.get_mut(dev) else { return };
        let Some(device) = snapshot.device(&gd.id) else { return };
        let shown = gd.volume.reconcile(device.volume);
        if with_apps {
            for (index, app) in gd.apps.iter_mut().enumerate() {
                let Some(group) = device.groups.iter().find(|g| g.app_id == app.id) else { continue };
                let level = app.level.reconcile(group.volume);
                let row = view::app_row(
                    device,
                    group,
                    &settings,
                    view::AppRowInputs {
                        shown_level: level,
                        meter: 0,
                        icon: None,
                        position: gd.positions.get(index).copied().unwrap_or_default(),
                    },
                );
                if let Some(current) = gd.apps_model.row_data(index) {
                    let row = ui::AppRow {
                        meter: current.meter,
                        icon: current.icon.clone(),
                        has_icon: current.has_icon,
                        ..row
                    };
                    let row = self.with_icon(row, group.icon.as_ref());
                    if row != current {
                        gd.apps_model.set_row_data(index, row);
                    }
                }
            }
        }
        let structure = &generation.structure[dev];
        let Some(current) = self.devices_model.row_data(dev) else { return };
        let row = view::device_row(
            device,
            structure,
            &settings,
            view::DeviceRowInputs {
                shown_volume: shown,
                meter: current.meter,
                apps: current.apps.clone(),
                now_ms: now,
            },
        );
        if row != current {
            drop(s);
            self.devices_model.set_row_data(dev, row);
        }
    }

    fn with_icon(&self, mut row: ui::AppRow, icon: Option<&Arc<IconPixels>>) -> ui::AppRow {
        match icon {
            Some(pixels) => {
                row.icon = self.icon_image(pixels);
                row.has_icon = true;
            }
            None => {
                row.icon = Image::default();
                row.has_icon = false;
            }
        }
        row
    }

    /// One `slint::Image` per icon allocation (cached by pointer while the pixels live).
    fn icon_image(&self, pixels: &Arc<IconPixels>) -> Image {
        let key = Arc::as_ptr(pixels) as usize;
        let mut icons = self.icons.borrow_mut();
        if let Some((weak, image)) = icons.get(&key)
            && weak.upgrade().is_some_and(|live| Arc::ptr_eq(&live, pixels))
        {
            return image.clone();
        }
        let image = if pixels.rgba.len() == (pixels.width as usize) * (pixels.height as usize) * 4 {
            let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&pixels.rgba, pixels.width, pixels.height);
            Image::from_rgba8(buffer)
        } else {
            log::warn!("VolumeFlyout: app icon {}x{} has {} bytes", pixels.width, pixels.height, pixels.rgba.len());
            Image::default()
        };
        icons.insert(key, (Arc::downgrade(pixels), image.clone()));
        image
    }

    // ---------------------------------------------------------------- rebuild

    fn build_generation(&self) -> Result<(), String> {
        let window = self.window().ok_or_else(|| "no flyout window".to_owned())?;
        let settings = self.settings.get();
        let (snapshot, settings_rev, old) = {
            let mut s = self.state.borrow_mut();
            (s.snapshot.clone(), s.settings_rev, s.generation.take())
        };
        let structure = view::build_structure(&snapshot, &settings, &self.settings.devices(), self.own_pid);
        let now = tick_count_ms();
        self.icons.borrow_mut().retain(|_, (weak, _)| weak.strong_count() > 0);

        let mut devices = Vec::with_capacity(structure.len());
        let mut meters = Vec::new();
        let mut rows = Vec::with_capacity(structure.len());
        for (dev, ds) in structure.iter().enumerate() {
            let Some(device) = snapshot.device(&ds.id) else {
                return Err(format!("listed device {} is missing", ds.id));
            };
            let old_device = old.as_ref().and_then(|g| g.devices.iter().find(|d| d.id == ds.id));
            let mut volume = old_device.map_or_else(|| VolumeSlot::new(device.volume), |d| d.volume.clone());
            volume.adjusting = false;
            volume.deferred = false;
            let shown = volume.reconcile(device.volume);
            let device_meter = meters.len();
            meters.push(meter_slot(old.as_ref(), MeterOwner::Device(dev), &ds.id, None));
            let positions = view::grid_positions(ds, &settings);
            let apps_model = Rc::new(VecModel::default());
            let mut apps = Vec::with_capacity(ds.apps.len());
            for (index, app_id) in ds.apps.iter().enumerate() {
                let Some(group) = device.groups.iter().find(|g| g.app_id == *app_id) else { continue };
                let mut level = old_device
                    .and_then(|d| d.apps.iter().find(|a| a.id == *app_id))
                    .map_or_else(|| VolumeSlot::new(group.volume), |a| a.level.clone());
                level.adjusting = false;
                level.deferred = false;
                let shown_level = level.reconcile(group.volume);
                let meter = meters.len();
                meters.push(meter_slot(old.as_ref(), MeterOwner::App(dev, apps.len()), &ds.id, Some(app_id)));
                let row = view::app_row(
                    device,
                    group,
                    &settings,
                    view::AppRowInputs {
                        shown_level,
                        meter: meter as i32,
                        icon: group.icon.as_ref().map(|icon| self.icon_image(icon)),
                        position: positions.get(index).copied().unwrap_or_default(),
                    },
                );
                apps_model.push(row);
                apps.push(GenApp { id: app_id.clone(), level });
            }
            rows.push(view::device_row(
                device,
                ds,
                &settings,
                view::DeviceRowInputs {
                    shown_volume: shown,
                    meter: device_meter as i32,
                    apps: ModelRc::from(apps_model.clone()),
                    now_ms: now,
                },
            ));
            devices.push(GenDevice { id: ds.id.clone(), volume, apps, apps_model, positions });
        }
        let meter_values: Vec<ui::MeterValue> =
            meters.iter().map(|m| ui::MeterValue { min: m.shown.0, max: m.shown.1 }).collect();
        let group_meters = view::visible_group_meters(&structure);
        drop(settings);

        // Close editors of the old generation and drop a stale tooltip.
        window.global::<ui::Tip>().invoke_hide_all();
        let state_global = window.global::<ui::FlyoutState>();
        state_global.set_percent_device(-1);
        state_global.set_rename_device(-1);
        let previous_offset = window.get_list_content_y();

        self.devices_model.set_vec(rows);
        self.meters_model.set_vec(meter_values);
        {
            let mut s = self.state.borrow_mut();
            s.percent_editor = None;
            s.rename_device = None;
            s.interaction.percent_editors = 0;
            s.interaction.rename_active = false;
            s.hovered_default_buttons = 0;
            s.ordering_rebuild_pending = false;
            s.generation = Some(Generation {
                structure,
                structure_rev: snapshot.structure_rev,
                settings_rev,
                defaults: snapshot.defaults.clone(),
                devices,
                meters,
            });
        }
        self.apply_work_area_max_height();
        self.set_scroll_offset(previous_offset);
        self.send_visible_group_meters(group_meters);
        Ok(())
    }

    fn send_visible_group_meters(&self, meters: Vec<(DeviceId, AppId)>) {
        let send = {
            let mut s = self.state.borrow_mut();
            let send = s.visible && s.visible_group_meters != meters;
            if send {
                s.visible_group_meters = meters.clone();
            }
            send
        };
        if send {
            self.audio.send(AudioCommand::SetVisibleGroupMeters(meters));
        }
    }

    /// `ApplyCellsScrollOffset`: clamps against the extent of the current layout.
    fn set_scroll_offset(&self, content_y: f32) {
        let Some(window) = self.window() else { return };
        let max = (window.get_list_extent_height() - window.get_list_height()).max(0.0);
        window.set_list_content_y(content_y.clamp(-max, 0.0));
    }

    fn scroll_to_bottom(&self) {
        self.set_scroll_offset(f32::NEG_INFINITY);
    }

    // ---------------------------------------------------------------- placement

    fn tray_anchor(&self) -> (i32, i32) {
        match self.host.tray_icon_rect() {
            Some(icon) => icon.center(),
            None => self.window().map_or((0, 0), |w| {
                let p = w.window().position();
                (p.x, p.y)
            }),
        }
    }

    fn work_area(&self) -> Rect {
        let (x, y) = self.tray_anchor();
        shell_placement::resolve_work_area(x, y)
    }

    fn pixel_size(&self) -> (i32, i32) {
        let height = self.window().map_or(1.0, |w| f64::from(w.get_content_height()));
        placement::pixel_size(metrics::WINDOW_WIDTH, height, self.scale())
    }

    /// `ApplyWorkAreaMaxHeight` + the cells' `ResolveMaxHeight(ContentHeightReserve)`.
    fn apply_work_area_max_height(&self) {
        let Some(window) = self.window() else { return };
        let work_area = self.work_area();
        let scale = self.scale();
        window.set_window_max_height(placement::resolve_max_height(work_area, scale, 0.0) as f32);
        window.set_list_max_height(
            placement::resolve_max_height(work_area, scale, metrics::CONTENT_HEIGHT_RESERVE) as f32
        );
    }

    fn position_near_tray(&self) {
        let Some(window) = self.window() else { return };
        let position = self.state.borrow().docking.resolve_position(self);
        let height = window.get_content_height();
        window.window().set_size(slint::LogicalSize::new(metrics::WINDOW_WIDTH as f32, height));
        window.window().set_position(slint::PhysicalPosition::new(position.0, position.1));
    }

    /// `QueuePositionNearTray`: after layout; skipped when hidden, superseded, or dragging.
    fn queue_position_near_tray(&self) {
        let generation = self.state.borrow().visibility_generation;
        let weak = self.this.clone();
        Timer::single_shot(Duration::ZERO, move || {
            let Some(inner) = weak.upgrade() else { return };
            let skip = {
                let s = inner.state.borrow();
                !s.visible || s.visibility_generation != generation || s.interaction.window_drag
            };
            if !skip {
                inner.position_near_tray();
            }
        });
    }

    // ---------------------------------------------------------------- show / hide

    pub(crate) fn show_docked(&self, activate: bool) {
        let change = self.state.borrow_mut().docking.redock(self);
        if let Some(change) = change {
            self.on_dock_change(change);
        }
        self.show_at(activate);
    }

    pub(crate) fn restore_on_startup(&self) {
        let restore = DockingController::should_restore_on_startup(&dock_settings_of(&self.settings.get()));
        if restore {
            self.show_at(true);
        }
    }

    /// `ShowAt`.
    fn show_at(&self, activate: bool) {
        self.eviction_timer.stop();
        let window = match self.ensure_window() {
            Ok(window) => window,
            Err(error) => {
                log::error!("VolumeFlyout: cannot create the flyout window: {error}");
                return;
            }
        };
        let (generation, redocked) = {
            let mut s = self.state.borrow_mut();
            s.visibility_generation += 1;
            s.suppress_next_auto_hide = false;
            (s.visibility_generation, s.docking.redock_if_undocking_disabled(self))
        };
        if let Some(change) = redocked {
            self.on_dock_change(change);
        }
        // OnShowing: re-enumerate sessions (the radio refresh rides on SetFlyoutActive).
        self.audio.send(AudioCommand::ReconcileSessions);
        self.apply_work_area_max_height();
        let was_visible = {
            let mut s = self.state.borrow_mut();
            std::mem::replace(&mut s.visible, true)
        };
        self.position_near_tray();
        if !was_visible {
            let first = !self.window_created.replace(true);
            let shown = if first {
                let weak = self.this.clone();
                shell_window::when_created(window.window(), move |hwnd| {
                    if let Some(inner) = weak.upgrade() {
                        let rounded = inner.palette.borrow().as_ref().is_none_or(|p| p.rounded_corners);
                        shell_window::set_rounded_corners(hwnd, rounded);
                    }
                });
                backend::with_window_kind(WindowKind::Flyout, || window.show())
            } else {
                window.show()
            };
            if let Err(error) = shown {
                log::error!("VolumeFlyout: show failed: {error}");
                self.state.borrow_mut().visible = false;
                return;
            }
        }
        self.scheduler.rebuild_now();
        self.apply_work_area_max_height();
        self.position_near_tray();
        self.scroll_to_bottom();

        // Loaded: re-measure, place, reveal, start activity, activate.
        let weak = self.this.clone();
        Timer::single_shot(Duration::ZERO, move || {
            let Some(inner) = weak.upgrade() else { return };
            if inner.state.borrow().visibility_generation != generation || !inner.is_visible() {
                return;
            }
            inner.apply_work_area_max_height();
            inner.position_near_tray();
            inner.scroll_to_bottom();
            inner.start_activity();
            if let Some(window) = inner.window() {
                window.invoke_focus_root();
            }
            if activate && let Some(hwnd) = inner.hwnd() {
                shell_window::activate(hwnd);
            }
        });
    }

    pub(crate) fn hide(&self) {
        let window = self.window();
        {
            let mut s = self.state.borrow_mut();
            if !s.visible {
                return;
            }
            s.visible = false;
            s.visibility_generation += 1;
            s.suppress_next_auto_hide = false;
            s.chrome_drag = false;
            s.undock_gesture = None;
            s.interaction.window_drag = false;
            s.interaction.slider_drags = 0;
        }
        // OnHiding.
        self.close_menu();
        self.stop_activity();
        self.close_dialog();
        if let Some(window) = &window {
            window.global::<ui::Tip>().invoke_hide_all();
            if let Err(error) = window.hide() {
                log::warn!("VolumeFlyout: hide failed: {error}");
            }
        }
        if let Some(hwnd) = self.hwnd() {
            self.foreground.forget(hwnd);
        }
        if !self.settings.get().common.keep_flyout_warm {
            let weak = self.this.clone();
            self.eviction_timer.start(
                TimerMode::SingleShot,
                Duration::from_millis(WARM_WINDOW_IDLE_EVICTION_DELAY_MS),
                move || {
                    if let Some(inner) = weak.upgrade() {
                        inner.evict_window();
                    }
                },
            );
        }
    }

    /// Releases a non-warm window after the idle delay (`CloseForWarmEviction`).
    fn evict_window(&self) {
        if self.is_visible() || self.settings.get().common.keep_flyout_warm {
            return;
        }
        let window = self.window.borrow_mut().take();
        {
            let mut s = self.state.borrow_mut();
            s.generation = None;
        }
        self.devices_model.set_vec(Vec::new());
        self.meters_model.set_vec(Vec::new());
        self.icons.borrow_mut().clear();
        drop(window);
        self.window_created.set(false);
    }

    fn start_activity(&self) {
        let groups = {
            let mut s = self.state.borrow_mut();
            if s.meters_active {
                return;
            }
            s.meters_active = true;
            s.visible_group_meters.clear();
            s.generation.as_ref().map(|g| view::visible_group_meters(&g.structure)).unwrap_or_default()
        };
        self.audio.send(AudioCommand::SetFlyoutActive(true));
        self.send_visible_group_meters(groups);
        self.start_meter_timer();
        self.update_countdown_timer();
    }

    /// Render tick at `MeterPeakFps` (`1000 / fps` ms).
    fn start_meter_timer(&self) {
        let fps = self.settings.get().meter_peak_fps.max(1) as u64;
        let interval = Duration::from_micros(1_000_000 / fps);
        if self.meter_timer.running() && self.meter_timer.interval() == interval {
            return;
        }
        let weak = self.this.clone();
        self.meter_timer.start(TimerMode::Repeated, interval, move || {
            if let Some(inner) = weak.upgrade() {
                inner.meter_tick();
            }
        });
    }

    fn stop_activity(&self) {
        let was_active = {
            let mut s = self.state.borrow_mut();
            let was = std::mem::replace(&mut s.meters_active, false);
            s.visible_group_meters.clear();
            if let Some(generation) = s.generation.as_mut() {
                for slot in &mut generation.meters {
                    slot.lerp.reset();
                    slot.shown = (0.0, 0.0);
                }
            }
            was
        };
        self.meter_timer.stop();
        self.countdown_timer.stop();
        if was_active {
            self.audio.send(AudioCommand::SetFlyoutActive(false));
            self.audio.send(AudioCommand::SetVisibleGroupMeters(Vec::new()));
        }
        for index in 0..self.meters_model.row_count() {
            self.meters_model.set_row_data(index, ui::MeterValue { min: 0.0, max: 0.0 });
        }
    }

    pub(crate) fn toggle_from_tray(&self) {
        let hwnd = self.hwnd();
        let visible = self.is_visible();
        let minimized = hwnd.is_some_and(shell_window::is_minimized);
        match self.foreground.decide(hwnd, visible, minimized) {
            TrayToggleAction::None => {}
            TrayToggleAction::Hide => self.hide(),
            TrayToggleAction::Activate => {
                if let Some(hwnd) = hwnd {
                    shell_window::activate(hwnd);
                }
            }
            TrayToggleAction::Show => self.show_docked(true),
        }
    }

    // ---------------------------------------------------------------- focus group / keep-open

    fn on_focus_changed(&self, focused: bool) {
        if focused {
            return;
        }
        let queue = {
            let mut s = self.state.borrow_mut();
            s.deactivated_since_armed = true;
            !std::mem::replace(&mut s.focus_evaluation_queued, true)
        };
        if queue {
            self.queue_focus_evaluation();
        }
    }

    fn queue_focus_evaluation(&self) {
        self.state.borrow_mut().focus_evaluation_queued = true;
        let weak = self.this.clone();
        Timer::single_shot(Duration::ZERO, move || {
            if let Some(inner) = weak.upgrade() {
                inner.state.borrow_mut().focus_evaluation_queued = false;
                inner.evaluate_focus_group();
            }
        });
    }

    fn evaluate_focus_group(&self) {
        if !self.is_visible() || self.is_active() {
            return;
        }
        let (hold, undocked, child_open) = {
            let s = self.state.borrow();
            (s.hold_open, s.docking.is_undocked(), s.interaction.menu_open || s.interaction.dialog_open)
        };
        if child_open {
            return;
        }
        if hold {
            // Coordinator: keep while the settings window (or a window it owns) is active.
            if self.foreground_in_settings() {
                return;
            }
            self.hide_from_coordinator();
            return;
        }
        // ShouldAutoHideWhenDeactivated: only while docked.
        if undocked || self.consume_suppression() {
            return;
        }
        self.hide();
    }

    fn foreground_in_settings(&self) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::{GA_ROOTOWNER, GetAncestor, GetForegroundWindow};
        let Some(settings) = self.host.settings_hwnd() else { return false };
        // SAFETY: plain queries on window handles; an invalid handle yields a null result.
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_invalid() {
            return false;
        }
        // SAFETY: as above.
        let root_owner = unsafe { GetAncestor(foreground, GA_ROOTOWNER) };
        foreground == settings || root_owner == settings
    }

    fn consume_suppression(&self) -> bool {
        std::mem::take(&mut self.state.borrow_mut().suppress_next_auto_hide)
    }

    /// `OpensWindow`: the deactivation the opened window causes does not hide the flyout.
    fn arm_suppression(&self) {
        let mut s = self.state.borrow_mut();
        s.suppress_next_auto_hide = true;
        s.deactivated_since_armed = false;
    }

    pub(crate) fn set_hold_open(&self, hold: bool) {
        self.state.borrow_mut().hold_open = hold;
    }

    pub(crate) fn hide_from_coordinator(&self) {
        let can_hide = {
            let mut s = self.state.borrow_mut();
            s.hold_open = false;
            s.visible && !s.docking.is_undocked() && !s.interaction.menu_open && !s.interaction.dialog_open
        };
        if can_hide && !self.consume_suppression() {
            self.hide();
        }
    }

    /// A child window (menu, dialog) closed because it lost activation: re-evaluate.
    fn child_closed_from_deactivation(&self) {
        self.state.borrow_mut().suppress_next_auto_hide = false;
        self.queue_focus_evaluation();
    }

    fn on_any_press(&self) {
        // A press inside the flyout means no window opened from the last activation took focus.
        {
            let mut s = self.state.borrow_mut();
            if s.suppress_next_auto_hide && !s.deactivated_since_armed {
                s.suppress_next_auto_hide = false;
            }
        }
        if self.state.borrow().rename_device.is_some() {
            let text = self.window().map(|w| w.global::<ui::FlyoutState>().get_rename_text().to_string());
            if let Some(text) = text {
                self.commit_rename(Some(&text));
            }
        }
    }

    // ---------------------------------------------------------------- docking

    fn on_dock_change(&self, change: DockChange) {
        let undocked = self.state.borrow().docking.is_undocked();
        if let Some(window) = self.window() {
            window.set_undocked(undocked);
        }
        if matches!(change, DockChange::Undocked | DockChange::Redocked) {
            self.scheduler.rebuild_now();
        }
        if change == DockChange::Redocked {
            self.queue_position_near_tray();
        }
    }

    fn toggle_undocked(&self) {
        let change = self.state.borrow_mut().docking.toggle_undocked(self);
        if let Some(change) = change {
            self.on_dock_change(change);
        }
    }

    fn begin_window_drag(&self) {
        let Some(window) = self.window() else { return };
        let pointer = shell_window::cursor_pos();
        let position = window.window().position();
        let mut s = self.state.borrow_mut();
        let (docked, tolerance) = s.docking.capture_docked_position(self);
        s.docking.drag.begin_drag(pointer, (position.x, position.y), docked, tolerance);
        s.interaction.window_drag = true;
    }

    fn move_window_drag(&self) -> Option<(i32, i32)> {
        let natural = self.state.borrow().docking.drag.compute_natural(shell_window::cursor_pos());
        Some(natural)
    }

    fn apply_window_drag(&self, natural: (i32, i32)) {
        let position = self.state.borrow_mut().docking.drag.apply_drag_position(natural);
        if let Some(window) = self.window() {
            window.window().set_position(slint::PhysicalPosition::new(position.0, position.1));
        }
    }

    /// `FlyoutUndockButtonController`.
    fn on_undock_pointer(&self, kind: i32, inside: bool) {
        match kind {
            0 => {
                if !self.is_visible() {
                    return;
                }
                self.begin_window_drag();
                self.state.borrow_mut().undock_gesture = Some(UndockGesture { dragged: false });
            }
            1 => {
                let Some(natural) = self.move_window_drag() else { return };
                let threshold = metrics::DRAG_THRESHOLD * self.scale();
                let undock = {
                    let mut s = self.state.borrow_mut();
                    let crossed = s.docking.drag.exceeds_threshold(natural, threshold);
                    match s.undock_gesture.as_mut() {
                        None => return,
                        Some(gesture) if !gesture.dragged && crossed => {
                            gesture.dragged = true;
                            true
                        }
                        Some(gesture) if !gesture.dragged => return,
                        Some(_) => false,
                    }
                };
                if undock {
                    let change = self.state.borrow_mut().docking.set_undocked_from_drag(self);
                    if let (Some(change), Some(window)) = (change, self.window()) {
                        window.set_undocked(true);
                        let _ = change;
                    }
                }
                self.apply_window_drag(natural);
            }
            2 | 3 => {
                let Some(gesture) = self.state.borrow_mut().undock_gesture.take() else { return };
                self.state.borrow_mut().interaction.window_drag = false;
                let change = if gesture.dragged {
                    self.state.borrow_mut().docking.commit_drag_position(self)
                } else if kind == 2 && inside {
                    self.state.borrow_mut().docking.toggle_undocked(self)
                } else {
                    None
                };
                if let Some(change) = change {
                    self.on_dock_change(change);
                    if change == DockChange::PositionSaved {
                        self.scheduler.rebuild_now();
                    }
                }
                self.scheduler.flush();
            }
            _ => {}
        }
    }

    /// `FlyoutChromeDragController`: drags the undocked flyout by its non-interactive areas.
    fn on_chrome_pointer(&self, kind: i32) {
        match kind {
            0 => {
                if !self.is_visible() || !self.state.borrow().docking.is_undocked() {
                    return;
                }
                self.begin_window_drag();
                self.state.borrow_mut().chrome_drag = true;
            }
            1 => {
                if !self.state.borrow().chrome_drag {
                    return;
                }
                if let Some(natural) = self.move_window_drag() {
                    self.apply_window_drag(natural);
                }
            }
            2 | 3 => {
                if !std::mem::take(&mut self.state.borrow_mut().chrome_drag) {
                    return;
                }
                self.state.borrow_mut().interaction.window_drag = false;
                let change = self.state.borrow_mut().docking.commit_drag_position(self);
                if let Some(change) = change {
                    self.on_dock_change(change);
                }
                self.scheduler.flush();
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- meters

    fn meter_tick(&self) {
        let (fps, rate, ceiling, logarithmic) = {
            let settings = self.settings.get();
            (
                settings.meter_peak_fps.max(1),
                settings.meter_peak_sample_rate.max(1),
                settings.meter_peak_change_ceiling,
                settings.use_logarithmic_volume_scale,
            )
        };
        let steps = (fps / rate).max(1) as u32;
        let max_step = ceiling as f32 / 100.0;
        let mut s = self.state.borrow_mut();
        let state = &mut *s;
        let Some(generation) = state.generation.as_mut() else { return };
        let last_seq = &mut state.last_meter_seq;
        self.audio.meters().read(|frame| {
            if frame.seq == *last_seq {
                return;
            }
            *last_seq = frame.seq;
            for slot in &mut generation.meters {
                let raw = match &slot.app {
                    None => frame.devices.get(&slot.device).copied(),
                    Some(app) => frame.groups.get(&(slot.device.clone(), app.clone())).copied(),
                };
                slot.lerp.on_new_sample(raw.unwrap_or((0.0, 0.0)), steps);
            }
        });
        for (index, slot) in generation.meters.iter_mut().enumerate() {
            slot.lerp.on_render_tick(max_step);
            let display = slot.lerp.display;
            let shown = match slot.owner {
                MeterOwner::Device(_) => display,
                MeterOwner::App(dev, app) => {
                    let level =
                        generation.devices.get(dev).and_then(|d| d.apps.get(app)).map_or(0.0, |a| a.level.shown);
                    (
                        scale::meter_peak_fraction(display.0, level, logarithmic),
                        scale::meter_peak_fraction(display.1, level, logarithmic),
                    )
                }
            };
            if shown != slot.shown {
                slot.shown = shown;
                self.meters_model.set_row_data(index, ui::MeterValue { min: shown.0, max: shown.1 });
            }
        }
    }

    /// Countdown pie frames: per frame with animations, else once a second; only while visible and
    /// a listed device has a pending reconnect.
    fn update_countdown_timer(&self) {
        let (run, animations) = {
            let s = self.state.borrow();
            let pending = s.generation.as_ref().is_some_and(|g| {
                g.devices.iter().any(|d| s.snapshot.device(&d.id).is_some_and(|device| device.bt_pending()))
            });
            (s.visible && s.meters_active && pending, s.animations)
        };
        if !run {
            self.countdown_timer.stop();
            return;
        }
        let interval = if animations {
            COUNTDOWN_ANIMATION_FRAME
        } else {
            Duration::from_millis(metrics::COUNTDOWN_STEPPED_INTERVAL_MS)
        };
        if self.countdown_timer.running() && self.countdown_timer.interval() == interval {
            return;
        }
        let weak = self.this.clone();
        self.countdown_timer.start(TimerMode::Repeated, interval, move || {
            if let Some(inner) = weak.upgrade() {
                inner.countdown_tick();
            }
        });
    }

    fn countdown_tick(&self) {
        let now = tick_count_ms();
        let updates: Vec<(usize, i32)> = {
            let s = self.state.borrow();
            let Some(generation) = s.generation.as_ref() else { return };
            generation
                .devices
                .iter()
                .enumerate()
                .filter_map(|(index, d)| {
                    let deadline = s.snapshot.device(&d.id)?.bt.as_ref()?.pending_deadline_ms?;
                    Some((index, view::pie_step_at(deadline, now)))
                })
                .collect()
        };
        for (index, step) in updates {
            if let Some(mut row) = self.devices_model.row_data(index)
                && row.bt_pie_step != step
            {
                row.bt_pie_step = step;
                self.devices_model.set_row_data(index, row);
            }
        }
    }

    // ---------------------------------------------------------------- row actions

    fn device_id(&self, dev: i32) -> Option<DeviceId> {
        let s = self.state.borrow();
        s.generation.as_ref()?.devices.get(usize::try_from(dev).ok()?).map(|d| d.id.clone())
    }

    fn send_for_device(&self, dev: i32, command: impl FnOnce(DeviceId) -> AudioCommand) {
        if let Some(id) = self.device_id(dev) {
            self.audio.send(command(id));
        }
    }

    fn on_device_mute(&self, dev: i32) {
        let Some(id) = self.device_id(dev) else { return };
        let muted = self.state.borrow().snapshot.device(&id).is_some_and(|d| d.muted);
        self.audio.send(AudioCommand::SetDeviceMute { id, muted: !muted });
    }

    fn on_device_slider_edited(&self, dev: i32, value: f32) {
        let Ok(index) = usize::try_from(dev) else { return };
        let volume = (f64::from(value) / 100.0) as f32;
        let (id, dragging) = {
            let mut s = self.state.borrow_mut();
            let Some(gd) = s.generation.as_mut().and_then(|g| g.devices.get_mut(index)) else { return };
            gd.volume.user_wrote(volume);
            (gd.id.clone(), gd.volume.adjusting)
        };
        self.audio.send(AudioCommand::SetDeviceVolume { id: id.clone(), volume });
        self.patch_device(index, false);
        if !dragging {
            self.audio.send(AudioCommand::PlayDeviceFeedback { id, timing: FeedbackTiming::Debounced });
        }
    }

    fn on_device_slider_drag(&self, dev: i32, started: bool) {
        let Ok(index) = usize::try_from(dev) else { return };
        let id = {
            let mut s = self.state.borrow_mut();
            let state = &mut *s;
            let Some(gd) = state.generation.as_mut().and_then(|g| g.devices.get_mut(index)) else { return };
            if started {
                if gd.volume.adjusting {
                    return;
                }
                gd.volume.begin_adjust();
                state.interaction.slider_drags += 1;
                return;
            }
            if !gd.volume.adjusting {
                return;
            }
            gd.volume.end_adjust();
            state.interaction.slider_drags = state.interaction.slider_drags.saturating_sub(1);
            gd.id.clone()
        };
        self.patch_device(index, false);
        self.scheduler.flush();
        self.audio.send(AudioCommand::PlayDeviceFeedback { id, timing: FeedbackTiming::Immediate });
    }

    fn app_ids(&self, dev: i32, app: i32) -> Option<(usize, usize, DeviceId, AppId)> {
        let (dev, app) = (usize::try_from(dev).ok()?, usize::try_from(app).ok()?);
        let s = self.state.borrow();
        let gd = s.generation.as_ref()?.devices.get(dev)?;
        Some((dev, app, gd.id.clone(), gd.apps.get(app)?.id.clone()))
    }

    fn on_app_mute(&self, dev: i32, app: i32) {
        let Some((_, _, device, app)) = self.app_ids(dev, app) else { return };
        let muted = {
            let s = self.state.borrow();
            s.snapshot.device(&device).and_then(|d| d.groups.iter().find(|g| g.app_id == app)).is_some_and(|g| g.muted)
        };
        self.audio.send(AudioCommand::SetGroupMute { device, app, muted: !muted });
    }

    fn on_app_slider_edited(&self, dev: i32, app: i32, value: f32) {
        let Some((dev, index, device, app)) = self.app_ids(dev, app) else { return };
        let level =
            scale::level_from_slider_percent(f64::from(value), self.settings.get().use_logarithmic_volume_scale);
        let dragging = {
            let mut s = self.state.borrow_mut();
            let Some(ga) =
                s.generation.as_mut().and_then(|g| g.devices.get_mut(dev)).and_then(|d| d.apps.get_mut(index))
            else {
                return;
            };
            ga.level.user_wrote(level);
            ga.level.adjusting
        };
        self.audio.send(AudioCommand::SetGroupVolume { device: device.clone(), app: app.clone(), volume: level });
        self.patch_app_value(dev, index, value);
        if !dragging {
            self.audio.send(AudioCommand::PlayAppFeedback { device, app, timing: FeedbackTiming::Debounced });
        }
    }

    /// The slider position the user set, shown at once (the level round trip may quantize it).
    fn patch_app_value(&self, dev: usize, index: usize, position: f32) {
        let model =
            self.state.borrow().generation.as_ref().and_then(|g| g.devices.get(dev)).map(|d| d.apps_model.clone());
        if let Some(model) = model
            && let Some(mut row) = model.row_data(index)
        {
            row.value = position;
            row.percent = slider::percent_text(f64::from(position)).into();
            model.set_row_data(index, row);
        }
    }

    fn on_app_slider_drag(&self, dev: i32, app: i32, started: bool) {
        let Some((dev, index, device, app)) = self.app_ids(dev, app) else { return };
        {
            let mut s = self.state.borrow_mut();
            let state = &mut *s;
            let Some(ga) =
                state.generation.as_mut().and_then(|g| g.devices.get_mut(dev)).and_then(|d| d.apps.get_mut(index))
            else {
                return;
            };
            if started {
                if !ga.level.adjusting {
                    ga.level.begin_adjust();
                    state.interaction.slider_drags += 1;
                }
                return;
            }
            if !ga.level.adjusting {
                return;
            }
            ga.level.end_adjust();
            state.interaction.slider_drags = state.interaction.slider_drags.saturating_sub(1);
        }
        self.patch_device(dev, true);
        self.scheduler.flush();
        self.audio.send(AudioCommand::PlayAppFeedback { device, app, timing: FeedbackTiming::Immediate });
    }

    fn on_bt_button(&self, dev: i32, mods: Modifiers) {
        let Some(id) = self.device_id(dev) else { return };
        let (disconnected, pending) = {
            let s = self.state.borrow();
            let Some(device) = s.snapshot.device(&id) else { return };
            (device.state.is_disconnected(), device.bt_pending())
        };
        match present::bluetooth_button_action(disconnected, pending, mods) {
            present::BluetoothButtonAction::Connect | present::BluetoothButtonAction::Retry => {
                self.audio.send(AudioCommand::ConnectBluetooth { id });
            }
            present::BluetoothButtonAction::Disconnect => self.audio.send(AudioCommand::DisconnectBluetooth { id }),
            present::BluetoothButtonAction::None => {}
        }
    }

    fn on_eq_button(&self, dev: i32, mods: Modifiers) {
        let Some(id) = self.device_id(dev) else { return };
        let state = self.state.borrow().snapshot.device(&id).map(|d| d.eq_apo).unwrap_or_default();
        match present::equalizer_button_action(mods, state) {
            present::EqualizerButtonAction::OpenEditor => self.audio.send(AudioCommand::OpenEqualizerApoEditor { id }),
            present::EqualizerButtonAction::ShowUnavailableDialog => self.show_eq_unavailable_dialog(),
            present::EqualizerButtonAction::Toggle => self.audio.send(AudioCommand::ToggleEqualizerApo { id }),
        }
    }

    fn on_listen_button(&self, dev: i32, mods: Modifiers) {
        let Some(id) = self.device_id(dev) else { return };
        if mods.control {
            self.audio.send(AudioCommand::SetListenTarget { id, target: None });
        } else {
            let listening = self.state.borrow().snapshot.device(&id).is_some_and(|d| d.listen_enabled);
            self.audio.send(AudioCommand::SetListenEnabled { id, enabled: !listening });
        }
    }

    fn on_default_button(&self, dev: i32, mods: Modifiers) {
        let Some(id) = self.device_id(dev) else { return };
        let active = self.state.borrow().snapshot.device(&id).is_some_and(|d| d.state.is_active());
        let command = match present::default_button_action(mods, active) {
            present::DefaultButtonAction::SetDefaultComms => AudioCommand::SetDefaultComms { id },
            present::DefaultButtonAction::SetEnabled(enabled) => AudioCommand::SetEnabled { id, enabled },
            present::DefaultButtonAction::SetDefault => AudioCommand::SetDefault { id },
        };
        self.audio.send(command);
    }

    fn toggle_drawer(&self, dev: i32) {
        let Some(id) = self.device_id(dev) else { return };
        let has_groups = {
            let s = self.state.borrow();
            s.generation
                .as_ref()
                .and_then(|g| g.structure.iter().find(|d| d.id == id))
                .is_some_and(|d| !d.apps.is_empty())
        };
        if !has_groups {
            return;
        }
        let default_expanded = self.settings.get().default_app_drawer_expanded;
        let expanded = self.settings.devices().is_drawer_expanded(&id, default_expanded);
        self.settings.update_devices(|devices| {
            devices.get_or_create(&id, default_expanded).is_app_drawer_expanded = !expanded;
        });
        self.scheduler.rebuild_now();
        self.queue_position_near_tray();
    }

    // ---------------------------------------------------------------- header actions

    fn on_settings_button(&self) {
        self.arm_suppression();
        self.host.open_settings();
    }

    fn toggle_disabled_devices(&self) {
        let show = !present::shows_disabled_devices(&self.settings.get());
        self.settings.update(|s| {
            s.show_disabled_playback_devices = show;
            s.show_disabled_recording_devices = show;
        });
    }

    fn on_bluetooth_radio_button(&self, mods: Modifiers) {
        let settings = self.settings.get();
        if !settings.show_bluetooth_radio_button_in_flyout_header {
            return;
        }
        let gesture = settings.flyout_bluetooth_radio_button_click_gesture;
        drop(settings);
        let (radio, in_flight) = {
            let s = self.state.borrow();
            let in_flight =
                s.radio_toggle.is_some_and(|(_, since)| since.elapsed() < BLUETOOTH_RADIO_STATE_CHANGE_TIMEOUT);
            (s.snapshot.radio, in_flight)
        };
        let can_toggle = !in_flight && radio != RadioState::Unavailable;
        match present::bluetooth_radio_button_action(gesture, mods, can_toggle) {
            present::BluetoothRadioButtonAction::ToggleRadio => {
                self.state.borrow_mut().radio_toggle = Some((radio, Instant::now()));
                self.audio.send(AudioCommand::ToggleBluetoothRadio);
            }
            // Like the sound settings button: the flyout auto-hides when the panel takes focus.
            present::BluetoothRadioButtonAction::OpenQuickSettings => links::open_bluetooth_quick_settings(),
            present::BluetoothRadioButtonAction::None => {}
        }
    }

    // ---------------------------------------------------------------- percent editor

    fn on_percent_pressed(&self, dev: i32, app: i32, button: i32, double: bool) -> bool {
        if double {
            return self.open_percent_editor(dev, app);
        }
        let now = Instant::now();
        let double_click = Duration::from_millis(u64::from(shell_window::double_click_time_ms()));
        let previous = self.state.borrow_mut().last_percent_press.replace((
            usize::try_from(dev).unwrap_or(usize::MAX),
            app,
            button,
            now,
        ));
        let is_double = previous.is_some_and(|(d, a, b, at)| {
            d == usize::try_from(dev).unwrap_or(usize::MAX)
                && a == app
                && b == button
                && now.duration_since(at) <= double_click
        });
        if is_double {
            self.state.borrow_mut().last_percent_press = None;
            return self.open_percent_editor(dev, app);
        }
        false
    }

    fn open_percent_editor(&self, dev: i32, app: i32) -> bool {
        let Ok(index) = usize::try_from(dev) else { return false };
        let app_index = usize::try_from(app).ok();
        let Some(window) = self.window() else { return false };
        let text = if let Some(app_index) = app_index {
            let model = self
                .state
                .borrow()
                .generation
                .as_ref()
                .and_then(|g| g.devices.get(index))
                .map(|d| d.apps_model.clone());
            match model.and_then(|m| m.row_data(app_index)) {
                Some(row) => row.percent,
                None => return false,
            }
        } else {
            match self.devices_model.row_data(index) {
                Some(row) => row.percent,
                None => return false,
            }
        };
        {
            let mut s = self.state.borrow_mut();
            s.percent_editor = Some((index, app_index));
            s.interaction.percent_editors = 1;
        }
        let state = window.global::<ui::FlyoutState>();
        state.set_percent_text(text);
        state.set_percent_app(app);
        state.set_percent_device(dev);
        true
    }

    fn close_percent_editor(&self) {
        let was_open = {
            let mut s = self.state.borrow_mut();
            s.interaction.percent_editors = 0;
            s.percent_editor.take().is_some()
        };
        if let Some(window) = self.window() {
            let state = window.global::<ui::FlyoutState>();
            state.set_percent_device(-1);
            state.set_percent_app(-1);
            window.invoke_focus_root();
        }
        if was_open {
            self.scheduler.flush();
        }
    }

    /// `Commit` + `ApplyPercentText`.
    fn commit_percent_editor(&self, text: &str) {
        let Some((dev, app)) = self.state.borrow().percent_editor else { return };
        self.close_percent_editor();
        match app {
            None => {
                let Some(row) = self.devices_model.row_data(dev) else { return };
                let Some((value, changed)) =
                    slider::apply_percent_text(text, f64::from(row.value), &self.decimal_separator)
                else {
                    return;
                };
                if changed {
                    self.on_device_slider_edited(dev as i32, value as f32);
                    if let Some(id) = self.device_id(dev as i32) {
                        self.audio.send(AudioCommand::PlayDeviceFeedback { id, timing: FeedbackTiming::Immediate });
                    }
                }
            }
            Some(app) => {
                let model = self
                    .state
                    .borrow()
                    .generation
                    .as_ref()
                    .and_then(|g| g.devices.get(dev))
                    .map(|d| d.apps_model.clone());
                let Some(row) = model.and_then(|m| m.row_data(app)) else { return };
                let Some((value, changed)) =
                    slider::apply_percent_text(text, f64::from(row.value), &self.decimal_separator)
                else {
                    return;
                };
                if changed {
                    self.on_app_slider_edited(dev as i32, app as i32, value as f32);
                    if let Some((_, _, device, app)) = self.app_ids(dev as i32, app as i32) {
                        self.audio.send(AudioCommand::PlayAppFeedback {
                            device,
                            app,
                            timing: FeedbackTiming::Immediate,
                        });
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------- rename

    fn begin_rename(&self, dev: i32) {
        let Some(id) = self.device_id(dev) else { return };
        let Some(name) = self.state.borrow().snapshot.device(&id).map(|d| d.name.clone()) else { return };
        let Some(window) = self.window() else { return };
        {
            let mut s = self.state.borrow_mut();
            s.rename_device = usize::try_from(dev).ok();
            s.interaction.rename_active = true;
        }
        let state = window.global::<ui::FlyoutState>();
        state.set_rename_text(name.into());
        state.set_rename_device(dev);
    }

    /// Commit (`Some(text)`) or cancel (`None`) the rename editor.
    fn commit_rename(&self, text: Option<&str>) {
        let Some(dev) = self.state.borrow_mut().rename_device.take() else { return };
        self.state.borrow_mut().interaction.rename_active = false;
        if let Some(window) = self.window() {
            window.global::<ui::FlyoutState>().set_rename_device(-1);
            window.invoke_focus_root();
        }
        if let Some(text) = text
            && let Some(id) = self.device_id(dev as i32)
        {
            // `SetCustomFriendlyName`: blank clears the override; an unchanged name is a no-op.
            let current = self.state.borrow().snapshot.device(&id).map(|d| d.name.clone()).unwrap_or_default();
            let trimmed = text.trim();
            if trimmed.is_empty() {
                self.audio.send(AudioCommand::Rename { id, name: None });
            } else if trimmed != current {
                self.audio.send(AudioCommand::Rename { id, name: Some(trimmed.to_owned()) });
            }
        }
        self.scheduler.flush();
    }

    // ---------------------------------------------------------------- menus

    /// Screen rectangle (physical px) of a window-relative logical rectangle.
    fn screen_rect(&self, rect: (f32, f32, f32, f32)) -> Rect {
        let Some(window) = self.window() else { return Rect::default() };
        let origin = window.window().position();
        let scale = self.scale();
        let to = |v: f32| (f64::from(v) * scale).round_ties_even() as i32;
        Rect {
            left: origin.x + to(rect.0),
            top: origin.y + to(rect.1),
            right: origin.x + to(rect.0 + rect.2),
            bottom: origin.y + to(rect.1 + rect.3),
        }
    }

    fn show_format_menu(&self, dev: i32, anchor: (f32, f32, f32, f32)) {
        let Some(id) = self.device_id(dev) else { return };
        let active = self.state.borrow().snapshot.device(&id).is_some_and(|d| d.state.is_active());
        let rect = self.screen_rect(anchor);
        let generation = self.state.borrow().visibility_generation;
        let reply_id = id.clone();
        self.audio.send(AudioCommand::QueryFormats {
            id,
            reply: Box::new(move |query: FormatQuery| {
                let posted = slint::invoke_from_event_loop(move || {
                    with_current(|inner| inner.open_format_menu(&reply_id, query, active, rect, generation));
                });
                if let Err(error) = posted {
                    log::warn!("VolumeFlyout: format menu reply lost: {error}");
                }
            }),
        });
    }

    /// `ShowDefaultFormatMenu`.
    fn open_format_menu(&self, id: &DeviceId, query: FormatQuery, active: bool, anchor: Rect, generation: u64) {
        if !self.is_visible() || self.state.borrow().visibility_generation != generation {
            return;
        }
        let current = query.current;
        let mut formats = if active { query.supported } else { Vec::new() };
        if formats.is_empty()
            && let Some(current) = current
        {
            formats = present::fallback_formats(current);
        }
        if formats.is_empty() {
            return;
        }
        let items: Vec<MenuItem> = formats
            .iter()
            .enumerate()
            .map(|(index, format)| {
                let item = MenuItem::new(index as i32, vtadn_model::text::format_text(format));
                if current == Some(*format) { item.with_glyph(present::glyphs::CIRCLE) } else { item }
            })
            .collect();
        let max_height = (formats.len() > metrics::FORMAT_MENU_MAX_VISIBLE_ITEMS).then_some(
            (metrics::FORMAT_MENU_MAX_VISIBLE_ITEMS as f64 * metrics::FORMAT_MENU_ITEM_HEIGHT
                + metrics::FORMAT_MENU_PADDING_RESERVE) as f32,
        );
        let audio = self.audio.clone();
        let id = id.clone();
        self.show_menu(items, anchor, max_height, move |index| {
            if let Some(format) = usize::try_from(index).ok().and_then(|i| formats.get(i)) {
                audio.send(AudioCommand::SetFormat { id, format: *format });
            }
        });
    }

    /// `ShowListenTargetMenu`.
    fn show_listen_menu(&self, dev: i32, anchor: (f32, f32, f32, f32)) {
        let Some(id) = self.device_id(dev) else { return };
        let rect = self.screen_rect(anchor);
        let (current, mut targets) = {
            let s = self.state.borrow();
            let Some(device) = s.snapshot.device(&id) else { return };
            let targets: Vec<(DeviceId, String)> = s
                .snapshot
                .devices
                .iter()
                .filter(|d| d.flow == Flow::Render && d.state.is_active())
                .map(|d| (d.id.clone(), d.name.clone()))
                .collect();
            (device.listen_target.clone(), targets)
        };
        targets.sort_by_cached_key(|(_, name)| name.to_lowercase());
        let mut items = vec![{
            let item = MenuItem::new(0, present::LISTEN_MENU_DEFAULT_PLAYBACK);
            if current.is_none() { item.with_glyph(present::glyphs::CIRCLE) } else { item }
        }];
        for (index, (target, name)) in targets.iter().enumerate() {
            let item = MenuItem::new(index as i32 + 1, name.as_str());
            items.push(if current.as_ref() == Some(target) { item.with_glyph(present::glyphs::CIRCLE) } else { item });
        }
        let audio = self.audio.clone();
        self.show_menu(items, rect, None, move |index| {
            let target = if index == 0 {
                None
            } else {
                usize::try_from(index - 1).ok().and_then(|i| targets.get(i)).map(|t| t.0.clone())
            };
            if index == 0 || target.is_some() {
                audio.send(AudioCommand::SetListenTarget { id, target });
            }
        });
    }

    /// `ShowFlyoutMenu`: holds rebuilds and auto-hide while open; on close re-activates the flyout,
    /// or re-evaluates the focus group when the menu lost activation.
    fn show_menu(
        &self,
        items: Vec<MenuItem>,
        anchor: Rect,
        max_height: Option<f32>,
        on_activate: impl FnOnce(i32) + 'static,
    ) {
        self.close_menu();
        self.state.borrow_mut().interaction.menu_open = true;
        let options = MenuOptions {
            font_size: self.settings.get().common.context_menu_font_size,
            max_height,
            ..MenuOptions::default()
        };
        let weak = self.this.clone();
        self.menu.show(items, MenuPlacement::ForRect(anchor), options, on_activate, move |reason| {
            let Some(inner) = weak.upgrade() else { return };
            inner.state.borrow_mut().interaction.menu_open = false;
            if reason == MenuCloseReason::Deactivated {
                inner.child_closed_from_deactivation();
            } else if inner.is_visible()
                && let Some(hwnd) = inner.hwnd()
            {
                shell_window::activate(hwnd);
            }
            inner.scheduler.flush();
        });
    }

    fn close_menu(&self) {
        if self.state.borrow().interaction.menu_open {
            self.menu.close();
        }
    }

    // ---------------------------------------------------------------- dialog

    /// `ShowEqualizerAPONotAvailableDialog`: owned confirmation; confirming opens the installer link.
    fn show_eq_unavailable_dialog(&self) {
        self.close_dialog();
        let dialog = match backend::with_window_kind(WindowKind::Flyout, ui::ConfirmDialog::new) {
            Ok(dialog) => dialog,
            Err(error) => {
                log::error!("VolumeFlyout: cannot create the Equalizer APO dialog: {error}");
                return;
            }
        };
        if let Some(palette) = self.palette.borrow().as_ref() {
            tadn_common::apply_palette!(dialog.global::<ui::Theme>(), palette);
        }
        dialog.set_heading(present::EQ_UNAVAILABLE_TITLE.into());
        dialog.set_body(present::EQ_UNAVAILABLE_BODY.into());
        dialog.set_confirm_text(present::EQ_UNAVAILABLE_DOWNLOAD.into());
        dialog.set_cancel_text(present::CANCEL.into());
        let weak = self.this.clone();
        dialog.on_confirmed(move || {
            if let Some(inner) = weak.upgrade() {
                inner.close_dialog();
                if let Err(error) = tadn_common::shell::open_uri(metrics::EQUALIZER_APO_LATEST_INSTALLER_URL) {
                    log::warn!("VolumeFlyoutWindow.ShowEqualizerAPONotAvailableDialog: {error}");
                }
                inner.child_closed_from_deactivation();
            }
        });
        let weak = self.this.clone();
        dialog.on_cancelled(move || {
            if let Some(inner) = weak.upgrade() {
                inner.close_dialog();
                inner.child_closed_from_deactivation();
            }
        });
        // CenterOwner.
        if let Some(window) = self.window() {
            let origin = window.window().position();
            let scale = self.scale();
            let flyout_height = f64::from(window.get_content_height());
            let dialog_height = f64::from(dialog.window().size().height) / scale;
            let x = origin.x + ((metrics::WINDOW_WIDTH - 300.0) / 2.0 * scale).round_ties_even() as i32;
            let y = origin.y + ((flyout_height - dialog_height.max(150.0)) / 2.0 * scale).round_ties_even() as i32;
            dialog.window().set_position(slint::PhysicalPosition::new(x, y));
        }
        self.state.borrow_mut().interaction.dialog_open = true;
        let shown = backend::with_window_kind(WindowKind::Flyout, || dialog.show());
        if let Err(error) = shown {
            log::error!("VolumeFlyout: dialog show failed: {error}");
            self.state.borrow_mut().interaction.dialog_open = false;
            return;
        }
        shell_window::when_created(dialog.window(), shell_window::activate);
        *self.dialog.borrow_mut() = Some(dialog);
    }

    fn close_dialog(&self) {
        let dialog = self.dialog.borrow_mut().take();
        if let Some(dialog) = dialog {
            if let Err(error) = dialog.hide() {
                log::warn!("VolumeFlyout: dialog hide failed: {error}");
            }
            self.state.borrow_mut().interaction.dialog_open = false;
            self.scheduler.flush();
        }
    }
}

impl RebuildHooks for Inner {
    fn can_show_rebuild(&self) -> bool {
        let s = self.state.borrow();
        self.window.borrow().is_some() && (s.visible || s.generation.is_none())
    }

    fn is_held(&self) -> bool {
        self.state.borrow().interaction.holds_rebuild()
    }

    fn rebuild(&self) -> Result<(), String> {
        self.build_generation()?;
        self.update_countdown_timer();
        Ok(())
    }

    fn committed(&self) {
        self.queue_position_near_tray();
    }
}

impl DockHost for Inner {
    fn dock_settings(&self) -> DockSettings {
        dock_settings_of(&self.settings.get())
    }

    fn save_dock_settings(&self, dock: DockSettings) {
        self.settings.set_state(|s| {
            s.flyout_undocked = dock.undocked;
            s.flyout_has_saved_position = dock.has_saved_position;
            s.flyout_left = dock.left;
            s.flyout_top = dock.top;
        });
    }

    fn current_position(&self) -> (i32, i32) {
        self.window().map_or((0, 0), |w| {
            let p = w.window().position();
            (p.x, p.y)
        })
    }

    fn set_position(&self, position: (i32, i32)) {
        if let Some(window) = self.window() {
            window.window().set_position(slint::PhysicalPosition::new(position.0, position.1));
        }
    }

    fn docked_position(&self) -> (i32, i32) {
        let icon = self.host.tray_icon_rect();
        let work_area = self.work_area();
        let pad = placement::edge_padding_px(self.scale());
        shell_placement::docked_popup_position(icon, work_area, self.pixel_size(), pad)
    }

    fn clamp_saved_position(&self, position: (i32, i32)) -> (i32, i32) {
        let work_area = shell_placement::work_area_at(position.0, position.1).unwrap_or_else(|| self.work_area());
        placement::clamp_saved_position(
            position,
            self.pixel_size(),
            work_area,
            placement::edge_padding_px(self.scale()),
        )
    }

    fn snap_tolerance(&self) -> i32 {
        placement::snap_tolerance(self.work_area())
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.scheduler.dispose();
        self.meter_timer.stop();
        self.countdown_timer.stop();
        self.eviction_timer.stop();
    }
}

fn dock_settings_of(settings: &AppSettings) -> DockSettings {
    DockSettings {
        allow_undock: settings.allow_flyout_undock,
        restore_undocked_on_startup: settings.restore_flyout_undocked_on_startup,
        clamp_undocked_to_screen: settings.clamp_undocked_flyout_to_screen,
        undocked: settings.flyout_undocked,
        has_saved_position: settings.flyout_has_saved_position,
        left: settings.flyout_left,
        top: settings.flyout_top,
    }
}

fn meter_slot(old: Option<&Generation>, owner: MeterOwner, device: &DeviceId, app: Option<&AppId>) -> MeterSlot {
    let previous = old.and_then(|g| g.meters.iter().find(|m| m.device == *device && m.app.as_ref() == app));
    MeterSlot {
        owner,
        device: device.clone(),
        app: app.cloned(),
        lerp: previous.map_or_else(MeterLerp::default, |m| m.lerp),
        shown: previous.map_or((0.0, 0.0), |m| m.shown),
    }
}

fn modifiers(mods: ui::ClickModifiers) -> Modifiers {
    Modifiers { control: mods.control, alt: mods.alt, shift: mods.shift }
}

/// `GetTickCount64` (the clock of `BtState::pending_deadline_ms`).
fn tick_count_ms() -> u64 {
    // SAFETY: no arguments, no preconditions.
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}
