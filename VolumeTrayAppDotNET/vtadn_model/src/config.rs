//! Settings the audio service reads, extracted from [`AppSettings`] on the UI thread and sent with
//! `AudioCommand::Configure`.

use std::collections::HashMap;

use crate::device_settings::DeviceSettings;
use crate::settings::AppSettings;

#[derive(Clone, Debug, PartialEq)]
pub struct AudioBackendConfig {
    pub meter_peak_sample_rate: i32,
    pub unified_peak_meter: bool,
    pub unified_meter_low_channel_bias_multiplier: i32,
    pub activate_recording_devices_for_peak_meters: bool,
    pub show_device_codec_text: bool,
    pub set_default_comms_to_default: bool,
    pub icon_retry_interval_ms: i32,
    pub icon_lru_limit: i32,
    pub play_device_volume_change_sound: bool,
    pub play_app_volume_change_sound: bool,
    pub suppress_device_volume_change_sound_when_audio_playing: bool,
    pub ding_suppression_peak_threshold_percent: i32,
    /// Persisted last-known defaults (fallback for `is_default` while the default is not Active).
    pub last_known_default_playback: Option<String>,
    pub last_known_default_comms_playback: Option<String>,
    pub last_known_default_recording: Option<String>,
    pub last_known_default_comms_recording: Option<String>,
    /// Bluetooth custom names from devices.xml, by device id.
    pub custom_names: HashMap<String, String>,
}

impl AudioBackendConfig {
    pub fn new(settings: &AppSettings, devices: &DeviceSettings) -> Self {
        Self {
            meter_peak_sample_rate: settings.meter_peak_sample_rate,
            unified_peak_meter: settings.unified_peak_meter,
            unified_meter_low_channel_bias_multiplier: settings.unified_meter_low_channel_bias_multiplier,
            activate_recording_devices_for_peak_meters: settings.activate_recording_devices_for_peak_meters,
            show_device_codec_text: settings.show_device_codec_text,
            set_default_comms_to_default: settings.set_default_comms_to_default,
            icon_retry_interval_ms: settings.icon_retry_interval_ms,
            icon_lru_limit: settings.icon_lru_limit,
            play_device_volume_change_sound: settings.play_device_volume_change_sound,
            play_app_volume_change_sound: settings.play_app_volume_change_sound,
            suppress_device_volume_change_sound_when_audio_playing: settings
                .suppress_device_volume_change_sound_when_audio_playing,
            ding_suppression_peak_threshold_percent: settings.ding_suppression_peak_threshold_percent,
            last_known_default_playback: settings.last_known_default_playback_device_id.clone(),
            last_known_default_comms_playback: settings.last_known_default_comms_playback_device_id.clone(),
            last_known_default_recording: settings.last_known_default_recording_device_id.clone(),
            last_known_default_comms_recording: settings.last_known_default_comms_recording_device_id.clone(),
            custom_names: devices
                .devices
                .iter()
                .filter(|d| !d.custom_friendly_name.trim().is_empty())
                .map(|d| (d.id.clone(), d.custom_friendly_name.clone()))
                .collect(),
        }
    }
}
