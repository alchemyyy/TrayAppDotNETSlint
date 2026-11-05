//! The settings window (port of `UI/Settings/*` and the Common settings shell:
//! `SettingsWindowCommon`, `SettingsWindowCommon.Sections`, `Controls/SettingsUI`,
//! `SettingsWindowSearch`, `Settings/SettingsSearchView`, `Settings/TrayAppDotNETHotkeysSection`,
//! `Settings/TrayAppDotNETAboutPage`, `Settings/TrayAppDotNETGeneralSettingsSection`,
//! `Settings/TrayAppDotNETRenderingSettingsSection`, `Controls/ColorPickerWindow`). No
//! installation or update cards: this build has no installer or updater.
//!
//! Pages are flat lists of rows built from the live settings (`pages.rs`); every settings change
//! rebuilds the shown list and diffs it into the Slint model, so cards that depend on other
//! settings ("only shown while ...") appear and disappear like the C# `Maybe(...)` cards.

mod color_picker;
mod hotkeys;
mod items;
mod pages;
mod search;
mod strings;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, Timer, VecModel};
use tadn_common::hotkey::HotkeyAction;
use tadn_common::settings::RenderingBackend;
use tadn_common::theme::{Argb, Palette, ThemeFile};
use tadn_shell::hotkeys::HotkeyRegistration;
use vtadn_model::settings::AppSettings;
use vtadn_model::settings_handle::{SettingsHandle, Subscription};
use windows::Win32::Foundation::HWND;

use crate::color_picker::{ColorPicker, PickerSink};
use crate::hotkeys::HotkeyUi;
use crate::items::{Action, ColorTarget, Item};
use crate::pages::{BuildContext, PAGES};
use crate::strings as s;

/// Slint-generated components and globals.
pub mod ui {
    slint::include_modules!();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    General,
    Flyout,
    Devices,
    AppDrawers,
    TrayIcon,
    Hotkeys,
    Theme,
    About,
}

/// What the settings window needs from the app.
pub trait SettingsHost {
    fn palette(&self) -> Palette;
    /// Run on startup: whether the Startup shortcut exists.
    fn startup_enabled(&self) -> bool;
    fn set_startup_enabled(&self, enabled: bool) -> Result<(), String>;
    /// Where the Startup shortcut points (shown on the card).
    fn startup_target(&self) -> Option<String>;
    /// Re-registers `settings.common.hotkeys`; one result per binding, in order. A result list
    /// whose length differs from the bindings' means the hotkey service is unavailable.
    fn register_hotkeys(&self) -> Vec<HotkeyRegistration>;
    /// Restart now (rendering backend / language change).
    fn restart(&self);
    /// The data folder (About > Open settings folder).
    fn data_folder(&self) -> PathBuf;
    /// Settings window activated or clicked: keep the flyout open alongside it.
    fn settings_activated(&self);
    /// Settings window closed or minimized.
    fn settings_closed(&self);
    /// Settings window lost focus without being minimized or closed.
    fn settings_deactivated(&self) {}
    /// The loaded `theme.xml` (color cards fall back to its foreground/background).
    fn theme(&self) -> ThemeFile {
        ThemeFile::default()
    }
    /// The last registration result of `settings.common.hotkeys` without re-registering (shown
    /// when the Hotkeys page opens). Same contract as [`SettingsHost::register_hotkeys`].
    fn hotkey_results(&self) -> Vec<HotkeyRegistration> {
        self.register_hotkeys()
    }
}

/// `SettingsWindowCommon.SidebarWidth` default and limits (DIPs).
const SIDEBAR_DEFAULT_WIDTH: f64 = 230.0;
const SIDEBAR_MIN_WIDTH: f64 = 140.0;
const SIDEBAR_MAX_WIDTH: f64 = 520.0;
/// Default window size (`SettingsWindowCommon.axaml` Standard profile), DIPs.
const WINDOW_WIDTH: f32 = 960.0;
const WINDOW_HEIGHT: f32 = 670.0;
/// How long cards reached from a search result stay highlighted. The C# window has no highlight;
/// this is the Slint port's navigation aid.
const SEARCH_HIGHLIGHT_DURATION: Duration = Duration::from_millis(1500);

/// `ResolvePersistedWidth`: 0, negative or NaN mean the default; then clamp.
fn resolve_sidebar_width(persisted: f64) -> f64 {
    let width = if persisted.is_finite() && persisted > 0.0 { persisted } else { SIDEBAR_DEFAULT_WIDTH };
    width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)
}

