//! English UI strings, copied verbatim from `VolumeTrayAppDotNET/src/Localization/Strings.resx`
//! (app keys win) and `TrayAppDotNETCommon/src/Localization/CommonStrings.resx`. Constant names
//! are the resx keys. Only English ships.

#![allow(non_upper_case_globals)]

/// CommonStrings.resx
pub(crate) const ColorPicker_ARGBHexLabel: &str = r#"argb hex:"#;
/// CommonStrings.resx
pub(crate) const ColorPicker_ChannelLabel_Alpha: &str = r#"alpha"#;
/// CommonStrings.resx
pub(crate) const ColorPicker_ChannelLabel_B: &str = r#"b"#;
/// CommonStrings.resx
pub(crate) const ColorPicker_ChannelLabel_G: &str = r#"g"#;
/// CommonStrings.resx
pub(crate) const ColorPicker_ChannelLabel_Hue: &str = r#"hue"#;
/// CommonStrings.resx
pub(crate) const ColorPicker_ChannelLabel_R: &str = r#"r"#;
/// CommonStrings.resx
pub(crate) const ColorPicker_CloseTooltip: &str = r#"Close"#;
/// CommonStrings.resx
pub(crate) const ColorPicker_DefaultButton: &str = r#"Default"#;
/// CommonStrings.resx
pub(crate) const ColorPicker_DefaultTitle: &str = r#"Color picker"#;
/// CommonStrings.resx
pub(crate) const ColorPicker_RGBAHexLabel: &str = r#"rgba hex:"#;
/// CommonStrings.resx
pub(crate) const ColorPicker_ResetButton: &str = r#"Reset"#;
/// Strings.resx
pub(crate) const Common_ContextMenu_Header: &str = r#"Context menu"#;
/// CommonStrings.resx
pub(crate) const Common_MillisecondsSuffix: &str = r#" ms"#;
/// CommonStrings.resx
pub(crate) const Common_PercentSuffix: &str = r#"%"#;
/// CommonStrings.resx
pub(crate) const ScrollBarMenu_Bottom: &str = r#"Bottom"#;
/// CommonStrings.resx
pub(crate) const ScrollBarMenu_PageDown: &str = r#"Page Down"#;
/// CommonStrings.resx
pub(crate) const ScrollBarMenu_PageUp: &str = r#"Page Up"#;
/// CommonStrings.resx
pub(crate) const ScrollBarMenu_ScrollDown: &str = r#"Scroll Down"#;
/// CommonStrings.resx
pub(crate) const ScrollBarMenu_ScrollHere: &str = r#"Scroll Here"#;
/// CommonStrings.resx
pub(crate) const ScrollBarMenu_ScrollUp: &str = r#"Scroll Up"#;
/// CommonStrings.resx
pub(crate) const ScrollBarMenu_Top: &str = r#"Top"#;
/// CommonStrings.resx
pub(crate) const SettingsWindow_ConfirmOverlay_OK: &str = r#"OK"#;
/// CommonStrings.resx
pub(crate) const SettingsWindow_Header: &str = r#"Settings"#;
/// CommonStrings.resx
pub(crate) const SettingsWindow_SearchNoMatchesFormat: &str = r#"No settings match "{0}"."#;
/// CommonStrings.resx
pub(crate) const SettingsWindow_SearchPlaceholder: &str = r#"Search settings"#;
/// CommonStrings.resx
pub(crate) const SettingsWindow_SearchResults: &str = r#"Search results"#;
/// Strings.resx
pub(crate) const SettingsWindow_SearchSynonymGroups_App: &str = r#"maximum|max|highest|largest|greatest|upper limit|upper bound
minimum|lowest|smallest|lower limit|lower bound
threshold|cutoff|trigger level
playback device|playback devices|audio output device|audio output devices|output endpoint|output endpoints|render endpoint|render endpoints
recording device|recording devices|audio input device|audio input devices|capture endpoint|capture endpoints|input endpoint|input endpoints
default device|default audio device|default endpoint|primary audio device|primary audio endpoint
communications device|comms device|communications role device|communications role endpoint
disabled device|disabled devices|deactivated device|deactivated devices
disconnected device|disconnected devices|unplugged device|unplugged devices|disconnected endpoint|disconnected endpoints
not present device|not present devices|ghost device|ghost devices|phantom device|phantom devices|stale endpoint|stale endpoints
exclusive mode|exclusive control|application exclusive audio
equalizer|EQ
listen to this device|input monitoring|microphone monitoring
communications ducking|audio ducking|communications attenuation
volume change sound|volume change beep|volume feedback sound|volume feedback tone|audio feedback tone
logarithmic volume scale|logarithmic volume curve|perceptual volume curve|audio taper
default format|default audio format|audio endpoint format|Windows mix format
Bluetooth codec|A2DP codec|Bluetooth audio codec
peak meter|audio level meter|signal peak meter
unified peak meter|combined channel meter|single bar meter
peak meter refresh rate|meter redraw rate|meter frame rate|meter FPS
peak meter sample rate|meter polling rate|peak sampling frequency
peak meter change ceiling|meter slew limit|maximum change per redraw|single frame jump limit
low channel bias|quieter channel weighting|channel weighting multiplier
flyout|volume flyout|volume control flyout|tray volume flyout
undocked flyout|detached flyout|floating flyout
opens docked|stays anchored to the tray|attached to the tray
restore undock state|restore flyout state|remember flyout state|persist flyout state
keep undocked flyout on screen|clamp flyout to screen|constrain flyout to work area
flyout title bar|flyout header
app drawer|app drawers|application drawer|application drawers|volume mixer drawer|volume mixer drawers
app slider|app sliders|application volume slider|application volume sliders|mixer slider|mixer sliders|audio fader|audio faders
intermix|interleave|mix together
Bluetooth power button|Bluetooth power control|Bluetooth radio control|Bluetooth power toggle
actively capturing apps|capture activity indicator|microphone in use indicator|recording activity indicator
volume step|volume increment|volume change amount
device name max length|device name maximum length|device label character limit|truncate device name
modified actions|tray action mapping|mouse action mapping|gesture action assignment
centered|centred|center aligned|centre aligned|middle aligned
stack direction|flow direction|layout orientation
device sort|sort order|device ordering|endpoint ordering
tray icon|system tray icon|notification area icon
context menu|right click menu|tray menu
mouse wheel|scroll wheel
touchpad|trackpad
tooltip|tool tip|hover tip
slider thumb|slider handle|slider knob"#;
/// CommonStrings.resx
pub(crate) const SettingsWindow_Title: &str = r#"Settings"#;
/// CommonStrings.resx
pub(crate) const Settings_About_AuthorLabel: &str = r#"Author"#;
/// Strings.resx
pub(crate) const Settings_About_BluetoothCodecNotDisplaying_Description: &str = r#"This is a Windows Bluetooth limitation. The `Microsoft.Windows.Bluetooth.BthA2dp` ETW provider only emits the `A2dpStreaming` event on AVDTP `SET_CONFIGURATION` or `RECONFIGURE` -- i.e. when audio actually begins streaming, not at the moment a Bluetooth endpoint connects. While the headset is connected but idle, no codec event fires and the suffix remains empty. As soon as something pushes audio to the endpoint, the event fires and the codec text populates. There is no on-demand query path and it is not possible to accurately deduce codecs from sample rates and other parameters."#;
/// Strings.resx
pub(crate) const Settings_About_BluetoothCodecNotDisplaying_Title: &str = r#"Bluetooth device codec not displaying"#;
/// CommonStrings.resx
pub(crate) const Settings_About_BuildLabel: &str = r#"Build"#;
/// CommonStrings.resx
pub(crate) const Settings_About_CommitHashLabel: &str = r#"Commit"#;
/// CommonStrings.resx
pub(crate) const Settings_About_CommitHashUnavailable: &str = r#"Unavailable"#;
/// CommonStrings.resx
pub(crate) const Settings_About_Files_Header: &str = r#"Files"#;
/// CommonStrings.resx
pub(crate) const Settings_About_GithubLabel: &str = r#"GitHub"#;
/// CommonStrings.resx
pub(crate) const Settings_About_KnownIssues_Header: &str = r#"Known issues"#;
/// CommonStrings.resx
pub(crate) const Settings_About_OpenSettingsFolder_Button: &str = r#"Open"#;
/// CommonStrings.resx
pub(crate) const Settings_About_OpenSettingsFolder_Description: &str =
    r#"Open the folder containing this app's settings and logs."#;
