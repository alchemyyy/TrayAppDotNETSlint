//! Flyout placement math in physical pixels (port of `FlyoutTrayPlacement` and the clamp rules of
//! `TrayPopupPositioning`). Docked centring/clamping itself is `tadn_shell::placement`.

use tadn_shell::placement::{self, Rect};

use crate::metrics::{EDGE_PADDING, PIXEL_MIN_SIZE, SNAP_TOLERANCE_PERCENT, WORK_AREA_MIN_HEIGHT};

/// `ToPixels`: `max(0, round(logical * scale))`.
pub fn to_pixels(logical: f64, scale: f64) -> i32 {
    ((logical * scale).round_ties_even() as i32).max(0)
}

/// `EdgePaddingPixels` for a monitor scale.
pub fn edge_padding_px(scale: f64) -> i32 {
    to_pixels(EDGE_PADDING, scale)
}

/// `ResolveMaxHeight`: the tallest logical height that fits the work area with edge padding on both
/// sides, less `extra_reserve`, never below the minimum flyout height.
pub fn resolve_max_height(work_area: Rect, scale: f64, extra_reserve: f64) -> f64 {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    WORK_AREA_MIN_HEIGHT.max(f64::from(work_area.height()) / scale - 2.0 * EDGE_PADDING - extra_reserve.max(0.0))
}

/// `ResolveSnapTolerance`: 2% of the shorter work-area side, at least one pixel.
pub fn snap_tolerance(work_area: Rect) -> i32 {
    let shorter = f64::from(work_area.width().min(work_area.height()));
    PIXEL_MIN_SIZE.max((shorter * SNAP_TOLERANCE_PERCENT).round_ties_even() as i32)
}

/// `MeasurePixelSize`: `ceil(logical * scale)`, at least one pixel.
pub fn pixel_size(width: f64, height: f64, scale: f64) -> (i32, i32) {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    (PIXEL_MIN_SIZE.max((width * scale).ceil() as i32), PIXEL_MIN_SIZE.max((height * scale).ceil() as i32))
}

/// The persisted floating position (`(int)Math.Round(FlyoutLeft/Top)`).
pub fn saved_position(left: f64, top: f64) -> (i32, i32) {
    (left.round_ties_even() as i32, top.round_ties_even() as i32)
}

/// `ClampToSavedMonitor`: clamps into the work area of the monitor holding the saved point.
pub fn clamp_saved_position(
    saved: (i32, i32),
    size: (i32, i32),
    saved_monitor_work_area: Rect,
    edge_pad: i32,
) -> (i32, i32) {
    placement::clamp_into(saved, size, saved_monitor_work_area, edge_pad)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK_AREA: Rect = Rect { left: 0, top: 0, right: 1920, bottom: 1032 };

    #[test]
    fn max_height_reserves_edges_and_header() {
        assert_eq!(resolve_max_height(WORK_AREA, 1.0, 0.0), 1016.0);
        assert_eq!(resolve_max_height(WORK_AREA, 1.0, 48.0), 968.0);
        assert_eq!(resolve_max_height(WORK_AREA, 1.5, 48.0), 1032.0 / 1.5 - 16.0 - 48.0);
        let tiny = Rect { left: 0, top: 0, right: 800, bottom: 200 };
        assert_eq!(resolve_max_height(tiny, 1.0, 48.0), 220.0);
        assert_eq!(resolve_max_height(WORK_AREA, 0.0, -5.0), 1016.0);
    }

    #[test]
    fn snap_tolerance_is_two_percent_of_the_shorter_side() {
        assert_eq!(snap_tolerance(WORK_AREA), 21);
        assert_eq!(snap_tolerance(Rect { left: 0, top: 0, right: 10, bottom: 10 }), 1);
        // 0.5 rounds to even.
        assert_eq!(snap_tolerance(Rect { left: 0, top: 0, right: 1000, bottom: 125 }), 2);
    }

    #[test]
    fn pixel_sizes_round_up_and_padding_rounds_half_even() {
        assert_eq!(pixel_size(350.0, 401.2, 1.5), (525, 602));
        assert_eq!(pixel_size(0.0, 0.0, 1.0), (1, 1));
        assert_eq!(edge_padding_px(1.0), 8);
        assert_eq!(edge_padding_px(1.25), 10);
        assert_eq!(edge_padding_px(1.0625), 8);
        assert_eq!(to_pixels(-3.0, 1.0), 0);
    }

    #[test]
    fn saved_positions_round_and_clamp_into_their_monitor() {
        assert_eq!(saved_position(10.5, 11.5), (10, 12));
        let second = Rect { left: -1920, top: 0, right: 0, bottom: 1040 };
        assert_eq!(clamp_saved_position((-100, 900), (350, 500), second, 8), (-358, 532));
        assert_eq!(clamp_saved_position((-1000, 100), (350, 500), second, 8), (-1000, 100));
    }

    #[test]
    fn docked_position_centres_on_the_icon_and_clamps() {
        let icon = Rect { left: 948, top: 1040, right: 972, bottom: 1064 };
        assert_eq!(placement::docked_popup_position(Some(icon), WORK_AREA, (350, 600), 8), (785, 424));
        assert_eq!(placement::docked_popup_position(None, WORK_AREA, (350, 600), 8), (1562, 424));
    }
}
