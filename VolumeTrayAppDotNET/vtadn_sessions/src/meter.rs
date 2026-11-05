//! `MeterReader.ReadStereoPeaks`: COM side of the peak meter, weighted by
//! `vtadn_model::policy::meter::weigh_channel_peaks`.

use vtadn_model::policy::meter::weigh_channel_peaks;
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;

/// `MaxStackChannels`: endpoints with more metering channels fall back to `GetPeakValue`.
const MAX_STACK_CHANNELS: usize = 32;

/// `(min, max)` over the first two channels (both the single peak for mono and for more than 32
/// channels); unified mode collapses both to the weighted value. Any failure reads as `(0, 0)`.
/// Allocation-free (stack buffer).
pub fn read_meter_peaks(meter: &IAudioMeterInformation, unified: bool, low_channel_bias: i32) -> (f32, f32) {
    // SAFETY: plain COM reads on a live meter; the buffer length is passed through the slice.
    unsafe {
        let Ok(count) = meter.GetMeteringChannelCount() else { return (0.0, 0.0) };
        let count = count as usize;
        if count == 0 {
            return (0.0, 0.0);
        }
        if count == 1 || count > MAX_STACK_CHANNELS {
            return meter.GetPeakValue().map(|peak| (peak, peak)).unwrap_or((0.0, 0.0));
        }
        let mut buffer = [0f32; MAX_STACK_CHANNELS];
        let channels = &mut buffer[..count];
        if meter.GetChannelsPeakValues(channels).is_err() {
            return (0.0, 0.0);
        }
        weigh_channel_peaks(channels, unified, low_channel_bias)
    }
}