/// CommonStrings.resx
pub(crate) const Settings_About_OpenSettingsFolder_SearchKeywords: &str =
    r#"configuration config files directory logs location browse"#;
/// CommonStrings.resx
pub(crate) const Settings_About_OpenSettingsFolder_Title: &str = r#"Open settings folder"#;
/// CommonStrings.resx
pub(crate) const Settings_About_RuntimeLabel: &str = r#"Runtime"#;
/// CommonStrings.resx
pub(crate) const Settings_About_SectionHeader: &str = r#"About"#;
/// Strings.resx
pub(crate) const Settings_About_Tagline: &str = r#"A replacement for Windows' Volume tray app"#;
/// CommonStrings.resx
pub(crate) const Settings_Common_Page_About: &str = r#"About"#;
/// Strings.resx
pub(crate) const Settings_Common_Page_DeviceAppDrawers: &str = r#"App drawers"#;
/// Strings.resx
pub(crate) const Settings_Common_Page_Devices: &str = r#"Devices"#;
/// CommonStrings.resx
pub(crate) const Settings_Common_Page_Flyout: &str = r#"Flyout"#;
/// CommonStrings.resx
pub(crate) const Settings_Common_Page_General: &str = r#"General"#;
/// CommonStrings.resx
pub(crate) const Settings_Common_Page_Hotkeys: &str = r#"Hotkeys"#;
/// CommonStrings.resx
pub(crate) const Settings_Common_Page_Theme: &str = r#"Theme"#;
/// CommonStrings.resx
pub(crate) const Settings_Common_Page_TrayIcon: &str = r#"Tray icon"#;
/// Strings.resx
pub(crate) const Settings_Common_Playback: &str = r#"Playback"#;
/// Strings.resx
pub(crate) const Settings_Common_Recording: &str = r#"Recording"#;
/// Strings.resx
pub(crate) const Settings_DeviceAppDrawers_DefaultAppDrawerExpanded_Description: &str = r#"Initial expanded state for a device's app drawer the first time it appears. Devices you've explicitly collapsed or expanded with the chevron keep their per-device state; this only controls the default for devices without an override."#;
/// Strings.resx
pub(crate) const Settings_DeviceAppDrawers_DefaultAppDrawerExpanded_SearchKeywords: &str =
    r#"open unfold reveal mixer applications"#;
/// Strings.resx
pub(crate) const Settings_DeviceAppDrawers_DefaultAppDrawerExpanded_Title: &str = r#"Expand app drawers by default"#;
/// Strings.resx
pub(crate) const Settings_DeviceAppDrawers_SectionHeader: &str = r#"App drawers"#;
/// Strings.resx
pub(crate) const Settings_Devices_ActivateRecordingDevicesForPeakMeters_Description: &str = r#"While the flyout is visible, open a shared capture stream on each active recording device so software-only peak meters can update. Windows will treat VolumeTrayAppDotNET as using those inputs, and exclusive-mode audio applications may be affected. Off by default."#;
/// Strings.resx
pub(crate) const Settings_Devices_ActivateRecordingDevicesForPeakMeters_SearchKeywords: &str =
    r#"microphone input level activity visualization monitor"#;
/// Strings.resx
pub(crate) const Settings_Devices_ActivateRecordingDevicesForPeakMeters_Title: &str =
    r#"Activate recording devices for peak meters"#;
/// Strings.resx
pub(crate) const Settings_Devices_RowButtons_Header: &str = r#"Row buttons"#;
/// Strings.resx
pub(crate) const Settings_Devices_SectionHeader: &str = r#"Devices"#;
/// Strings.resx
pub(crate) const Settings_Devices_SetDefaultCommsToDefault_Description: &str = r#"When you set a default device through this app, also promote it to the communications-role default. Only triggered by your own clicks; never reacts to other apps' default-changes."#;
/// Strings.resx
pub(crate) const Settings_Devices_SetDefaultCommsToDefault_SearchKeywords: &str =
    r#"communications role calls headset primary endpoint"#;
