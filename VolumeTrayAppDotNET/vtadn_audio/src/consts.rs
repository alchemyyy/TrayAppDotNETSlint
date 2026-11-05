//! Timing constants and thresholds, named after their C# `TimeConstants` / `AudioDevice` names.

use std::time::Duration;

/// `TimeConstants.VolumeWriteRateDefaultMs`: cooldown between throttled endpoint volume writes.
pub const VOLUME_WRITE_RATE_DEFAULT_MS: Duration = Duration::from_millis(30);

/// `TimeConstants.DefaultsRefreshCoalesceDwellMs`: trailing debounce of `UpdateAllDefaults` after
/// a burst of add/remove/state/default notifications.
pub const DEFAULTS_REFRESH_COALESCE_DWELL_MS: Duration = Duration::from_millis(50);

/// `TimeConstants.DeviceListRefreshAfterMissingDefaultMs`.
pub const DEVICE_LIST_REFRESH_AFTER_MISSING_DEFAULT_MS: Duration = Duration::from_millis(1_000);

/// `TimeConstants.DeviceListRefreshAfterResumeMs`.
pub const DEVICE_LIST_REFRESH_AFTER_RESUME_MS: Duration = Duration::from_millis(2_000);

/// `TimeConstants.DefaultDeviceRoleChangeTimeoutMs`: `ForceCycleEndpoint` waits at most this long
/// for the demotion and for the promotion fan-out.
pub const DEFAULT_DEVICE_ROLE_CHANGE_TIMEOUT_MS: Duration = Duration::from_millis(2_000);

/// `TimeConstants.DeviceVisibilityToggleSettleDelayMs`: `ForceCycleEndpoint` pause between hide and
/// show when the endpoint holds no default role.
pub const DEVICE_VISIBILITY_TOGGLE_SETTLE_DELAY_MS: Duration = Duration::from_millis(250);

/// `TimeConstants.VolumeFeedbackDingDelayMs`: trailing debounce of a `Debounced` ding.
pub const VOLUME_FEEDBACK_DING_DELAY_MS: Duration = Duration::from_millis(350);

/// `TimeConstants.VolumeFeedbackDingDwellPollSliceMs`: suppression re-check cadence while a ding
/// dwells.
pub const VOLUME_FEEDBACK_DING_DWELL_POLL_SLICE_MS: Duration = Duration::from_millis(10);

/// `TimeConstants.VolumeFeedbackDingMeterBypassGraceMs`: added to the WAV duration for the
/// endpoint ding-suppression bypass window.
pub const VOLUME_FEEDBACK_DING_METER_BYPASS_GRACE_MS: Duration = Duration::from_millis(250);

/// Bounded wait for the audio thread in `AudioService::shutdown` (same bound as the C# worker
/// joins `CaptureMeterActivationWorkerJoinTimeoutMs` / `BluetoothCodecWorkerJoinTimeoutMs`).
pub const SERVICE_JOIN_TIMEOUT_MS: Duration = Duration::from_millis(2_000);

/// `AudioDevice.VolumeEqualityEpsilon`: endpoint volume changes smaller than this are no-ops.
pub const VOLUME_EQUALITY_EPSILON: f32 = 0.0005;
