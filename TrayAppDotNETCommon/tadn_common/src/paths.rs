//! Data folder resolution (port of the data-folder parts of `ProgramStartup.cs`).
//!
//! Default folder: `%LOCALAPPDATA%\TrayAppDotNETSlint\<AppName>`; `--settings-dir <path>`
//! overrides it. On first run (no `settings.xml` in the default folder, no override) the .NET
//! app's `settings.xml`, `devices.xml` and `theme.xml` are copied from
//! `%LOCALAPPDATA%\TrayAppDotNET\<AppName>` when present (the originals are only read).

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::OnceLock;

use crate::identity::{AppIdentity, DATA_ROOT_FOLDER, LEGACY_DATA_ROOT_FOLDER};

/// `TrayAppDotNETProgram.SettingsDirectoryArgument`.
pub const SETTINGS_DIR_ARGUMENT: &str = "--settings-dir";
/// The settings file name inside the data folder.
pub const SETTINGS_FILE_NAME: &str = "settings.xml";
/// The per-device file name inside the data folder.
pub const DEVICES_FILE_NAME: &str = "devices.xml";
/// The user-authored theme file name inside the data folder.
pub const THEME_FILE_NAME: &str = "theme.xml";
/// Files the first run copies from the .NET app's folder.
pub const IMPORTED_FILE_NAMES: [&str; 3] = [SETTINGS_FILE_NAME, DEVICES_FILE_NAME, THEME_FILE_NAME];

#[derive(Clone, Debug)]
pub struct DataDirs {
    /// Folder holding settings.xml, devices.xml, theme.xml and the logs.
    pub app_dir: PathBuf,
    /// The `--settings-dir` override, absolute, when one was given.
    pub settings_dir_override: Option<PathBuf>,
    /// True when this run created `settings.xml` from nothing (no file, nothing imported, no
    /// `--settings-dir` override): the first-run defaults (Run on startup) apply.
    pub is_first_run: bool,
    /// Files copied from the .NET app's folder on this run.
    pub imported_files: Vec<PathBuf>,
}

impl DataDirs {
    pub fn settings_path(&self) -> PathBuf {
        self.app_dir.join(SETTINGS_FILE_NAME)
    }

    pub fn devices_path(&self) -> PathBuf {
        self.app_dir.join(DEVICES_FILE_NAME)
    }

    pub fn theme_path(&self) -> PathBuf {
        self.app_dir.join(THEME_FILE_NAME)
    }
}

#[derive(Debug)]
pub enum PathError {
    /// `--settings-dir` value that cannot be used (empty, trailing quote, invalid characters).
    InvalidArgument(String),
    /// Folder could not be created or accessed.
    Io(PathBuf, std::io::Error),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidArgument(message) => write!(f, "{message}"),
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for PathError {}

static DIRS: OnceLock<DataDirs> = OnceLock::new();

/// Resolves and creates the data folder, runs the first-run import, and stores the result for
/// [`get`]. Call once, before logging starts (import problems are logged once [`crate::log::init`]
/// runs). A second call returns the first result.
pub fn initialize(
    identity: &AppIdentity,
    settings_dir_override: Option<&OsStr>,
) -> Result<&'static DataDirs, PathError> {
    if let Some(dirs) = DIRS.get() {
        return Ok(dirs);
    }
    let settings_dir_override = match settings_dir_override {
        Some(raw) => {
            let cwd = std::env::current_dir().map_err(|error| PathError::Io(PathBuf::from("."), error))?;
            Some(resolve_settings_dir_argument(raw, &cwd)?)
        }
        None => None,
    };
    let dirs = prepare(default_app_dir(identity), &legacy_app_dir(identity), settings_dir_override)?;
    Ok(DIRS.get_or_init(|| dirs))
}

/// Creates the folder and runs the import rules; separated from [`initialize`] for tests.
fn prepare(
    default_dir: PathBuf,
    legacy_dir: &Path,
    settings_dir_override: Option<PathBuf>,
) -> Result<DataDirs, PathError> {
    let app_dir = settings_dir_override.clone().unwrap_or(default_dir);
    std::fs::create_dir_all(&app_dir).map_err(|error| PathError::Io(app_dir.clone(), error))?;
    let settings_existed = app_dir.join(SETTINGS_FILE_NAME).exists();
    let imported_files = if settings_existed || settings_dir_override.is_some() {
        Vec::new()
    } else {
        import_legacy_files(legacy_dir, &app_dir)
    };
    let imported_settings = imported_files.iter().any(|p| p.file_name() == Some(OsStr::new(SETTINGS_FILE_NAME)));
    Ok(DataDirs {
        is_first_run: !settings_existed && !imported_settings && settings_dir_override.is_none(),
        app_dir,
        settings_dir_override,
        imported_files,
    })
}