/// Strings.resx
pub(crate) const Settings_Devices_SetDefaultCommsToDefault_Title: &str =
    r#"Tie default comms device to default device"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowDefaultCommsEvenIfDisabled_Description: &str =
    r#"Always include the communications-role default device, even when it's disabled in Windows."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowDefaultCommsEvenIfDisabled_SearchKeywords: &str =
    r#"calls headset communications role hidden endpoint"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowDefaultCommsEvenIfDisabled_Title: &str =
    r#"Show default comms device even if disabled"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowDefaultEvenIfDisabled_Description: &str =
    r#"Always include the default device in lists, even when it's disabled in Windows."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowDefaultEvenIfDisabled_SearchKeywords: &str =
    r#"primary speaker microphone hidden endpoint"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowDefaultEvenIfDisabled_Title: &str = r#"Show default device even if disabled"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowDisabled_Description: &str = r#"List devices that are disabled in Windows."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowDisabled_SearchKeywords: &str = r#"inactive hidden unavailable audio endpoints"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowDisabled_Title: &str = r#"Show disabled devices"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowDisconnectedPlayback_Description: &str = r#"List devices Windows reports as currently unplugged but still recognized. Registry-only ghost endpoints have their own setting above."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowDisconnectedPlayback_SearchKeywords: &str =
    r#"unplugged offline unavailable hardware endpoints"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowDisconnectedPlayback_Title: &str = r#"Show disconnected devices"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowNotPresent_Description: &str = r#"List endpoints Windows still has on file but no driver is currently loaded for - old USB DACs, removed GPUs' HDMI outputs, paired Bluetooth devices while Bluetooth is off. Most surface as "Unknown Device". Bluetooth endpoints of unpaired devices or older pairings are never listed. Off by default."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowNotPresent_SearchKeywords: &str =
    r#"phantom obsolete orphaned stale hardware endpoints"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowNotPresent_Title: &str = r#"Show registry ghost devices"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowPlaybackBatteryButton_Description: &str = r#"A battery glyph on Bluetooth rows showing the remaining charge, or a Bluetooth glyph when no charge is available. Disconnected devices keep their last known charge and the button reconnects them."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowPlaybackBatteryButton_SearchKeywords: &str =
    r#"headset earbuds charge wireless reconnect"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowPlaybackBatteryButton_Title: &str = r#"Show Bluetooth battery indicator"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowPlaybackDefaultDeviceButton_Description: &str = r#"The device icon button on the right edge of the row. Left-click sets default, right-click opens the device menu."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowPlaybackDefaultDeviceButton_SearchKeywords: &str =
    r#"switch primary speakers microphone output input"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowPlaybackDefaultDeviceButton_Title: &str = r#"Show default-device button"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowPlaybackEqualizerAPOButton_Description: &str =
    r#"The equalizer glyph that reflects Equalizer APO state and toggles install / enable on click."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowPlaybackEqualizerAPOButton_SearchKeywords: &str =
    r#"EQ effects enhancements DSP"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowPlaybackEqualizerAPOButton_Title: &str = r#"Show Equalizer APO button"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowPlaybackLockButton_Description: &str =
    r#"The lock / unlock glyph that toggles the endpoint's "Allow applications to take exclusive control" bit."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowPlaybackLockButton_SearchKeywords: &str =
    r#"exclusive control shared mode audio endpoint"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowPlaybackLockButton_Title: &str = r#"Show exclusive-mode (lock) button"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowRecordingListenButton_Description: &str = r#"Adds a button to each recording row that toggles the Windows "Listen to this device" feature. Left-click toggles, Ctrl + left-click enables with Default Playback as the target, right-click picks a specific target."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowRecordingListenButton_SearchKeywords: &str =
    r#"microphone monitoring sidetone loopback hear input"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowRecordingListenButton_Title: &str = r#"Show "Listen to this device" button"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowRecording_Description: &str =
    r#"Master switch for recording-device entries. When off, every recording-side option below is ignored."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowRecording_SearchKeywords: &str = r#"microphone input capture endpoints"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowRecording_Title: &str = r#"Show recording devices"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowTrayCommunicationsLink_Description: &str =
    r#"Add an entry that opens the classic Sound control panel on the Communications tab."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowTrayCommunicationsLink_SearchKeywords: &str =
    r#"control panel calls ducking tab"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowTrayCommunicationsLink_Title: &str = r#"Show "Communications" link in tray menu"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowTrayDeviceLinks_Description: &str = r#"Add a sub-menu entry per visible enabled device. Each entry opens the matching classic Sound tab (same as Ctrl + click on the device icon in the flyout)."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowTrayDeviceLinks_SearchKeywords: &str =
    r#"speakers microphones endpoint properties submenu"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowTrayDeviceLinks_Title: &str = r#"Show per-device links in tray menu"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowTrayRecordingLink_Description: &str =
    r#"Add an entry that opens the classic Sound control panel on the Recording tab."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowTrayRecordingLink_SearchKeywords: &str =
    r#"control panel input microphones tab context menu"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowTrayRecordingLink_Title: &str = r#"Show "Recording" link in tray menu"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowTraySoundsLink_Description: &str =
    r#"Add an entry that opens the classic Sound control panel on the Sounds tab."#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowTraySoundsLink_SearchKeywords: &str =
    r#"control panel sound scheme alerts events tab"#;
/// Strings.resx
pub(crate) const Settings_Devices_ShowTraySoundsLink_Title: &str = r#"Show "Sounds" link in tray menu"#;
/// Strings.resx
pub(crate) const Settings_Devices_VisibilityColumn_Header: &str = r#"Visibility"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconScale_Description: &str = r#"Scales the app icons in the grid drawer. 100 matches the slider-drawer baseline; values above 100 enlarge the icons while keeping eight per row."#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconScale_SearchKeywords: &str = r#"size zoom dimensions artwork"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconScale_Title: &str = r#"Icon scale (percent)"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsCenterMode_Centered: &str = r#"Centered"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsCenterMode_Off: &str = r#"Off"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsCenterMode_SoftMax: &str = r#"Centered soft-max"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsCenterSoftMax_Description: &str = r#"Anchors the growing row at the position a centered row of this many icons would occupy. Past this count the layout switches to fully centered."#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsCenterSoftMax_SearchKeywords: &str = r#"alignment anchor target width"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsCenterSoftMax_Title: &str = r#"Soft-max width (icons)"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsCentered_Description: &str = r#"How the trailing partial row anchors. Off keeps it left-aligned; Centered shifts it to the middle; Centered soft-max left-anchors at the position a centered soft-max-icon row would occupy until the row exceeds soft-max, then crosses over to fully centered."#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsCentered_SearchKeywords: &str =
    r#"alignment justify partial trailing row"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsCentered_Title: &str = r#"Icon grid centering"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsPerColumn_Description: &str = r#"Caps how many icons fit in a column in the grid drawer. Once a column fills, the next icon starts a new column alongside it."#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsPerColumn_SearchKeywords: &str = r#"rows height vertical grid capacity"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsPerColumn_Title: &str = r#"Max apps per column"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsPerRow_Description: &str = r#"Caps how many icons fit on a row in the grid drawer. Values above 8 shrink each cell to keep the grid inside the drawer; values below 8 leave the cells at their default size and the grid reads narrower."#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsPerRow_SearchKeywords: &str =
    r#"columns width horizontal grid capacity"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerIconsPerRow_Title: &str = r#"Max apps per row"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerMaxApps_Icons_Description: &str =
    r#"Caps how many icon rows render before the drawer enters scrolling overflow."#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerMaxApps_Icons_SearchKeywords: &str = r#"grid height overflow limit"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerMaxApps_Icons_Title: &str = r#"Max rows before scrolling - icon grid"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerMaxApps_Sliders_Description: &str =
    r#"Caps how many app slider rows render before the drawer enters scrolling overflow."#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerMaxApps_Sliders_SearchKeywords: &str = r#"mixer list height overflow limit"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerMaxApps_Sliders_Title: &str = r#"Max apps before scrolling - full sliders"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerStackDirection_Auto: &str = r#"Auto"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerStackDirection_BottomTop: &str = r#"Bottom to top"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerStackDirection_Description: &str = r#"How the app icons stack inside the grid drawer. Auto picks the direction that puts the first app closest to the device row based on whether apps sit above or below it."#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerStackDirection_LeftRight: &str = r#"Left to right"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerStackDirection_RightLeft: &str = r#"Right to left"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerStackDirection_SearchKeywords: &str = r#"flow order orientation grid"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerStackDirection_Title: &str = r#"Icon stack direction"#;
/// Strings.resx
pub(crate) const Settings_Flyout_AppDrawerStackDirection_TopBottom: &str = r#"Top to bottom"#;
/// Strings.resx
pub(crate) const Settings_Flyout_BluetoothRadioButtonClickGesture_AltLeftClick: &str = r#"Alt + left-click"#;
/// Strings.resx
pub(crate) const Settings_Flyout_BluetoothRadioButtonClickGesture_ControlLeftClick: &str = r#"Ctrl + left-click"#;
/// Strings.resx
pub(crate) const Settings_Flyout_BluetoothRadioButtonClickGesture_Description: &str = r#"Choose the left-click gesture that turns the Windows Bluetooth radio on or off. Any other left-click opens Windows Bluetooth quick settings."#;
/// Strings.resx
pub(crate) const Settings_Flyout_BluetoothRadioButtonClickGesture_LeftClick: &str = r#"Left-click"#;
/// Strings.resx
pub(crate) const Settings_Flyout_BluetoothRadioButtonClickGesture_SearchKeywords: &str =
    r#"modifier shortcut mouse action power toggle quick settings menu"#;
/// Strings.resx
pub(crate) const Settings_Flyout_BluetoothRadioButtonClickGesture_ShiftLeftClick: &str = r#"Shift + left-click"#;
/// Strings.resx
pub(crate) const Settings_Flyout_BluetoothRadioButtonClickGesture_Title: &str =
    r#"Bluetooth power button click action"#;
