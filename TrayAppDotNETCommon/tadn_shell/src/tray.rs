//! Notification-area icon (port of `UI/Tray/TrayAppDotNETShellTrayIcon.cs`, `NativeIcon.cs`,
//! `TrayClickModifiers.cs`, `TrayWheelNotchAccumulator.cs`, `TrayTooltipText.cs`,
//! `TrayAppDotNETTrayIconMetrics.cs`, the raw-input parts of `Interop/InputHelper.cs`, the
//! precision-touchpad recognizer and the shell parts of `TrayIconPresenter.cs`).
//!
//! `Shell_NotifyIconW` with `NOTIFYICON_VERSION_4` and GUID identity, a hidden top-level message
//! window (so `TaskbarCreated` arrives), recovery after Explorer restarts, one semantic event per
//! gesture, raw-input wheel and precision-touchpad scrolling registered only while the cursor is
//! over the icon, and flicker-free icon swaps.
//!
//! State lives in `Cell`s that are never borrowed across a native call: `Shell_NotifyIconW` waits
//! on Explorer, and Explorer's callbacks can re-enter the window procedure meanwhile. Events reach
//! the app handler through a queue drained from the event loop, never from the window procedure.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::time::Duration;

use windows::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, LOGPIXELSX, ReleaseDC};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT, RIDEV_DEVNOTIFY, RIDEV_INPUTSINK,
    RIDEV_REMOVE, RIM_TYPEHID, RIM_TYPEMOUSE, RegisterRawInputDevices,
};
use windows::Win32::UI::Shell::{
    NIF_GUID, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETFOCUS,
    NIM_SETVERSION, NIN_POPUPCLOSE, NIN_POPUPOPEN, NIN_SELECT, NOTIFY_ICON_DATA_FLAGS, NOTIFY_ICON_MESSAGE,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, DestroyIcon, FindWindowW, GetCursorPos, GetMessagePos, HICON, MSGFLT_ALLOW,
    RegisterWindowMessageW, SM_CXSMICON, WM_CONTEXTMENU, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_INPUT, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_USER,
};
use windows_core::{GUID, w};

use crate::msgwnd::MessageWindow;
use crate::placement::Rect;
use crate::raster::{IconImage, create_hicon};
use crate::util::{post, post_delayed, tick_ms};

