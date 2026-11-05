# TrayAppDotNETSlint

A Rust + [Slint](https://slint.dev) rewrite of **VolumeTrayAppDotNET** from
[TrayAppDotNET](https://github.com/alchemyyy/TrayAppDotNET): the EarTrumpet-like tray volume mixer with
bottom-up device cards, per-app drawers, peak meters, Bluetooth, Equalizer APO, ducking, hotkeys and the full
settings window. The C# app under `../TrayAppDotNET/VolumeTrayAppDotNET` is the behavioral reference; its user
documentation (`../TrayAppDotNET/documentation/docs/apps/volume/`) describes this build too, except where noted
below.

## Scope

Included: every VolumeTrayAppDotNET feature (tray icon with glyph tiers, tooltip, wheel and precision-touchpad
scrolling, modified click actions, tray menu; flyout with docking/undock, device cards, app drawers, meters, row
buttons, rename, percent editing, format and listen menus, Bluetooth countdown; all settings pages with search,
hotkeys, theme and about), plus the shared shell it relies on: single instance with replace-running, run on
startup, `--settings-dir`, `--autostart`, `--hidden`, logging.

Not included: installer, uninstaller, self-update and the crash watcher (they target the .NET release channel).
`--install*`, `--uninstall*`, `--update-*` and watcher arguments show an error and exit with code 2. The update
settings stay in `settings.xml` only so the file round-trips unchanged.

## Side by side with the .NET app

| | .NET app | This build |
|---|---|---|
| Data folder | `%LocalAppData%\TrayAppDotNET\VolumeTrayAppDotNET` | `%LocalAppData%\TrayAppDotNETSlint\VolumeTrayAppDotNET` |
| Single instance, exit event | `VolumeTrayAppDotNET…` | `VolumeTrayAppDotNETSlint…` |
| Startup shortcut | `VolumeTrayAppDotNET.lnk` | `VolumeTrayAppDotNETSlint.lnk` |
| Tray icon GUID | `1ac1ef49-…` | `e7c5d03c-9c5f-4f28-999d-f91ec7897b7a` |

`settings.xml`, `devices.xml` and `theme.xml` use the .NET app's exact format (byte-identical round trip). On the
first run without `--settings-dir`, if the Slint data folder has no `settings.xml`, those three files are copied
from the .NET app's folder (the originals are only read).

## Build

Requirements: Windows 10/11 x64, Rust stable (MSVC toolchain, `rust-version` 1.92+), the Windows SDK resource
compiler (used by `embed-resource`), and network access on the first build (the Skia binaries are downloaded).

```bat
cargo build --release -p volume_tray_app_dotnet
target\release\VolumeTrayAppDotNET.exe
```

Rendering follows the shared **Rendering backend** setting: *GPU preferred* uses Slint's Skia renderer (GPU, with
its own software fallback; if Skia cannot start at all the Slint software renderer is used), *Software* uses the
Slint software renderer. A change takes effect after a restart.

## Logs

`active.log` / `old.log` in the data folder (10 MB / 7 day rollover, same format as the .NET apps). Set
`TADN_LOG_LEVEL` to `error`, `warn`, `info`, `debug` or `trace` to override the level of the app's own lines
(`trace` also logs every tray icon callback, useful when diagnosing tray input).

## Layout

| Crate | Path | Ports |
|---|---|---|
| `tadn_common` | `TrayAppDotNETCommon/tadn_common` | identity, data folder + first-run import, logging, XML settings store, common settings, theme.xml + palette, hotkey model, settings search |
| `tadn_shell` | `TrayAppDotNETCommon/tadn_shell` | notification icon (raw-input wheel, precision touchpad), glyph rasterizer, global hotkeys, single instance, startup shortcut, placement, tray toggle, window helpers, Slint backend selection, system events |
| `tadn_menu` | `TrayAppDotNETCommon/tadn_menu` | context menu window (tray menu and in-window menus) |
| shared Slint UI | `TrayAppDotNETCommon/ui` | `Theme` global, glyphs, menu list, flyout slider, settings controls, color picker (imported as `@tadn/...`) |
| `vtadn_model` | `VolumeTrayAppDotNET/vtadn_model` | audio snapshot/command/bus contracts, settings model + XML mapping, settings handle, pure policies (visibility, ordering, drawers, log scale, meters, ding suppression), shell links |
| `vtadn_audio` | `VolumeTrayAppDotNET/vtadn_audio` | audio service thread: endpoints, defaults, IPolicyConfig, formats, listen, exclusive, rename, meters, ding policy |
| `vtadn_sessions` | `VolumeTrayAppDotNET/vtadn_sessions` | audio sessions, app groups, app icons, capture meter activation |
| `vtadn_native` | `VolumeTrayAppDotNET/vtadn_native` | Bluetooth (radio, pairing, connect/disconnect, battery, codec), Equalizer APO, ding playback, ducking |
| `vtadn_flyout` | `VolumeTrayAppDotNET/vtadn_flyout` | the flyout window |
| `vtadn_settings` | `VolumeTrayAppDotNET/vtadn_settings` | the settings window |
| `volume_tray_app_dotnet` | `VolumeTrayAppDotNET/app` | composition root, tray icon renderer and actions, tray menu entries; binary `VolumeTrayAppDotNET.exe` |

Threading: the Slint/winit event loop thread owns every window and the tray/hotkey/system message windows; one
MTA audio thread owns all Core Audio objects and publishes immutable snapshots through `vtadn_model::bus`; meters
go through a shared board read by the flyout's render timer.

## Differences from the .NET app

- No update button, update/installation cards or update notifications (see Scope).
- The settings window uses the native Windows title bar; search results briefly highlight the matched cards.
- Flyout and menu windows are kept off the taskbar and out of Alt+Tab by a hidden owner window rather than
  `WS_EX_TOOLWINDOW`; menus use Windows' native rounding and shadow instead of a drawn shadow.
- When an endpoint stops being active its app rows disappear at once (the .NET app kept them until each session
  disconnected).
- An Equalizer APO toggle and the endpoint restart it causes run as one job; a second click on the same device
  while that job runs is ignored.
- The volume-change ding is skipped until its WAV has loaded (the .NET app waited for it).
- The Bluetooth codec readout decodes the ETW events with the Windows TDH API (the .NET app used TraceEvent);
  its trace session is named `VolumeTrayAppDotNETSlint-BthA2dp` so it never stops the .NET app's session.
- The *Rendering backend* choices map to Slint renderers (see Build).

## Checks

```bat
cargo test --workspace
cargo clippy --workspace --all-targets
```