/// Copies each of [`IMPORTED_FILE_NAMES`] present in `legacy_dir` and absent in `app_dir`. The
/// originals are read only; each copy is written atomically. Returns the copied destination paths.
fn import_legacy_files(legacy_dir: &Path, app_dir: &Path) -> Vec<PathBuf> {
    let mut imported = Vec::new();
    for name in IMPORTED_FILE_NAMES {
        let source = legacy_dir.join(name);
        let destination = app_dir.join(name);
        if !source.is_file() || destination.exists() {
            continue;
        }
        match std::fs::read(&source).and_then(|bytes| crate::xml::atomic_write(&destination, &bytes)) {
            Ok(()) => {
                crate::log::early(format!("Imported {} from {}", destination.display(), source.display()));
                imported.push(destination);
            }
            Err(error) => crate::log::early(format!(
                "WARN: could not import {} into {}: {error}",
                source.display(),
                app_dir.display()
            )),
        }
    }
    imported
}

/// The folders resolved by [`initialize`]. Panics if called before it.
pub fn get() -> &'static DataDirs {
    DIRS.get().expect("tadn_common::paths::initialize must run before paths::get")
}

/// `%LOCALAPPDATA%` (`Environment.SpecialFolder.LocalApplicationData`).
pub fn local_app_data() -> PathBuf {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: the returned buffer is copied, then freed with CoTaskMemFree exactly once.
    let known = unsafe {
        SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None).map(|path| {
            let owned = OsString::from_wide(path.as_wide());
            CoTaskMemFree(Some(path.0.cast()));
            PathBuf::from(owned)
        })
    };
    known.unwrap_or_else(|error| {
        crate::log::early(format!("WARN: SHGetKnownFolderPath(LocalAppData) failed: {error}"));
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default()
    })
}

/// `%LOCALAPPDATA%\TrayAppDotNETSlint\<app>`.
pub fn default_app_dir(identity: &AppIdentity) -> PathBuf {
    local_app_data().join(DATA_ROOT_FOLDER).join(identity.app_name)
}

/// `%LOCALAPPDATA%\TrayAppDotNET\<app>` (the .NET app's folder).
pub fn legacy_app_dir(identity: &AppIdentity) -> PathBuf {
    local_app_data().join(LEGACY_DATA_ROOT_FOLDER).join(identity.app_name)
}

/// Characters Windows rejects in a path (`"<>|*?` and U+0000–U+001F).
fn is_invalid_settings_dir_char(unit: u16) -> bool {
    unit < 0x20 || matches!(unit, 0x22 | 0x3C | 0x3E | 0x7C | 0x2A | 0x3F)
}

/// `Environment.ExpandEnvironmentVariables` (`ExpandEnvironmentStringsW`: defined `%NAME%`
/// references are replaced, others left as written).
fn expand_environment_variables(value: &[u16]) -> Vec<u16> {
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
    use windows_core::PCWSTR;
    let source: Vec<u16> = value.iter().copied().chain(std::iter::once(0)).collect();
    let mut capacity = source.len() + 64;
    for _ in 0..4 {
        let mut buffer = vec![0u16; capacity];
        // SAFETY: `source` is NUL-terminated and `buffer` is a live buffer of `capacity` units.
        let needed = unsafe { ExpandEnvironmentStringsW(PCWSTR(source.as_ptr()), Some(&mut buffer)) } as usize;
        if needed == 0 {
            break;
        }
        if needed <= capacity {
            buffer.truncate(needed.saturating_sub(1));
            return buffer;
        }
        capacity = needed;
    }
    value.to_vec()
}

