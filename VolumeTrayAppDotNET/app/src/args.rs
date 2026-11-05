//! Command line (port of the tray-mode parts of `ProgramStartup.cs`). Flags match
//! case-insensitively; `--settings-dir` takes the next argument. Install, uninstall, update and
//! crash-watcher arguments belong to features this build does not ship: they are reported and the
//! process exits with the usage error code.

use std::ffi::{OsStr, OsString};

pub const SETTINGS_DIR_ARGUMENT: &str = "--settings-dir";
pub const AUTOSTART_ARGUMENT: &str = "--autostart";
pub const HIDDEN_ARGUMENT: &str = "--hidden";

/// Arguments of the .NET build's installer, uninstaller, updater and crash watcher.
const UNSUPPORTED_ARGUMENTS: &[&str] = &[
    "--watcher",
    "--monitored",
    "--watcher-pid",
    "--installer",
    "--install-gui",
    "--install",
    "--installlocal",
    "--installsystem",
    "--install-headless",
    "--install-system",
    "--sync-start-menu",
    "--uninstall-prepare",
    "--uninstall",
    "--uninstall-gui",
    "--uninstall-headless",
    "--update-apply",
    "--update-restart",
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaunchArgs {
    /// Raw `--settings-dir` value (resolved by `tadn_common::paths::initialize`).
    pub settings_dir: Option<OsString>,
    /// `--autostart`: started by the Startup shortcut. Accepted, no effect (as in the C# app).
    pub autostart: bool,
    /// `--hidden`: start without a window. Accepted, no effect (the app never opens one at start).
    pub hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgError {
    /// `--settings-dir` without a usable value.
    MissingSettingsDir,
    /// An install/update/watcher argument.
    Unsupported(String),
}

impl std::fmt::Display for ArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSettingsDir => write!(f, "{SETTINGS_DIR_ARGUMENT} must be followed by a folder path."),
            Self::Unsupported(argument) => write!(
                f,
                "{argument} is not supported by this build of VolumeTrayAppDotNET (it has no installer, updater or crash watcher)."
            ),
        }
    }
}

fn is_flag(argument: &OsStr, flag: &str) -> bool {
    argument.to_str().is_some_and(|a| a.eq_ignore_ascii_case(flag))
}

/// Parses the arguments after the program name.
pub fn parse(args: &[OsString]) -> Result<LaunchArgs, ArgError> {
    let mut parsed = LaunchArgs::default();
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if let Some(unsupported) = UNSUPPORTED_ARGUMENTS.iter().find(|flag| is_flag(argument, flag)) {
            return Err(ArgError::Unsupported((*unsupported).to_owned()));
        }
        if is_flag(argument, SETTINGS_DIR_ARGUMENT) {
            let value = args.get(index + 1).ok_or(ArgError::MissingSettingsDir)?;
            let text = value.to_string_lossy();
            let trimmed = text.trim().trim_matches('"');
            // A flag in the value position means the path itself was left out.
            if trimmed.is_empty() || trimmed.starts_with("--") {
                return Err(ArgError::MissingSettingsDir);
            }
            parsed.settings_dir = Some(value.clone());
            index += 2;
            continue;
        }
        if is_flag(argument, AUTOSTART_ARGUMENT) {
            parsed.autostart = true;
        } else if is_flag(argument, HIDDEN_ARGUMENT) {
            parsed.hidden = true;
        }
        index += 1;
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn empty_command_line() {
        assert_eq!(parse(&[]), Ok(LaunchArgs::default()));
    }

    #[test]
    fn flags_are_case_insensitive() {
        let parsed = parse(&args(&["--AUTOSTART", "--Hidden", "--Settings-Dir", "C:\\x"])).unwrap();
        assert!(parsed.autostart && parsed.hidden);
        assert_eq!(parsed.settings_dir, Some(OsString::from("C:\\x")));
    }

    #[test]
    fn settings_dir_needs_a_value() {
        assert_eq!(parse(&args(&["--settings-dir"])), Err(ArgError::MissingSettingsDir));
        assert_eq!(parse(&args(&["--settings-dir", "  \"\" "])), Err(ArgError::MissingSettingsDir));
        assert_eq!(parse(&args(&["--settings-dir", "--hidden"])), Err(ArgError::MissingSettingsDir));
    }

    #[test]
    fn unknown_arguments_are_ignored() {
        assert_eq!(parse(&args(&["--something", "x"])), Ok(LaunchArgs::default()));
    }

    #[test]
    fn install_update_and_watcher_arguments_are_rejected() {
        for flag in ["--install", "--Uninstall", "--watcher", "--update-apply", "--monitored"] {
            assert!(matches!(parse(&args(&[flag])), Err(ArgError::Unsupported(_))), "{flag}");
        }
    }
}
