//! Flyout metrics in DIPs (port of the `Flyout.*` resources in `VolumeFlyoutWindow.axaml`, the
//! `FlyoutPlacement` / `FlyoutUndockButton` resources and the `FlyoutSlider` constants). The Slint
//! files repeat the visual ones; the values here drive Rust-side math (placement, drawer heights,
//! icon-grid layout, menus).

/// `Flyout.WindowWidth`.
pub const WINDOW_WIDTH: f64 = 350.0;
/// `Flyout.ContentHeightReserve`: reserved for the header when capping the device list height.
pub const CONTENT_HEIGHT_RESERVE: f64 = 48.0;
/// `Flyout.DragThreshold` (DIP; multiplied by the render scale).
pub const DRAG_THRESHOLD: f64 = 4.0;
/// `Flyout.ChromeInnerMargin` + the `FlyoutFrame` border thickness, per side.
pub const CHROME_INSET: f64 = 2.0;

/// `FlyoutPlacement.EdgePadding`.
pub const EDGE_PADDING: f64 = 8.0;
/// `FlyoutPlacement.WorkAreaMinHeight`.
pub const WORK_AREA_MIN_HEIGHT: f64 = 220.0;
/// `FlyoutPlacement.SnapTolerancePercent`.
pub const SNAP_TOLERANCE_PERCENT: f64 = 0.02;
/// `FlyoutPlacement.PixelMinSize`.
pub const PIXEL_MIN_SIZE: i32 = 1;

/// `Flyout.AppSliderRowHeight`: per-row cap of the slider drawer height.
pub const APP_SLIDER_ROW_HEIGHT: f64 = 28.0;
/// `Flyout.AppIconGridSlotSize`.
pub const APP_ICON_GRID_SLOT_SIZE: f64 = 36.0;
/// `Flyout.AppIconImageSize`.
pub const APP_ICON_IMAGE_SIZE: f64 = 22.0;
/// `Flyout.AppIconGlyphSize`.
pub const APP_ICON_GLYPH_SIZE: f64 = 20.0;
/// `Flyout.AppIconCellPillExtra`.
pub const APP_ICON_CELL_PILL_EXTRA: f64 = 2.0;
/// `Flyout.DeviceAppBandGridPadding` left + right (13 + 11).
pub const APP_BAND_GRID_HORIZONTAL_PADDING: f64 = 24.0;

/// `Flyout.FormatMenuItemHeight`.
pub const FORMAT_MENU_ITEM_HEIGHT: f64 = 34.0;
/// `Flyout.FormatMenuMaxVisibleItems`.
pub const FORMAT_MENU_MAX_VISIBLE_ITEMS: usize = 8;
/// `Flyout.FormatMenuPaddingReserve`.
pub const FORMAT_MENU_PADDING_RESERVE: f64 = 8.0;

/// `FlyoutSlider.DefaultMaximum`.
pub const SLIDER_MAXIMUM: f64 = 100.0;
/// `FlyoutSlider` value change threshold: sets closer than this are ignored.
pub const SLIDER_VALUE_EPSILON: f64 = 0.001;

/// `TimeConstants.BluetoothConnectionAttemptTimeoutMs`: the reconnect observation window.
pub const BLUETOOTH_CONNECTION_ATTEMPT_TIMEOUT_MS: u64 = 30_000;
/// `BluetoothConnectionCountdownOverlay.PieSteps`: one pie geometry per degree.
pub const PIE_STEPS: i32 = 360;
/// Stepped countdown interval when animations are off (`DispatcherTimer` 1 s).
pub const COUNTDOWN_STEPPED_INTERVAL_MS: u64 = 1_000;

/// `EqualizerAPOMonitor.LatestInstallerURL`: pinned Sourceforge mirror of the x64 installer.
pub const EQUALIZER_APO_LATEST_INSTALLER_URL: &str =
    "https://sourceforge.net/projects/equalizerapo/files/1.4.2/EqualizerAPO-x64-1.4.2.exe/download";

/// Inner width of the icon-grid drawer: window minus chrome minus the grid band padding.
pub const ICON_GRID_AVAILABLE_WIDTH: f64 = WINDOW_WIDTH - 2.0 * CHROME_INSET - APP_BAND_GRID_HORIZONTAL_PADDING;
