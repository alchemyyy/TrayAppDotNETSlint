//! Pure layout rules of the device cards and app drawers (`VolumeFlyoutWindow.BuildCell`,
//! `BuildIconGridDrawer` + Avalonia `WrapPanel`, `ResolveDeviceBandPadding`,
//! `ResolveDeviceBandRadius`, drawer height caps).

use vtadn_model::settings::{
    AppDrawerIconsCenterMode, AppDrawerStackDirection, AppSettings, FlyoutDeviceLayout, FlyoutDeviceTitlePosition,
};

use crate::metrics::{APP_ICON_GRID_SLOT_SIZE, APP_SLIDER_ROW_HEIGHT};

/// Left/top/right/bottom thickness in DIPs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Thickness {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Thickness {
    pub const fn new(left: f64, top: f64, right: f64, bottom: f64) -> Self {
        Self { left, top, right, bottom }
    }
}

/// `ResolvedStackDirection`: `Auto` stacks bottom-up when the apps sit above the device.
pub fn resolved_stack_direction(settings: &AppSettings) -> AppDrawerStackDirection {
    match settings.app_drawer_stack_direction {
        AppDrawerStackDirection::Auto => match settings.flyout_device_layout {
            FlyoutDeviceLayout::AppsAboveDevice => AppDrawerStackDirection::BottomTop,
            FlyoutDeviceLayout::AppsBelowDevice => AppDrawerStackDirection::TopBottom,
        },
        direction => direction,
    }
}

/// `BuildIconGridDrawer` inputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconGridOptions {
    pub direction: AppDrawerStackDirection,
    pub center_mode: AppDrawerIconsCenterMode,
    /// `AppDrawerIconsPerRow` (clamped 1..16).
    pub icons_per_row: i32,
    /// Width the drawer's scroll viewer offers the panel.
    pub available_width: f64,
}

impl IconGridOptions {
    pub fn from_settings(settings: &AppSettings, available_width: f64) -> Self {
        Self {
            direction: resolved_stack_direction(settings),
            center_mode: settings.app_drawer_icons_center_mode,
            icons_per_row: settings.app_drawer_icons_per_row,
            available_width,
        }
    }
}

/// Placement of the icon cells.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IconGridLayout {
    /// Cell top-left per item, in the order of the groups given to [`icon_grid_layout`].
    pub positions: Vec<(f64, f64)>,
    /// Content size (extent inside the drawer scroll viewer).
    pub width: f64,
    pub height: f64,
}

/// Lays out `count` 36x36 cells the way the C# `WrapPanel` does:
/// - `BottomTop`/`RightLeft` only reverse the item order; wrapping still fills left to right, top to
///   bottom.
/// - Horizontal directions wrap at `min(available, 36 * per_row) / 36` cells per line.
/// - Vertical directions (`LeftRight`/`RightLeft`) measure with infinite height inside the vertical
///   scroller, so they never wrap: one column.
/// - `Off` aligns the panel left; `Centered` and `CenteredSoftMax` centre the whole panel (a
///   partial last line stays left-aligned inside the centred block).
pub fn icon_grid_layout(count: usize, options: &IconGridOptions) -> IconGridLayout {
    let slot = APP_ICON_GRID_SLOT_SIZE;
    let per_row = options.icons_per_row.clamp(1, 16) as usize;
    let max_width = slot * per_row as f64;
    let vertical = matches!(options.direction, AppDrawerStackDirection::LeftRight | AppDrawerStackDirection::RightLeft);
    let reversed = matches!(options.direction, AppDrawerStackDirection::BottomTop | AppDrawerStackDirection::RightLeft);
    let per_line =
        if vertical { usize::MAX } else { ((options.available_width.min(max_width) / slot).floor() as usize).max(1) };
    let (columns, rows) = if count == 0 {
        (0, 0)
    } else if vertical {
        (1, count)
    } else {
        (count.min(per_line), count.div_ceil(per_line))
    };
    let panel_width = columns as f64 * slot;
    let offset_x = match options.center_mode {
        AppDrawerIconsCenterMode::Off => 0.0,
        AppDrawerIconsCenterMode::Centered | AppDrawerIconsCenterMode::CenteredSoftMax => {
            ((options.available_width - panel_width) / 2.0).max(0.0)
        }
    };
    let positions = (0..count)
        .map(|item| {
            let slot_index = if reversed { count - 1 - item } else { item };
            let (column, row) = if vertical { (0, slot_index) } else { (slot_index % per_line, slot_index / per_line) };
            (offset_x + column as f64 * slot, row as f64 * slot)
        })
        .collect();
    IconGridLayout { positions, width: options.available_width, height: rows as f64 * slot }
}

/// `ResolveSliderDrawerMaxHeight`.
pub fn slider_drawer_max_height(settings: &AppSettings, capture: bool) -> f64 {
    let apps = if capture {
        settings.recording_app_drawer_sliders_max_apps
    } else {
        settings.playback_app_drawer_sliders_max_apps
    };
    f64::from(apps.max(1)) * APP_SLIDER_ROW_HEIGHT
}

