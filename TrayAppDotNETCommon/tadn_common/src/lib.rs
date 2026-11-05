//! UI-toolkit-agnostic infrastructure shared by TrayAppDotNET Slint apps (port of the non-UI parts
//! of `TrayAppDotNETCommon`): app identity, data folder, logging, XML settings persistence, theme
//! palette, hotkey model, settings search scoring and shell launching.
//!
//! Nothing here touches Slint; UI crates push [`theme::Palette`] into their `Theme` global with
//! [`apply_palette!`].

pub mod hotkey;
pub mod identity;
pub mod log;
pub mod paths;
pub mod search;
pub mod settings;
pub mod shell;
pub mod theme;
pub mod xml;
