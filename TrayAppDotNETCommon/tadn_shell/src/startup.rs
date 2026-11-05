//! Run on startup through a Startup-folder shortcut (port of `Services/Install/StartupManager.cs`
//! and `Interop/ShellLink.cs`, minus install-scope preference: this build has no installer, so the
//! target is always the running exe).
//!
//! Shortcut: `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\<instance_name>.lnk`,
//! arguments `--autostart` plus ` --settings-dir <abs>` (quoted per `CommandLineToArgvW` when it
//! holds spaces) when an override is active, working
//! directory = the exe's folder, description = the app name.

use std::path::{Path, PathBuf};

use tadn_common::identity::AppIdentity;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile, STGM_READ};
use windows::Win32::UI::Shell::{FOLDERID_Startup, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink};
use windows_core::{Interface, PCWSTR};

use crate::util::{ComScope, from_wide, wide};

/// `TrayAppDotNETProgram.AutostartArgument`.
pub const AUTOSTART_ARGUMENT: &str = "--autostart";
/// `TrayAppDotNETProgram.SettingsDirectoryArgument`.
pub const SETTINGS_DIR_ARGUMENT: &str = "--settings-dir";
/// `SLGP_RAWPATH`: the target as stored, without environment-variable expansion.
const SLGP_RAWPATH: u32 = 0x0004;
/// Long-path and argument maximum (32,767 characters plus the terminator).
const MAX_PATH_CHARS: usize = 32_768;

/// Full path of the shortcut file.
pub fn shortcut_path(identity: &AppIdentity) -> PathBuf {
    startup_folder().join(identity.startup_shortcut_file_name())
}

/// `Environment.SpecialFolder.Startup` (`FOLDERID_Startup`), else the conventional path under
/// `%APPDATA%`.
fn startup_folder() -> PathBuf {
    // SAFETY: the returned buffer is copied and freed with CoTaskMemFree.
    unsafe {
        match SHGetKnownFolderPath(&FOLDERID_Startup, KF_FLAG_DEFAULT, None) {
            Ok(path) => {
                let folder = PathBuf::from(path.to_string().unwrap_or_default());
                CoTaskMemFree(Some(path.0 as *const _));
                if !folder.as_os_str().is_empty() {
                    return folder;
                }
            }
            Err(error) => log::warn!("startup: SHGetKnownFolderPath(Startup) failed: {error}"),
        }
    }
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(r"Microsoft\Windows\Start Menu\Programs\Startup")
}

/// True when the shortcut exists (the toggle mirrors this).
pub fn is_enabled(identity: &AppIdentity) -> bool {
    shortcut_path(identity).is_file()
}

/// Creates/replaces or deletes the shortcut.
pub fn set_enabled(
    identity: &AppIdentity,
    enabled: bool,
    settings_dir_override: Option<&Path>,
) -> windows_core::Result<()> {
    let path = shortcut_path(identity);
    if !enabled {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(&error)),
        };
    }
    let target = startup_target().ok_or_else(|| windows_core::Error::new(E_FAIL, "the running exe path is unknown"))?;
    create_shortcut(&path, &target, &autostart_arguments(settings_dir_override), identity.app_name)
}

/// Where the existing shortcut points, for the settings card.
pub fn shortcut_target(identity: &AppIdentity) -> Option<PathBuf> {
    let path = shortcut_path(identity);
    if !path.is_file() {
        return None;
    }
    match read_shortcut(&path) {
        Ok((target, _)) if !target.is_empty() => Some(PathBuf::from(target)),
        Ok(_) => None,
        Err(error) => {
            log::warn!("startup: reading {} failed: {error}", path.display());
            None
        }
    }
}

/// Startup repair: an existing shortcut whose target no longer exists is repointed at this exe,
/// and one that lacks `--autostart` gains it; existing arguments (a settings folder override, for
/// one) are kept (`RepairShortcutIfStale`). A shortcut without arguments that has to be rewritten
/// gets the arguments for `settings_dir_override`.
pub fn repair_if_stale(identity: &AppIdentity, settings_dir_override: Option<&Path>) {
    let path = shortcut_path(identity);
    if !path.is_file() {
        return;
    }
    let (target, arguments) = match read_shortcut(&path) {
        Ok(read) => read,
        Err(error) => {
            log::warn!("startup: reading {} failed: {error}", path.display());
            return;
        }
    };
    let target_exists = !target.is_empty() && Path::new(&target).is_file();
    let has_autostart = has_autostart_argument(&arguments);
    if target_exists && has_autostart {
        return;
    }

    let new_target = if target_exists {
        PathBuf::from(&target)
    } else {
        match startup_target() {
            Some(exe) => exe,
            None => return,
        }
    };
    let new_arguments = if arguments.trim().is_empty() {
        autostart_arguments(settings_dir_override)
    } else {
        preserved_arguments(&arguments)
    };
    log::info!("startup: repairing {} → {} {new_arguments}", path.display(), new_target.display());
    if let Err(error) = create_shortcut(&path, &new_target, &new_arguments, identity.app_name) {
        log::warn!("startup: repairing {} failed: {error}", path.display());
    }
}

/// `ResolveStartupTarget` without install layouts: the running exe, which must end in `.exe`.
fn startup_target() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe")).then_some(exe)
}

/// `--autostart`, plus ` --settings-dir <abs>` (quoted when needed) when an override is active.
pub fn autostart_arguments(settings_dir_override: Option<&Path>) -> String {
    match settings_dir_override {
        Some(dir) => {
            let absolute = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
            format!("{AUTOSTART_ARGUMENT} {SETTINGS_DIR_ARGUMENT} {}", quote_argument(&absolute.to_string_lossy()))
        }
        None => AUTOSTART_ARGUMENT.to_owned(),
    }
}