type ConfirmHandler = Box<dyn FnOnce(&Rc<Inner>, bool)>;

struct OpenPicker {
    target: ColorTarget,
    /// Variant pickers: the light (true) or dark (false) side. Single-color pickers: None.
    variant: Option<bool>,
    picker: Rc<ColorPicker>,
}

struct Inner {
    settings: SettingsHandle,
    host: Rc<dyn SettingsHost>,
    window: ui::SettingsWindow,
    model: Rc<VecModel<ui::SettingsItem>>,
    /// Rows currently shown (parallel to `model`).
    items: RefCell<Vec<Item>>,
    page: Cell<SettingsPage>,
    /// Trimmed search query; empty = showing `page`.
    query: RefCell<String>,
    hotkeys: RefCell<HotkeyUi>,
    palette: RefCell<Palette>,
    theme: ThemeFile,
    startup: RefCell<(bool, Option<String>)>,
    pickers: RefCell<Vec<OpenPicker>>,
    confirm: RefCell<Option<ConfirmHandler>>,
    scroll_offsets: RefCell<HashMap<SettingsPage, f32>>,
    highlight: RefCell<Vec<String>>,
    highlight_timer: Timer,
    refresh_pending: Cell<bool>,
    subscription: RefCell<Option<Subscription>>,
}

pub struct SettingsWindow {
    inner: Rc<Inner>,
}

impl SettingsWindow {
    pub fn new(settings: SettingsHandle, host: Rc<dyn SettingsHost>) -> Result<Self, slint::PlatformError> {
        let window = ui::SettingsWindow::new()?;
        let palette = host.palette();
        tadn_common::apply_palette!(window.global::<ui::Theme>(), palette);
        let theme = host.theme();
        let startup = (host.startup_enabled(), host.startup_target());
        let model = Rc::new(VecModel::default());
        window.set_items(ModelRc::from(model.clone()));

        let inner = Rc::new(Inner {
            settings: settings.clone(),
            host: host.clone(),
            window,
            model,
            items: RefCell::new(Vec::new()),
            page: Cell::new(SettingsPage::General),
            query: RefCell::new(String::new()),
            hotkeys: RefCell::new(HotkeyUi::new()),
            palette: RefCell::new(palette),
            theme,
            startup: RefCell::new(startup),
            pickers: RefCell::new(Vec::new()),
            confirm: RefCell::new(None),
            scroll_offsets: RefCell::new(HashMap::new()),
            highlight: RefCell::new(Vec::new()),
            highlight_timer: Timer::default(),
            refresh_pending: Cell::new(false),
            subscription: RefCell::new(None),
        });
        inner.init_shell();
        {
            let bindings = settings.get().common.hotkeys.clone();
            let results = host.hotkey_results();
            inner.hotkeys.borrow_mut().set_results(&bindings, results);
        }
        Inner::wire(&inner);
        let weak = Rc::downgrade(&inner);
        let subscription = settings.subscribe(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.on_settings_changed();
            }
        });
        *inner.subscription.borrow_mut() = Some(subscription);
        inner.refresh();
        Ok(Self { inner })
    }

    /// Shows (or brings forward) the window, optionally on `page`.
    pub fn show(&self, page: Option<SettingsPage>) {
        let inner = &self.inner;
        if let Some(page) = page {
            inner.navigate(page, Vec::new());
        }
        let window = inner.window.window();
        // ShowAtDefaultPositionAndActivate: center on the primary monitor's work area.
        let monitor = tadn_shell::placement::monitor_from_point(0, 0);
        let scale = monitor.scale.max(0.5);
        let width = if window.is_visible() { window.size().width as i32 } else { (WINDOW_WIDTH * scale).ceil() as i32 };
        let height =
            if window.is_visible() { window.size().height as i32 } else { (WINDOW_HEIGHT * scale).ceil() as i32 };
        let area = monitor.work_area;
        window.set_position(slint::PhysicalPosition::new(
            area.left + (area.width() - width) / 2,
            area.top + (area.height() - height) / 2,
        ));
        if window.is_visible() {
            if let Some(hwnd) = tadn_shell::window::hwnd(window) {
                tadn_shell::window::activate(hwnd);
            }
        } else {
            if let Err(error) = inner.window.show() {
                log::error!("settings window show failed: {error}");
                return;
            }
            let weak = Rc::downgrade(inner);
            tadn_shell::window::when_created(window, move |hwnd| {
                if let Some(inner) = weak.upgrade() {
                    inner.apply_native_chrome(hwnd);
                    tadn_shell::window::activate(hwnd);
                }
            });
        }
        inner.host.settings_activated();
    }

    pub fn hide(&self) {
        self.inner.on_closed();
        if let Err(error) = self.inner.window.hide() {
            log::warn!("settings window hide failed: {error}");
        }
    }

    pub fn is_visible(&self) -> bool {
        self.inner.window.window().is_visible()
    }

    pub fn apply_palette(&self, palette: &Palette) {
        self.inner.apply_palette(palette);
    }

    pub fn hwnd(&self) -> Option<HWND> {
        tadn_shell::window::hwnd(self.inner.window.window())
    }
}