/// CommonStrings.resx
pub(crate) const Settings_Flyout_ClampUndockedToScreen_Description: &str =
    r#"Keep the undocked flyout fully inside one monitor's work area when it restores or repositions."#;
/// Strings.resx
pub(crate) const Settings_Flyout_ClampUndockedToScreen_SearchKeywords: &str =
    r#"monitor bounds work area keep visible floating window"#;
/// CommonStrings.resx
pub(crate) const Settings_Flyout_ClampUndockedToScreen_Title: &str = r#"Keep undocked flyout on screen"#;
/// Strings.resx
pub(crate) const Settings_Flyout_CollapseDisconnectedBluetoothDevices_Description: &str = r#"Shows one row per disconnected Bluetooth device instead of one row for each of its playback and recording endpoints, as Windows Settings lists one entry per device. The row takes the slot of the device's highest-priority playback endpoint, or of its recording endpoint when no playback endpoint is listed. A device with any active endpoint keeps all of its rows."#;
/// Strings.resx
pub(crate) const Settings_Flyout_CollapseDisconnectedBluetoothDevices_SearchKeywords: &str =
    r#"headset earbuds wireless combine merge group single row hands-free stereo endpoints"#;
/// Strings.resx
pub(crate) const Settings_Flyout_CollapseDisconnectedBluetoothDevices_Title: &str =
    r#"Collapse disconnected Bluetooth devices"#;
/// Strings.resx
pub(crate) const Settings_Flyout_CommunicationsButtonVisibility_AlwaysShow: &str = r#"Always show"#;
/// Strings.resx
pub(crate) const Settings_Flyout_CommunicationsButtonVisibility_Description: &str = r#"Controls when the communications ducking button appears in the flyout title-bar. Ducking is the classic "Mute all other sounds by 80%" etc. setting. "When ducking is on" hides it whenever Windows is set to "Do nothing" on the Communications tab."#;
/// Strings.resx
pub(crate) const Settings_Flyout_CommunicationsButtonVisibility_Hidden: &str = r#"Hidden"#;
/// Strings.resx
pub(crate) const Settings_Flyout_CommunicationsButtonVisibility_SearchKeywords: &str =
    r#"phone calls attenuation lower other audio"#;
/// Strings.resx
pub(crate) const Settings_Flyout_CommunicationsButtonVisibility_Title: &str = r#"Communications ducking button"#;
/// Strings.resx
pub(crate) const Settings_Flyout_CommunicationsButtonVisibility_WhenDuckingOn: &str = r#"When ducking is on"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceLayout_AppsAbove: &str = r#"Apps above device"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceLayout_AppsBelow: &str = r#"Apps below device"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceLayout_Description: &str =
    r#"Where each device's row sits relative to its app sliders."#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceLayout_SearchKeywords: &str = r#"mixer applications position order"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceLayout_Title: &str = r#"Device layout"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceSort_Description: &str = r#"Default sort: default, default communications, enabled, disabled, disconnected. The default device sits at the bottom of the flyout. Windows sort: untouched MMDevice enumeration order."#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceSort_SearchKeywords: &str = r#"ordering arrange endpoint priority"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceSort_StateGrouped: &str = r#"Default"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceSort_Title: &str = r#"Device sort"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceSort_WindowsEnumeration: &str = r#"Windows order"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceTitlePosition_AboveSlider: &str = r#"Above slider"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceTitlePosition_BelowSlider: &str = r#"Below slider"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceTitlePosition_Description: &str =
    r#"Where the device name and per-device action buttons sit relative to the device slider."#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceTitlePosition_SearchKeywords: &str =
    r#"device name controls buttons above below"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DeviceTitlePosition_Title: &str = r#"Device title row position"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DisconnectedBluetoothDevices_AlwaysShow: &str = r#"Always show"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DisconnectedBluetoothDevices_AlwaysShowIntermixed: &str =
    r#"Always show (intermixed)"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DisconnectedBluetoothDevices_Description: &str = r#"Applies to paired Bluetooth devices that are not connected. Show follows the disconnected and registry-ghost switches on the Devices page. Always show overrides those switches and puts disconnected Bluetooth endpoints after both the normal playback and recording devices in the configured sort direction. The intermixed option instead sorts each endpoint with its playback or recording devices. Endpoints left behind by unpaired devices or older pairings are never listed."#;
/// Strings.resx
pub(crate) const Settings_Flyout_DisconnectedBluetoothDevices_NeverShow: &str = r#"Never show"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DisconnectedBluetoothDevices_SearchKeywords: &str =
    r#"headset earbuds wireless offline unavailable"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DisconnectedBluetoothDevices_Show: &str = r#"Show"#;
/// Strings.resx
pub(crate) const Settings_Flyout_DisconnectedBluetoothDevices_Title: &str = r#"Disconnected Bluetooth audio devices"#;
/// Strings.resx
pub(crate) const Settings_Flyout_HeaderAtBottom_Description: &str = r#"Render the flyout's title bar (Settings cluster and Undock button) at the bottom of the flyout instead of the top."#;
/// Strings.resx
pub(crate) const Settings_Flyout_HeaderAtBottom_SearchKeywords: &str = r#"toolbar titlebar controls position"#;
/// Strings.resx
pub(crate) const Settings_Flyout_HeaderAtBottom_Title: &str = r#"Title bar at the bottom"#;
/// Strings.resx
pub(crate) const Settings_Flyout_IntermixRecording_Description: &str = r#"When on, recording devices interleave with playback devices inside each sort bucket. When off, all playback devices show first and recording devices group together at the top of the list."#;
/// Strings.resx
pub(crate) const Settings_Flyout_IntermixRecording_SearchKeywords: &str = r#"microphone speaker ordering grouping"#;
/// Strings.resx
pub(crate) const Settings_Flyout_IntermixRecording_Title: &str = r#"Intermix recording with playback"#;
/// CommonStrings.resx
pub(crate) const Settings_Flyout_Layout_Header: &str = r#"Layout"#;
/// Strings.resx
pub(crate) const Settings_Flyout_PlaybackDrawer_Header: &str = r#"Playback drawers"#;
/// Strings.resx
pub(crate) const Settings_Flyout_RecordingAppDrawerDisplayType_Description: &str = r#"Displaying applications with volume mixer sliders for recording devices is pointless, as recording devices do not go through a volume mixing layer. But this is still here if you want to make the flyout look consistent."#;
/// Strings.resx
pub(crate) const Settings_Flyout_RecordingAppDrawerDisplayType_Icons: &str = r#"Icon grid"#;
/// Strings.resx
pub(crate) const Settings_Flyout_RecordingAppDrawerDisplayType_SearchKeywords: &str =
    r#"microphone applications mixer grid presentation"#;
/// Strings.resx
pub(crate) const Settings_Flyout_RecordingAppDrawerDisplayType_Sliders: &str = r#"Full sliders"#;
/// Strings.resx
pub(crate) const Settings_Flyout_RecordingAppDrawerDisplayType_Title: &str = r#"App drawer display type"#;
/// Strings.resx
pub(crate) const Settings_Flyout_RecordingDrawer_Header: &str = r#"Recording drawers"#;
/// CommonStrings.resx
pub(crate) const Settings_Flyout_RestoreUndockState_Description: &str = r#"When the app launches, restore the flyout's docked or undocked state from the previous session. When off, the flyout always opens docked."#;
/// Strings.resx
pub(crate) const Settings_Flyout_RestoreUndockState_SearchKeywords: &str =
    r#"remember popup panel position launch boot login"#;
