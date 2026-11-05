//! Slint backend selection and creation-time native window attributes.
//!
//! Slint creates a component's window adapter, and runs the winit window-attributes hook installed
//! by [`init`], when the component is constructed (`Component::new()`); the winit window itself
//! follows on the first `show()`. Creation-time attributes (no taskbar button, not in Alt+Tab,
//! topmost, no frame) are therefore keyed by the [`WindowKind`] set with [`with_window_kind`]
//! around the component's construction.

use std::cell::Cell;

use slint::winit_030::winit;
use tadn_common::settings::RenderingBackend;
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{WindowAttributes, WindowLevel};

use windows::Win32::Foundation::HWND;

use crate::msgwnd::MessageWindow;

/// How a window should look and behave natively.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowKind {
    /// Ordinary top-level window with a taskbar button (settings window).
    #[default]
    Normal,
    /// Borderless, topmost, no taskbar button, not in Alt+Tab (flyout).
    Flyout,
    /// Borderless, topmost, no taskbar button, not in Alt+Tab, never steals activation from its
    /// owner more than needed (context menus).
    Menu,
    /// Borderless, not topmost, no taskbar button, not in Alt+Tab, owned by `owner`: stays above
    /// it and hides while it is minimized (the settings window's color picker).
    Owned { owner: HWND },
}

/// Which renderer actually started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererInUse {
    Skia,
    Software,
}

thread_local! {
    static CURRENT_KIND: Cell<WindowKind> = const { Cell::new(WindowKind::Normal) };
    /// Hidden top-level window that owns every Flyout/Menu window. Owned windows get no taskbar
    /// button and are left out of Alt+Tab (the C# windows used `WS_EX_TOOLWINDOW` for that).
    static TOOL_OWNER: std::cell::OnceCell<Option<MessageWindow>> = const { std::cell::OnceCell::new() };
}

/// Selects the winit backend with the renderer for `backend` (`GPUPreferred` → Skia, falling back
/// to software when Skia cannot start; `Software` → software) and installs the window-attributes
/// hook. Call once on the UI thread before creating any Slint component.
pub fn init(backend: RenderingBackend) -> Result<RendererInUse, slint::PlatformError> {
    if backend == RenderingBackend::GPUPreferred {
        match select("skia") {
            Ok(()) => return Ok(RendererInUse::Skia),
            Err(error) => log::warn!("Slint Skia renderer unavailable ({error}); falling back to software rendering"),
        }
    }
    select("software").map(|()| RendererInUse::Software)
}

fn select(renderer: &str) -> Result<(), slint::PlatformError> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer.into())
        .with_winit_window_attributes_hook(apply_window_kind)
        .select()
}

/// Runs `f` (the component's `new()`, optionally together with its first `show()`) with `kind` as
/// the attributes the hook applies to any window created during `f`.
pub fn with_window_kind<R>(kind: WindowKind, f: impl FnOnce() -> R) -> R {
    struct Restore(WindowKind);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT_KIND.with(|current| current.set(self.0));
        }
    }

    let _restore = Restore(CURRENT_KIND.with(|current| current.replace(kind)));
    f()
}

fn apply_window_kind(attributes: WindowAttributes) -> WindowAttributes {
    match CURRENT_KIND.with(Cell::get) {
        WindowKind::Normal => attributes,
        WindowKind::Flyout | WindowKind::Menu => {
            let attributes =
                attributes.with_skip_taskbar(true).with_window_level(WindowLevel::AlwaysOnTop).with_decorations(false);
            match tool_owner() {
                Some(owner) => attributes.with_owner_window(owner),
                None => attributes,
            }
        }
        WindowKind::Owned { owner } => {
            attributes.with_skip_taskbar(true).with_decorations(false).with_owner_window(owner.0 as isize)
        }
    }
}

fn tool_owner() -> Option<isize> {
    TOOL_OWNER.with(|owner| {
        owner
            .get_or_init(|| {
                match MessageWindow::new(
                    "TrayAppDotNETSlint.ToolWindowOwner",
                    false,
                    std::rc::Rc::new(|_, _, _, _| None),
                ) {
                    Ok(window) => Some(window),
                    Err(error) => {
                        log::warn!("backend: tool window owner not created ({error}); flyouts may show in Alt+Tab");
                        None
                    }
                }
            })
            .as_ref()
            .map(|window| window.hwnd().0 as isize)
    })
}
