//! VolumeTrayAppDotNET composition root (port of `Program.cs`, `App.cs`, `AppServices.cs`,
//! `UI/Tray/VolumeTrayIcon.cs`, `UI/Tray/VolumeTrayMenuEntries.cs` and the non-install parts of
//! Common `ProgramStartup` / `TrayAppDotNETAppHost`).

mod app;
pub mod args;
mod persistence;
pub mod tray;

use std::ffi::OsString;
use std::rc::Rc;

use tadn_shell::instance::SingleInstance;
use vtadn_model::IDENTITY;
use vtadn_model::settings_handle::SettingsHandle;

use crate::app::{App, ExitAction, StartupState};
use crate::persistence::Stores;

/// `ProgramStartup.UsageErrorExitCode`.
const USAGE_ERROR_EXIT_CODE: i32 = 2;
const FAILURE_EXIT_CODE: i32 = 1;

/// Runs the app; returns the process exit code (0 normal exit, 2 bad arguments, 1 failure).
pub fn run() -> i32 {
    let raw_args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let launch = match args::parse(&raw_args) {
        Ok(launch) => launch,
        Err(error) => {
            tadn_shell::dialogs::error(IDENTITY.app_name, &error.to_string());
            return USAGE_ERROR_EXIT_CODE;
        }
    };

    let dirs = match tadn_common::paths::initialize(&IDENTITY, launch.settings_dir.as_deref()) {
        Ok(dirs) => dirs,
        Err(error) => {
            tadn_shell::dialogs::error(IDENTITY.app_name, &error.to_string());
            return match error {
                tadn_common::paths::PathError::InvalidArgument(_) => USAGE_ERROR_EXIT_CODE,
                tadn_common::paths::PathError::Io(..) => FAILURE_EXIT_CODE,
            };
        }
    };
    if let Err(error) = tadn_common::log::init(&dirs.app_dir) {
        // Logging is best effort; the app still runs.
        eprintln!("log init failed: {error}");
    }
    tadn_common::log::install_panic_hook();
    log::info!(
        "starting {} {} (autostart={}, hidden={}, data={}, imported={:?})",
        IDENTITY.app_name,
        env!("CARGO_PKG_VERSION"),
        launch.autostart,
        launch.hidden,
        dirs.app_dir.display(),
        dirs.imported_files
    );

    let instance = match SingleInstance::acquire(&IDENTITY) {
        Ok(instance) => instance,
        Err(error) => {
            log::error!("single instance: {error}");
            tadn_common::log::flush();
            return FAILURE_EXIT_CODE;
        }
    };

    let exit = run_tray(dirs, &instance);
    drop(instance);

    let code = match exit {
        Ok(ExitAction::Exit(code)) => code,
        Ok(ExitAction::Restart(arguments)) => restart(&arguments),
        Err(error) => {
            log::error!("startup failed: {error}");
            tadn_shell::dialogs::error(IDENTITY.app_name, &error);
            FAILURE_EXIT_CODE
        }
    };
    log::info!("exit code {code}");
    tadn_common::log::flush();
    code
}

/// Settings, theme, backend, services and the event loop (`BeginTrayMode` .. `Shutdown`).
fn run_tray(dirs: &'static tadn_common::paths::DataDirs, instance: &SingleInstance) -> Result<ExitAction, String> {
    let loaded = Stores::load(&dirs.app_dir);
    let run_on_startup = loaded.settings.common.run_on_startup;
    let backend = loaded.settings.common.rendering_backend;

    // First run without a settings folder override applies RunOnStartup (default: on). The
    // shortcut is shared by every settings folder, so a fresh override folder leaves it alone.
    if dirs.is_first_run
        && dirs.settings_dir_override.is_none()
        && let Err(error) = tadn_shell::startup::set_enabled(&IDENTITY, run_on_startup, None)
    {
        log::warn!("first run: run on startup: {error}");
    }
    tadn_shell::startup::repair_if_stale(&IDENTITY, dirs.settings_dir_override.as_deref());

    let theme = tadn_common::theme::load_theme_file(&dirs.app_dir.join(persistence::THEME_FILE_NAME));
    let renderer = tadn_shell::backend::init(backend).map_err(|e| format!("Slint backend: {e}"))?;
    log::info!("renderer: {renderer:?} (requested {backend:?})");

    let stores = Rc::new(loaded.stores);
    let settings = SettingsHandle::new(loaded.settings, loaded.devices);
    settings.set_persistence(stores.clone());

    let app = App::start(StartupState { dirs, settings, stores, theme })?;
    instance.listen_for_exit_request(|| {
        log::info!("exit requested by a newer instance");
        app::with_current(|app| app.exit());
    });

    let loop_result = slint::run_event_loop_until_quit();
    // Every exit path normally shut down already; this covers the loop ending on its own.
    app.shutdown();
    let action = app.exit_action();
    drop(app);
    loop_result.map_err(|e| format!("event loop: {e}"))?;
    Ok(action)
}

/// `TrayAppDotNETAppLifetime.Restart`: start the exe again with the same settings folder.
fn restart(arguments: &[OsString]) -> i32 {
    let started = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).args(arguments).spawn());
    match started {
        Ok(_) => 0,
        Err(error) => {
            log::error!("restart failed: {error}");
            tadn_shell::dialogs::error("Restart failed", &error.to_string());
            FAILURE_EXIT_CODE
        }
    }
}
