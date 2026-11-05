//! Top-level context menu window shared by the tray menu and in-window menus (port of
//! `UI/ContextMenus/ContextMenuWindow.cs`, `MenuKeyboardSelection.cs`, `UI/Tray/TrayMenuWindow.cs`,
//! `UI/Tray/TrayMenuController.cs`).
//!
//! One Slint window is kept per [`ContextMenu`] and reused for every show (the C# warm slot): a
//! dismissed menu is hidden, not destroyed. Placement is computed in physical pixels from the
//! menu's natural size before the window is shown, then re-checked against the real window size
//! once the native window exists.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use slint::platform::Key;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tadn_common::theme::Palette;
use tadn_shell::backend::{self, WindowKind};
pub use tadn_shell::placement::Rect;
use tadn_shell::placement::{self as shell_placement};

/// Slint-generated components and globals.
pub mod ui {
    slint::include_modules!();
}

/// `ContextMenuWindow.EdgePadding`: distance kept from the work-area edges (physical px).
pub const EDGE_PADDING: i32 = 8;
/// `ContextMenuWindow.OffscreenPosition`.
const OFFSCREEN_POSITION: i32 = -32000;
/// `ContextMenuWindow.PixelMinSize`.
const PIXEL_MIN_SIZE: i32 = 1;

/// One menu row. Mirrors the Slint `MenuEntry` struct (`TrayAppDotNETCommon/ui/menu.slint`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuItem {
    /// Passed to `on_activate` when the row is invoked.
    pub id: i32,
    pub text: String,
    /// Optional leading icon-font glyph; empty for none.
    pub glyph: String,
    pub checked: bool,
    pub enabled: bool,
    pub separator: bool,
}

impl MenuItem {
    pub fn new(id: i32, text: impl Into<String>) -> Self {
        Self { id, text: text.into(), enabled: true, ..Self::default() }
    }

    pub fn separator() -> Self {
        Self { id: -1, separator: true, ..Self::default() }
    }