impl Inner {
    fn init_shell(&self) {
        let w = &self.window;
        w.set_window_title(s::SettingsWindow_Title.into());
        w.set_header_text(s::SettingsWindow_Header.into());
        w.set_search_placeholder(s::SettingsWindow_SearchPlaceholder.into());
        w.set_menu_scroll_here(s::ScrollBarMenu_ScrollHere.into());
        w.set_menu_top(s::ScrollBarMenu_Top.into());
        w.set_menu_bottom(s::ScrollBarMenu_Bottom.into());
        w.set_menu_page_up(s::ScrollBarMenu_PageUp.into());
        w.set_menu_page_down(s::ScrollBarMenu_PageDown.into());
        w.set_menu_scroll_up(s::ScrollBarMenu_ScrollUp.into());
        w.set_menu_scroll_down(s::ScrollBarMenu_ScrollDown.into());
        let nav: Vec<ui::NavEntry> = PAGES
            .iter()
            .map(|page| ui::NavEntry { label: pages::page_label(*page).into(), glyph: pages::page_glyph(*page).into() })
            .collect();
        w.set_nav_items(items::model(nav));
        w.set_sidebar_width(resolve_sidebar_width(self.settings.get().common.settings_sidebar_width) as f32);
    }

    fn wire(this: &Rc<Self>) {
        let w = &this.window;
        macro_rules! on {
            ($setter:ident, |$inner:ident $(, $arg:ident : $ty:ty)*| $body:expr) => {{
                let weak: Weak<Inner> = Rc::downgrade(this);
                w.$setter(move |$($arg: $ty),*| {
                    if let Some($inner) = weak.upgrade() {
                        $body
                    }
                });
            }};
        }
        on!(on_nav_activated, |inner, index: i32| inner.nav_activated(index, false));
        on!(on_nav_open, |inner, index: i32| inner.nav_activated(index, true));
        on!(on_search_edited, |inner, text: slint::SharedString| inner.search_edited(&text));
        on!(on_search_enter, |inner| inner.focus_first_control());
        on!(on_sidebar_resized, |inner, width: f32| inner.sidebar_resized(f64::from(width)));
        on!(on_sidebar_reset, |inner| inner.sidebar_reset());
        on!(on_confirm_finished, |inner, result: bool| inner.confirm_finished(result));
        on!(on_toggled, |inner, row: i32, value: bool| inner.toggled(row, value));
        on!(on_number_committed, |inner, row: i32, value: i32| inner.number_committed(row, value));
        on!(on_combo_selected, |inner, row: i32, index: i32| inner.combo_selected(row, index));
        on!(on_pair_toggled, |inner, row: i32, right: bool, value: bool| inner.pair_toggled(row, right, value));
        on!(on_color_clicked, |inner, row: i32| inner.color_clicked(row));
        on!(on_color_reset, |inner, row: i32| inner.color_reset(row));
        on!(on_button_clicked, |inner, row: i32| inner.button_clicked(row));
        on!(on_link_clicked, |inner, row: i32| inner.link_clicked(row));
        on!(on_hotkey_modifiers, |inner, row: i32, index: i32| inner.hotkey_modifiers(row, index));
        on!(on_hotkey_add, |inner, row: i32| inner.hotkey_add(row));
        on!(on_hotkey_delete, |inner, row: i32, entry: i32| inner.hotkey_delete(row, entry));
        {
            let weak = Rc::downgrade(this);
            w.on_hotkey_key(move |row, text| weak.upgrade().is_some_and(|inner| inner.hotkey_key(row, &text)));
        }
        {
            let weak = Rc::downgrade(this);
            w.window().on_close_requested(move || {
                if let Some(inner) = weak.upgrade() {
                    inner.on_closed();
                }
                slint::CloseRequestResponse::HideWindow
            });
        }
        {
            let weak = Rc::downgrade(this);
            tadn_shell::window::on_focus_changed(w.window(), move |focused| {
                let Some(inner) = weak.upgrade() else { return };
                if focused {
                    inner.host.settings_activated();
                } else if tadn_shell::window::hwnd(inner.window.window()).is_some_and(tadn_shell::window::is_minimized)
                {
                    inner.host.settings_closed();
                } else {
                    inner.host.settings_deactivated();
                }
            });
        }
    }

