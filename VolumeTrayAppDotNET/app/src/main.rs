// The binary is named after the product exe (VolumeTrayAppDotNET.exe); all code lives in the
// `volume_tray_app_dotnet` library.
#![allow(non_snake_case)]
#![windows_subsystem = "windows"]

fn main() {
    std::process::exit(volume_tray_app_dotnet::run());
}
