//! Color picker (port of `UI/Controls/ColorPickerWindow.cs`): the HSV/alpha/hex color model and
//! the non-modal picker window with its 50 ms live-preview cadence. The settings window decides
//! what a preview or a commit means for the edited setting.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use slint::{ComponentHandle, Timer, TimerMode};
use tadn_common::theme::{Argb, Palette};
use windows::Win32::Foundation::HWND;

use crate::strings as s;
use crate::ui::{self, ColorPickerWindow};

/// `TimeConstants.ColorPickerChangeCooldownMs`: live previews are sent at most this often.
pub(crate) const COLOR_PICKER_CHANGE_COOLDOWN: Duration = Duration::from_millis(50);

fn channels(c: Argb) -> (u8, u8, u8, u8) {
    ((c.0 >> 24) as u8, (c.0 >> 16) as u8, (c.0 >> 8) as u8, c.0 as u8)
}

fn from_channels(a: u8, r: u8, g: u8, b: u8) -> Argb {
    Argb((u32::from(a) << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b))
}

/// `ToByte`: `(byte)Math.Round(clamp(v, 0, 255))`.
fn to_byte(v: f64) -> u8 {
    v.clamp(0.0, 255.0).round_ties_even() as u8
}

/// `HsvToRgb` (alpha always 0xFF).
pub(crate) fn hsv_to_rgb(hue: f64, saturation: f64, value: f64) -> Argb {
    let s = saturation.clamp(0.0, 1.0);
    let v = value.clamp(0.0, 1.0);
    if s <= 0.0 {
        let gray = to_byte(v * 255.0);
        return from_channels(0xFF, gray, gray, gray);
    }
    let h = (hue % 360.0 + 360.0) % 360.0 / 60.0;
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i as i32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    from_channels(0xFF, to_byte(r * 255.0), to_byte(g * 255.0), to_byte(b * 255.0))
}

/// `RgbToHsv`: hue in [0, 360), saturation and value in [0, 1].
pub(crate) fn rgb_to_hsv(color: Argb) -> (f64, f64, f64) {
    let (_, r, g, b) = channels(color);
    let (r, g, b) = (f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    let mut hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    if hue < 0.0 {
        hue += 360.0;
    }
    (hue, saturation, max)
}

/// Hex box layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HexOrder {
    /// `RRGGBBAA`.
    Rgba,
    /// `AARRGGBB`.
    Argb,
}