/// CommonStrings.resx
pub(crate) const Settings_Flyout_RestoreUndockState_Title: &str = r#"Restore undock state on startup"#;
/// CommonStrings.resx
pub(crate) const Settings_Flyout_SectionHeader: &str = r#"Flyout"#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowBluetoothDevicesOnlyWhenOn_Description: &str = r#"Hides all Bluetooth playback and recording devices from the flyout while the Windows Bluetooth radio is off. This overrides the disconnected Bluetooth visibility mode above."#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowBluetoothDevicesOnlyWhenOn_SearchKeywords: &str =
    r#"wireless radio powered off hide endpoints"#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowBluetoothDevicesOnlyWhenOn_Title: &str =
    r#"Only show Bluetooth devices while Bluetooth is on"#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowBluetoothRadioButton_Description: &str = r#"Show the Bluetooth button in the flyout header for opening Windows Bluetooth quick settings and turning the Windows Bluetooth radio on or off."#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowBluetoothRadioButton_SearchKeywords: &str =
    r#"wireless radio toggle switch power quick settings menu"#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowBluetoothRadioButton_Title: &str = r#"Show Bluetooth power button"#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowDeviceCodecText_Description: &str = r#"Suffix the format strip with the live A2DP codec (SBC, AAC, aptX, LDAC, etc.) on Bluetooth playback devices. Independent of the format readout toggle - with that off, the codec name renders alone for Bluetooth devices. Requires running the app elevated; otherwise the codec stays unknown. Off by default."#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowDeviceCodecText_SearchKeywords: &str = r#"A2DP SBC AAC aptX LDAC wireless format"#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowDeviceCodecText_Title: &str = r#"Show Bluetooth codec on the format strip"#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowDeviceFormatText_Description: &str = r#"The compact format strip (sample rate, bit depth, channels) drawn under each device name. Right-clicking the strip opens the format picker. Off by default."#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowDeviceFormatText_SearchKeywords: &str =
    r#"sample rate bit depth channels audio quality"#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowDeviceFormatText_Title: &str = r#"Show default-format readout"#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowRecordingDevices_Description: &str = r#"Include capture endpoints (microphones, line-in) in the flyout's device list. Still gated by the global recording-devices toggle on the Devices page."#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowRecordingDevices_SearchKeywords: &str = r#"microphone input capture endpoints"#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowRecordingDevices_Title: &str = r#"Show recording devices"#;
/// CommonStrings.resx
pub(crate) const Settings_Flyout_ShowUndockButton_Description: &str =
    r#"Show the undock button in the flyout. When off, the flyout always stays anchored to the tray."#;
/// Strings.resx
pub(crate) const Settings_Flyout_ShowUndockButton_SearchKeywords: &str = r#"detach floating window popup panel unpin"#;
/// CommonStrings.resx
pub(crate) const Settings_Flyout_ShowUndockButton_Title: &str = r#"Show undock button"#;
/// Strings.resx
pub(crate) const Settings_Flyout_SoundSettingsTarget_Description: &str =
    r#"Which Windows surface the flyout's sound-settings titlebar button opens."#;
/// Strings.resx
pub(crate) const Settings_Flyout_SoundSettingsTarget_Legacy: &str = r#"Classic Sound control panel"#;
/// Strings.resx
pub(crate) const Settings_Flyout_SoundSettingsTarget_Modern: &str = r#"Windows Settings app"#;
/// Strings.resx
pub(crate) const Settings_Flyout_SoundSettingsTarget_SearchKeywords: &str =
    r#"control panel mmsys CPL modern Windows preferences"#;
/// Strings.resx
pub(crate) const Settings_Flyout_SoundSettingsTarget_Title: &str = r#"Sound settings button target"#;
/// Strings.resx
pub(crate) const Settings_Flyout_UseDynamicPlaybackVolumeGlyph_Description: &str = r#"When on, playback-device buttons in the flyout use the current volume tier glyph. When off, unmuted playback devices use the static low-volume glyph."#;
/// Strings.resx
pub(crate) const Settings_Flyout_UseDynamicPlaybackVolumeGlyph_SearchKeywords: &str =
    r#"speaker icon level loudness mute"#;
/// Strings.resx
pub(crate) const Settings_Flyout_UseDynamicPlaybackVolumeGlyph_Title: &str =
    r#"Use dynamic playback volume glyph for playback devices in flyout"#;
/// CommonStrings.resx
pub(crate) const Settings_Flyout_Visibility_Header: &str = r#"Visibility"#;
/// Strings.resx
pub(crate) const Settings_General_CaptureActivityIndicator_ActiveGlyph: &str = r#"Active glyph"#;
/// Strings.resx
pub(crate) const Settings_General_CaptureActivityIndicator_Description: &str =
    r#"How the flyout marks apps that currently have the microphone open inside a recording device's drawer."#;
/// Strings.resx
pub(crate) const Settings_General_CaptureActivityIndicator_DimInactive: &str = r#"Dim inactive"#;
/// Strings.resx
pub(crate) const Settings_General_CaptureActivityIndicator_HideInactive: &str = r#"Hide inactive"#;
/// Strings.resx
pub(crate) const Settings_General_CaptureActivityIndicator_None: &str = r#"Don't indicate"#;
/// Strings.resx
pub(crate) const Settings_General_CaptureActivityIndicator_SearchKeywords: &str =
    r#"microphone privacy in use live recording"#;
/// Strings.resx
pub(crate) const Settings_General_CaptureActivityIndicator_Title: &str = r#"Visually indicate actively capturing apps"#;
/// CommonStrings.resx
pub(crate) const Settings_General_ContextMenu_Header: &str = r#"Context menu"#;
/// Strings.resx
pub(crate) const Settings_General_DingSuppressionPeakThreshold_Description: &str = r#"Only suppress the beep when the recent rolling peak is above this percentage, scaled by the volume being set. Lower values are stricter; older peaks continuously lose influence through exponential falloff."#;
/// Strings.resx
pub(crate) const Settings_General_DingSuppressionPeakThreshold_SearchKeywords: &str =
    r#"beep sensitivity cutoff level"#;
/// Strings.resx
pub(crate) const Settings_General_DingSuppressionPeakThreshold_Title: &str = r#"Peak meter threshold (%)"#;
/// Strings.resx
pub(crate) const Settings_General_IconRetryInterval_Description: &str = r#"Wait between attempts when an app icon fails to resolve. Four attempts run with linear backoff (1x, 2x, 3x this value), so the worst case with the default is roughly 1.5 seconds before the fallback glyph stays."#;
/// Strings.resx
pub(crate) const Settings_General_IconRetryInterval_SearchKeywords: &str =
    r#"missing blank application artwork fallback"#;
/// Strings.resx
pub(crate) const Settings_General_IconRetryInterval_Title: &str = r#"Icon retry interval (ms)"#;
/// CommonStrings.resx
pub(crate) const Settings_General_KeepFlyoutWarm_Description: &str = r#"Keep the flyout created in the background so it opens faster. When off, hidden UI resources are released after a short idle delay."#;
/// CommonStrings.resx
pub(crate) const Settings_General_KeepFlyoutWarm_Title: &str = r#"Keep flyout warm"#;
/// CommonStrings.resx
pub(crate) const Settings_General_KeepTrayContextMenuWarm_Description: &str = r#"Keep the tray context menu created in the background so it opens faster. When off, hidden UI resources are released after a short idle delay."#;
/// CommonStrings.resx
pub(crate) const Settings_General_KeepTrayContextMenuWarm_Title: &str = r#"Keep tray context menu warm"#;
/// CommonStrings.resx
pub(crate) const Settings_General_KeepWarm_SearchKeywords: &str =
    r#"preload cache instant fast performance background"#;
