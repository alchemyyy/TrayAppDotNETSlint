use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let tadn_ui = manifest_dir.join("../../TrayAppDotNETCommon/ui");
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_library_paths(HashMap::from([("tadn".to_owned(), tadn_ui)]));
    slint_build::compile_with_config("ui/settings_window.slint", config).expect("Slint compilation failed");

    // About page facts (`BuildInfo` / `RuntimeInformation.FrameworkDescription`).
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let rustc_version = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "rustc".to_owned());
    println!("cargo:rustc-env=TADN_RUNTIME_DESCRIPTION={rustc_version}, Slint {}", slint_version(&manifest_dir));
    if let Some(hash) = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(&manifest_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
    {
        println!("cargo:rustc-env=TADN_COMMIT_HASH={hash}");
    }
    println!("cargo:rerun-if-env-changed=TADN_BUILD_NUMBER");
}

/// The resolved `slint` version from the workspace `Cargo.lock`.
fn slint_version(manifest_dir: &std::path::Path) -> String {
    let lock = manifest_dir.join("../../Cargo.lock");
    let text = std::fs::read_to_string(lock).unwrap_or_default();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "name = \"slint\""
            && let Some(version) = lines.next().and_then(|l| l.trim().strip_prefix("version = \""))
        {
            return version.trim_end_matches('"').to_owned();
        }
    }
    "1.18".to_owned()
}