/// `ResolveGridDrawerMaxHeight`.
pub fn grid_drawer_max_height(settings: &AppSettings, capture: bool) -> f64 {
    let rows = if capture {
        settings.recording_app_drawer_icons_max_rows
    } else {
        settings.playback_app_drawer_icons_max_rows
    };
    f64::from(rows.max(1)) * APP_ICON_GRID_SLOT_SIZE
}

/// `UsesGridDrawer`: capture devices with the icon display type.
pub fn uses_grid_drawer(settings: &AppSettings, capture: bool) -> bool {
    capture && settings.recording_app_drawer_display_type == vtadn_model::settings::AppDrawerDisplayType::Icons
}

/// `ResolveDeviceBandPadding`.
pub fn device_band_padding(settings: &AppSettings) -> Thickness {
    let apps_bottom = settings.flyout_device_layout == FlyoutDeviceLayout::AppsBelowDevice;
    match (settings.flyout_device_title_position, apps_bottom) {
        (FlyoutDeviceTitlePosition::AboveSlider, true) => Thickness::new(14.0, 0.0, 12.0, 1.0),
        (FlyoutDeviceTitlePosition::AboveSlider, false) => Thickness::new(17.0, 0.0, 12.0, 1.0),
        (FlyoutDeviceTitlePosition::BelowSlider, true) => Thickness::new(14.0, 0.0, 12.0, 1.0),
        (FlyoutDeviceTitlePosition::BelowSlider, false) => Thickness::new(17.0, 0.0, 12.0, 12.0),
    }
}

/// App band padding: icon grid, slider drawer below the device, slider drawer above it.
pub fn app_band_padding(grid: bool, apps_bottom: bool) -> Thickness {
    match (grid, apps_bottom) {
        (true, _) => Thickness::new(13.0, 1.0, 11.0, 2.0),
        (false, true) => Thickness::new(10.0, 0.0, 12.0, 10.0),
        (false, false) => Thickness::new(14.0, 0.0, 12.0, 4.0),
    }
}

/// App band margin (grid drawers only).
pub fn app_band_margin(grid: bool, apps_bottom: bool) -> Thickness {
    match (grid, apps_bottom) {
        (true, true) => Thickness::new(0.0, -2.0, 0.0, 2.0),
        (true, false) => Thickness::new(0.0, -6.0, 0.0, -4.0),
        (false, _) => Thickness::default(),
    }
}

/// Bottom corner radius of the device band (`ResolveDeviceBandRadius`); top corners are square.
pub fn device_band_bottom_radius(is_last: bool, apps_bottom: bool, drawer_visible: bool) -> bool {
    is_last && (!apps_bottom || !drawer_visible)
}