/// `PreservedShortcutArguments`: the existing arguments with `--autostart` added in front when
/// missing.
fn preserved_arguments(arguments: &str) -> String {
    let arguments = arguments.trim();
    if arguments.is_empty() {
        AUTOSTART_ARGUMENT.to_owned()
    } else if has_autostart_argument(arguments) {
        arguments.to_owned()
    } else {
        format!("{AUTOSTART_ARGUMENT} {arguments}")
    }
}

/// `ShortcutHasAutostartArgument`: space-separated tokens, case-insensitive.
fn has_autostart_argument(arguments: &str) -> bool {
    arguments.split(' ').any(|token| token.eq_ignore_ascii_case(AUTOSTART_ARGUMENT))
}

/// `QuoteCommandLineArgument`: an argument without spaces, tabs or quotes stays as is; anything
/// else is wrapped in quotes with backslashes doubled before a quote and at the end and quotes
/// escaped, so `CommandLineToArgvW` reads it back unchanged.
pub fn quote_argument(argument: &str) -> String {
    if !argument.is_empty() && !argument.contains([' ', '\t', '"']) {
        return argument.to_owned();
    }
    let mut quoted = String::with_capacity(argument.len() + 2);
    quoted.push('"');
    let mut backslashes = 0usize;
    for c in argument.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(c);
                backslashes = 0;
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

fn io_error(error: &std::io::Error) -> windows_core::Error {
    match error.raw_os_error() {
        Some(code) => windows_core::Error::from_hresult(windows_core::HRESULT::from_win32(code as u32)),
        None => windows_core::Error::new(E_FAIL, error.to_string()),
    }
}

/// `ShellLink.Create`: target, arguments, working directory = the target's folder, description.
fn create_shortcut(path: &Path, target: &Path, arguments: &str, description: &str) -> windows_core::Result<()> {
    if let Some(folder) = path.parent()
        && let Err(error) = std::fs::create_dir_all(folder)
    {
        return Err(io_error(&error));
    }
    let _com = ComScope::enter();
    let target_wide = wide(&target.to_string_lossy());
    let arguments_wide = wide(arguments);
    let folder_wide = target.parent().map(|folder| wide(&folder.to_string_lossy()));
    let description_wide = wide(description);
    let path_wide = wide(&path.to_string_lossy());
    // SAFETY: COM is initialized for the scope; every string is NUL-terminated and outlives the
    // call that receives it.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(PCWSTR(target_wide.as_ptr()))?;
        if !arguments.is_empty() {
            link.SetArguments(PCWSTR(arguments_wide.as_ptr()))?;
        }
        if let Some(folder) = &folder_wide {
            link.SetWorkingDirectory(PCWSTR(folder.as_ptr()))?;
        }
        link.SetDescription(PCWSTR(description_wide.as_ptr()))?;
        link.cast::<IPersistFile>()?.Save(PCWSTR(path_wide.as_ptr()), true)
    }
}

/// `ShellLink.TryRead` + `TryReadArguments`: the raw target path and the arguments.
fn read_shortcut(path: &Path) -> windows_core::Result<(String, String)> {
    let _com = ComScope::enter();
    let path_wide = wide(&path.to_string_lossy());
    // SAFETY: COM is initialized for the scope; the buffers hold MAX_PATH_CHARS units.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.cast::<IPersistFile>()?.Load(PCWSTR(path_wide.as_ptr()), STGM_READ)?;
        let mut buffer = vec![0u16; MAX_PATH_CHARS];
        link.GetPath(&mut buffer, std::ptr::null_mut(), SLGP_RAWPATH)?;
        let target = from_wide(&buffer);
        buffer.fill(0);
        link.GetArguments(&mut buffer)?;
        Ok((target, from_wide(&buffer)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_quoted_for_command_line_to_argv() {
        assert_eq!(quote_argument(r"C:\Data\Volume"), r"C:\Data\Volume");
        assert_eq!(quote_argument(""), r#""""#);
        assert_eq!(quote_argument(r"C:\Data dir\"), r#""C:\Data dir\\""#);
        assert_eq!(quote_argument(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_argument(r#"a\"b"#), r#""a\\\"b""#);
    }

    #[test]
    fn autostart_arguments_carry_the_settings_override() {
        assert_eq!(autostart_arguments(None), "--autostart");
        assert_eq!(autostart_arguments(Some(Path::new(r"C:\Some Dir"))), r#"--autostart --settings-dir "C:\Some Dir""#);
    }

    #[test]
    fn preserved_arguments_gain_the_autostart_marker_once() {
        assert_eq!(preserved_arguments(""), "--autostart");
        assert_eq!(preserved_arguments(r#"--settings-dir "C:\x""#), r#"--autostart --settings-dir "C:\x""#);
        assert_eq!(preserved_arguments("--AUTOSTART --hidden"), "--AUTOSTART --hidden");
    }

    #[test]
    fn shortcut_round_trips_in_a_temp_folder() {
        let folder = std::env::temp_dir().join(format!("tadn_shell_startup_test_{}", std::process::id()));
        let path = folder.join("TadnShellStartupTest.lnk");
        let target = std::env::current_exe().expect("test exe");

        create_shortcut(&path, &target, "--autostart --hidden", "TadnShellStartupTest").expect("create shortcut");
        let (read_target, read_arguments) = read_shortcut(&path).expect("read shortcut");
        let _ = std::fs::remove_dir_all(&folder);

        assert!(read_target.eq_ignore_ascii_case(&target.to_string_lossy()));
        assert_eq!(read_arguments, "--autostart --hidden");
    }
}