/// CommonStrings.resx
pub(crate) const Settings_General_LanguageRestart_Message: &str =
    r#"The new language applies after the app restarts. Restart now?"#;
/// CommonStrings.resx
pub(crate) const Settings_General_LanguageRestart_Title: &str = r#"Restart to change the language"#;
/// CommonStrings.resx
pub(crate) const Settings_General_Language_Auto: &str = r#"Auto"#;
/// CommonStrings.resx
pub(crate) const Settings_General_Language_Description: &str = r#"Auto uses the Windows display language when a translation for it is available. A change applies after a restart."#;
/// CommonStrings.resx
pub(crate) const Settings_General_Language_SearchKeywords: &str =
    r#"locale translation culture display language region localization"#;
/// CommonStrings.resx
pub(crate) const Settings_General_Language_Title: &str = r#"Language"#;
/// Strings.resx
pub(crate) const Settings_General_LogarithmicVolumeScale_Description: &str = r#"Map app volume sliders to a perceptual curve so equal slider steps feel like equal loudness steps. Device sliders already follow the Windows volume curve and stay as they are."#;
/// Strings.resx
pub(crate) const Settings_General_LogarithmicVolumeScale_SearchKeywords: &str =
    r#"audio taper natural slider curve app session decibel dB perceptual loudness"#;
/// Strings.resx
pub(crate) const Settings_General_LogarithmicVolumeScale_Title: &str = r#"Use logarithmic volume scale"#;
/// CommonStrings.resx
pub(crate) const Settings_General_NotNow_Button: &str = r#"Not now"#;
/// Strings.resx
pub(crate) const Settings_General_Notifications_Header: &str = r#"Volume changed ping"#;
/// Strings.resx
pub(crate) const Settings_General_Other_Header: &str = r#"Other"#;
/// Strings.resx
pub(crate) const Settings_General_PeakMeter_Header: &str = r#"Peak meter"#;
/// CommonStrings.resx
pub(crate) const Settings_General_Performance_Header: &str = r#"Performance"#;
/// Strings.resx
pub(crate) const Settings_General_PlayAppVolumeChangeSound_Description: &str = r#"Play a Windows notification sound when you change a per-app slider, scaled to the app's mixed volume so quieter apps preview quieter."#;
/// Strings.resx
pub(crate) const Settings_General_PlayAppVolumeChangeSound_SearchKeywords: &str =
    r#"mixer application audio feedback preview chime"#;
/// Strings.resx
pub(crate) const Settings_General_PlayAppVolumeChangeSound_Title: &str = r#"Play a sound when changing app volume"#;
/// Strings.resx
pub(crate) const Settings_General_PlayDeviceVolumeChangeSound_Description: &str = r#"Play a beep through the playback device whose volume you adjust. Recording devices stay silent, and per-app slider sounds use their own setting."#;
/// Strings.resx
pub(crate) const Settings_General_PlayDeviceVolumeChangeSound_SearchKeywords: &str =
    r#"audio feedback speaker ding chime"#;
/// Strings.resx
pub(crate) const Settings_General_PlayDeviceVolumeChangeSound_Title: &str =
    r#"Play a sound when changing playback device volume"#;
/// Strings.resx
pub(crate) const Settings_General_PlayTrayScrollVolumeChangeSound_Description: &str =
    r#"Play the volume-change beep after scrolling the tray icon."#;
/// Strings.resx
pub(crate) const Settings_General_PlayTrayScrollVolumeChangeSound_SearchKeywords: &str =
    r#"wheel audio feedback chime"#;
/// Strings.resx
pub(crate) const Settings_General_PlayTrayScrollVolumeChangeSound_Title: &str =
    r#"Play a sound when scrolling the tray icon"#;
/// CommonStrings.resx (reworded: this build renders with Slint's Skia renderer, not Avalonia)
pub(crate) const Settings_General_RenderingBackend_Description: &str = r#""GPU preferred" renders with Skia on the GPU and falls back to software rendering when the GPU path cannot start. "Software" forces CPU rendering. GPU rendering is faster but has more RAM overhead. Restart required."#;
/// CommonStrings.resx
pub(crate) const Settings_General_RenderingBackend_GPUPreferred: &str = r#"GPU preferred"#;
/// CommonStrings.resx
pub(crate) const Settings_General_RenderingBackend_SearchKeywords: &str =
    r#"graphics GPU CPU software hardware acceleration renderer"#;
/// CommonStrings.resx
pub(crate) const Settings_General_RenderingBackend_Software: &str = r#"Software"#;
/// CommonStrings.resx
pub(crate) const Settings_General_RenderingBackend_Title: &str = r#"Rendering backend"#;
/// CommonStrings.resx
pub(crate) const Settings_General_RenderingRestart_Button: &str = r#"Restart"#;
/// CommonStrings.resx
pub(crate) const Settings_General_RenderingRestart_Message: &str =
    r#"Restart the app now to apply the selected rendering backend?"#;
/// CommonStrings.resx
pub(crate) const Settings_General_RenderingRestart_Title: &str = r#"Restart required"#;
/// CommonStrings.resx
pub(crate) const Settings_General_Rendering_Header: &str = r#"Rendering"#;
/// CommonStrings.resx
pub(crate) const Settings_General_RunOnStartup_Description: &str = r#"Start the app automatically when you sign in."#;
/// CommonStrings.resx
pub(crate) const Settings_General_RunOnStartup_OnHeaderLine: &str = r#"Startup shortcut target:"#;
/// CommonStrings.resx
pub(crate) const Settings_General_RunOnStartup_SearchKeywords: &str =
    r#"launch login logon boot autostart start automatically start with Windows"#;
/// CommonStrings.resx
pub(crate) const Settings_General_RunOnStartup_Title: &str = r#"Run on startup"#;
/// CommonStrings.resx
pub(crate) const Settings_General_SectionHeader: &str = r#"General"#;
/// Strings.resx
pub(crate) const Settings_General_SuppressDeviceVolumeChangeSoundWhenAudioPlaying_Description: &str = r#"Don't play the volume-change beep when the recent rolling peak is above the volume-adjusted threshold, so the feedback won't step on music, calls, or games."#;
/// Strings.resx
pub(crate) const Settings_General_SuppressDeviceVolumeChangeSoundWhenAudioPlaying_SearchKeywords: &str =
    r#"silence mute feedback while listening"#;
/// Strings.resx
pub(crate) const Settings_General_SuppressDeviceVolumeChangeSoundWhenAudioPlaying_Title: &str =
    r#"Skip the sound when audio is already playing"#;
/// Strings.resx
pub(crate) const Settings_General_WheelVolumeStepCoarsePercent_Description: &str =
    r#"Coarse amount to raise or lower volume when a tray-icon scroll uses the Coarse action."#;
/// Strings.resx
pub(crate) const Settings_General_WheelVolumeStepCoarsePercent_SearchKeywords: &str =
    r#"large fast scroll adjustment increment"#;
