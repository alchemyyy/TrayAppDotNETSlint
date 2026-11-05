use std::collections::HashMap;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let tadn_ui = manifest_dir.join("../ui");
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent".into())
        .with_library_paths(HashMap::from([("tadn".to_owned(), tadn_ui)]));
    slint_build::compile_with_config("ui/menu_window.slint", config).expect("Slint compilation failed");
}