    // --- Shell state --------------------------------------------------------------------------

    fn apply_native_chrome(&self, hwnd: HWND) {
        let palette = self.palette.borrow();
        tadn_shell::window::set_dark_title_bar(hwnd, !palette.is_light);
        tadn_shell::window::set_rounded_corners(hwnd, palette.rounded_corners);
    }

    fn apply_palette(self: &Rc<Self>, palette: &Palette) {
        let previous_light = {
            let mut current = self.palette.borrow_mut();
            let previous = current.is_light;
            *current = palette.clone();
            previous
        };
        tadn_common::apply_palette!(self.window.global::<ui::Theme>(), palette);
        if let Some(hwnd) = tadn_shell::window::hwnd(self.window.window()) {
            self.apply_native_chrome(hwnd);
        }
        if previous_light != palette.is_light {
            // VariantColorCard shows the swatch of the effective mode.
            self.schedule_refresh();
        }
    }

    fn on_settings_changed(self: &Rc<Self>) {
        let palette = self.host.palette();
        if *self.palette.borrow() != palette {
            self.apply_palette(&palette);
        }
        self.schedule_refresh();
    }

    /// Coalesces rebuilds onto the next event-loop turn (edits arrive inside Slint callbacks).
    fn schedule_refresh(self: &Rc<Self>) {
        if self.refresh_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        Timer::single_shot(Duration::ZERO, move || {
            if let Some(inner) = weak.upgrade() {
                inner.refresh_pending.set(false);
                inner.refresh();
            }
        });
    }

    fn on_closed(self: &Rc<Self>) {
        // Pickers close with the window and keep what they had.
        let pickers: Vec<Rc<ColorPicker>> = self.pickers.borrow_mut().drain(..).map(|p| p.picker).collect();
        for picker in pickers {
            picker.close();
        }
        if let Some(handler) = self.confirm.borrow_mut().take() {
            self.window.set_confirm_visible(false);
            handler(self, false);
        }
        // A reopened window starts fresh on its default page (C# builds a new window).
        self.scroll_offsets.borrow_mut().clear();
        self.query.borrow_mut().clear();
        self.window.set_search_text("".into());
        self.page.set(SettingsPage::General);
        self.hotkeys.borrow_mut().reset_rows();
        self.highlight.borrow_mut().clear();
        self.window.set_scroll_y(0.0);
        self.refresh();
        self.host.settings_closed();
    }

    /// Rebuilds the shown rows and diffs them into the model.
    fn refresh(&self) {
        let settings = self.settings.get().clone();
        let hotkeys = self.hotkeys.borrow();
        let startup = self.startup.borrow().clone();
        let ctx = BuildContext {
            settings: &settings,
            is_light: self.palette.borrow().is_light,
            theme: &self.theme,
            startup_enabled: startup.0,
            startup_target: startup.1.clone(),
            hotkeys: &hotkeys,
        };
        let query = self.query.borrow().clone();
        let mut rows = if query.is_empty() {
            pages::build_page(self.page.get(), &ctx)
        } else {
            search::build_results(&query, &ctx)
        };
        drop(hotkeys);

        let highlight = self.highlight.borrow();
        if !highlight.is_empty() {
            let mut first = true;
            for row in rows.iter_mut() {
                if highlight.contains(&row.id) {
                    row.view.highlighted = true;
                    row.view.scroll_target = first;
                    first = false;
                }
            }
        }
        drop(highlight);

        self.window.set_selected_page(if query.is_empty() {
            PAGES.iter().position(|p| *p == self.page.get()).map_or(-1, |i| i as i32)
        } else {
            -1
        });
        self.window.set_windows11_nav(settings.common.use_windows11_settings_navigation);
        self.sync_model(rows);
    }