/// `WM_CALLBACKMOUSEMSG`: the shell callback message.
const WM_TRAY_CALLBACK: u32 = WM_USER + 1024;
/// `NIN_KEYSELECT` (`NIN_SELECT | NINF_KEY`).
const NIN_KEYSELECT: u32 = NIN_SELECT | 0x1;
/// `uID` of the icon under the hWnd/uID identity fallback.
const FALLBACK_ICON_UID: u32 = 1;
/// `TrayIconLocationRefreshCooldownMs`: how long a `Shell_NotifyIconGetRect` result is reused.
const TRAY_ICON_LOCATION_REFRESH_COOLDOWN_MS: u64 = 250;
/// `TaskbarRecoveryRetryCount`: re-add attempts after `TaskbarCreated`.
const TASKBAR_RECOVERY_RETRY_COUNT: u32 = 5;
/// `TaskbarRecoveryRetryDelayMs`: pause between re-add attempts.
const TASKBAR_RECOVERY_RETRY_DELAY_MS: u64 = 500;
/// `RawInputBufferInitialSize`.
const RAW_INPUT_BUFFER_INITIAL_SIZE: usize = 256;
/// `DefaultPrecisionTouchpadUnitsPerScrollStep`.
pub const DEFAULT_PRECISION_TOUCHPAD_UNITS_PER_SCROLL_STEP: u32 = 90;
/// `TrayAppDotNETTrayIconMetrics.DefaultDPI`.
const DEFAULT_DPI: u32 = 96;
/// `TrayAppDotNETTrayIconMetrics.DefaultSmallIconSize`.
const DEFAULT_SMALL_ICON_SIZE: u32 = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Modifiers {
    /// Keys held right now (`GetAsyncKeyState(vk) & 0x8000`), as the C# handlers read them when
    /// they run.
    pub fn current() -> Self {
        fn down(vk: i32) -> bool {
            // SAFETY: no preconditions.
            (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
        }
        Self { ctrl: down(VK_CONTROL.0 as i32), alt: down(VK_MENU.0 as i32), shift: down(VK_SHIFT.0 as i32) }
    }

    /// `TrayClickModifiers.Resolve`: `ctrl` while Ctrl is held and `alt` while Alt is held,
    /// unless that variant is `nothing`, which falls through to `fallback`. Ctrl wins over Alt.
    pub fn resolve<T: PartialEq>(&self, ctrl: T, alt: T, fallback: T, nothing: T) -> T {
        if self.ctrl && ctrl != nothing {
            return ctrl;
        }
        if self.alt && alt != nothing {
            return alt;
        }
        fallback
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// Left button released (also Enter/Space on the focused icon in the Windows 11 tray).
    LeftClick { x: i32, y: i32, modifiers: Modifiers },
    /// Second click of a double-click.
    DoubleClick { x: i32, y: i32, modifiers: Modifiers },
    /// Right button released, or a keyboard menu request (Shift+F10 / Apps key).
    RightClick { x: i32, y: i32, modifiers: Modifiers },
    /// Classic notification area `NIN_KEYSELECT` (Enter/Space): show the UI, never toggle it.
    KeySelect,
    /// Whole wheel notches over the icon (high-resolution deltas accumulated); positive = up.
    Wheel { notches: i32, modifiers: Modifiers },
    /// Precision-touchpad two-finger scroll over the icon, one event per step (`steps` is +1 or
    /// -1, like the C# `PrecisionTouchpadScrolled`); positive = up.
    TouchpadScroll { steps: i32 },
    /// The icon was re-added after `TaskbarCreated` (Explorer restart).
    Recreated,
    /// C# `RefreshNeeded`: a display/DPI change or a `TaskbarCreated` recovery attempt. Re-query
    /// [`TrayIcon::icon_size_px`], re-render the icon and re-set the tooltip.
    RefreshNeeded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayInputOptions {
    /// Register raw-input wheel while hovering the icon (`TrayScrollEnabled`).
    pub wheel: bool,
    /// Recognize precision-touchpad scrolling (`PrecisionTouchpadScrollEnabled`).
    pub touchpad: bool,
    /// Raw touchpad units per step (`PrecisionTouchpadUnitsPerScrollStep`, 1..1000, default 90).
    pub touchpad_units_per_step: u32,
}

pub struct TrayIconConfig {
    /// `NOTIFYICONDATAW::guidItem` (`AppIdentity::tray_icon_guid`).
    pub guid: GUID,
    /// Prefix for the message window class name (`AppIdentity::instance_name`).
    pub class_prefix: &'static str,
    pub input: TrayInputOptions,
}

/// The notification icon. Not added to the shell until the first [`TrayIcon::set_icon`].
pub struct TrayIcon {
    shared: Rc<Shared>,
}

impl TrayIcon {
    /// Creates the message window on the UI thread. `handler` is called on the UI thread,
    /// asynchronously after the window procedure returns.
    pub fn new(config: TrayIconConfig, handler: impl Fn(TrayEvent) + 'static) -> windows_core::Result<Self> {
        // SAFETY: registers a process-wide message name; no preconditions.
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        let mut touchpad = TouchpadScrollRecognizer::new(DEFAULT_PRECISION_TOUCHPAD_UNITS_PER_SCROLL_STEP as i32);
        touchpad.set_units_per_scroll_step(config.input.touchpad_units_per_step as i32);
        let shared = Rc::new(Shared {
            window: RefCell::new(None),
            hwnd: Cell::new(HWND::default()),
            taskbar_created,
            guid: config.guid,
            use_guid: Cell::new(true),
            handler: Box::new(handler),
            disposed: Cell::new(false),
            current_icon: Cell::new(HICON::default()),
            shell_icon: Cell::new(HICON::default()),
            last_image: RefCell::new(None),
            is_created: Cell::new(false),
            stale_icon_cleared: Cell::new(false),
            force_full_update: Cell::new(false),
            tooltip: RefCell::new(String::new()),
            tooltip_dirty: Cell::new(false),
            tooltip_show_requested: Cell::new(false),
            tooltip_keep_open: Cell::new(false),
            tooltip_hover_sync_pending: Cell::new(true),
            pointer_over: Cell::new(false),
            has_processed_button_up: Cell::new(false),
            has_processed_right_button_up: Cell::new(false),
            scroll_enabled: Cell::new(config.input.wheel),
            touchpad_enabled: Cell::new(config.input.touchpad),
            listening: Cell::new(false),
            wheel: Cell::new(WheelNotchAccumulator::default()),
            touchpad: RefCell::new(touchpad),
            raw_buffer: RefCell::new(vec![0; RAW_INPUT_BUFFER_INITIAL_SIZE]),
            location: Cell::new(None),
            location_tick: Cell::new(0),
            icon_update_queued: Cell::new(false),
            tooltip_update_queued: Cell::new(false),
            mouse_refresh_queued: Cell::new(false),
            recovery_queued: Cell::new(false),
            display_refresh_queued: Cell::new(false),
            events: RefCell::new(VecDeque::new()),
            events_scheduled: Cell::new(false),
        });

        let weak = Rc::downgrade(&shared);
        let class_prefix =
            if config.class_prefix.trim().is_empty() { "TrayAppDotNETShellTrayIcon" } else { config.class_prefix };
        // TaskbarCreated is broadcast only to top-level windows, not HWND_MESSAGE windows.
        let window = MessageWindow::new(
            &format!("{class_prefix}.TrayIcon"),
            false,
            Rc::new(move |_, message, wparam, lparam| {
                weak.upgrade().and_then(|s| s.on_message(message, wparam, lparam))
            }),
        )?;
        let hwnd = window.hwnd();
        shared.hwnd.set(hwnd);
        *shared.window.borrow_mut() = Some(window);

        // An elevated instance would otherwise never hear TaskbarCreated from the unelevated
        // Explorer (UIPI), and its icon would stay missing after an Explorer restart.
        // SAFETY: `hwnd` is this thread's window.
        if let Err(error) = unsafe { ChangeWindowMessageFilterEx(hwnd, taskbar_created, MSGFLT_ALLOW, None) } {
            log::warn!("TrayIcon: ChangeWindowMessageFilterEx(TaskbarCreated) failed: {error}");
        }
        Ok(Self { shared })
    }

    /// Replaces the icon (adds the shell icon on first call). Identical pixels are skipped; the old
    /// HICON is destroyed only after the shell has the new one.
    pub fn set_icon(&self, image: &IconImage) {
        let s = &self.shared;
        if s.disposed.get() || s.last_image.borrow().as_ref() == Some(image) {
            return;
        }
        match create_hicon(image) {
            Ok(icon) => {
                *s.last_image.borrow_mut() = Some(image.clone());
                s.replace_current_icon(icon);
                s.request_icon_update();
            }
            Err(error) => log::warn!("TrayIcon.set_icon: CreateIconIndirect failed: {error}"),
        }
    }

    /// Updates the tooltip. Pushed to the shell at once only while the pointer is over the icon or
    /// `show_now` is set (after a scroll, so the tooltip pops up with the new level); otherwise it
    /// rides on the next icon update or hover.
    pub fn set_tooltip(&self, text: &str, show_now: bool) {
        let s = &self.shared;
        if s.disposed.get() {
            return;
        }
        let unchanged = *s.tooltip.borrow() == text && !s.tooltip_dirty.get();
        if !unchanged {
            s.set_tooltip_text(text);
            if s.pointer_over.get() || s.tooltip_keep_open.get() {
                s.request_tooltip_update();
            }
        }
        if show_now {
            s.show_tooltip();
        }
    }

    /// `Shell_NotifyIconGetRect`.
    pub fn rect(&self) -> Option<Rect> {
        self.shared.icon_rect().map(|rect| Rect {
            left: rect.left,
            top: rect.top,
            right: rect.right.max(rect.left),
            bottom: rect.bottom.max(rect.top),
        })
    }

    /// `GetSystemMetricsForDpi(SM_CXSMICON, taskbar DPI)` with the C# fallbacks.
    pub fn icon_size_px(&self) -> u32 {
        taskbar_small_icon_size()
    }

    pub fn set_input_options(&self, input: TrayInputOptions) {
        let s = &self.shared;
        if s.scroll_enabled.replace(input.wheel) != input.wheel {
            s.refresh_mouse_input_registration();
        }
        if s.touchpad_enabled.replace(input.touchpad) != input.touchpad {
            s.touchpad.borrow_mut().reset();
        }
        s.touchpad.borrow_mut().set_units_per_scroll_step(input.touchpad_units_per_step as i32);
    }

    /// `NIM_SETFOCUS`: returns keyboard focus to the icon after a menu closes from the keyboard.
    pub fn restore_focus(&self) {
        let s = &self.shared;
        if s.disposed.get() || !s.is_created.get() {
            return;
        }
        if let Err(error) = s.notify(NIM_SETFOCUS, &s.make_data(NOTIFY_ICON_DATA_FLAGS(0))) {
            log::debug!("TrayIcon.restore_focus: NIM_SETFOCUS failed (0x{error:08X})");
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        self.shared.dispose();
    }
}

/// `TrayAppDotNETTrayIconMetrics.GetTaskbarSmallIconSize`: `SM_CXSMICON` at the taskbar's DPI.
pub fn taskbar_small_icon_size() -> u32 {
    small_icon_size_for_dpi(taskbar_dpi())
}

/// `GetDpiForWindow(Shell_TrayWnd)`, else the screen DC's `LOGPIXELSX`, else 96.
pub fn taskbar_dpi() -> u32 {
    // SAFETY: plain queries; the DC is released below.
    unsafe {
        if let Ok(taskbar) = FindWindowW(w!("Shell_TrayWnd"), None) {
            let dpi = GetDpiForWindow(taskbar);
            if dpi > 0 {
                return dpi;
            }
        }
        let dc = GetDC(None);
        if dc.is_invalid() {
            return DEFAULT_DPI;
        }
        let dpi = GetDeviceCaps(Some(dc), LOGPIXELSX);
        ReleaseDC(None, dc);
        if dpi > 0 { dpi as u32 } else { DEFAULT_DPI }
    }
}

/// `GetSmallIconSizeForDPI`: `SM_CXSMICON` at `dpi`, else `round(16 * dpi / 96)`.
pub fn small_icon_size_for_dpi(dpi: u32) -> u32 {
    // SAFETY: no preconditions.
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) };
    if size > 0 {
        size as u32
    } else {
        (DEFAULT_SMALL_ICON_SIZE as f64 * dpi as f64 / DEFAULT_DPI as f64).round_ties_even() as u32
    }
}

/// Events waiting for the app handler; modifiers are read when the handler runs, like C#.
#[derive(Clone, Copy, Debug)]
enum Pending {
    LeftClick(i32, i32),
    DoubleClick(i32, i32),
    RightClick(i32, i32),
    KeySelect,
    Wheel(i32),
    TouchpadStep(i32),
    Recreated,
    RefreshNeeded,
}

impl Pending {
    fn into_event(self, modifiers: Modifiers) -> TrayEvent {
        match self {
            Self::LeftClick(x, y) => TrayEvent::LeftClick { x, y, modifiers },
            Self::DoubleClick(x, y) => TrayEvent::DoubleClick { x, y, modifiers },
            Self::RightClick(x, y) => TrayEvent::RightClick { x, y, modifiers },
            Self::KeySelect => TrayEvent::KeySelect,
            Self::Wheel(notches) => TrayEvent::Wheel { notches, modifiers },
            Self::TouchpadStep(steps) => TrayEvent::TouchpadScroll { steps },
            Self::Recreated => TrayEvent::Recreated,
            Self::RefreshNeeded => TrayEvent::RefreshNeeded,
        }
    }
}

struct Shared {
    window: RefCell<Option<MessageWindow>>,
    hwnd: Cell<HWND>,
    taskbar_created: u32,
    guid: GUID,
    /// GUID identity; false after `NIM_ADD` with `NIF_GUID` failed but hWnd/uID worked.
    use_guid: Cell<bool>,
    handler: Box<dyn Fn(TrayEvent)>,
    disposed: Cell<bool>,
    current_icon: Cell<HICON>,
    shell_icon: Cell<HICON>,
    last_image: RefCell<Option<IconImage>>,
    is_created: Cell<bool>,
    stale_icon_cleared: Cell<bool>,
    force_full_update: Cell<bool>,
    tooltip: RefCell<String>,
    tooltip_dirty: Cell<bool>,
    tooltip_show_requested: Cell<bool>,
    tooltip_keep_open: Cell<bool>,
    tooltip_hover_sync_pending: Cell<bool>,
    pointer_over: Cell<bool>,
    has_processed_button_up: Cell<bool>,
    has_processed_right_button_up: Cell<bool>,
    scroll_enabled: Cell<bool>,
    touchpad_enabled: Cell<bool>,
    listening: Cell<bool>,
    wheel: Cell<WheelNotchAccumulator>,
    touchpad: RefCell<TouchpadScrollRecognizer>,
    raw_buffer: RefCell<Vec<u8>>,
    location: Cell<Option<RECT>>,
    /// Tick of the last `Shell_NotifyIconGetRect`; 0 forces a refresh.
    location_tick: Cell<u64>,
    icon_update_queued: Cell<bool>,
    tooltip_update_queued: Cell<bool>,
    mouse_refresh_queued: Cell<bool>,
    recovery_queued: Cell<bool>,
    display_refresh_queued: Cell<bool>,
    events: RefCell<VecDeque<Pending>>,
    events_scheduled: Cell<bool>,
}

impl Shared {
    // ---- posting -------------------------------------------------------------------------------

    fn post(self: &Rc<Self>, f: impl FnOnce(&Rc<Self>) + 'static) {
        let weak = Rc::downgrade(self);
        post(move || run_live(&weak, f));
    }

    fn emit(self: &Rc<Self>, event: Pending) {
        if self.disposed.get() {
            return;
        }
        self.events.borrow_mut().push_back(event);
        if !self.events_scheduled.replace(true) {
            self.post(|s| s.drain_events());
        }
    }

    fn drain_events(self: &Rc<Self>) {
        self.events_scheduled.set(false);
        loop {
            if self.disposed.get() {
                self.events.borrow_mut().clear();
                return;
            }
            let Some(event) = self.events.borrow_mut().pop_front() else { return };
            (self.handler)(event.into_event(Modifiers::current()));
        }
    }

    // ---- shell data ----------------------------------------------------------------------------

    // One identity for the icon's full lifetime; switching between GUID and hWnd/uID can create a
    // second shell icon, so the fallback is decided once, at the first successful add.
    fn make_data(&self, flags: NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd.get(),
            uFlags: flags,
            ..Default::default()
        };
        if self.use_guid.get() {
            data.uFlags |= NIF_GUID;
            data.guidItem = self.guid;
        } else {
            data.uID = FALLBACK_ICON_UID;
        }
        if has(flags, NIF_MESSAGE) {
            data.uCallbackMessage = WM_TRAY_CALLBACK;
        }
        if has(flags, NIF_ICON) {
            data.hIcon = self.current_icon.get();
        }
        let tip = truncate_tooltip(&self.tooltip.borrow(), TOOLTIP_MAX_LENGTH);
        for (slot, unit) in data.szTip.iter_mut().zip(tip.encode_utf16().take(TOOLTIP_MAX_LENGTH)) {
            *slot = unit;
        }
        data
    }

    fn make_identifier(&self) -> NOTIFYICONIDENTIFIER {
        let mut id = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.hwnd.get(),
            ..Default::default()
        };
        if self.use_guid.get() {
            id.guidItem = self.guid;
        } else {
            id.uID = FALLBACK_ICON_UID;
        }
        id
    }

    /// `Shell_NotifyIconW`; the error is the thread's last Win32 error.
    fn notify(&self, message: NOTIFY_ICON_MESSAGE, data: &NOTIFYICONDATAW) -> Result<(), u32> {
        // SAFETY: `data` is a fully initialized, correctly sized NOTIFYICONDATAW.
        if unsafe { Shell_NotifyIconW(message, data) }.as_bool() {
            Ok(())
        } else {
            // SAFETY: no preconditions.
            Err(unsafe { GetLastError() }.0)
        }
    }

    fn tooltip_is_blank(&self) -> bool {
        self.tooltip.borrow().trim().is_empty()
    }

    fn tooltip_shell_flags(&self, include_empty_tip: bool) -> NOTIFY_ICON_DATA_FLAGS {
        if !self.tooltip_is_blank() {
            return NIF_TIP | NIF_SHOWTIP;
        }
        if include_empty_tip && self.tooltip_dirty.get() { NIF_TIP } else { NOTIFY_ICON_DATA_FLAGS(0) }
    }

    // ---- icon add / modify / delete ------------------------------------------------------------

    fn replace_current_icon(&self, icon: HICON) {
        let old = self.current_icon.replace(icon);
        if !old.is_invalid() && old != self.shell_icon.get() {
            destroy_icon(old);
        }
    }

    fn commit_shell_icon(&self) {
        let old = self.shell_icon.replace(self.current_icon.get());
        if !old.is_invalid() && old != self.shell_icon.get() {
            destroy_icon(old);
        }
    }

    fn request_icon_update(self: &Rc<Self>) {
        if self.disposed.get() || self.current_icon.get().is_invalid() || self.icon_update_queued.replace(true) {
            return;
        }
        self.post(|s| {
            s.icon_update_queued.set(false);
            s.update_icon_and_tooltip();
        });
    }

    fn update_icon_and_tooltip(self: &Rc<Self>) {
        if self.disposed.get() || self.current_icon.get().is_invalid() {
            return;
        }
        let force_full_update = self.force_full_update.get();
        let syncs_tooltip = force_full_update || self.tooltip_dirty.get();
        let full = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        let icon_flags = if force_full_update {
            full
        } else if self.tooltip_dirty.get() {
            NIF_ICON | self.tooltip_shell_flags(true)
        } else {
            NIF_ICON
        };

        if self.is_created.get() {
            let Err(modify_error) = self.try_modify(icon_flags, syncs_tooltip, false) else { return };
            let Err(add_error) = self.try_add(full, true) else { return };
            let Err(retry_error) = self.try_modify(full, true, true) else { return };
            log::warn!(
                "TrayIcon.update: NIM_MODIFY failed (0x{modify_error:08X}); recovery NIM_ADD failed \
                 (0x{add_error:08X}); retry NIM_MODIFY failed (0x{retry_error:08X})"
            );
            self.force_full_update.set(true);
            self.queue_taskbar_recovery();
            return;
        }

        // A killed instance leaves its icon behind until the pointer crosses it. The first add of
        // this process removes whatever is still registered under the same GUID, so the new icon
        // never sits next to a ghost.
        if !self.stale_icon_cleared.replace(true) {
            let _ = self.notify(NIM_DELETE, &self.make_data(NOTIFY_ICON_DATA_FLAGS(0)));
        }

        let Err(pre_add_modify_error) = self.try_modify(icon_flags, syncs_tooltip, true) else { return };
        let Err(add_error) = self.try_add(full, false) else { return };
        if self.use_guid.get() && self.try_add_with_window_identity(full, add_error) {
            return;
        }
        let Err(recovery_modify_error) = self.try_modify(full, true, true) else { return };

        self.is_created.set(false);
        log::warn!(
            "TrayIcon.update: pre-add NIM_MODIFY failed (0x{pre_add_modify_error:08X}); NIM_ADD failed \
             (0x{add_error:08X}); recovery NIM_MODIFY failed (0x{recovery_modify_error:08X})"
        );
        self.force_full_update.set(true);
        self.queue_taskbar_recovery();
    }

    /// The shell binds a tray GUID to one exe path, and dev builds run from several target dirs:
    /// when the GUID add fails but an hWnd/uID add works, the GUID was the problem, and the icon
    /// keeps the hWnd/uID identity for the rest of the session. When both fail the shell itself
    /// is not ready, and the GUID identity stays.
    fn try_add_with_window_identity(self: &Rc<Self>, flags: NOTIFY_ICON_DATA_FLAGS, guid_error: u32) -> bool {
        self.use_guid.set(false);
        match self.try_add(flags, false) {
            Ok(()) => {
                log::warn!(
                    "TrayIcon: NIM_ADD with tray GUID {:?} failed (0x{guid_error:08X}), probably because the shell \
                     bound that GUID to another exe path; using hWnd/uID identity for this session",
                    self.guid
                );
                true
            }
            Err(_) => {
                self.use_guid.set(true);
                false
            }
        }
    }

    fn try_modify(
        self: &Rc<Self>,
        flags: NOTIFY_ICON_DATA_FLAGS,
        syncs_tooltip: bool,
        set_version: bool,
    ) -> Result<(), u32> {
        let data = self.make_data(flags);
        self.notify(NIM_MODIFY, &data)?;

        let force_show_after_icon_change = self.tooltip_show_requested.get() || self.tooltip_keep_open.get();
        let should_refresh_visible_tooltip = force_show_after_icon_change || self.pointer_over.get();
        self.is_created.set(true);
        self.force_full_update.set(false);
        if syncs_tooltip {
            self.tooltip_dirty.set(false);
            self.tooltip_show_requested.set(false);
            self.tooltip_hover_sync_pending.set(false);
        }
        if set_version {
            self.set_version(&data);
        }
        self.complete_icon_update();
        if !syncs_tooltip || should_refresh_visible_tooltip {
            self.request_tooltip_update_after_icon_change(force_show_after_icon_change);
        }
        Ok(())
    }

    fn try_add(self: &Rc<Self>, flags: NOTIFY_ICON_DATA_FLAGS, preserve_created_on_failure: bool) -> Result<(), u32> {
        let data = self.make_data(flags);
        if let Err(error) = self.notify(NIM_ADD, &data) {
            if !preserve_created_on_failure {
                self.is_created.set(false);
            }
            return Err(error);
        }

        self.is_created.set(true);
        self.force_full_update.set(false);
        if has(data.uFlags, NIF_TIP) {
            self.tooltip_dirty.set(false);
            self.tooltip_show_requested.set(false);
            self.tooltip_hover_sync_pending.set(true);
        }
        self.set_version(&data);
        self.refresh_mouse_input_registration();
        self.complete_icon_update();
        Ok(())
    }

    fn set_version(&self, data: &NOTIFYICONDATAW) {
        let mut data = *data;
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = self.notify(NIM_SETVERSION, &data);
    }

    fn complete_icon_update(self: &Rc<Self>) {
        self.commit_shell_icon();
        self.location_tick.set(0);
        self.request_mouse_refresh();
    }

    fn dispose(&self) {
        if self.disposed.replace(true) {
            return;
        }
        self.stop_listening();
        if self.is_created.replace(false)
            && let Err(error) = self.notify(NIM_DELETE, &self.make_data(NOTIFY_ICON_DATA_FLAGS(0)))
        {
            log::warn!("TrayIcon: NIM_DELETE failed (0x{error:08X})");
        }
        let shell = self.shell_icon.replace(HICON::default());
        let current = self.current_icon.replace(HICON::default());
        if !current.is_invalid() {
            destroy_icon(current);
        }
        if !shell.is_invalid() && shell != current {
            destroy_icon(shell);
        }
        self.events.borrow_mut().clear();
        let window = self.window.borrow_mut().take();
        drop(window);
    }

    // ---- tooltip -------------------------------------------------------------------------------

    fn set_tooltip_text(&self, text: &str) {
        if *self.tooltip.borrow() == text {
            return;
        }
        *self.tooltip.borrow_mut() = text.to_owned();
        self.tooltip_dirty.set(true);
        self.tooltip_hover_sync_pending.set(true);
    }

    fn show_tooltip(self: &Rc<Self>) {
        if self.disposed.get() || self.tooltip_is_blank() {
            return;
        }
        self.tooltip_keep_open.set(true);
        self.tooltip_show_requested.set(true);
        self.request_tooltip_update();
        self.request_mouse_refresh();
    }

    fn sync_tooltip(self: &Rc<Self>, force: bool) {
        if self.disposed.get() {
            return;
        }
        if self.tooltip_is_blank() && !self.tooltip_dirty.get() {
            return;
        }
        if !force && !self.tooltip_dirty.get() && !self.tooltip_show_requested.get() {
            return;
        }

        match self.notify(NIM_MODIFY, &self.make_data(self.tooltip_shell_flags(true))) {
            Ok(()) => {
                self.is_created.set(true);
                self.tooltip_dirty.set(false);
                self.tooltip_show_requested.set(false);
                self.tooltip_hover_sync_pending.set(false);
                self.request_mouse_refresh();
            }
            Err(error) => {
                log::warn!("TrayIcon.sync_tooltip: NIM_MODIFY failed (0x{error:08X})");
                self.force_full_update.set(true);
                self.request_icon_update();
            }
        }
    }

    fn request_tooltip_update(self: &Rc<Self>) {
        if self.disposed.get() || (self.tooltip_is_blank() && !self.tooltip_dirty.get()) {
            return;
        }
        if self.tooltip_update_queued.replace(true) {
            return;
        }
        self.post(|s| {
            s.tooltip_update_queued.set(false);
            s.sync_tooltip(false);
        });
    }

    /// Re-shows a hover-requested tooltip after a shell icon swap.
    fn request_tooltip_update_after_icon_change(self: &Rc<Self>, force_show: bool) {
        if force_show || self.tooltip_keep_open.get() || self.pointer_over.get() {
            self.tooltip_show_requested.set(true);
            self.request_tooltip_update();
        }
    }

    /// Stops preserving the tooltip once the cursor has left the tray icon.
    fn clear_tooltip_keep_open_if_pointer_left(&self) {
        if !self.tooltip_keep_open.get() {
            return;
        }
        let inside = self.try_update_location() && cursor().is_some_and(|c| self.location_contains(c));
        if !inside {
            self.pointer_left();
        }
    }

    fn pointer_left(&self) {
        self.pointer_over.set(false);
        self.tooltip_hover_sync_pending.set(true);
        self.tooltip_keep_open.set(false);
    }

    // ---- icon location and raw input -----------------------------------------------------------

    /// `TryGetIconRect`.
    fn icon_rect(&self) -> Option<RECT> {
        let id = self.make_identifier();
        // SAFETY: `id` is a fully initialized NOTIFYICONIDENTIFIER.
        let rect = unsafe { Shell_NotifyIconGetRect(&id) }.ok()?;
        self.is_created.set(true);
        self.cache_location(rect);
        Some(rect)
    }

    fn try_update_location(&self) -> bool {
        let valid = self.location.get().is_some();
        if valid && tick_ms().saturating_sub(self.location_tick.get()) < TRAY_ICON_LOCATION_REFRESH_COOLDOWN_MS {
            return true;
        }
        let id = self.make_identifier();
        // SAFETY: `id` is a fully initialized NOTIFYICONIDENTIFIER.
        match unsafe { Shell_NotifyIconGetRect(&id) } {
            Ok(rect) => {
                self.is_created.set(true);
                self.cache_location(rect);
                true
            }
            Err(_) => valid,
        }
    }

    fn cache_location(&self, rect: RECT) {
        self.location.set(Some(rect));
        self.location_tick.set(tick_ms().max(1));
    }

    fn clear_location(&self) {
        self.location.set(None);
        self.location_tick.set(0);
    }

    /// Inclusive on all edges, like the C# `RECT.Contains`.
    fn location_contains(&self, point: POINT) -> bool {
        self.location
            .get()
            .is_some_and(|r| point.x >= r.left && point.x <= r.right && point.y >= r.top && point.y <= r.bottom)
    }

    fn request_mouse_refresh(self: &Rc<Self>) {
        if self.disposed.get() || !self.is_created.get() || self.mouse_refresh_queued.replace(true) {
            return;
        }
        self.post(|s| {
            s.mouse_refresh_queued.set(false);
            s.sync_tooltip(false);
            s.refresh_mouse_input_registration();
        });
    }

    fn refresh_mouse_input_registration(&self) {
        if !self.scroll_enabled.get() || !self.is_created.get() || self.disposed.get() || self.hwnd.get().is_invalid() {
            self.stop_listening();
            return;
        }
        if !self.try_update_location() {
            self.clear_location();
            self.stop_listening();
            return;
        }
        match cursor() {
            Some(point) => {
                self.update_input_registration_for_cursor(point);
            }
            None => self.stop_listening(),
        }
    }

    fn update_input_registration_for_cursor(&self, cursor: POINT) -> bool {
        let inside = self.location_contains(cursor);
        if inside {
            self.start_listening();
        } else {
            self.pointer_left();
            self.stop_listening();
        }
        inside
    }

    fn start_listening(&self) {
        if !self.listening.get() {
            self.listening.set(register_raw_input(self.hwnd.get()));
        }
    }

    fn stop_listening(&self) {
        if !self.listening.replace(false) {
            return;
        }
        self.touchpad.borrow_mut().reset();
        self.wheel.set(WheelNotchAccumulator::default());
        unregister_raw_input();
    }

    fn on_raw_input(self: &Rc<Self>, lparam: LPARAM) {
        if !self.scroll_enabled.get() || self.disposed.get() || !self.try_update_location() {
            self.stop_listening();
            return;
        }
        // SAFETY: no preconditions.
        let position = unsafe { GetMessagePos() };
        if !self.update_input_registration_for_cursor(point_from_packed(position as isize)) {
            return;
        }

        let mut buffer = self.raw_buffer.borrow_mut();
        let Some(packet) = read_raw_input(HRAWINPUT(lparam.0 as *mut _), &mut buffer) else { return };
        if packet.wheel_delta != 0 {
            let mut wheel = self.wheel.get();
            let notches = wheel.add(packet.wheel_delta);
            self.wheel.set(wheel);
            if notches != 0 {
                self.emit(Pending::Wheel(notches));
            }
            return;
        }
        if !self.touchpad_enabled.get() || !packet.is_hid() {
            return;
        }

        let now = tick_ms();
        let mut steps = 0;
        {
            let mut touchpad = self.touchpad.borrow_mut();
            for index in 0..packet.hid_report_count {
                if let Some(report_steps) = touchpad.try_process(packet.hid_report(&buffer, index), now) {
                    steps += report_steps;
                }
            }
        }
        drop(buffer);
        if steps == 0 {
            return;
        }
        log::debug!("TrayInputDiag.PrecisionTouchpad.Scroll: steps={steps}");
        for _ in 0..steps.unsigned_abs() {
            self.emit(Pending::TouchpadStep(steps.signum()));
        }
    }

    // ---- taskbar and display changes -----------------------------------------------------------

    /// Coalesces a burst of display and DPI messages into one refresh request.
    fn queue_display_refresh(self: &Rc<Self>) {
        if self.disposed.get() || self.display_refresh_queued.replace(true) {
            return;
        }
        self.post(|s| {
            s.display_refresh_queued.set(false);
            s.clear_location();
            s.emit(Pending::RefreshNeeded);
        });
    }

    fn queue_taskbar_recovery(self: &Rc<Self>) {
        if self.disposed.get() || self.recovery_queued.replace(true) {
            return;
        }
        self.post(|s| s.recover_after_taskbar_created());
    }

    fn recover_after_taskbar_created(self: &Rc<Self>) {
        // TaskbarCreated can be rebroadcast while Explorer still owns the icon.
        if self.icon_rect().is_some() {
            self.recovery_queued.set(false);
            return;
        }
        self.is_created.set(false);
        self.clear_location();
        self.stop_listening();
        self.recovery_attempt(0);
    }

    fn recovery_attempt(self: &Rc<Self>, attempt: u32) {
        if self.disposed.get() {
            self.recovery_queued.set(false);
            return;
        }
        self.update_icon_and_tooltip();
        self.emit(Pending::RefreshNeeded);
        if self.is_created.get() {
            self.emit(Pending::Recreated);
            self.recovery_queued.set(false);
            return;
        }
        if attempt + 1 >= TASKBAR_RECOVERY_RETRY_COUNT {
            log::warn!("TrayIcon: the icon could not be re-added after {TASKBAR_RECOVERY_RETRY_COUNT} attempts");
            self.recovery_queued.set(false);
            return;
        }
        let weak = Rc::downgrade(self);
        post_delayed(Duration::from_millis(TASKBAR_RECOVERY_RETRY_DELAY_MS), move || {
            if let Some(s) = weak.upgrade() {
                s.recovery_attempt(attempt + 1);
            }
        });
    }

    // ---- window procedure ----------------------------------------------------------------------

    fn on_message(self: &Rc<Self>, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        if self.disposed.get() {
            return None;
        }
        if message == self.taskbar_created {
            self.queue_taskbar_recovery();
            return Some(LRESULT(0));
        }
        match message {
            // The taskbar's icon size follows the primary display's scale, so a display or DPI
            // change asks the app for a refresh; it re-renders only when the icon size moved.
            WM_DISPLAYCHANGE | WM_DPICHANGED => {
                self.queue_display_refresh();
                None
            }
            WM_INPUT => {
                self.on_raw_input(lparam);
                // DefWindowProcW performs the raw input cleanup for RIM_INPUT.
                None
            }
            WM_TRAY_CALLBACK => {
                self.on_shell_callback((lparam.0 & 0xFFFF) as u32, point_from_packed(wparam.0 as isize));
                Some(LRESULT(0))
            }
            _ => None,
        }
    }

    /// NOTIFYICON_VERSION_4: LOWORD(lParam) is the notification and wParam the anchor point.
    /// Windows 11 sends the button release and then its version 4 notification for each gesture
    /// (`WM_LBUTTONUP` + `NIN_SELECT`, `WM_RBUTTONUP` + `WM_CONTEXTMENU`); one event is raised per
    /// gesture. The classic area sends a keyboard menu request as `WM_CONTEXTMENU` alone.
    fn on_shell_callback(self: &Rc<Self>, notification: u32, point: POINT) {
        log::trace!("TrayIcon: shell callback 0x{notification:04X} at ({}, {})", point.x, point.y);
        self.is_created.set(true);
        match notification {
            WM_LBUTTONDOWN => self.has_processed_button_up.set(false),
            NIN_SELECT | WM_LBUTTONUP => {
                if !self.has_processed_button_up.replace(true) {
                    self.emit(Pending::LeftClick(point.x, point.y));
                }
            }
            WM_LBUTTONDBLCLK => self.emit(Pending::DoubleClick(point.x, point.y)),
            NIN_KEYSELECT => self.emit(Pending::KeySelect),
            WM_RBUTTONDOWN => self.has_processed_right_button_up.set(false),
            WM_RBUTTONUP => {
                self.has_processed_right_button_up.set(true);
                self.emit(Pending::RightClick(point.x, point.y));
            }
            WM_CONTEXTMENU => {
                if !self.has_processed_right_button_up.replace(false) {
                    self.emit(Pending::RightClick(point.x, point.y));
                }
            }
            WM_MOUSEMOVE => {
                let is_new_hover = !self.pointer_over.replace(true);
                self.sync_tooltip(is_new_hover || self.tooltip_hover_sync_pending.get());
                self.refresh_mouse_input_registration();
            }
            NIN_POPUPOPEN => {
                self.tooltip_keep_open.set(true);
                self.pointer_over.set(true);
                self.tooltip_show_requested.set(false);
                self.tooltip_hover_sync_pending.set(false);
            }
            NIN_POPUPCLOSE => self.clear_tooltip_keep_open_if_pointer_left(),
            _ => {}
        }
    }
}