/// Strings.resx
pub(crate) const Settings_General_WheelVolumeStepCoarsePercent_Title: &str = r#"Wheel volume step (%) - coarse"#;
/// Strings.resx
pub(crate) const Settings_General_WheelVolumeStepFinePercent_Description: &str =
    r#"Fine amount to raise or lower volume when a tray-icon scroll uses the Fine action."#;
/// Strings.resx
pub(crate) const Settings_General_WheelVolumeStepFinePercent_SearchKeywords: &str =
    r#"precise small scroll adjustment increment"#;
/// Strings.resx
pub(crate) const Settings_General_WheelVolumeStepFinePercent_Title: &str = r#"Wheel volume step (%) - fine"#;
/// Strings.resx
pub(crate) const Settings_General_WheelVolumeStepPercent_Description: &str =
    r#"Default amount to raise or lower volume for each mouse-wheel notch over the tray icon or flyout sliders."#;
/// Strings.resx
pub(crate) const Settings_General_WheelVolumeStepPercent_SearchKeywords: &str =
    r#"scroll sensitivity increment amount"#;
/// Strings.resx
pub(crate) const Settings_General_WheelVolumeStepPercent_Title: &str = r#"Wheel volume step (%)"#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_Add_Button: &str = r#"Add"#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_ChordInUse_Format: &str = r#"{0} is already used by {1}."#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_DeleteHotkey_ToolTip: &str = r#"Delete this hotkey"#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_Error_DuplicateInApp_Format: &str =
    r#"Also bound to {0} in this app, which keeps it."#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_Error_ReservedKey: &str = r#"F12 is reserved by the debugger."#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_Exists_Button: &str = r#"Exists"#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_KeyNotSupported: &str = r#"This key cannot be used in a hotkey."#;
/// Strings.resx
pub(crate) const Settings_Hotkeys_OpenFlyout_Description: &str = r#"Show the volume flyout near the tray icon."#;
/// Strings.resx
pub(crate) const Settings_Hotkeys_OpenFlyout_SearchKeywords: &str =
    r#"popup panel volume mixer global shortcut accelerator"#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_OpenFlyout_Title: &str = r#"Open flyout"#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_OpenSettings_Description: &str = r#"Open this settings window."#;
/// Strings.resx
pub(crate) const Settings_Hotkeys_OpenSettings_SearchKeywords: &str =
    r#"preferences configuration options global shortcut accelerator"#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_OpenSettings_Title: &str = r#"Open settings"#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_SectionDescription: &str = r#"Bind a system-wide keyboard shortcut to each action. Bindings require at least one modifier key (Ctrl, Alt, Shift, or Win). All actions start unbound."#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_SectionHeader: &str = r#"Hotkeys"#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_Status_HotkeyServiceUnavailable: &str = r#"Hotkey service not available."#;
/// CommonStrings.resx
pub(crate) const Settings_Hotkeys_Status_Registered: &str = r#"Registered."#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_Animations_Description: &str =
    r#"Controls whether tooltip fades and other UI animations are allowed."#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_Animations_Disabled: &str = r#"Disabled"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_Animations_Enabled: &str = r#"Enabled"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_Animations_SearchKeywords: &str =
    r#"motion transitions fade visual effects accessibility reduce motion"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_Animations_System: &str = r#"System"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_Animations_Title: &str = r#"Animations"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_Appearance_Header: &str = r#"Appearance"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_BackgroundColor_DarkTooltip: &str = r#"Dark theme background color"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_BackgroundColor_Description: &str =
    r#"Override the default background color. Each variant falls back to the default when unset."#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_BackgroundColor_LightTooltip: &str = r#"Light theme background color"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_BackgroundColor_SearchKeywords: &str = r#"backdrop fill surface canvas window color"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_BackgroundColor_Title: &str = r#"Background color"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_Flyout_Header: &str = r#"Flyout"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_FontSize_Description: &str =
    r#"Controls the overall size of the tray context menu. All elements scale from font size."#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_FontSize_SearchKeywords: &str = r#"text scale typography zoom accessibility menu"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_FontSize_Title: &str = r#"Font size"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakChangeCeiling_Description: &str = r#"Maximum amount, in volume units (0 to 100), that the peak meter can move in a single redraw. Caps single-frame jumps so a sudden silence-to-loud transition ramps over a few frames. 0 freezes the meter; 100 disables the cap."#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakChangeCeiling_SearchKeywords: &str =
    r#"attack speed jump limit smoothing responsiveness"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakChangeCeiling_Title: &str = r#"Peak meter change ceiling"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakColor_Description: &str =
    r#"Color of the peak meter overlay drawn on top of the volume slider track."#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakColor_SearchKeywords: &str = r#"level indicator overlay tint VU"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakColor_Title: &str = r#"Peak meter color"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakColor_Tooltip: &str = r#"Edit peak meter color"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakFps_Description: &str = r#"How often the meter is redrawn. Higher than the sample rate buys smoothness via linear interpolation between samples; equal to the sample rate snaps with no interpolation."#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakFps_SearchKeywords: &str =
    r#"animation smoothness redraw frequency performance"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakFps_Title: &str = r#"Peak meter refresh rate (FPS)"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakSampleRate_Description: &str = r#"How often the raw audio peak is read from Windows. Lower than the refresh rate to enable smoothing; higher than the refresh rate wastes CPU."#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakSampleRate_SearchKeywords: &str =
    r#"polling capture frequency CPU performance"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakSampleRate_Title: &str = r#"Peak meter sample rate"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakStereoColor_Description: &str = r#"Color of the stereo overlay drawn on top of the base bar to the loudest channel. Translucent by default so the channel mismatch reads as a halo extending past the base."#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakStereoColor_SearchKeywords: &str =
    r#"left right channel difference halo tint"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakStereoColor_Title: &str = r#"Peak meter stereo color"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeakStereoColor_Tooltip: &str = r#"Edit peak meter stereo color"#;
/// Strings.resx
pub(crate) const Settings_Theme_MeterPeak_Header: &str = r#"Peak meter"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_PickerTitle_DarkVariant: &str = r#"dark"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_PickerTitle_Format: &str = r#"{0} ({1})"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_PickerTitle_LightVariant: &str = r#"light"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_Reset: &str = r#"Reset"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_RoundedCorners_Description: &str =
    r#"Applies rounded corners to windows, settings cards, buttons, and menus. Turn off for a squared-off look."#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_RoundedCorners_SearchKeywords: &str =
    r#"square sharp edges rectangular radius window shape"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_RoundedCorners_Title: &str = r#"Rounded corners"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_SectionHeader: &str = r#"Theme"#;
/// Strings.resx
pub(crate) const Settings_Theme_SliderIndicator_Description: &str =
    r#"The shape drawn as the volume-slider thumb. Each option keeps its own font and proportions."#;