    fn sync_model(&self, mut rows: Vec<Item>) {
        let model = &self.model;
        let mut old_ids: Vec<String> = self.items.borrow().iter().map(|i| i.id.clone()).collect();
        let new_ids: Vec<String> = rows.iter().map(|i| i.id.clone()).collect();
        let mut i = 0;
        while i < rows.len() {
            if i < old_ids.len() && old_ids[i] == rows[i].id {
                if let Some(old) = model.row_data(i) {
                    let view = &mut rows[i].view;
                    reuse_model(&old.options, &mut view.options);
                    reuse_model(&old.entries, &mut view.entries);
                    reuse_model(&old.modifier_options, &mut view.modifier_options);
                    if old != *view {
                        model.set_row_data(i, view.clone());
                    }
                }
                i += 1;
            } else if i < old_ids.len() && !new_ids.contains(&old_ids[i]) {
                model.remove(i);
                old_ids.remove(i);
            } else {
                model.insert(i, rows[i].view.clone());
                old_ids.insert(i, rows[i].id.clone());
                i += 1;
            }
        }
        while model.row_count() > rows.len() {
            model.remove(model.row_count() - 1);
        }
        *self.items.borrow_mut() = rows;
    }

    // --- Navigation and search ----------------------------------------------------------------

    fn save_scroll(&self) {
        if self.query.borrow().is_empty() {
            self.scroll_offsets.borrow_mut().insert(self.page.get(), -self.window.get_scroll_y());
        }
    }

    fn navigate(self: &Rc<Self>, page: SettingsPage, highlight: Vec<String>) {
        self.save_scroll();
        self.query.borrow_mut().clear();
        self.window.set_search_text("".into());
        self.page.set(page);
        let has_highlight = !highlight.is_empty();
        *self.highlight.borrow_mut() = highlight;
        self.refresh();
        let offset = if has_highlight { 0.0 } else { self.scroll_offsets.borrow().get(&page).copied().unwrap_or(0.0) };
        self.window.set_scroll_y(-offset);
        if has_highlight {
            let weak = Rc::downgrade(self);
            self.highlight_timer.start(slint::TimerMode::SingleShot, SEARCH_HIGHLIGHT_DURATION, move || {
                if let Some(inner) = weak.upgrade() {
                    inner.highlight.borrow_mut().clear();
                    inner.refresh();
                }
            });
        }
    }

    fn nav_activated(self: &Rc<Self>, index: i32, open: bool) {
        let Some(page) = usize::try_from(index).ok().and_then(|i| PAGES.get(i)).copied() else { return };
        let searching = !self.query.borrow().is_empty();
        if page == self.page.get() && !searching {
            // Clicking the shown page scrolls it to the top; Space/Right only moves focus.
            if !open {
                self.scroll_offsets.borrow_mut().insert(page, 0.0);
                self.window.set_scroll_y(0.0);
            }
        } else {
            self.navigate(page, Vec::new());
        }
        if open {
            self.focus_first_control();
        }
    }

    /// Space/Right on a page row, Enter in the search box: focus the first control of the shown
    /// rows (the window falls back to the page itself when there is none).
    fn focus_first_control(&self) {
        let row = items::first_focus_row(&self.items.borrow());
        self.window.set_focus_row(row.map_or(-1, |r| r as i32));
    }

    fn search_edited(self: &Rc<Self>, text: &str) {
        let query = text.trim().to_owned();
        if *self.query.borrow() == query {
            return;
        }
        let was_searching = !self.query.borrow().is_empty();
        if !was_searching {
            self.save_scroll();
        }
        *self.query.borrow_mut() = query.clone();
        self.highlight.borrow_mut().clear();
        self.refresh();
        let offset = if query.is_empty() {
            self.scroll_offsets.borrow().get(&self.page.get()).copied().unwrap_or(0.0)
        } else {
            0.0
        };
        self.window.set_scroll_y(-offset);
    }

    fn sidebar_resized(&self, width: f64) {
        let width = width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
        self.settings.set_state(|s| s.common.settings_sidebar_width = width);
    }

    fn sidebar_reset(&self) {
        self.window.set_sidebar_width(SIDEBAR_DEFAULT_WIDTH as f32);
        self.settings.set_state(|s| s.common.settings_sidebar_width = 0.0);
    }

    // --- Confirm overlay ----------------------------------------------------------------------