fn run_live(weak: &Weak<Shared>, f: impl FnOnce(&Rc<Shared>)) {
    if let Some(shared) = weak.upgrade()
        && !shared.disposed.get()
    {
        f(&shared);
    }
}

fn has(flags: NOTIFY_ICON_DATA_FLAGS, flag: NOTIFY_ICON_DATA_FLAGS) -> bool {
    flags.0 & flag.0 != 0
}

fn destroy_icon(icon: HICON) {
    // SAFETY: the icon was created by this module and is no longer referenced by the shell.
    if let Err(error) = unsafe { DestroyIcon(icon) } {
        log::debug!("TrayIcon: DestroyIcon failed: {error}");
    }
}

fn cursor() -> Option<POINT> {
    let mut point = POINT::default();
    // SAFETY: out-pointer to a local.
    unsafe { GetCursorPos(&mut point) }.ok().map(|()| point)
}

/// Signed 16-bit (x, y) packed in the low 32 bits.
fn point_from_packed(packed: isize) -> POINT {
    POINT { x: (packed & 0xFFFF) as u16 as i16 as i32, y: ((packed >> 16) & 0xFFFF) as u16 as i16 as i32 }
}

// ---- tooltip text -------------------------------------------------------------------------------

/// `TrayTooltipText.MaxLength`: `NOTIFYICONDATAW.szTip` holds 128 UTF-16 units with its NUL.
pub const TOOLTIP_MAX_LENGTH: usize = 127;