/// `Path.GetFullPath`: makes `path` absolute against `cwd` and normalizes it (separators, `.` and
/// `..` segments, trailing dots/spaces of segments) through `GetFullPathNameW`.
fn full_path(path: &Path, cwd: &Path) -> Result<PathBuf, String> {
    use windows::Win32::Storage::FileSystem::GetFullPathNameW;
    use windows_core::PCWSTR;
    let mut components = path.components();
    let absolute: PathBuf = match components.next() {
        Some(Component::Prefix(prefix)) => {
            let has_root = matches!(components.next(), Some(Component::RootDir));
            if has_root {
                path.to_path_buf()
            } else {
                // Drive-relative ("C:folder"): relative to cwd when cwd is on that drive.
                let same_drive = match (prefix.kind(), cwd.components().next()) {
                    (Prefix::Disk(a), Some(Component::Prefix(p))) => {
                        matches!(p.kind(), Prefix::Disk(b) | Prefix::VerbatimDisk(b) if a.eq_ignore_ascii_case(&b))
                    }
                    _ => false,
                };
                let rest: PathBuf = path.components().skip(1).collect();
                if same_drive {
                    cwd.join(rest)
                } else {
                    let mut rooted = OsString::from(prefix.as_os_str());
                    rooted.push("\\");
                    PathBuf::from(rooted).join(rest)
                }
            }
        }
        Some(Component::RootDir) => match cwd.components().next() {
            Some(Component::Prefix(prefix)) => {
                let mut rooted = OsString::from(prefix.as_os_str());
                rooted.push(path.as_os_str());
                PathBuf::from(rooted)
            }
            _ => path.to_path_buf(),
        },
        _ => cwd.join(path),
    };
    let wide: Vec<u16> = absolute.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut capacity = wide.len() + 16;
    for _ in 0..4 {
        let mut buffer = vec![0u16; capacity];
        // SAFETY: `wide` is NUL-terminated; `buffer` is a live buffer of `capacity` units.
        let length = unsafe { GetFullPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buffer), None) } as usize;
        if length == 0 {
            return Err(windows_core::Error::from_thread().message());
        }
        if length < capacity {
            buffer.truncate(length);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        capacity = length + 1;
    }
    Err("the path is too long".to_owned())
}

/// `Path.TrimEndingDirectorySeparator`: drops one trailing separator unless the path is a root.
fn trim_ending_separator(path: PathBuf) -> PathBuf {
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    let is_separator = |u: u16| u == u16::from(b'\\') || u == u16::from(b'/');
    if wide.len() > 1 && wide.last().is_some_and(|&u| is_separator(u)) {
        let trimmed = PathBuf::from(OsString::from_wide(&wide[..wide.len() - 1]));
        let is_root = path.parent().is_none() || (wide.len() == 3 && wide[1] == u16::from(b':'));
        if !is_root {
            return trimmed;
        }
    }
    path
}

/// Expands environment variables, resolves relative paths against `cwd`, trims surrounding
/// quotes and rejects a value ending in `"` (the `"C:\dir\"` quoting trap)
/// (`TryResolveSettingsDirectory`): the value is trimmed, then stripped of `"` at both ends;
/// empty or `--`-prefixed values, and values containing `"<>|*?` or control characters, are
/// rejected; the result is absolute with no trailing separator.
pub fn resolve_settings_dir_argument(raw: &OsStr, cwd: &Path) -> Result<PathBuf, PathError> {
    let text = raw.to_string_lossy();
    let value = text.trim().trim_matches('"');
    if value.is_empty() || value.starts_with("--") {
        return Err(PathError::InvalidArgument(format!("{SETTINGS_DIR_ARGUMENT} must be followed by a folder path.")));
    }
    let units: Vec<u16> = OsStr::new(value).encode_wide().collect();
    let expanded = expand_environment_variables(&units);
    if expanded.iter().any(|&u| is_invalid_settings_dir_char(u)) {
        return Err(PathError::InvalidArgument(format!(
            "{SETTINGS_DIR_ARGUMENT} path '{value}' contains a character Windows does not allow in a folder path. \
             A path that ends in a backslash must not be quoted."
        )));
    }
    let expanded = PathBuf::from(OsString::from_wide(&expanded));
    let full = full_path(&expanded, cwd).map_err(|message| {
        PathError::InvalidArgument(format!("{SETTINGS_DIR_ARGUMENT} path '{value}' is invalid: {message}"))
    })?;
    Ok(trim_ending_separator(full))
}