    /// `ConfirmAsync` / `ShowMessage` (no `cancel` = message mode, completes with false).
    fn confirm(self: &Rc<Self>, title: &str, message: &str, ok: &str, cancel: Option<&str>, then: ConfirmHandler) {
        if let Some(previous) = self.confirm.borrow_mut().take() {
            previous(self, false);
        }
        let w = &self.window;
        w.set_confirm_title(title.into());
        w.set_confirm_message(message.into());
        w.set_confirm_ok_text(ok.into());
        w.set_confirm_cancel_text(cancel.unwrap_or_default().into());
        *self.confirm.borrow_mut() = Some(then);
        w.set_confirm_visible(true);
    }

    fn show_message(self: &Rc<Self>, title: &str, message: &str) {
        self.confirm(title, message, s::SettingsWindow_ConfirmOverlay_OK, None, Box::new(|_, _| {}));
    }

    fn confirm_finished(self: &Rc<Self>, result: bool) {
        self.window.set_confirm_visible(false);
        let handler = self.confirm.borrow_mut().take();
        if let Some(handler) = handler {
            handler(self, result);
        }
    }

    /// `TrayAppDotNETRestartPrompt.PromptAsync`.
    fn offer_restart(self: &Rc<Self>, title: &str, message: &str) {
        self.confirm(
            title,
            message,
            s::Settings_General_RenderingRestart_Button,
            Some(s::Settings_General_NotNow_Button),
            Box::new(|inner, restart| {
                if restart {
                    inner.host.restart();
                }
            }),
        );
    }

    // --- Edits --------------------------------------------------------------------------------

    fn with_action<R>(&self, row: i32, f: impl FnOnce(&Item) -> R) -> Option<R> {
        let items = self.items.borrow();
        usize::try_from(row).ok().and_then(|row| items.get(row)).map(f)
    }

    fn toggled(self: &Rc<Self>, row: i32, value: bool) {
        enum Edit {
            Set(items::SetBool),
            Startup,
        }
        let edit = self.with_action(row, |item| match &item.action {
            Action::Bool(f) => Some(Edit::Set(*f)),
            Action::Startup => Some(Edit::Startup),
            _ => None,
        });
        match edit.flatten() {
            Some(Edit::Set(f)) => self.settings.update_explicit(|s| f(s, value)),
            Some(Edit::Startup) => self.set_run_on_startup(value),
            None => {}
        }
    }

    /// `SetRunOnStartup`: the shortcut first, then `RunOnStartup` + explicit save.
    fn set_run_on_startup(self: &Rc<Self>, enabled: bool) {
        match self.host.set_startup_enabled(enabled) {
            Ok(()) => {
                *self.startup.borrow_mut() = (self.host.startup_enabled(), self.host.startup_target());
                self.settings.update_explicit(|s| s.common.run_on_startup = enabled);
                self.schedule_refresh();
            }
            Err(message) => {
                log::warn!("run on startup change failed: {message}");
                *self.startup.borrow_mut() = (self.host.startup_enabled(), self.host.startup_target());
                self.schedule_refresh();
                self.show_message(s::Settings_General_RunOnStartup_Title, &message);
            }
        }
    }

    fn number_committed(&self, row: i32, value: i32) {
        if let Some(Some(f)) = self.with_action(row, |item| match item.action {
            Action::Int(f) => Some(f),
            _ => None,
        }) {
            self.settings.update_explicit(|s| f(s, value));
        }
    }