/// `TrayTooltipText.Truncate`: text that fits `max_len` UTF-16 units. Longer text is cut after its
/// last complete line that fits, or, when not even the first line fits, at the last whole
/// character, never between the two halves of a surrogate pair. An ellipsis marks the cut.
pub fn truncate_tooltip(text: &str, max_len: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let max_len = max_len.max(1);
    if units.len() <= max_len {
        return text.to_owned();
    }
    let budget = max_len - 1;
    let line_break = units[..=budget].iter().rposition(|&u| u == u16::from(b'\n'));
    let mut cut = match line_break {
        Some(index) if index > 0 => index,
        _ => budget,
    };
    if cut > 0 && (0xD800..0xDC00).contains(&units[cut - 1]) {
        cut -= 1;
    }
    let mut result = String::from_utf16_lossy(&units[..cut]).trim_end().to_owned();
    result.push('\u{2026}');
    result
}

// ---- wheel ----------------------------------------------------------------------------------------

/// `TrayWheelNotchAccumulator.WheelDeltaPerNotch`.
pub const WHEEL_DELTA_PER_NOTCH: i32 = 120;

/// Turns raw wheel deltas into whole notches (port of `TrayWheelNotchAccumulator`). A standard
/// wheel reports 120 per detent; a high-resolution wheel reports fractions of it, which accumulate
/// until they make a notch. Turning the other way drops the remainder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WheelNotchAccumulator {
    remainder: i32,
}