/// Strings.resx
pub(crate) const Settings_Theme_SliderIndicator_SearchKeywords: &str = r#"thumb handle knob shape glyph"#;
/// Strings.resx
pub(crate) const Settings_Theme_SliderIndicator_Title: &str = r#"Slider indicator"#;
/// Strings.resx
pub(crate) const Settings_Theme_SliderThumb_Capsule: &str = r#"Capsule"#;
/// Strings.resx
pub(crate) const Settings_Theme_SliderThumb_Circle: &str = r#"Circle"#;
/// Strings.resx
pub(crate) const Settings_Theme_SliderThumb_Diamond: &str = r#"Diamond"#;
/// Strings.resx
pub(crate) const Settings_Theme_SliderThumb_Heart: &str = r#"Heart"#;
/// Strings.resx
pub(crate) const Settings_Theme_SliderThumb_Square: &str = r#"Square"#;
/// Strings.resx
pub(crate) const Settings_Theme_SliderThumb_Star: &str = r#"Star"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_StaticIconColor_DarkTooltip: &str = r#"Dark theme tray icon color"#;
/// Strings.resx
pub(crate) const Settings_Theme_StaticIconColor_Description: &str = r#"Override the tray icon color when Tray icon style is set to Static. Each variant falls back to the default when unset."#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_StaticIconColor_LightTooltip: &str = r#"Light theme tray icon color"#;
/// Strings.resx
pub(crate) const Settings_Theme_StaticIconColor_SearchKeywords: &str = r#"notification area taskbar glyph tint"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_StaticIconColor_Title: &str = r#"Tray icon color"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_TextColor_DarkTooltip: &str = r#"Dark theme text color"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_TextColor_Description: &str =
    r#"Override the default text color. Each variant falls back to the default when unset."#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_TextColor_LightTooltip: &str = r#"Light theme text color"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_TextColor_SearchKeywords: &str =
    r#"foreground font lettering contrast typography tint"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_TextColor_Title: &str = r#"Text color"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_ThemeStyle_Dark: &str = r#"Dark"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_ThemeStyle_Description: &str =
    r#"Follow Windows, or force light / dark mode for this app."#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_ThemeStyle_Light: &str = r#"Light"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_ThemeStyle_SearchKeywords: &str =
    r#"appearance color scheme Windows preference night day"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_ThemeStyle_System: &str = r#"System"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_ThemeStyle_Title: &str = r#"Theme style"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_ToolTipShowDelay_Description: &str =
    r#"Milliseconds to wait before showing a tooltip."#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_ToolTipShowDelay_SearchKeywords: &str = r#"hover help popup wait latency timing"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_ToolTipShowDelay_Title: &str = r#"Tooltip delay"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_TrayIcon_Header: &str = r#"Tray icon"#;
/// Strings.resx
pub(crate) const Settings_Theme_UnifiedMeterBias_Description: &str = r#"How strongly the quieter channel weights the combined value. 0 keeps the louder channel; 1 averages the channels; higher values dampen stereo flutter by biasing the result toward the quieter channel."#;
/// Strings.resx
pub(crate) const Settings_Theme_UnifiedMeterBias_SearchKeywords: &str = r#"channel weighting balance stereo smoothing"#;
/// Strings.resx
pub(crate) const Settings_Theme_UnifiedMeterBias_Title: &str = r#"Unified meter low channel bias multiplier"#;
/// Strings.resx
pub(crate) const Settings_Theme_UnifiedPeakMeter_Description: &str = r#"Combine the left and right channels into a single weighted bar instead of drawing the stereo overlay on top of the base meter."#;
/// Strings.resx
pub(crate) const Settings_Theme_UnifiedPeakMeter_SearchKeywords: &str =
    r#"mono combine stereo channels level visualization"#;
/// Strings.resx
pub(crate) const Settings_Theme_UnifiedPeakMeter_Title: &str = r#"Unified peak meter"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_Windows11Navigation_Description: &str =
    r#"Show page glyphs and use a Windows 11-style layout for the settings menu."#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_Windows11Navigation_SearchKeywords: &str =
    r#"settings navigation sidebar menu Windows 11 icons glyphs selection indicator"#;
/// CommonStrings.resx
pub(crate) const Settings_Theme_Windows11Navigation_Title: &str = r#"Windows 11-style settings menu"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_AltDoubleLeftClick_Title: &str = r#"Alt + double-click"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_AltLeftClick_Title: &str = r#"Alt + left-click"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_AltMouseWheel_Title: &str = r#"Alt + mouse wheel"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_AltRightClick_Title: &str = r#"Alt + right-click"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_ClickAction_Nothing: &str = r#"Nothing"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_ClickAction_OpenSettings: &str = r#"Open settings"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_CtrlDoubleLeftClick_Title: &str = r#"Ctrl + double-click"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_CtrlLeftClick_Title: &str = r#"Ctrl + left-click"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_CtrlMouseWheel_Title: &str = r#"Ctrl + mouse wheel"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_CtrlRightClick_Title: &str = r#"Ctrl + right-click"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_DeviceNameMaxLength_Description: &str =
    r#"Names longer than this character count are truncated with a 2-dot ellipsis suffix."#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_DeviceNameMaxLength_SearchKeywords: &str =
    r#"truncate ellipsis characters clipping"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_DeviceNameMaxLength_Title: &str = r#"Device name max length"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_DeviceName_Model: &str = r#"Model"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_DeviceName_Name: &str = r#"Name"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_DeviceName_NameAndModel: &str = r#"Name and model"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_DoubleLeftClick_Title: &str = r#"Double-click"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_MenuPosition_Classic: &str = r#"Classic"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_MenuPosition_Description: &str =
    r#"Classic opens at the cursor. Modern docks the menu above the taskbar like the Windows 11 system flyouts."#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_MenuPosition_Modern: &str = r#"Modern"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_MenuPosition_SearchKeywords: &str = r#"popup cursor taskbar anchor dock Windows 11"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_MenuPosition_Title: &str = r#"Menu position"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_ModifiedActions_Description: &str = r#"Assign actions to clicks or scrolls on the tray icon. Modified entries set to Nothing fall back to the default behavior."#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_ModifiedActions_Header: &str = r#"Modified actions"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_ModifiedClickActions_SearchKeywords: &str =
    r#"mouse gesture shortcut binding assign behavior"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_ModifiedWheelActions_SearchKeywords: &str =
    r#"scroll gesture binding assign behavior"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_MouseWheel_Description: &str = r#"Scroll the mouse wheel over the tray icon to adjust the default device's volume. Does *not* use an always-active global mouse hook."#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_MouseWheel_SearchKeywords: &str =
    r#"scroll notification area taskbar adjust sound speaker"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_MouseWheel_Title: &str = r#"Mouse wheel"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_PlaybackDeviceName_Description: &str =
    r#"Which slice of each playback device's name appears in the tray context menu."#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_PlaybackDeviceName_SearchKeywords: &str =
    r#"speaker output endpoint label model context menu"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_PlaybackDeviceName_Title: &str = r#"Playback device names"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_PrecisionTouchpadScroll_Description: &str =
    r#"Use precision touchpad scroll reports over the tray icon for finer volume control."#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_PrecisionTouchpadScroll_SearchKeywords: &str =
    r#"trackpad gesture scrolling laptop"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_PrecisionTouchpadScroll_Title: &str = r#"Precision touchpad"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_PrecisionTouchpadUnitsPerScrollStep_Description: &str =
    r#"Raw touchpad movement units required for each 1% volume step. Lower values are more sensitive."#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_PrecisionTouchpadUnitsPerScrollStep_SearchKeywords: &str =
    r#"trackpad speed responsiveness movement threshold"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_PrecisionTouchpadUnitsPerScrollStep_Title: &str = r#"Touchpad sensitivity"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_RecordingDeviceName_Description: &str =
    r#"Which slice of each recording device's name appears in the tray context menu."#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_RecordingDeviceName_SearchKeywords: &str =
    r#"microphone input capture endpoint label model context menu"#;
/// Strings.resx
pub(crate) const Settings_TrayIcon_RecordingDeviceName_Title: &str = r#"Recording device names"#;
/// CommonStrings.resx
pub(crate) const Settings_TrayIcon_SectionHeader: &str = r#"Tray icon"#;