    fn combo_selected(self: &Rc<Self>, row: i32, index: i32) {
        let Ok(index) = usize::try_from(index) else { return };
        enum Edit {
            Set(items::ComboSetter),
            Thumb(vtadn_model::settings::SliderThumbOption),
            Language(&'static str),
            Backend,
        }
        let edit = self.with_action(row, |item| match &item.action {
            Action::Combo(f) => Some(Edit::Set(f.clone())),
            Action::SliderThumb(catalog) => catalog.get(index).cloned().map(Edit::Thumb),
            Action::Language(tags) => tags.get(index).copied().map(Edit::Language),
            Action::RenderingBackend => Some(Edit::Backend),
            _ => None,
        });
        match edit.flatten() {
            Some(Edit::Set(f)) => self.settings.update_explicit(|s| f(s, index)),
            Some(Edit::Thumb(option)) => self.settings.update_explicit(|s| s.slider_thumb = option),
            Some(Edit::Language(tag)) => {
                if self.settings.get().common.language != tag {
                    self.settings.update_explicit(|s| s.common.language = tag.to_owned());
                }
                self.offer_restart(
                    s::Settings_General_LanguageRestart_Title,
                    s::Settings_General_LanguageRestart_Message,
                );
            }
            Some(Edit::Backend) => {
                let backend = if index == 0 { RenderingBackend::GPUPreferred } else { RenderingBackend::Software };
                if self.settings.get().common.rendering_backend == backend {
                    return;
                }
                self.settings.update_explicit(|s| s.common.rendering_backend = backend);
                self.offer_restart(
                    s::Settings_General_RenderingRestart_Title,
                    s::Settings_General_RenderingRestart_Message,
                );
            }
            None => {}
        }
    }

    fn pair_toggled(&self, row: i32, right: bool, value: bool) {
        if let Some(Some(f)) = self.with_action(row, |item| match item.action {
            Action::Pair { left, right: r } => {
                if right {
                    r
                } else {
                    left
                }
            }
            _ => None,
        }) {
            self.settings.update_explicit(|s| f(s, value));
        }
    }

    fn color_target(&self, row: i32) -> Option<(ColorTarget, String)> {
        self.with_action(row, |item| match item.action {
            Action::Color(target) => Some((target, item.view.title.to_string())),
            _ => None,
        })
        .flatten()
    }

    fn color_reset(&self, row: i32) {
        let Some((target, _)) = self.color_target(row) else { return };
        self.settings.update_explicit(|s| {
            if target.is_variant() {
                *pages::variant_value_mut(s, target) = Default::default();
            } else {
                *pages::single_value_mut(s, target) = None;
            }
        });
    }

    fn color_clicked(self: &Rc<Self>, row: i32) {
        let Some((target, title)) = self.color_target(row) else { return };
        let is_light = self.palette.borrow().is_light;
        let variant = target.is_variant().then_some(is_light);
        if variant.is_some() {
            // One picker per (color, variant): bring the open one forward.
            let pickers = self.pickers.borrow();
            if let Some(open) =
                pickers.iter().find(|p| p.target == target && p.variant == variant && !p.picker.is_closed())
            {
                open.picker.activate();
                return;
            }
        }
        let settings = self.settings.get().clone();
        let (original, start, default, picker_title) = match variant {
            Some(light) => {
                let (fallback_light, fallback_dark) = pages::variant_fallback(&self.theme, target);
                let value = pages::variant_value(&settings, target);
                let (current, fallback) =
                    if light { (value.light, fallback_light) } else { (value.dark, fallback_dark) };
                let variant_text = if light {
                    s::Settings_Theme_PickerTitle_LightVariant
                } else {
                    s::Settings_Theme_PickerTitle_DarkVariant
                };
                let title = s::Settings_Theme_PickerTitle_Format.replace("{0}", &title).replace("{1}", variant_text);
                (current, current.unwrap_or(fallback), fallback, title)
            }
            None => {
                let (current, default) = pages::single_value(&settings, target);
                (current, current.unwrap_or(default), default, title)
            }
        };
        let window = self.window.window();
        let owner = tadn_shell::window::hwnd(window);
        let position = window.position();
        let size = window.size();
        let sink = PickerTarget { inner: Rc::downgrade(self), target, variant, original };
        let palette = self.palette.borrow().clone();
        match ColorPicker::open(
            &picker_title,
            start,
            default,
            &palette,
            owner,
            Some((position.x, position.y, size.width, size.height)),
            Box::new(sink),
        ) {
            Ok(picker) => self.pickers.borrow_mut().push(OpenPicker { target, variant, picker }),
            Err(error) => log::error!("color picker failed to open: {error}"),
        }
    }

    fn button_clicked(&self, row: i32) {
        if let Some(true) = self.with_action(row, |item| matches!(item.action, Action::OpenFolder)) {
            let folder = self.host.data_folder();
            if let Err(error) = tadn_common::shell::open_folder(&folder) {
                log::warn!("open settings folder {} failed: {error}", folder.display());
            }
        }
    }

    fn link_clicked(self: &Rc<Self>, row: i32) {
        enum Link {
            Uri(&'static str),
            Navigate(SettingsPage, Vec<String>),
        }
        let link = self.with_action(row, |item| match &item.action {
            Action::OpenLink(uri) => Some(Link::Uri(uri)),
            Action::Navigate(page, ids) => Some(Link::Navigate(*page, ids.clone())),
            _ => None,
        });
        match link.flatten() {
            Some(Link::Uri(uri)) => {
                if let Err(error) = tadn_common::shell::open_uri(uri) {
                    log::warn!("About page failed to open a link: {error}");
                }
            }
            Some(Link::Navigate(page, ids)) => self.navigate(page, ids),
            None => {}
        }
    }

    // --- Hotkeys ------------------------------------------------------------------------------

    fn hotkey_action(&self, row: i32) -> Option<HotkeyAction> {
        self.with_action(row, |item| match item.action {
            Action::Hotkey(action) => Some(action),
            _ => None,
        })
        .flatten()
    }

    fn hotkey_modifiers(self: &Rc<Self>, row: i32, index: i32) {
        let (Some(action), Ok(index)) = (self.hotkey_action(row), usize::try_from(index)) else { return };
        self.hotkeys.borrow_mut().select_modifiers(action, index);
        self.schedule_refresh();
    }

    fn hotkey_key(self: &Rc<Self>, row: i32, text: &str) -> bool {
        let Some(action) = self.hotkey_action(row) else { return false };
        if self.hotkeys.borrow_mut().key_pressed(action, text) {
            self.schedule_refresh();
        }
        true
    }

    /// `SaveAndApply`: explicit save, register once, refresh every card.
    fn save_and_apply(self: &Rc<Self>, edit: impl FnOnce(&mut Vec<tadn_common::hotkey::HotkeyBinding>)) {
        self.settings.update_explicit(|s| edit(&mut s.common.hotkeys));
        let bindings = self.settings.get().common.hotkeys.clone();
        let results = self.host.register_hotkeys();
        self.hotkeys.borrow_mut().set_results(&bindings, results);
        self.schedule_refresh();
    }

    fn hotkey_add(self: &Rc<Self>, row: i32) {
        let Some(action) = self.hotkey_action(row) else { return };
        let bindings = self.settings.get().common.hotkeys.clone();
        let Some(binding) = self.hotkeys.borrow_mut().take_new_binding(action, &bindings) else { return };
        self.save_and_apply(|list| list.push(binding));
    }

    fn hotkey_delete(self: &Rc<Self>, row: i32, entry: i32) {
        let Some(action) = self.hotkey_action(row) else { return };
        let Ok(entry) = usize::try_from(entry) else { return };
        let binding_id = {
            let settings = self.settings.get();
            HotkeyUi::row_bindings(action, &settings.common.hotkeys).get(entry).map(|b| b.binding_id)
        };
        if let Some(binding_id) = binding_id {
            self.save_and_apply(|list| hotkeys::delete_binding(list, action, binding_id));
        }
    }
}

/// Keeps the old model instance when the contents are unchanged (avoids needless row updates).
fn reuse_model<T: Clone + PartialEq + 'static>(old: &ModelRc<T>, new: &mut ModelRc<T>) {
    if old == new {
        return;
    }
    if old.row_count() == new.row_count() && (0..old.row_count()).all(|i| old.row_data(i) == new.row_data(i)) {
        *new = old.clone();
    }
}

/// Applies a color picker's preview and commit to its setting.
struct PickerTarget {
    inner: Weak<Inner>,
    target: ColorTarget,
    variant: Option<bool>,
    /// The stored value when the picker opened.
    original: Option<Argb>,
}

impl PickerTarget {
    fn set(&self, settings: &mut AppSettings, value: Option<Argb>) {
        match self.variant {
            Some(light) => {
                let pair = pages::variant_value_mut(settings, self.target);
                if light {
                    pair.light = value;
                } else {
                    pair.dark = value;
                }
            }
            None => *pages::single_value_mut(settings, self.target) = value,
        }
    }
}

impl PickerSink for PickerTarget {
    fn preview(&self, color: Argb) {
        if let Some(inner) = self.inner.upgrade() {
            inner.settings.preview(|s| self.set(s, Some(color)));
        }
    }

    fn closed(&self, color: Argb, changed: bool) {
        let Some(inner) = self.inner.upgrade() else { return };
        inner.pickers.borrow_mut().retain(|p| !p.picker.is_closed());
        // End the preview, then save the final color only when it changed.
        inner.settings.preview(|s| self.set(s, self.original));
        if changed {
            inner.settings.update_explicit(|s| self.set(s, Some(color)));
        }
    }
}

#[cfg(test)]
mod tests;