impl WheelNotchAccumulator {
    /// Adds a raw wheel delta and returns the whole notches it completes, signed like the delta.
    pub fn add(&mut self, wheel_delta: i32) -> i32 {
        if wheel_delta == 0 {
            return 0;
        }
        if wheel_delta.signum() != self.remainder.signum() {
            self.remainder = 0;
        }
        let total = self.remainder + wheel_delta;
        let notches = total / WHEEL_DELTA_PER_NOTCH;
        self.remainder = total - notches * WHEEL_DELTA_PER_NOTCH;
        notches
    }

    pub fn reset(&mut self) {
        self.remainder = 0;
    }
}

// ---- precision touchpad ---------------------------------------------------------------------------

/// `PrecisionTouchpadScrollRecognizer`: two-finger vertical scrolling from raw HID touchpad
/// reports with a fixed layout (report id 4; `report[1]` = contact id << 4 | flags; Y = u16 LE at
/// 4..6; scan time = u16 LE at 6..8). A frame is the run of reports sharing a scan time and is
/// completed when a report with a different scan time arrives.
#[derive(Clone, Debug)]
pub struct TouchpadScrollRecognizer {
    frame_contact_seen: [bool; 16],
    has_frame: bool,
    frame_scan_time: u16,
    frame_contact_count: i32,
    frame_y_sum: f64,
    has_last_frame: bool,
    last_average_y: f64,
    last_frame_tick: u64,
    accumulator: f64,
    units_per_scroll_step: i32,
}