/// Hex box parse: trim, strip leading `#`, exactly 6 (`RRGGBB`, A=FF) or 8 digits.
pub(crate) fn parse_hex(text: &str, order: HexOrder) -> Option<Argb> {
    let digits = text.trim().trim_start_matches('#');
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
    match digits.len() {
        6 => Some(from_channels(0xFF, byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(match order {
            HexOrder::Rgba => from_channels(byte(6)?, byte(0)?, byte(2)?, byte(4)?),
            HexOrder::Argb => from_channels(byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        }),
        _ => None,
    }
}

pub(crate) fn format_hex(color: Argb, order: HexOrder) -> String {
    let (a, r, g, b) = channels(color);
    match order {
        HexOrder::Rgba => format!("{r:02X}{g:02X}{b:02X}{a:02X}"),
        HexOrder::Argb => format!("{a:02X}{r:02X}{g:02X}{b:02X}"),
    }
}

/// Where a change came from (the free hue is only re-read for changes from elsewhere).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    HueOrSquare,
    Other,
}

/// The picker's color state.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PickerModel {
    pub color: Argb,
    pub opening: Argb,
    pub default: Argb,
    /// `freePickHue`: kept across grays so the square keeps its hue.
    pub free_hue: f64,
}

impl PickerModel {
    pub(crate) fn new(start: Argb, default: Argb) -> Self {
        let (hue, saturation, _) = rgb_to_hsv(start);
        Self { color: start, opening: start, default, free_hue: if saturation > 0.0 { hue } else { 0.0 } }
    }

    /// `ApplyColor`: returns whether a preview should be queued.
    fn apply(&mut self, color: Argb, source: Source, forced: bool) -> bool {
        if !forced && color == self.color {
            return false;
        }
        self.color = color;
        if source == Source::Other {
            let (hue, saturation, _) = rgb_to_hsv(color);
            if saturation > 0.0 {
                self.free_hue = hue;
            }
        }
        true
    }

    pub(crate) fn square(&mut self, saturation: f64, value: f64) -> bool {
        let rgb = hsv_to_rgb(self.free_hue, saturation, value);
        let color = Argb((self.color.0 & 0xFF00_0000) | (rgb.0 & 0x00FF_FFFF));
        self.apply(color, Source::HueOrSquare, false)
    }

    pub(crate) fn hue(&mut self, hue: f64) -> bool {
        self.free_hue = hue.clamp(0.0, 360.0);
        let (_, saturation, value) = rgb_to_hsv(self.color);
        let rgb = hsv_to_rgb(self.free_hue, saturation, value);
        let color = Argb((self.color.0 & 0xFF00_0000) | (rgb.0 & 0x00FF_FFFF));
        self.apply(color, Source::HueOrSquare, true)
    }

    /// Channel slider: 0 = alpha, 1 = red, 2 = green, 3 = blue.
    pub(crate) fn channel(&mut self, channel: usize, value: f64) -> bool {
        let (mut a, mut r, mut g, mut b) = channels(self.color);
        let v = to_byte(value);
        match channel {
            0 => a = v,
            1 => r = v,
            2 => g = v,
            _ => b = v,
        }
        self.apply(from_channels(a, r, g, b), Source::Other, false)
    }

    pub(crate) fn hex(&mut self, text: &str, order: HexOrder) -> bool {
        match parse_hex(text, order) {
            Some(color) => self.apply(color, Source::Other, false),
            None => false,
        }
    }

    pub(crate) fn reset_to_default(&mut self) -> bool {
        self.apply(self.default, Source::Other, true)
    }

    pub(crate) fn reset_to_opening(&mut self) -> bool {
        self.apply(self.opening, Source::Other, true)
    }

    pub(crate) fn changed(&self) -> bool {
        self.color != self.opening
    }
}

/// Alpha thumb border: white when the color blended over `background` is dark, else black.
fn alpha_thumb_border(color: Argb, background: Argb) -> Argb {
    let (a, r, g, b) = channels(color);
    let (_, br, bg, bb) = channels(background);
    let alpha = f64::from(a) / 255.0;
    let blend = |c: u8, back: u8| f64::from(c) * alpha * (2.0 - alpha) + f64::from(back) * (1.0 - alpha).powi(2);
    let luminance = 0.2126 * blend(r, br) + 0.7152 * blend(g, bg) + 0.0722 * blend(b, bb);
    if luminance < 128.0 { Argb(0xFFFF_FFFF) } else { Argb(0xFF00_0000) }
}

/// What the picker reports to the settings window.
pub(crate) trait PickerSink {
    /// Throttled live preview.
    fn preview(&self, color: Argb);
    /// The picker closed; `color` is the final color, `changed` = differs from the opening one.
    fn closed(&self, color: Argb, changed: bool);
}

/// A slider callback: applies a value to the model, true when the color changed.
type ValueHandler = Box<dyn Fn(&ColorPicker, f32) -> bool>;

pub(crate) struct ColorPicker {
    window: ColorPickerWindow,
    model: RefCell<PickerModel>,
    pending: Cell<Option<Argb>>,
    timer: Timer,
    sink: Box<dyn PickerSink>,
    background: Argb,
    closed: Cell<bool>,
    /// Hex box being typed in (not rewritten while editing).
    editing: Cell<Option<HexOrder>>,
}

impl ColorPicker {
    /// Opens the picker owned by `owner`, centered on `owner_rect` (physical x, y, w, h).
    pub(crate) fn open(
        title: &str,
        start: Argb,
        default: Argb,
        palette: &Palette,
        owner: Option<HWND>,
        owner_rect: Option<(i32, i32, u32, u32)>,
        sink: Box<dyn PickerSink>,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let window = match owner {
            Some(owner) => tadn_shell::backend::with_window_kind(
                tadn_shell::backend::WindowKind::Owned { owner },
                ColorPickerWindow::new,
            )?,
            None => ColorPickerWindow::new()?,
        };
        // A palette snapshot: the picker's chrome does not follow the live preview.
        tadn_common::apply_palette!(window.global::<ui::Theme>(), palette);
        let title = if title.trim().is_empty() { s::ColorPicker_DefaultTitle } else { title };
        window.set_window_title(title.into());
        window.set_close_tooltip(s::ColorPicker_CloseTooltip.into());
        window.set_hue_label(s::ColorPicker_ChannelLabel_Hue.into());
        window.set_alpha_label(s::ColorPicker_ChannelLabel_Alpha.into());
        window.set_r_label(s::ColorPicker_ChannelLabel_R.into());
        window.set_g_label(s::ColorPicker_ChannelLabel_G.into());
        window.set_b_label(s::ColorPicker_ChannelLabel_B.into());
        window.set_rgba_label(s::ColorPicker_RGBAHexLabel.into());
        window.set_argb_label(s::ColorPicker_ARGBHexLabel.into());
        window.set_default_text(s::ColorPicker_DefaultButton.into());
        window.set_reset_text(s::ColorPicker_ResetButton.into());

        let picker = Rc::new(Self {
            window,
            model: RefCell::new(PickerModel::new(start, default)),
            pending: Cell::new(None),
            timer: Timer::default(),
            sink,
            background: palette.background,
            closed: Cell::new(false),
            editing: Cell::new(None),
        });
        picker.sync(None);
        picker.wire();

        if let Some((x, y, w, h)) = owner_rect {
            let scale = picker.window.window().scale_factor();
            let pw = (408.0 * scale).round() as i32;
            let ph = (284.0 * scale).round() as i32;
            let left = x + (w as i32 - pw) / 2;
            let top = y + (h as i32 - ph) / 2;
            picker.window.window().set_position(slint::PhysicalPosition::new(left, top));
        }
        picker.window.show()?;
        Ok(picker)
    }

    fn wire(self: &Rc<Self>) {
        let w = &self.window;
        let weak = Rc::downgrade(self);
        let on = move |f: fn(&Self) -> bool| {
            let weak: Weak<Self> = weak.clone();
            move || {
                if let Some(p) = weak.upgrade()
                    && f(&p)
                {
                    p.changed();
                }
            }
        };
        let weak = Rc::downgrade(self);
        let with = move |f: ValueHandler| {
            let weak = weak.clone();
            move |v: f32| {
                if let Some(p) = weak.upgrade()
                    && f(&p, v)
                {
                    p.changed();
                }
            }
        };
        w.on_hue_changed(with(Box::new(|p, v| p.model.borrow_mut().hue(f64::from(v)))));
        w.on_alpha_changed(with(Box::new(|p, v| p.model.borrow_mut().channel(0, f64::from(v)))));
        w.on_red_changed(with(Box::new(|p, v| p.model.borrow_mut().channel(1, f64::from(v)))));
        w.on_green_changed(with(Box::new(|p, v| p.model.borrow_mut().channel(2, f64::from(v)))));
        w.on_blue_changed(with(Box::new(|p, v| p.model.borrow_mut().channel(3, f64::from(v)))));
        w.on_default_clicked(on(|p| p.model.borrow_mut().reset_to_default()));
        w.on_reset_clicked(on(|p| p.model.borrow_mut().reset_to_opening()));

        let weak = Rc::downgrade(self);
        w.on_square_changed(move |sat, val| {
            if let Some(p) = weak.upgrade()
                && p.model.borrow_mut().square(f64::from(sat), f64::from(val))
            {
                p.changed();
            }
        });
        for order in [HexOrder::Rgba, HexOrder::Argb] {
            let weak = Rc::downgrade(self);
            let handler = move |text: slint::SharedString| {
                if let Some(p) = weak.upgrade()
                    && p.model.borrow_mut().hex(&text, order)
                {
                    p.editing.set(Some(order));
                    p.changed();
                    p.editing.set(None);
                }
            };
            match order {
                HexOrder::Rgba => w.on_rgba_edited(handler),
                HexOrder::Argb => w.on_argb_edited(handler),
            }
        }
        let weak = Rc::downgrade(self);
        w.on_close_requested(move || {
            if let Some(p) = weak.upgrade() {
                p.close();
            }
        });
        let weak = Rc::downgrade(self);
        w.window().on_close_requested(move || {
            if let Some(p) = weak.upgrade() {
                p.close();
            }
            slint::CloseRequestResponse::HideWindow
        });
        let weak = Rc::downgrade(self);
        w.on_drag_requested(move || {
            if let Some(p) = weak.upgrade() {
                use slint::winit_030::WinitWindowAccessor;
                p.window.window().with_winit_window(|winit| {
                    if let Err(error) = winit.drag_window() {
                        log::warn!("color picker drag failed: {error}");
                    }
                });
            }
        });
    }

    /// A user change: sync the controls and queue a throttled preview.
    fn changed(self: &Rc<Self>) {
        self.sync(self.editing.get());
        self.pending.set(Some(self.model.borrow().color));
        if !self.timer.running() {
            let weak = Rc::downgrade(self);
            self.timer.start(TimerMode::Repeated, COLOR_PICKER_CHANGE_COOLDOWN, move || {
                let Some(p) = weak.upgrade() else { return };
                match p.pending.take() {
                    Some(color) if !p.closed.get() => p.sink.preview(color),
                    _ => p.timer.stop(),
                }
            });
        }
    }

    fn sync(&self, editing: Option<HexOrder>) {
        let model = self.model.borrow();
        let color = model.color;
        let (a, r, g, b) = channels(color);
        let (_, saturation, value) = rgb_to_hsv(color);
        let w = &self.window;
        w.set_current_color(crate::items::slint_color(color));
        w.set_hue_color(crate::items::slint_color(hsv_to_rgb(model.free_hue, 1.0, 1.0)));
        w.set_hue(model.free_hue as f32);
        w.set_saturation(saturation as f32);
        w.set_brightness(value as f32);
        w.set_alpha(i32::from(a));
        w.set_red(i32::from(r));
        w.set_green(i32::from(g));
        w.set_blue(i32::from(b));
        w.set_alpha_thumb_border(crate::items::slint_color(alpha_thumb_border(color, self.background)));
        if editing != Some(HexOrder::Rgba) {
            w.set_rgba_text(format_hex(color, HexOrder::Rgba).into());
        }
        if editing != Some(HexOrder::Argb) {
            w.set_argb_text(format_hex(color, HexOrder::Argb).into());
        }
    }

    /// Closes the picker: drops a pending preview and reports the final color.
    pub(crate) fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        self.timer.stop();
        self.pending.set(None);
        if let Err(error) = self.window.hide() {
            log::warn!("color picker hide failed: {error}");
        }
        let model = self.model.borrow().clone();
        self.sink.closed(model.color, model.changed());
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.get()
    }

    /// Brings an open picker forward (un-minimizes it).
    pub(crate) fn activate(&self) {
        if let Some(hwnd) = tadn_shell::window::hwnd(self.window.window()) {
            tadn_shell::window::activate(hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trips_primary_colors() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), Argb(0xFFFF_0000));
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0), Argb(0xFF00_FF00));
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0), Argb(0xFF00_00FF));
        assert_eq!(hsv_to_rgb(360.0, 1.0, 1.0), Argb(0xFFFF_0000));
        assert_eq!(hsv_to_rgb(0.0, 0.0, 0.5), Argb(0xFF80_8080));
        let (h, s, v) = rgb_to_hsv(Argb(0xFF00_80FF));
        assert!((h - 209.882).abs() < 0.01 && (s - 1.0).abs() < 1e-9 && (v - 1.0).abs() < 1e-9);
        let (h, _, _) = rgb_to_hsv(Argb(0xFFFF_00FF));
        assert!((h - 300.0).abs() < 1e-9);
    }

    #[test]
    fn hex_boxes_parse_both_orders() {
        assert_eq!(parse_hex("#11223380", HexOrder::Rgba), Some(Argb(0x8011_2233)));
        assert_eq!(parse_hex("80112233", HexOrder::Argb), Some(Argb(0x8011_2233)));
        assert_eq!(parse_hex(" ##aabbcc ", HexOrder::Argb), Some(Argb(0xFFAA_BBCC)));
        assert_eq!(parse_hex("abc", HexOrder::Rgba), None);
        assert_eq!(parse_hex("zz112233", HexOrder::Rgba), None);
        assert_eq!(format_hex(Argb(0x8011_2233), HexOrder::Rgba), "11223380");
        assert_eq!(format_hex(Argb(0x8011_2233), HexOrder::Argb), "80112233");
    }

    #[test]
    fn grays_keep_the_free_hue() {
        let mut model = PickerModel::new(Argb(0xFF00_FF00), Argb(0xFFFF_FFFF));
        assert!((model.free_hue - 120.0).abs() < 1e-9);
        // Dragging the square to gray keeps the hue.
        assert!(model.square(0.0, 0.5));
        assert!((model.free_hue - 120.0).abs() < 1e-9);
        // A change from elsewhere that leaves the color gray does not re-read the hue.
        assert!(model.channel(0, 10.0));
        assert!((model.free_hue - 120.0).abs() < 1e-9);
        // A saturated channel change re-reads the hue.
        assert!(model.hex("FF0000", HexOrder::Rgba));
        assert!((model.free_hue - 0.0).abs() < 1e-9);
    }

    #[test]
    fn hue_moves_are_forced_and_reset_restores_the_opening_color() {
        let mut model = PickerModel::new(Argb(0x80FF_0000), Argb(0xFFFF_FFFF));
        assert!(model.hue(0.0)); // Forced even though unchanged.
        assert!(!model.changed());
        assert!(model.hue(120.0));
        assert_eq!(model.color, Argb(0x8000_FF00));
        assert!(model.changed());
        assert!(model.reset_to_opening());
        assert!(!model.changed());
        assert!(model.reset_to_default());
        assert_eq!(model.color, Argb(0xFFFF_FFFF));
        // Channel bytes round half to even.
        assert!(model.channel(0, 127.5));
        assert_eq!(model.color.0 >> 24, 128);
        assert!(model.channel(0, 126.5));
        assert_eq!(model.color.0 >> 24, 126);
    }

    #[test]
    fn alpha_thumb_border_contrasts_with_the_blend() {
        assert_eq!(alpha_thumb_border(Argb(0xFF00_0000), Argb(0xFFFF_FFFF)), Argb(0xFFFF_FFFF));
        assert_eq!(alpha_thumb_border(Argb(0xFFFF_FFFF), Argb(0xFF00_0000)), Argb(0xFF00_0000));
        assert_eq!(alpha_thumb_border(Argb(0x0000_0000), Argb(0xFFF3_F3F3)), Argb(0xFF00_0000));
    }
}