/// Finds the `--settings-dir` value in `args` (flag matched case-insensitively, value = next
/// argument; a flag with no following argument yields an empty value, which
/// [`resolve_settings_dir_argument`] rejects). None when the flag is absent.
pub fn settings_dir_argument<S: AsRef<OsStr>>(args: &[S]) -> Option<OsString> {
    let index = args.iter().position(|a| a.as_ref().to_string_lossy().eq_ignore_ascii_case(SETTINGS_DIR_ARGUMENT))?;
    Some(args.get(index + 1).map_or_else(OsString::new, |v| v.as_ref().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(value: &str) -> Result<PathBuf, PathError> {
        resolve_settings_dir_argument(OsStr::new(value), Path::new(r"C:\Work\Dir"))
    }

    #[test]
    fn settings_directory_resolves_to_an_absolute_folder() {
        for (value, expected) in [
            (r"D:\Settings\Volume", r"D:\Settings\Volume"),
            (r"D:\Settings\Volume\", r"D:\Settings\Volume"),
            ("D:\\Settings\\Volume\"", r"D:\Settings\Volume"),
            (r"D:\Settings\..\Other", r"D:\Other"),
            (r"D:\", r"D:\"),
        ] {
            assert_eq!(resolve(value).map_err(|e| e.to_string()), Ok(PathBuf::from(expected)), "{value}");
        }
    }

    #[test]
    fn relative_settings_directory_resolves_against_the_working_directory() {
        assert_eq!(resolve(r"portable\settings").ok(), Some(PathBuf::from(r"C:\Work\Dir\portable\settings")));
    }

    #[test]
    fn settings_directory_expands_environment_variables() {
        let expected = local_app_data().join("Custom");
        assert_eq!(resolve(r"%LOCALAPPDATA%\Custom").ok(), Some(expected));
    }

    #[test]
    fn settings_directory_without_a_usable_path_is_rejected() {
        for value in [
            "",
            "  ",
            "--hidden",
            "my settings\" --hidden",
            r"D:\Settings\<name>",
            r"D:\Settings\*",
            "D:\\Settings\tTab",
        ] {
            assert!(matches!(resolve(value), Err(PathError::InvalidArgument(_))), "{value:?}");
        }
        assert_eq!(settings_dir_argument(&["--settings-dir"]), Some(OsString::new()));
        assert!(settings_dir_argument(&["--hidden"]).is_none());
        assert_eq!(settings_dir_argument(&["--SETTINGS-DIR", "x"]), Some(OsString::from("x")));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tadn-paths-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn first_run_imports_legacy_files_without_touching_them() {
        let root = temp_dir("import");
        let legacy = root.join("legacy");
        std::fs::create_dir_all(&legacy).unwrap_or_default();
        std::fs::write(legacy.join(SETTINGS_FILE_NAME), b"<AppSettings />").unwrap_or_default();
        std::fs::write(legacy.join(DEVICES_FILE_NAME), b"<Devices />").unwrap_or_default();
        let dirs = prepare(root.join("slint"), &legacy, None).map_err(|e| e.to_string());
        let dirs = dirs.as_ref().map_err(String::clone).unwrap_or_else(|e| panic!("{e}"));
        assert!(!dirs.is_first_run);
        assert_eq!(dirs.imported_files.len(), 2);
        assert_eq!(std::fs::read(dirs.settings_path()).unwrap_or_default(), b"<AppSettings />");
        assert!(legacy.join(SETTINGS_FILE_NAME).exists() && legacy.join(DEVICES_FILE_NAME).exists());
        assert!(!dirs.theme_path().exists());

        // Second run: settings exist, nothing imported, not first run.
        let again = prepare(root.join("slint"), &legacy, None).map_err(|e| e.to_string());
        assert!(again.is_ok_and(|d| !d.is_first_run && d.imported_files.is_empty()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn first_run_without_legacy_files_or_with_override() {
        let root = temp_dir("fresh");
        let fresh = prepare(root.join("slint"), &root.join("missing"), None).map_err(|e| e.to_string());
        assert!(fresh.is_ok_and(|d| d.is_first_run && d.imported_files.is_empty() && d.app_dir.is_dir()));

        let legacy = root.join("legacy");
        std::fs::create_dir_all(&legacy).unwrap_or_default();
        std::fs::write(legacy.join(SETTINGS_FILE_NAME), b"x").unwrap_or_default();
        let overridden = prepare(root.join("unused"), &legacy, Some(root.join("override"))).map_err(|e| e.to_string());
        assert!(
            overridden
                .is_ok_and(|d| !d.is_first_run && d.imported_files.is_empty() && d.app_dir == root.join("override"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