impl TouchpadScrollRecognizer {
    /// `ReportByteCount`.
    const REPORT_BYTE_COUNT: usize = 12;
    /// `ReportId`.
    const REPORT_ID: u8 = 0x04;
    /// `MaxFrameGapMs`: frames further apart start a new gesture.
    const MAX_FRAME_GAP_MS: u64 = 200;
    /// `ActiveContactFlags` (tip switch + confidence).
    const ACTIVE_CONTACT_FLAGS: u8 = 0x03;

    pub fn new(units_per_scroll_step: i32) -> Self {
        Self {
            frame_contact_seen: [false; 16],
            has_frame: false,
            frame_scan_time: 0,
            frame_contact_count: 0,
            frame_y_sum: 0.0,
            has_last_frame: false,
            last_average_y: 0.0,
            last_frame_tick: 0,
            accumulator: 0.0,
            units_per_scroll_step: units_per_scroll_step.max(1),
        }
    }

    pub fn units_per_scroll_step(&self) -> i32 {
        self.units_per_scroll_step
    }

    /// Changing the step size resets the recognizer.
    pub fn set_units_per_scroll_step(&mut self, units: i32) {
        let next = units.max(1);
        if self.units_per_scroll_step != next {
            self.units_per_scroll_step = next;
            self.reset();
        }
    }

    /// Feeds one HID report at `now` (ms). Returns None when the report is not a touchpad contact
    /// report, else the steps (positive = fingers moved up) completed by the frame it closed.
    pub fn try_process(&mut self, report: &[u8], now: u64) -> Option<i32> {
        if report.len() < Self::REPORT_BYTE_COUNT || report[0] != Self::REPORT_ID {
            return None;
        }
        let contact_id = usize::from(report[1] >> 4);
        let active = (report[1] & 0x0F) & Self::ACTIVE_CONTACT_FLAGS == Self::ACTIVE_CONTACT_FLAGS;
        let y = u16::from_le_bytes([report[4], report[5]]);
        let scan_time = u16::from_le_bytes([report[6], report[7]]);

        let mut steps = 0;
        if !self.has_frame || scan_time != self.frame_scan_time {
            steps += self.complete_frame(now);
            self.clear_frame();
            self.has_frame = true;
            self.frame_scan_time = scan_time;
        }
        if active && contact_id < self.frame_contact_seen.len() && !self.frame_contact_seen[contact_id] {
            self.frame_contact_seen[contact_id] = true;
            self.frame_contact_count += 1;
            self.frame_y_sum += f64::from(y);
        }
        Some(steps)
    }

    pub fn reset(&mut self) {
        self.clear_frame();
        self.has_last_frame = false;
        self.last_average_y = 0.0;
        self.last_frame_tick = 0;
        self.accumulator = 0.0;
    }

    fn complete_frame(&mut self, now: u64) -> i32 {
        if !self.has_frame {
            return 0;
        }
        let mut steps = 0;
        if self.frame_contact_count >= 2 {
            let average_y = self.frame_y_sum / f64::from(self.frame_contact_count);
            if self.has_last_frame && now.saturating_sub(self.last_frame_tick) <= Self::MAX_FRAME_GAP_MS {
                // Finger movement upward lowers the HID Y coordinate: that is a positive delta.
                self.accumulator += self.last_average_y - average_y;
                steps = self.drain_steps();
            } else {
                self.accumulator = 0.0;
            }
            self.has_last_frame = true;
            self.last_average_y = average_y;
            self.last_frame_tick = now;
        } else {
            self.has_last_frame = false;
            self.accumulator = 0.0;
        }
        self.clear_frame();
        steps
    }

    fn drain_steps(&mut self) -> i32 {
        let unit = f64::from(self.units_per_scroll_step);
        let mut steps = 0;
        while self.accumulator >= unit {
            steps += 1;
            self.accumulator -= unit;
        }
        while self.accumulator <= -unit {
            steps -= 1;
            self.accumulator += unit;
        }
        steps
    }

    fn clear_frame(&mut self) {
        self.frame_contact_seen = [false; 16];
        self.has_frame = false;
        self.frame_scan_time = 0;
        self.frame_contact_count = 0;
        self.frame_y_sum = 0.0;
    }
}

// ---- raw input ------------------------------------------------------------------------------------

/// `HID_USAGE_PAGE_GENERIC` / `HID_USAGE_PAGE_DIGITIZER` usages registered while hovering: generic
/// mouse, generic pointer, generic wheel and digitizer touch pad.
const RAW_INPUT_USAGES: [(u16, u16); 4] = [(0x01, 0x02), (0x01, 0x01), (0x01, 0x38), (0x0D, 0x05)];