/// Bottom corner radius of the app band: the last card's drawer below the device.
pub fn app_band_bottom_radius(is_last: bool, apps_bottom: bool) -> bool {
    is_last && apps_bottom
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::ICON_GRID_AVAILABLE_WIDTH;

    fn options(
        direction: AppDrawerStackDirection,
        center_mode: AppDrawerIconsCenterMode,
        per_row: i32,
    ) -> IconGridOptions {
        IconGridOptions { direction, center_mode, icons_per_row: per_row, available_width: ICON_GRID_AVAILABLE_WIDTH }
    }

    #[test]
    fn available_width_fits_eight_icons() {
        assert_eq!(ICON_GRID_AVAILABLE_WIDTH, 322.0);
        let layout =
            icon_grid_layout(10, &options(AppDrawerStackDirection::TopBottom, AppDrawerIconsCenterMode::Off, 9));
        // 9 per row would need 324 > 322, so 8 per line.
        assert_eq!(layout.positions[7], (252.0, 0.0));
        assert_eq!(layout.positions[8], (0.0, 36.0));
        assert_eq!(layout.positions[9], (36.0, 36.0));
        assert_eq!(layout.height, 72.0);
    }

    #[test]
    fn per_row_caps_the_line_length() {
        let layout =
            icon_grid_layout(5, &options(AppDrawerStackDirection::TopBottom, AppDrawerIconsCenterMode::Off, 2));
        assert_eq!(layout.positions, vec![(0.0, 0.0), (36.0, 0.0), (0.0, 36.0), (36.0, 36.0), (0.0, 72.0)]);
        let clamped =
            icon_grid_layout(2, &options(AppDrawerStackDirection::TopBottom, AppDrawerIconsCenterMode::Off, 0));
        assert_eq!(clamped.positions, vec![(0.0, 0.0), (0.0, 36.0)]);
    }

    #[test]
    fn bottom_top_only_reverses_the_order() {
        let layout =
            icon_grid_layout(3, &options(AppDrawerStackDirection::BottomTop, AppDrawerIconsCenterMode::Off, 9));
        assert_eq!(layout.positions, vec![(72.0, 0.0), (36.0, 0.0), (0.0, 0.0)]);
        let wrapped =
            icon_grid_layout(3, &options(AppDrawerStackDirection::BottomTop, AppDrawerIconsCenterMode::Off, 2));
        // Reversed list [c, b, a] fills left to right, top to bottom.
        assert_eq!(wrapped.positions, vec![(0.0, 36.0), (36.0, 0.0), (0.0, 0.0)]);
    }

    #[test]
    fn vertical_directions_never_wrap() {
        let left_right =
            icon_grid_layout(3, &options(AppDrawerStackDirection::LeftRight, AppDrawerIconsCenterMode::Off, 1));
        assert_eq!(left_right.positions, vec![(0.0, 0.0), (0.0, 36.0), (0.0, 72.0)]);
        assert_eq!(left_right.height, 108.0);
        let right_left =
            icon_grid_layout(3, &options(AppDrawerStackDirection::RightLeft, AppDrawerIconsCenterMode::Off, 9));
        assert_eq!(right_left.positions, vec![(0.0, 72.0), (0.0, 36.0), (0.0, 0.0)]);
    }

    #[test]
    fn centering_centres_the_whole_panel() {
        let single =
            icon_grid_layout(2, &options(AppDrawerStackDirection::TopBottom, AppDrawerIconsCenterMode::Centered, 9));
        assert_eq!(single.positions, vec![(125.0, 0.0), (161.0, 0.0)]);
        // Soft max renders like Centered.
        let soft = icon_grid_layout(
            2,
            &options(AppDrawerStackDirection::TopBottom, AppDrawerIconsCenterMode::CenteredSoftMax, 9),
        );
        assert_eq!(soft.positions, single.positions);
        // Several lines: the block is centred and the partial last line stays left-aligned in it.
        let multi =
            icon_grid_layout(5, &options(AppDrawerStackDirection::TopBottom, AppDrawerIconsCenterMode::Centered, 3));
        assert_eq!(multi.positions, vec![(107.0, 0.0), (143.0, 0.0), (179.0, 0.0), (107.0, 36.0), (143.0, 36.0)]);
        let column =
            icon_grid_layout(2, &options(AppDrawerStackDirection::LeftRight, AppDrawerIconsCenterMode::Centered, 9));
        assert_eq!(column.positions, vec![(143.0, 0.0), (143.0, 36.0)]);
    }

    #[test]
    fn empty_grid_has_no_height() {
        let layout =
            icon_grid_layout(0, &options(AppDrawerStackDirection::TopBottom, AppDrawerIconsCenterMode::Centered, 9));
        assert!(layout.positions.is_empty());
        assert_eq!(layout.height, 0.0);
    }

    #[test]
    fn auto_direction_follows_the_device_layout() {
        let mut settings = AppSettings::default();
        assert_eq!(resolved_stack_direction(&settings), AppDrawerStackDirection::BottomTop);
        settings.flyout_device_layout = FlyoutDeviceLayout::AppsBelowDevice;
        assert_eq!(resolved_stack_direction(&settings), AppDrawerStackDirection::TopBottom);
        settings.app_drawer_stack_direction = AppDrawerStackDirection::RightLeft;
        assert_eq!(resolved_stack_direction(&settings), AppDrawerStackDirection::RightLeft);
    }

    #[test]
    fn band_paddings_and_radii() {
        let mut settings = AppSettings::default();
        assert_eq!(device_band_padding(&settings), Thickness::new(17.0, 0.0, 12.0, 12.0));
        settings.flyout_device_title_position = FlyoutDeviceTitlePosition::AboveSlider;
        assert_eq!(device_band_padding(&settings), Thickness::new(17.0, 0.0, 12.0, 1.0));
        settings.flyout_device_layout = FlyoutDeviceLayout::AppsBelowDevice;
        assert_eq!(device_band_padding(&settings), Thickness::new(14.0, 0.0, 12.0, 1.0));
        assert!(!device_band_bottom_radius(false, false, true));
        assert!(device_band_bottom_radius(true, false, true));
        assert!(!device_band_bottom_radius(true, true, true));
        assert!(device_band_bottom_radius(true, true, false));
        assert_eq!(app_band_padding(false, false), Thickness::new(14.0, 0.0, 12.0, 4.0));
        assert_eq!(app_band_margin(true, false), Thickness::new(0.0, -6.0, 0.0, -4.0));
    }

    #[test]
    fn drawer_height_caps() {
        let mut settings = AppSettings::default();
        assert_eq!(slider_drawer_max_height(&settings, false), 24.0 * 28.0);
        assert_eq!(grid_drawer_max_height(&settings, true), 360.0);
        settings.recording_app_drawer_icons_max_rows = 0;
        assert_eq!(grid_drawer_max_height(&settings, true), 36.0);
        assert!(uses_grid_drawer(&settings, true));
        assert!(!uses_grid_drawer(&settings, false));
    }
}