    pub fn with_glyph(mut self, glyph: impl Into<String>) -> Self {
        self.glyph = glyph.into();
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    fn is_selectable(&self) -> bool {
        !self.separator && self.enabled
    }
}

/// Where the menu opens (physical screen pixels). Port of `ContextMenuWindow.ResolvePosition`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuPlacement {
    /// `ContextMenuPosition::Classic`: at the cursor point, flipped to stay inside the work area.
    AtPoint { x: i32, y: i32 },
    /// `ContextMenuPosition::Modern` tray menu: docked above the taskbar on the monitor under the
    /// point, like the Windows 11 system flyouts.
    TrayDocked { x: i32, y: i32 },
    /// Opened from a control: below `rect`, or above it when there is no room below.
    ForRect(Rect),
    /// `ContextMenuPosition::Modern` with the tray icon rectangle known: the work area of the
    /// monitor under the cursor point `(x, y)`, the menu centered on `icon`
    /// (`TrayPopupPositioning.ResolveDockedPosition`); no icon → bottom-right corner.
    TrayIcon { x: i32, y: i32, icon: Option<Rect> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuOptions {
    /// `ContextMenuFontSize` (8..48); row height and text scale from it.
    pub font_size: i32,
    /// Open scrolled to the last row (tray menu: the most-used rows sit next to the taskbar).
    pub scroll_to_bottom: bool,
    /// `ContextMenuWindowOptions.MaxHeight`: caps the rows' scroll viewer (logical px) below the
    /// work-area limit; None = only the work area limits it.
    pub max_height: Option<f32>,
}

impl Default for MenuOptions {
    fn default() -> Self {
        Self { font_size: 15, scroll_to_bottom: false, max_height: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCloseReason {
    /// A row was invoked (`on_activate` already ran).
    Activated,
    /// Escape, or `close()` from code.
    Dismissed,
    /// The menu lost activation (click elsewhere).
    Deactivated,
}

/// `MenuSelectionMove`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMove {
    Previous,
    Next,
    First,
    Last,
}

/// `MenuKeyboardSelection.ResolveTargetIndex`: Up/Down wrap around the ends; with no selection
/// Down/Home select the first row and Up/End the last. Returns -1 when there is no selectable row.
pub fn resolve_target_index(row_count: i32, current: i32, movement: SelectionMove) -> i32 {
    if row_count <= 0 {
        return -1;
    }
    let has_selection = current >= 0 && current < row_count;
    match movement {
        SelectionMove::First => 0,
        SelectionMove::Last => row_count - 1,
        SelectionMove::Next => {
            if has_selection {
                (current + 1) % row_count
            } else {
                0
            }
        }
        SelectionMove::Previous => {
            if has_selection {
                (current - 1 + row_count) % row_count
            } else {
                row_count - 1
            }
        }
    }
}

/// Top-left of a `size` menu for `placement` inside `work_area` (all physical px).
pub fn resolve_position(placement: MenuPlacement, size: (i32, i32), work_area: Rect) -> (i32, i32) {
    let (w, h) = (size.0.max(PIXEL_MIN_SIZE), size.1.max(PIXEL_MIN_SIZE));
    match placement {
        MenuPlacement::AtPoint { x, y } => {
            // Classic: the X position is not clamped (C# quirk kept), Y stays inside the work area.
            let min_top = work_area.top + EDGE_PADDING;
            let max_top = (work_area.bottom - h - EDGE_PADDING).max(min_top);
            (x, y.clamp(min_top, max_top))
        }
        MenuPlacement::TrayDocked { x, y } => {
            let icon = Rect { left: x, top: y, right: x + 1, bottom: y + 1 };
            shell_placement::docked_popup_position(Some(icon), work_area, (w, h), EDGE_PADDING)
        }
        MenuPlacement::TrayIcon { icon, .. } => {
            shell_placement::docked_popup_position(icon, work_area, (w, h), EDGE_PADDING)
        }
        MenuPlacement::ForRect(rect) => {
            let below = rect.bottom;
            let fits_below = below + h + EDGE_PADDING <= work_area.bottom;
            let y = if fits_below { below } else { rect.top - h };
            shell_placement::clamp_into((rect.left, y), (w, h), work_area, EDGE_PADDING)
        }
    }
}

/// Position from the real window size; a size not known yet (0) falls back to `measured`.
pub fn settled_position(
    placement: MenuPlacement,
    actual: (i32, i32),
    measured: (i32, i32),
    work_area: Rect,
) -> (i32, i32) {
    let size = if actual.0 > 0 && actual.1 > 0 { actual } else { measured };
    resolve_position(placement, size, work_area)
}

/// The screen point whose monitor hosts the menu.
fn anchor_point(placement: MenuPlacement) -> (i32, i32) {
    match placement {
        MenuPlacement::AtPoint { x, y } | MenuPlacement::TrayDocked { x, y } | MenuPlacement::TrayIcon { x, y, .. } => {
            (x, y)
        }
        MenuPlacement::ForRect(rect) => rect.center(),
    }
}

/// Maps a key press to the menu command it triggers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyCommand {
    Move(SelectionMove),
    Invoke,
    Dismiss,
}

fn key_command(text: &str) -> Option<KeyCommand> {
    let is = |key: Key| text == SharedString::from(key).as_str();
    if is(Key::UpArrow) {
        Some(KeyCommand::Move(SelectionMove::Previous))
    } else if is(Key::DownArrow) {
        Some(KeyCommand::Move(SelectionMove::Next))
    } else if is(Key::Home) {
        Some(KeyCommand::Move(SelectionMove::First))
    } else if is(Key::End) {
        Some(KeyCommand::Move(SelectionMove::Last))
    } else if is(Key::Return) || text == "\n" || text == "\r" || text == " " {
        Some(KeyCommand::Invoke)
    } else if is(Key::Escape) {
        Some(KeyCommand::Dismiss)
    } else {
        None
    }
}

/// Callbacks of one show.
struct Session {
    generation: u64,
    on_activate: Box<dyn FnOnce(i32)>,
    on_closed: Box<dyn FnOnce(MenuCloseReason)>,
}

struct Inner {
    window: ui::MenuWindow,
    items: RefCell<Vec<MenuItem>>,
    model: Rc<VecModel<ui::MenuEntry>>,
    session: RefCell<Option<Session>>,
    generation: Cell<u64>,
    rounded: Cell<bool>,
}

/// A reusable menu window. Opening a new menu closes the current one first.
pub struct ContextMenu {
    inner: Rc<Inner>,
}

impl ContextMenu {
    pub fn new() -> Result<Self, slint::PlatformError> {
        let window = backend::with_window_kind(WindowKind::Menu, ui::MenuWindow::new)?;
        let model = Rc::new(VecModel::<ui::MenuEntry>::default());
        window.set_entries(ModelRc::from(model.clone()));
        let inner = Rc::new(Inner {
            window,
            items: RefCell::new(Vec::new()),
            model,
            session: RefCell::new(None),
            generation: Cell::new(0),
            rounded: Cell::new(true),
        });

        let weak = Rc::downgrade(&inner);
        inner.window.on_activated(move |id| {
            if let Some(inner) = weak.upgrade() {
                Inner::activate(&inner, id);
            }
        });
        let weak = Rc::downgrade(&inner);
        inner.window.on_key_pressed(move |text| weak.upgrade().is_some_and(|inner| Inner::on_key(&inner, &text)));
        let weak = Rc::downgrade(&inner);
        tadn_shell::window::on_focus_changed(inner.window.window(), move |focused| {
            if !focused {
                Inner::schedule_deactivation_check(&weak);
            }
        });
        Ok(Self { inner })
    }

    /// Shows `items` at `placement`, focused for keyboard navigation (Up/Down wrap, Home/End,
    /// Enter/Space invoke, Escape dismiss). `on_activate(id)` runs for an invoked row, then
    /// `on_closed` runs exactly once for every show.
    pub fn show(
        &self,
        items: Vec<MenuItem>,
        placement: MenuPlacement,
        options: MenuOptions,
        on_activate: impl FnOnce(i32) + 'static,
        on_closed: impl FnOnce(MenuCloseReason) + 'static,
    ) {
        self.inner.close_with(MenuCloseReason::Dismissed);
        let inner = &self.inner;
        let generation = inner.generation.get().wrapping_add(1);
        inner.generation.set(generation);
        *inner.session.borrow_mut() =
            Some(Session { generation, on_activate: Box::new(on_activate), on_closed: Box::new(on_closed) });

        let window = &inner.window;
        let glyph_column = items.iter().any(|i| !i.separator && (i.checked || !i.glyph.is_empty()));
        inner.model.set_vec(items.iter().map(to_entry).collect::<Vec<_>>());
        *inner.items.borrow_mut() = items;
        window.set_glyph_column(glyph_column);
        window.set_font_size(options.font_size.clamp(8, 48) as f32);
        window.set_selected_index(-1);
        window.set_keyboard_selection(false);

        let (ax, ay) = anchor_point(placement);
        let monitor = shell_placement::monitor_from_point(ax, ay);
        let scale = if monitor.scale > 0.0 { monitor.scale } else { 1.0 };
        // `MonitorFromPoint(MONITOR_DEFAULTTONULL)` work area with the C# fallbacks.
        let work_area = shell_placement::resolve_work_area(ax, ay);
        // The scroll viewer's height: work area minus the edge padding (`CapHeight` with the
        // optional MaxHeight); the Slint side subtracts the 2 px root padding on both sides.
        let mut max_content = ((work_area.height() - 2 * EDGE_PADDING) as f32 / scale).max(1.0);
        if let Some(cap) = options.max_height {
            max_content = max_content.min(cap.max(1.0) + 4.0);
        }
        window.set_max_content_height(max_content);

        let logical = (window.get_natural_width(), window.get_natural_height());
        let physical = (
            ((logical.0 * scale).ceil() as i32).max(PIXEL_MIN_SIZE),
            ((logical.1 * scale).ceil() as i32).max(PIXEL_MIN_SIZE),
        );
        window.window().set_size(slint::LogicalSize::new(logical.0, logical.1));
        // `OffscreenPosition`: the window becomes visible off-screen; it moves into place only once
        // the native window exists and its real size is known, so a wrong pre-creation measurement
        // can never leave the menu clipped or off-screen.
        window.window().set_position(slint::PhysicalPosition::new(OFFSCREEN_POSITION, OFFSCREEN_POSITION));
        if options.scroll_to_bottom {
            window.invoke_scroll_to_bottom();
        } else {
            window.invoke_scroll_to_top();
        }
        if let Err(error) = backend::with_window_kind(WindowKind::Menu, || window.show()) {
            log::error!("ContextMenu: show failed: {error}");
            inner.close_with(MenuCloseReason::Dismissed);
            return;
        }

        let weak = Rc::downgrade(inner);
        tadn_shell::window::when_created(window.window(), move |hwnd| {
            let Some(inner) = weak.upgrade() else { return };
            if !inner.is_current(generation) {
                return;
            }
            tadn_shell::window::set_rounded_corners(hwnd, inner.rounded.get());
            inner.settle(placement, physical, work_area);
            // Moving onto the target monitor can change the DPI and with it the size: settle again
            // once that resize and the next layout pass have run, then activate.
            let weak = Rc::downgrade(&inner);
            slint::Timer::single_shot(Duration::ZERO, move || {
                let Some(inner) = weak.upgrade() else { return };
                if !inner.is_current(generation) {
                    return;
                }
                inner.settle(placement, physical, work_area);
                if options.scroll_to_bottom {
                    inner.window.invoke_scroll_to_bottom();
                }
                tadn_shell::window::activate(hwnd);
            });
        });
    }

    pub fn close(&self) {
        self.inner.close_with(MenuCloseReason::Dismissed);
    }

    pub fn is_open(&self) -> bool {
        self.inner.session.borrow().is_some()
    }

    pub fn apply_palette(&self, palette: &Palette) {
        self.inner.rounded.set(palette.rounded_corners);
        tadn_common::apply_palette!(self.inner.window.global::<ui::Theme>(), palette);
    }
}

impl Drop for ContextMenu {
    fn drop(&mut self) {
        self.inner.close_with(MenuCloseReason::Dismissed);
    }
}

impl Inner {
    /// Places the window from its real size (falling back to the measured one).
    fn settle(&self, placement: MenuPlacement, measured: (i32, i32), work_area: Rect) {
        let size = self.window.window().size();
        let actual = (size.width as i32, size.height as i32);
        let (x, y) = settled_position(placement, actual, measured, work_area);
        self.window.window().set_position(slint::PhysicalPosition::new(x, y));
    }

    fn is_current(&self, generation: u64) -> bool {
        self.session.borrow().as_ref().is_some_and(|s| s.generation == generation)
    }

    /// Hides the window and runs `on_closed` of the current show (no-op when nothing is open).
    fn close_with(&self, reason: MenuCloseReason) {
        let Some(session) = self.session.borrow_mut().take() else { return };
        if let Err(error) = self.window.hide() {
            log::warn!("ContextMenu: hide failed: {error}");
        }
        (session.on_closed)(reason);
    }

    /// Invoke: the menu is dismissed first, then the action runs (`InvokeBeforeClose = false`).
    fn activate(this: &Rc<Self>, id: i32) {
        let enabled = this.items.borrow().iter().any(|i| i.id == id && i.is_selectable());
        if !enabled {
            return;
        }
        let Some(session) = this.session.borrow_mut().take() else { return };
        if let Err(error) = this.window.hide() {
            log::warn!("ContextMenu: hide failed: {error}");
        }
        (session.on_activate)(id);
        (session.on_closed)(MenuCloseReason::Activated);
    }

    fn on_key(this: &Rc<Self>, text: &str) -> bool {
        let Some(command) = key_command(text) else { return false };
        match command {
            KeyCommand::Dismiss => this.close_with(MenuCloseReason::Dismissed),
            KeyCommand::Invoke => {
                // The focused row (keyboard selection or the hovered row) is invoked.
                let index = this.window.get_selected_index();
                let id = usize::try_from(index)
                    .ok()
                    .and_then(|i| this.items.borrow().get(i).filter(|i| i.is_selectable()).map(|i| i.id));
                if let Some(id) = id {
                    Self::activate(this, id);
                }
            }
            KeyCommand::Move(movement) => {
                let selectable: Vec<usize> = this
                    .items
                    .borrow()
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| item.is_selectable())
                    .map(|(index, _)| index)
                    .collect();
                let current_row = this.window.get_selected_index();
                let current = selectable.iter().position(|&i| i as i32 == current_row).map_or(-1, |p| p as i32);
                let target = resolve_target_index(selectable.len() as i32, current, movement);
                if let Ok(target) = usize::try_from(target) {
                    this.window.set_selected_index(selectable[target] as i32);
                    this.window.set_keyboard_selection(true);
                }
            }
        }
        true
    }

    /// Deactivation is checked once, posted, so a focus hop between our own windows settles first.
    fn schedule_deactivation_check(weak: &Weak<Self>) {
        let Some(inner) = weak.upgrade() else { return };
        let Some(generation) = inner.session.borrow().as_ref().map(|s| s.generation) else { return };
        let weak = weak.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            let Some(inner) = weak.upgrade() else { return };
            if !inner.is_current(generation) {
                return;
            }
            let focused =
                tadn_shell::window::hwnd(inner.window.window()).is_some_and(tadn_shell::window::is_foreground);
            if !focused {
                inner.close_with(MenuCloseReason::Deactivated);
            }
        });
    }
}