fn register_raw_input(hwnd: HWND) -> bool {
    let mut any = false;
    for (page, usage) in RAW_INPUT_USAGES {
        let device = RAWINPUTDEVICE {
            usUsagePage: page,
            usUsage: usage,
            dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
            hwndTarget: hwnd,
        };
        // SAFETY: one fully initialized RAWINPUTDEVICE with its exact size.
        match unsafe { RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32) } {
            Ok(()) => any = true,
            Err(error) => {
                log::debug!("TrayInputDiag.RawInput.Register failed: page=0x{page:02X} usage=0x{usage:02X}: {error}")
            }
        }
    }
    any
}

fn unregister_raw_input() {
    for (page, usage) in RAW_INPUT_USAGES {
        let device =
            RAWINPUTDEVICE { usUsagePage: page, usUsage: usage, dwFlags: RIDEV_REMOVE, hwndTarget: HWND::default() };
        // SAFETY: one fully initialized RAWINPUTDEVICE with its exact size.
        if let Err(error) = unsafe { RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32) } {
            log::debug!("TrayInputDiag.RawInput.Unregister failed: page=0x{page:02X} usage=0x{usage:02X}: {error}");
        }
    }
}

/// Copies one WM_INPUT packet into `buffer` (growing it only when a packet is larger than any
/// before) and decodes it (`InputHelper.TryReadRawInput`).
fn read_raw_input(handle: HRAWINPUT, buffer: &mut Vec<u8>) -> Option<RawInputPacket> {
    let header_size = size_of::<RAWINPUTHEADER>() as u32;
    let mut size = 0u32;
    // SAFETY: size query with a null buffer.
    if unsafe { GetRawInputData(handle, RID_INPUT, None, &mut size, header_size) } != 0 || size < header_size {
        return None;
    }
    if buffer.len() < size as usize {
        buffer.resize(size as usize, 0);
    }
    // SAFETY: `buffer` holds at least `size` bytes.
    let written =
        unsafe { GetRawInputData(handle, RID_INPUT, Some(buffer.as_mut_ptr().cast()), &mut size, header_size) };
    if written != size {
        return None;
    }
    let packet = RawInputPacket::decode(&buffer[..written as usize]);
    (packet.length > 0).then_some(packet)
}

/// The decoded header of one raw input packet (`InputHelper.RawInputPacket`). Offsets follow the
/// native layout: `RAWINPUTHEADER` (24 bytes on x64), then `RAWMOUSE` or `RAWHID`'s two counts and
/// its report bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawInputPacket {
    pub kind: u32,
    pub mouse_button_flags: u16,
    pub wheel_delta: i32,
    pub horizontal_wheel_delta: i32,
    pub hid_report_size: usize,
    pub hid_report_count: usize,
    pub length: usize,
}

impl RawInputPacket {
    const UNION_OFFSET: usize = size_of::<RAWINPUTHEADER>();
    const HID_DATA_OFFSET: usize = Self::UNION_OFFSET + 8;
    /// `sizeof(RAWMOUSE)`.
    const RAW_MOUSE_SIZE: usize = 24;
    const RI_MOUSE_WHEEL: u16 = 0x0400;
    const RI_MOUSE_HWHEEL: u16 = 0x0800;

    pub fn is_mouse(&self) -> bool {
        self.kind == RIM_TYPEMOUSE.0
    }

    pub fn is_hid(&self) -> bool {
        self.kind == RIM_TYPEHID.0
    }

