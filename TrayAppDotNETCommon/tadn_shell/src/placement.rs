//! Monitor, work-area and taskbar geometry plus tray popup positioning (port of
//! `UI/Tray/TrayWorkArea.cs`, `UI/Tray/TrayPopupPositioning.cs` and the native parts of
//! `UI/FlyoutTrayPlacement.cs`). All values are physical screen pixels.

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY,
    MONITOR_FROM_FLAGS, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::{
    ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETTASKBARPOS, APPBARDATA, SHAppBarMessage,
};

/// Screen rectangle in physical pixels (right/bottom exclusive, like Win32 `RECT`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn width(&self) -> i32 {
        self.right - self.left
    }

    pub const fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub const fn center(&self) -> (i32, i32) {
        (self.left + self.width() / 2, self.top + self.height() / 2)
    }

    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

impl From<RECT> for Rect {
    fn from(rect: RECT) -> Self {
        Self { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskbarEdge {
    Left,
    Top,
    Right,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonitorInfo {
    pub monitor: Rect,
    pub work_area: Rect,
    /// Effective DPI / 96.
    pub scale: f32,
}

/// Fallback when no monitor can be queried (the C# 1920x1080 fallback).
const FALLBACK_MONITOR: MonitorInfo = MonitorInfo {
    monitor: Rect { left: 0, top: 0, right: 1920, bottom: 1080 },
    work_area: Rect { left: 0, top: 0, right: 1920, bottom: 1080 },
    scale: 1.0,
};

/// Monitor nearest to the point (`MonitorFromPoint(MONITOR_DEFAULTTONEAREST)`), falling back to
/// the primary monitor, then to 1920x1080 at scale 1.
pub fn monitor_from_point(x: i32, y: i32) -> MonitorInfo {
    query_monitor(x, y, MONITOR_DEFAULTTONEAREST)
        .or_else(|| query_monitor(0, 0, MONITOR_DEFAULTTOPRIMARY))
        .unwrap_or(FALLBACK_MONITOR)
}

/// `TrayWorkArea.TryGetCurrent`: the work area of the monitor containing the point
/// (`MONITOR_DEFAULTTONULL`), or None when the point is on no monitor or the area is empty.
pub fn work_area_at(x: i32, y: i32) -> Option<Rect> {
    query_monitor(x, y, MONITOR_DEFAULTTONULL).map(|info| info.work_area)
}

/// `TrayWorkArea.Resolve`: [`work_area_at`], else the nearest monitor's work area (which already
/// falls back to the primary monitor and 1920x1080).
pub fn resolve_work_area(x: i32, y: i32) -> Rect {
    work_area_at(x, y).unwrap_or_else(|| monitor_from_point(x, y).work_area)
}

fn query_monitor(x: i32, y: i32, flags: MONITOR_FROM_FLAGS) -> Option<MonitorInfo> {
    // SAFETY: plain query with a by-value point.
    let monitor: HMONITOR = unsafe { MonitorFromPoint(POINT { x, y }, flags) };
    if monitor.is_invalid() {
        return None;
    }
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: `info` is a properly sized MONITORINFO.
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    let work_area = Rect::from(info.rcWork);
    if work_area.width() <= 0 || work_area.height() <= 0 {
        return None;
    }
    let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
    // SAFETY: out-pointers to locals.
    let scale = match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } {
        Ok(()) if dpi_x > 0 => dpi_x as f32 / 96.0,
        _ => 1.0,
    };
    Some(MonitorInfo { monitor: Rect::from(info.rcMonitor), work_area, scale })
}

/// Primary taskbar edge via `SHAppBarMessage(ABM_GETTASKBARPOS)`; None when unavailable.
pub fn taskbar_edge() -> Option<TaskbarEdge> {
    let mut data = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, ..Default::default() };
    // SAFETY: `data` is a properly sized APPBARDATA.
    if unsafe { SHAppBarMessage(ABM_GETTASKBARPOS, &mut data) } == 0 {
        return None;
    }
    match data.uEdge {
        ABE_LEFT => Some(TaskbarEdge::Left),
        ABE_TOP => Some(TaskbarEdge::Top),
        ABE_RIGHT => Some(TaskbarEdge::Right),
        ABE_BOTTOM => Some(TaskbarEdge::Bottom),
        _ => None,
    }
}

/// `TrayPopupPositioning.ResolveDockedPosition`: centre the popup on the icon, then clamp into the
/// work area inset by `edge_pad` (every taskbar edge falls out of the clamp). With no icon rect,
/// bottom-right corner of the work area. Integer division truncates like C#; sizes below 1 count
/// as 1 and a negative pad as 0.
pub fn docked_popup_position(icon: Option<Rect>, work_area: Rect, size: (i32, i32), edge_pad: i32) -> (i32, i32) {
    let (w, h) = (size.0.max(1), size.1.max(1));
    let pad = edge_pad.max(0);
    let position = match icon {
        Some(icon) => {
            let (cx, cy) = icon.center();
            (cx - w / 2, cy - h / 2)
        }
        None => (work_area.right - w - pad, work_area.bottom - h - pad),
    };
    clamp_into(position, (w, h), work_area, pad)
}

/// `TrayPopupPositioning.ClampToWorkArea`: clamps a top-left position so a `size` window stays
/// inside `work_area` inset by `edge_pad` (an oversized window pins to the top-left inset).
pub fn clamp_into(position: (i32, i32), size: (i32, i32), work_area: Rect, edge_pad: i32) -> (i32, i32) {
    let pad = edge_pad.max(0);
    let (w, h) = (size.0.max(1), size.1.max(1));
    let min_x = work_area.left + pad;
    let min_y = work_area.top + pad;
    let max_x = (work_area.right - w - pad).max(min_x);
    let max_y = (work_area.bottom - h - pad).max(min_y);
    (position.0.clamp(min_x, max_x), position.1.clamp(min_y, max_y))
}

/// `TrayPopupPositioning.ClampToSavedMonitor`: clamps a saved position into the work area of the
/// monitor that contains it ([`resolve_work_area`]).
pub fn clamp_to_saved_monitor(position: (i32, i32), size: (i32, i32), edge_pad: i32) -> (i32, i32) {
    clamp_into(position, size, resolve_work_area(position.0, position.1), edge_pad)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDGE_PADDING: i32 = 8;

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect { left: x, top: y, right: x + width, bottom: y + height }
    }

    #[test]
    fn top_taskbar_places_popup_below_taskbar() {
        let position =
            docked_popup_position(Some(rect(948, 8, 24, 24)), rect(0, 48, 1920, 1032), (350, 600), EDGE_PADDING);
        assert_eq!(position, (785, 56));
    }

    #[test]
    fn bottom_taskbar_places_popup_above_taskbar() {
        let position =
            docked_popup_position(Some(rect(948, 1040, 24, 24)), rect(0, 0, 1920, 1032), (350, 600), EDGE_PADDING);
        assert_eq!(position, (785, 424));
    }

    #[test]
    fn left_taskbar_places_popup_right_of_taskbar() {
        let position =
            docked_popup_position(Some(rect(8, 528, 24, 24)), rect(48, 0, 1872, 1080), (350, 400), EDGE_PADDING);
        assert_eq!(position, (56, 340));
    }

    #[test]
    fn right_taskbar_places_popup_left_of_taskbar() {
        let position =
            docked_popup_position(Some(rect(1880, 528, 24, 24)), rect(0, 0, 1872, 1080), (350, 400), EDGE_PADDING);
        assert_eq!(position, (1514, 340));
    }

    #[test]
    fn negative_coordinate_monitor_retains_physical_coordinates() {
        let position =
            docked_popup_position(Some(rect(-1000, 8, 24, 24)), rect(-1920, 40, 1920, 1040), (400, 500), EDGE_PADDING);
        assert_eq!(position, (-1188, 48));
    }

    #[test]
    fn missing_icon_uses_bottom_right_fallback() {
        let position = docked_popup_position(None, rect(-1920, 0, 1920, 1040), (350, 500), EDGE_PADDING);
        assert_eq!(position, (-358, 532));
    }

    #[test]
    fn oversized_popup_collapses_to_minimum_padded_position() {
        let position =
            docked_popup_position(Some(rect(180, 90, 24, 24)), rect(100, 100, 200, 100), (400, 300), EDGE_PADDING);
        assert_eq!(position, (108, 108));
    }

    #[test]
    fn floating_popup_clamps_to_saved_monitor_work_area() {
        let position = clamp_into((-2100, 900), (400, 500), rect(-1920, 40, 1920, 1040), EDGE_PADDING);
        assert_eq!(position, (-1912, 572));
    }

    #[test]
    fn floating_popup_inside_work_area_keeps_saved_position() {
        let position = clamp_into((700, 300), (350, 500), rect(0, 0, 1920, 1040), EDGE_PADDING);
        assert_eq!(position, (700, 300));
    }

    #[test]
    fn degenerate_sizes_and_padding_are_normalized() {
        // Size 0 counts as 1 and a negative pad as 0, like the C# Math.Max guards.
        assert_eq!(clamp_into((5000, 5000), (0, 0), rect(0, 0, 100, 100), -4), (99, 99));
    }
}