fn to_entry(item: &MenuItem) -> ui::MenuEntry {
    ui::MenuEntry {
        id: item.id,
        text: item.text.as_str().into(),
        glyph: item.glyph.as_str().into(),
        checked: item.checked,
        enabled: item.enabled,
        separator: item.separator,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WA: Rect = Rect { left: 0, top: 0, right: 1920, bottom: 1032 };

    #[test]
    fn keyboard_selection_matches_win32_menus() {
        assert_eq!(resolve_target_index(0, -1, SelectionMove::Next), -1);
        assert_eq!(resolve_target_index(3, -1, SelectionMove::Next), 0);
        assert_eq!(resolve_target_index(3, -1, SelectionMove::Previous), 2);
        assert_eq!(resolve_target_index(3, -1, SelectionMove::First), 0);
        assert_eq!(resolve_target_index(3, -1, SelectionMove::Last), 2);
        assert_eq!(resolve_target_index(3, 2, SelectionMove::Next), 0);
        assert_eq!(resolve_target_index(3, 0, SelectionMove::Previous), 2);
        assert_eq!(resolve_target_index(3, 1, SelectionMove::Next), 2);
        assert_eq!(resolve_target_index(3, 7, SelectionMove::Next), 0);
    }

    #[test]
    fn classic_keeps_x_and_clamps_y() {
        assert_eq!(resolve_position(MenuPlacement::AtPoint { x: 1900, y: 1000 }, (200, 300), WA), (1900, 724));
        assert_eq!(resolve_position(MenuPlacement::AtPoint { x: 5, y: 2 }, (200, 300), WA), (5, 8));
    }

    #[test]
    fn modern_centers_on_icon_and_clamps() {
        let icon = Rect { left: 1700, top: 1040, right: 1724, bottom: 1072 };
        let pos = resolve_position(MenuPlacement::TrayIcon { x: 1710, y: 1050, icon: Some(icon) }, (200, 300), WA);
        assert_eq!(pos, (1612, 724));
        let corner = resolve_position(MenuPlacement::TrayIcon { x: 1710, y: 1050, icon: None }, (200, 300), WA);
        assert_eq!(corner, (1712, 724));
        let near_edge = Rect { left: 1900, top: 1040, right: 1920, bottom: 1072 };
        let pos = resolve_position(MenuPlacement::TrayIcon { x: 0, y: 0, icon: Some(near_edge) }, (200, 300), WA);
        assert_eq!(pos, (1712, 724));
    }

    #[test]
    fn for_rect_opens_below_or_above() {
        let rect = Rect { left: 100, top: 100, right: 140, bottom: 130 };
        assert_eq!(resolve_position(MenuPlacement::ForRect(rect), (200, 300), WA), (100, 130));
        let low = Rect { left: 100, top: 900, right: 140, bottom: 930 };
        assert_eq!(resolve_position(MenuPlacement::ForRect(low), (200, 300), WA), (100, 600));
    }

    #[test]
    fn oversized_menu_pins_to_top_edge() {
        assert_eq!(resolve_position(MenuPlacement::AtPoint { x: 10, y: 500 }, (200, 5000), WA), (10, 8));
    }

    #[test]
    fn settled_position_uses_real_size() {
        let point = MenuPlacement::AtPoint { x: 10, y: 1000 };
        // Pre-creation measurement too small: the real size wins and stays inside the work area.
        assert_eq!(settled_position(point, (200, 400), (100, 100), WA), (10, 624));
        assert_eq!(settled_position(point, (0, 0), (100, 100), WA), (10, 924));
        let icon = Rect { left: 1900, top: 1040, right: 1920, bottom: 1072 };
        let tray = MenuPlacement::TrayIcon { x: 1910, y: 1050, icon: Some(icon) };
        let (x, y) = settled_position(tray, (300, 500), (150, 100), WA);
        assert!(x + 300 <= WA.right - EDGE_PADDING && y + 500 <= WA.bottom - EDGE_PADDING);
    }

    #[test]
    fn key_mapping() {
        assert_eq!(
            key_command(SharedString::from(Key::UpArrow).as_str()),
            Some(KeyCommand::Move(SelectionMove::Previous))
        );
        assert_eq!(key_command(SharedString::from(Key::End).as_str()), Some(KeyCommand::Move(SelectionMove::Last)));
        assert_eq!(key_command(" "), Some(KeyCommand::Invoke));
        assert_eq!(key_command(SharedString::from(Key::Escape).as_str()), Some(KeyCommand::Dismiss));
        assert_eq!(key_command("a"), None);
    }
}
