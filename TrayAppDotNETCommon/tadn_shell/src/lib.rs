//! Win32 shell services for TrayAppDotNET Slint apps (port of the Win32 parts of
//! `TrayAppDotNETCommon`): Slint backend selection and per-window native styling, notification
//! area icon with raw-input wheel and precision-touchpad scrolling, glyph icon rasterization,
//! global hotkeys, single instance, Startup-folder shortcut, taskbar/monitor placement, the
//! taskbar foreground tracker behind tray toggling, and system change notifications.
//!
//! Threading: everything here is created and used on the Slint UI thread (the winit event-loop
//! thread, which pumps Win32 messages for every window of the thread). Callbacks into app code are
//! delivered asynchronously through `slint::invoke_from_event_loop`/timers, never re-entrantly from
//! inside a window procedure.

pub mod backend;
pub mod dialogs;
pub mod foreground;
pub mod hotkeys;
pub mod instance;
pub mod placement;
pub mod raster;
pub mod startup;
pub mod system;
pub mod tray;
pub mod window;

mod msgwnd;
mod util;