    /// One HID report of this packet from the buffer it was read into (empty when out of range).
    pub fn hid_report<'a>(&self, buffer: &'a [u8], index: usize) -> &'a [u8] {
        if !self.is_hid() || index >= self.hid_report_count || self.hid_report_size == 0 {
            return &[];
        }
        let start = Self::HID_DATA_OFFSET + index * self.hid_report_size;
        let end = start + self.hid_report_size;
        if end > self.length || end > buffer.len() {
            return &[];
        }
        &buffer[start..end]
    }

    /// Decodes the header, the mouse fields and the HID report layout of one packet without
    /// reading past `data`.
    pub fn decode(data: &[u8]) -> Self {
        if data.len() < Self::UNION_OFFSET {
            return Self::default();
        }
        let u32_at =
            |offset: usize| u32::from_le_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]);
        let u16_at = |offset: usize| u16::from_le_bytes([data[offset], data[offset + 1]]);
        let kind = u32_at(0);
        let mut packet = Self { kind, length: data.len(), ..Self::default() };
        if kind == RIM_TYPEMOUSE.0 && data.len() >= Self::UNION_OFFSET + Self::RAW_MOUSE_SIZE {
            // RAWMOUSE: usFlags u16 @0, padding, usButtonFlags u16 @4, usButtonData u16 @6.
            let flags = u16_at(Self::UNION_OFFSET + 4);
            let data = u16_at(Self::UNION_OFFSET + 6) as i16 as i32;
            packet.mouse_button_flags = flags;
            packet.wheel_delta = if flags & Self::RI_MOUSE_WHEEL != 0 { data } else { 0 };
            packet.horizontal_wheel_delta = if flags & Self::RI_MOUSE_HWHEEL != 0 { data } else { 0 };
        } else if kind == RIM_TYPEHID.0 && data.len() >= Self::HID_DATA_OFFSET {
            let report_size = u32_at(Self::UNION_OFFSET) as usize;
            let count = u32_at(Self::UNION_OFFSET + 4) as usize;
            let available = data.len() - Self::HID_DATA_OFFSET;
            packet.hid_report_size = report_size;
            packet.hid_report_count = available.checked_div(report_size).map_or(0, |fit| count.min(fit));
        }
        packet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_deltas_accumulate_into_whole_notches() {
        let mut accumulator = WheelNotchAccumulator::default();
        assert_eq!(accumulator.add(120), 1);
        assert_eq!(accumulator.add(240), 2);
        assert_eq!(accumulator.add(60), 0);
        assert_eq!(accumulator.add(40), 0);
        assert_eq!(accumulator.add(40), 1);
        assert_eq!(accumulator.add(0), 0);
    }

    #[test]
    fn turning_the_wheel_back_drops_the_remainder() {
        let mut accumulator = WheelNotchAccumulator::default();
        assert_eq!(accumulator.add(100), 0);
        assert_eq!(accumulator.add(-120), -1);
        assert_eq!(accumulator.add(-60), 0);
        assert_eq!(accumulator.add(-60), -1);

        accumulator.add(-100);
        accumulator.reset();
        assert_eq!(accumulator.add(-20), 0);
    }

    #[test]
    fn tooltip_text_that_fits_is_returned_unchanged() {
        let text = "x".repeat(TOOLTIP_MAX_LENGTH);
        assert_eq!(truncate_tooltip(&text, TOOLTIP_MAX_LENGTH), text);
    }

    #[test]
    fn long_tooltip_text_is_cut_after_its_last_complete_line() {
        assert_eq!(truncate_tooltip("CPU: 12%\nMemory: 40%\nDisk: 3%", 25), "CPU: 12%\nMemory: 40%\u{2026}");
    }

    #[test]
    fn a_long_first_line_is_cut_at_the_last_character_that_fits() {
        assert_eq!(truncate_tooltip("abcdefgh", 5), "abcd\u{2026}");
    }

    #[test]
    fn tooltip_text_is_never_cut_inside_a_surrogate_pair() {
        assert_eq!(truncate_tooltip("abc\u{1F600}def", 5), "abc\u{2026}");
    }

    #[test]
    fn truncated_tooltip_text_fits_the_shell_buffer() {
        let text = (0..40).map(|i| format!("Adapter {i}: connected")).collect::<Vec<_>>().join("\n");
        let truncated = truncate_tooltip(&text, TOOLTIP_MAX_LENGTH);
        assert!(truncated.encode_utf16().count() <= TOOLTIP_MAX_LENGTH);
        assert!(truncated.ends_with('\u{2026}'));
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum ModifiedAction {
        Nothing,
        Default,
        CtrlAction,
        AltAction,
    }

    #[test]
    fn modified_clicks_pick_their_action() {
        let cases = [
            (false, false, ModifiedAction::Default),
            (true, false, ModifiedAction::CtrlAction),
            (false, true, ModifiedAction::AltAction),
            (true, true, ModifiedAction::CtrlAction),
        ];
        for (ctrl, alt, expected) in cases {
            let modifiers = Modifiers { ctrl, alt, shift: false };
            let resolved = modifiers.resolve(
                ModifiedAction::CtrlAction,
                ModifiedAction::AltAction,
                ModifiedAction::Default,
                ModifiedAction::Nothing,
            );
            assert_eq!(resolved, expected, "ctrl={ctrl} alt={alt}");
        }
    }

    #[test]
    fn a_modifier_set_to_nothing_falls_back_to_the_normal_click() {
        for (ctrl, alt) in [(true, false), (false, true), (true, true)] {
            let modifiers = Modifiers { ctrl, alt, shift: false };
            let resolved = modifiers.resolve(
                ModifiedAction::Nothing,
                ModifiedAction::Nothing,
                ModifiedAction::Default,
                ModifiedAction::Nothing,
            );
            assert_eq!(resolved, ModifiedAction::Default);
        }
    }

    fn report(contact_id: u8, active: bool, y: u16, scan_time: u16) -> [u8; 12] {
        let mut bytes = [0u8; 12];
        bytes[0] = 0x04;
        bytes[1] = (contact_id << 4) | if active { 0x03 } else { 0x01 };
        bytes[4..6].copy_from_slice(&y.to_le_bytes());
        bytes[6..8].copy_from_slice(&scan_time.to_le_bytes());
        bytes
    }

    /// Feeds one two-finger frame (both contacts at `y`) and returns the steps it completed.
    fn two_finger_frame(recognizer: &mut TouchpadScrollRecognizer, y: u16, scan_time: u16, now: u64) -> i32 {
        recognizer.try_process(&report(0, true, y, scan_time), now).unwrap_or(0)
            + recognizer.try_process(&report(1, true, y, scan_time), now).unwrap_or(0)
    }

    #[test]
    fn touchpad_rejects_foreign_reports() {
        let mut recognizer = TouchpadScrollRecognizer::new(90);
        assert_eq!(recognizer.try_process(&[0x04; 11], 0), None);
        let mut wrong_id = report(0, true, 100, 1);
        wrong_id[0] = 0x05;
        assert_eq!(recognizer.try_process(&wrong_id, 0), None);
    }

    #[test]
    fn two_fingers_moving_up_scroll_up_one_step_per_unit_budget() {
        let mut recognizer = TouchpadScrollRecognizer::new(90);
        let mut steps = two_finger_frame(&mut recognizer, 1000, 1, 0);
        steps += two_finger_frame(&mut recognizer, 950, 2, 10);
        steps += two_finger_frame(&mut recognizer, 900, 3, 20);
        // Frame 3 completes only when the next frame starts.
        assert_eq!(steps, 0);
        steps += two_finger_frame(&mut recognizer, 900, 4, 30);
        // 1000 → 950 → 900: 100 units up = one 90-unit step, 10 left over.
        assert_eq!(steps, 1);
    }

    #[test]
    fn two_fingers_moving_down_scroll_down() {
        let mut recognizer = TouchpadScrollRecognizer::new(50);
        let mut steps = 0;
        for (index, y) in [100u16, 160, 220, 220].into_iter().enumerate() {
            steps += two_finger_frame(&mut recognizer, y, index as u16 + 1, index as u64 * 10);
        }
        // 120 units down at 50 per step = -2.
        assert_eq!(steps, -2);
    }

    #[test]
    fn one_finger_or_a_long_gap_resets_the_gesture() {
        let mut recognizer = TouchpadScrollRecognizer::new(90);
        two_finger_frame(&mut recognizer, 1000, 1, 0);
        // One finger: no scroll, gesture forgotten.
        recognizer.try_process(&report(0, true, 500, 2), 10);
        assert_eq!(two_finger_frame(&mut recognizer, 400, 3, 20), 0);
        assert_eq!(two_finger_frame(&mut recognizer, 200, 4, 30), 0);
        // Frame at 400 vs 200 would be 200 units, but it arrives after more than 200 ms.
        assert_eq!(two_finger_frame(&mut recognizer, 200, 5, 300), 0);
    }

    #[test]
    fn inactive_and_duplicate_contacts_are_ignored() {
        let mut recognizer = TouchpadScrollRecognizer::new(10);
        // Contact 0 twice + an inactive contact 1: only one counted contact, so no gesture.
        recognizer.try_process(&report(0, true, 100, 1), 0);
        recognizer.try_process(&report(0, true, 100, 1), 0);
        recognizer.try_process(&report(1, false, 100, 1), 0);
        assert_eq!(recognizer.try_process(&report(0, true, 0, 2), 10), Some(0));
    }

    #[test]
    fn changing_units_per_step_resets_and_clamps() {
        let mut recognizer = TouchpadScrollRecognizer::new(0);
        assert_eq!(recognizer.units_per_scroll_step(), 1);
        two_finger_frame(&mut recognizer, 1000, 1, 0);
        two_finger_frame(&mut recognizer, 900, 2, 10);
        recognizer.set_units_per_scroll_step(30);
        // The pending frame at 900 was discarded with the reset.
        assert_eq!(two_finger_frame(&mut recognizer, 800, 3, 20), 0);
    }

    const HEADER_SIZE: usize = 24;
    const HID_DATA_OFFSET: usize = HEADER_SIZE + 8;

    fn hid_packet(report_size: usize, report_count: usize) -> Vec<u8> {
        let mut packet = vec![0u8; HID_DATA_OFFSET + report_size * report_count];
        let len = packet.len() as u32;
        packet[0..4].copy_from_slice(&RIM_TYPEHID.0.to_le_bytes());
        packet[4..8].copy_from_slice(&len.to_le_bytes());
        packet[HEADER_SIZE..HEADER_SIZE + 4].copy_from_slice(&(report_size as u32).to_le_bytes());
        packet[HEADER_SIZE + 4..HEADER_SIZE + 8].copy_from_slice(&(report_count as u32).to_le_bytes());
        packet
    }

    #[test]
    fn a_short_hid_packet_decodes_within_its_own_length() {
        let mut packet = hid_packet(12, 1);
        for index in 0..12 {
            packet[HID_DATA_OFFSET + index] = index as u8 + 1;
        }
        let decoded = RawInputPacket::decode(&packet);
        assert_eq!(decoded.length, 44);
        assert_eq!(decoded.hid_report_count, 1);
        assert_eq!(decoded.hid_report_size, 12);
        assert_eq!(decoded.hid_report(&packet, 0), &packet[HID_DATA_OFFSET..HID_DATA_OFFSET + 12]);
        assert!(decoded.hid_report(&packet, 1).is_empty());
    }

    #[test]
    fn report_count_is_clamped_to_the_bytes_the_packet_holds() {
        let mut packet = hid_packet(12, 1);
        packet[HEADER_SIZE + 4..HEADER_SIZE + 8].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(RawInputPacket::decode(&packet).hid_report_count, 1);
    }

    #[test]
    fn a_truncated_mouse_packet_does_not_read_past_its_end() {
        let mut packet = vec![0u8; HEADER_SIZE + 4];
        packet[0..4].copy_from_slice(&RIM_TYPEMOUSE.0.to_le_bytes());
        let decoded = RawInputPacket::decode(&packet);
        assert!(decoded.is_mouse());
        assert_eq!(decoded.wheel_delta, 0);
        assert_eq!(decoded.mouse_button_flags, 0);
    }

    #[test]
    fn a_mouse_wheel_packet_decodes_its_signed_delta() {
        let mut packet = vec![0u8; HEADER_SIZE + 24];
        packet[0..4].copy_from_slice(&RIM_TYPEMOUSE.0.to_le_bytes());
        packet[HEADER_SIZE + 4..HEADER_SIZE + 6].copy_from_slice(&0x0400u16.to_le_bytes());
        packet[HEADER_SIZE + 6..HEADER_SIZE + 8].copy_from_slice(&(-120i16).to_le_bytes());
        let decoded = RawInputPacket::decode(&packet);
        assert_eq!(decoded.wheel_delta, -120);
        assert_eq!(decoded.horizontal_wheel_delta, 0);
    }

    #[test]
    fn packed_points_are_signed_shorts() {
        let packed = ((-5i16 as u16 as isize) << 16) | (-1200i16 as u16 as isize);
        assert_eq!(point_from_packed(packed), POINT { x: -1200, y: -5 });
    }
}
