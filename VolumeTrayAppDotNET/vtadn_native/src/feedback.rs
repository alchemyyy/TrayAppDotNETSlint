//! Volume-change ding playback (ports of `Audio/EndpointSoundPlayback.cs`, `Audio/WAVTemplate.cs`
//! and the playback half of `UI/Flyout/AppVolumeFeedbackPlayer.cs`). Policy (debounce,
//! suppression, settings) lives in the audio service.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_NOPERSIST,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    PlaySoundW, SND_ASYNC, SND_FLAGS, SND_MEMORY, SND_NODEFAULT, WAVE_FORMAT_PCM, WAVEFORMATEX,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows_core::{HSTRING, PCWSTR};

use crate::util::MtaGuard;

/// File under `%WINDIR%\Media` used for both dings.
const APP_FEEDBACK_WAV_NAME: &str = "Windows Background.wav";
/// `EndpointSoundPlaybackBufferDurationHns`.
const ENDPOINT_SOUND_PLAYBACK_BUFFER_DURATION_HNS: i64 = 2_000_000;
/// `EndpointSoundPlaybackPollSliceMs`.
const ENDPOINT_SOUND_PLAYBACK_POLL_SLICE: Duration = Duration::from_millis(30);
/// `EndpointSoundPlaybackMaxDrainMs`.
const ENDPOINT_SOUND_PLAYBACK_MAX_DRAIN_MS: u64 = 5_000;
/// `ScaleSkipThreshold`: volumes at or above this copy the samples unscaled.
const SCALE_SKIP_THRESHOLD: f32 = 0.999;

/// Parsed RIFF/WAVE buffer (port of `WAVTemplate`). Strict: rejects a data chunk before a valid
/// fmt chunk and a fmt chunk that does not populate channels, rate, block align and bit depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavTemplate {
    bytes: Vec<u8>,
    data_offset: usize,
    data_length: usize,
    channels: u16,
    samples_per_sec: u32,
    block_align: u16,
    bits_per_sample: u16,
    duration_ms: u64,
}

impl WavTemplate {
    pub fn from_file(path: &std::path::Path) -> Option<Self> {
        std::fs::read(path).ok().and_then(Self::from_bytes)
    }

    pub fn from_bytes(data: Vec<u8>) -> Option<Self> {
        if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
            return None;
        }
        let mut channels = 0u16;
        let mut samples_per_sec = 0u32;
        let mut block_align = 0u16;
        let mut bits_per_sample = 0u16;
        let mut fmt_seen = false;
        let u16_at = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]);
        let u32_at = |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);

        let mut pos = 12usize;
        while pos + 8 <= data.len() {
            // C# reads the size as Int32; negative sizes are rejected.
            let chunk_size = i32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
            let chunk_data = pos + 8;
            let Ok(chunk_size) = usize::try_from(chunk_size) else { return None };
            if chunk_data + chunk_size > data.len() {
                return None;
            }
            match &data[pos..pos + 4] {
                b"fmt " => {
                    if chunk_size < 16 {
                        return None;
                    }
                    channels = u16_at(chunk_data + 2);
                    samples_per_sec = u32_at(chunk_data + 4);
                    block_align = u16_at(chunk_data + 12);
                    bits_per_sample = u16_at(chunk_data + 14);
                    // C# stores the rate as int: rates above i32::MAX are not positive there.
                    fmt_seen = channels > 0
                        && samples_per_sec > 0
                        && samples_per_sec <= i32::MAX as u32
                        && block_align > 0
                        && bits_per_sample > 0;
                }
                b"data" => {
                    if !fmt_seen {
                        return None;
                    }
                    let duration_ms =
                        (chunk_size / usize::from(block_align)) as u64 * 1000 / u64::from(samples_per_sec);
                    return Some(Self {
                        bytes: data,
                        data_offset: chunk_data,
                        data_length: chunk_size,
                        channels,
                        samples_per_sec,
                        block_align,
                        bits_per_sample,
                        duration_ms,
                    });
                }
                _ => {}
            }
            pos = chunk_data + chunk_size;
            // RIFF chunks are word-aligned: odd-sized chunks carry a trailing pad byte.
            if chunk_size & 1 != 0 {
                pos += 1;
            }
        }
        None
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn data(&self) -> &[u8] {
        &self.bytes[self.data_offset..self.data_offset + self.data_length]
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn samples_per_sec(&self) -> u32 {
        self.samples_per_sec
    }

    pub fn block_align(&self) -> u16 {
        self.block_align
    }

    pub fn bits_per_sample(&self) -> u16 {
        self.bits_per_sample
    }

    /// Natural playback length of the PCM data (whole frames / rate).
    pub fn duration(&self) -> Duration {
        Duration::from_millis(self.duration_ms)
    }

    /// Copy of the whole file with PCM samples scaled by `volume` (clamped 0..1). Header bytes
    /// are copied verbatim; volumes ≥ 0.999 skip the per-sample pass; unsupported bit depths
    /// leave the data untouched.
    pub fn clone_scaled(&self, volume: f32) -> Vec<u8> {
        let mut scaled = self.bytes.clone();
        let clamped = volume.clamp(0.0, 1.0);
        if clamped >= SCALE_SKIP_THRESHOLD {
            return scaled;
        }
        let data = &mut scaled[self.data_offset..self.data_offset + self.data_length];
        match self.bits_per_sample {
            16 => {
                for sample in data.chunks_exact_mut(2) {
                    let value = i16::from_le_bytes([sample[0], sample[1]]);
                    let s = (f32::from(value) * clamped) as i32;
                    sample[0] = (s & 0xFF) as u8;
                    sample[1] = ((s >> 8) & 0xFF) as u8;
                }
            }
            24 => {
                for sample in data.chunks_exact_mut(3) {
                    let mut value = i32::from(sample[0]) | (i32::from(sample[1]) << 8) | (i32::from(sample[2]) << 16);
                    // Sign-extend the 24-bit sample to 32 bits before scaling.
                    if value & 0x80_0000 != 0 {
                        value |= 0xFF00_0000_u32 as i32;
                    }
                    let s = ((value as f32 * clamped) as i32).clamp(-0x80_0000, 0x7F_FFFF);
                    sample[0] = (s & 0xFF) as u8;
                    sample[1] = ((s >> 8) & 0xFF) as u8;
                    sample[2] = ((s >> 16) & 0xFF) as u8;
                }
            }
            32 => {
                for sample in data.chunks_exact_mut(4) {
                    let value = i32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
                    let s = (value as f32 * clamped) as i32;
                    sample.copy_from_slice(&s.to_le_bytes());
                }
            }
            _ => {}
        }
        scaled
    }
}

/// Plays the volume-change ding. Cheap to call from any thread; every call returns at once.
pub struct FeedbackPlayer {
    wav: Arc<OnceLock<Option<WavTemplate>>>,
    own_session: Arc<OwnSessionSound>,
}

/// The buffer `PlaySoundW(SND_MEMORY | SND_ASYNC)` is reading, kept alive while it plays.
struct OwnSessionSound {
    latest_request: AtomicU64,
    playing: Mutex<OwnSessionState>,
}

#[derive(Default)]
struct OwnSessionState {
    played_request: u64,
    buffer: Option<Vec<u8>>,
}

impl Default for FeedbackPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl FeedbackPlayer {
    /// Loads `%WINDIR%\Media\Windows Background.wav` on a background thread.
    pub fn new() -> Self {
        let wav = Arc::new(OnceLock::new());
        let loader = Arc::clone(&wav);
        let spawned = std::thread::Builder::new().name("vtadn-feedback-load".into()).spawn(move || {
            let template = WavTemplate::from_file(&feedback_wav_path());
            if template.is_none() {
                log::warn!("FeedbackPlayer: could not load {APP_FEEDBACK_WAV_NAME}");
            }
            let _ = loader.set(template);
        });
        if let Err(error) = spawned {
            log::warn!("FeedbackPlayer: loader thread failed to start: {error}");
            let _ = wav.set(None);
        }
        Self {
            wav,
            own_session: Arc::new(OwnSessionSound {
                latest_request: AtomicU64::new(0),
                playing: Mutex::new(OwnSessionState::default()),
            }),
        }
    }

    /// Duration of the loaded WAV (None until loaded or when loading failed).
    pub fn duration(&self) -> Option<Duration> {
        self.wav.get().and_then(Option::as_ref).map(WavTemplate::duration)
    }

    /// Plays through the endpoint (WASAPI shared mode) on a worker thread. Render endpoints only;
    /// failures (endpoint gone, format rejected) are logged and dropped.
    pub fn play_on_endpoint(&self, endpoint_id: &str) {
        if endpoint_id.is_empty() {
            return;
        }
        let wav = Arc::clone(&self.wav);
        let endpoint_id = endpoint_id.to_owned();
        let spawned = std::thread::Builder::new().name("vtadn-endpoint-ding".into()).spawn(move || {
            if let Some(template) = wav.wait() {
                let _mta = MtaGuard::enter();
                if let Err(error) = play_endpoint(&endpoint_id, template) {
                    log::warn!("EndpointSoundPlayback.Play: {error}");
                }
            }
        });
        if let Err(error) = spawned {
            log::warn!("FeedbackPlayer: endpoint playback thread failed to start: {error}");
        }
    }

    /// Plays a copy scaled by `level` through this process's own session on the default device; a
    /// newer sound stops the previous one.
    pub fn play_in_own_session(&self, level: f32) {
        let request = self.own_session.latest_request.fetch_add(1, Ordering::AcqRel) + 1;
        let wav = Arc::clone(&self.wav);
        let sound = Arc::clone(&self.own_session);
        let spawned = std::thread::Builder::new().name("vtadn-app-ding".into()).spawn(move || {
            let Some(template) = wav.wait() else { return };
            let scaled = template.clone_scaled(level);
            let mut state = sound.playing.lock().unwrap_or_else(PoisonError::into_inner);
            // A newer request already played (or is about to): this one is superseded.
            if state.played_request > request || sound.latest_request.load(Ordering::Acquire) > request {
                return;
            }
            stop_own_session_sound();
            state.buffer = Some(scaled);
            state.played_request = request;
            if let Some(buffer) = state.buffer.as_ref() {
                // SAFETY: SND_MEMORY reads `buffer`, which stays alive in `state` until a later
                // call or drop first stops playback synchronously with `PlaySoundW(NULL)`.
                let ok =
                    unsafe { PlaySoundW(PCWSTR(buffer.as_ptr().cast()), None, SND_ASYNC | SND_NODEFAULT | SND_MEMORY) };
                if !ok.as_bool() {
                    log::warn!("FeedbackPlayer: PlaySoundW failed");
                }
            }
        });
        if let Err(error) = spawned {
            log::warn!("FeedbackPlayer: app ding thread failed to start: {error}");
        }
    }
}

impl Drop for FeedbackPlayer {
    fn drop(&mut self) {
        let mut state = self.own_session.playing.lock().unwrap_or_else(PoisonError::into_inner);
        if state.buffer.take().is_some() {
            stop_own_session_sound();
        }
    }
}

/// Synchronously stops any waveform sound this process started with `PlaySoundW`.
fn stop_own_session_sound() {
    // SAFETY: a NULL sound name stops the current sound; no buffer is referenced.
    let _ = unsafe { PlaySoundW(PCWSTR::null(), None, SND_FLAGS(0)) };
}

fn feedback_wav_path() -> PathBuf {
    let windows_dir = std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    windows_dir.join("Media").join(APP_FEEDBACK_WAV_NAME)
}

/// WASAPI shared-mode playback of `wav` through `endpoint_id` (port of
/// `EndpointSoundPlayback.Play`). Runs on an MTA worker that owns every COM object it creates.
fn play_endpoint(endpoint_id: &str, wav: &WavTemplate) -> windows_core::Result<()> {
    // SAFETY: COM is initialized (MTA) on this thread; every interface below is created and
    // released on it.
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDevice(&HSTRING::from(endpoint_id))?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

        // A clean 18-byte WAVEFORMATEX matching the source PCM, without any extra fmt fields.
        let block_align = wav.block_align();
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: wav.channels(),
            nSamplesPerSec: wav.samples_per_sec(),
            nAvgBytesPerSec: wav.samples_per_sec().wrapping_mul(u32::from(block_align)),
            nBlockAlign: block_align,
            wBitsPerSample: wav.bits_per_sample(),
            cbSize: 0,
        };
        let flags = AUDCLNT_STREAMFLAGS_NOPERSIST
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            flags,
            ENDPOINT_SOUND_PLAYBACK_BUFFER_DURATION_HNS,
            0,
            &format,
            None,
        )?;
        let buffer_frames = client.GetBufferSize()?;
        if buffer_frames == 0 {
            return Ok(());
        }
        let render: IAudioRenderClient = client.GetService()?;

        let data = wav.data();
        let block_align = usize::from(block_align);
        // Initial fill before Start so the engine never plays a glitch of silence.
        let mut cursor = fill_buffer(&render, buffer_frames, data, block_align);
        client.Start()?;

        let mut waited = 0u64;
        while waited < ENDPOINT_SOUND_PLAYBACK_MAX_DRAIN_MS {
            std::thread::sleep(ENDPOINT_SOUND_PLAYBACK_POLL_SLICE);
            waited += ENDPOINT_SOUND_PLAYBACK_POLL_SLICE.as_millis() as u64;
            let Ok(padding) = client.GetCurrentPadding() else { break };
            if cursor >= data.len() && padding == 0 {
                break;
            }
            if cursor >= data.len() {
                continue;
            }
            let free_frames = buffer_frames.saturating_sub(padding);
            if free_frames > 0 {
                cursor += fill_buffer(&render, free_frames, &data[cursor..], block_align);
            }
        }
        // The device may have torn down already.
        let _ = client.Stop();
    }
    Ok(())
}

/// Writes up to `min(free_frames, source / block_align)` frames; returns the source bytes
/// consumed (0 when the engine refused the buffer or no whole frame remains).
fn fill_buffer(render: &IAudioRenderClient, free_frames: u32, source: &[u8], block_align: usize) -> usize {
    if free_frames == 0 || source.len() < block_align {
        return 0;
    }
    let data_frames = source.len() / block_align;
    let frames = (free_frames as usize).min(data_frames);
    if frames == 0 {
        return 0;
    }
    let Ok(frames_u32) = u32::try_from(frames) else { return 0 };
    // SAFETY: GetBuffer returns a writable region of `frames * block_align` bytes that stays
    // valid until ReleaseBuffer; `source` holds at least that many bytes.
    unsafe {
        let Ok(target) = render.GetBuffer(frames_u32) else { return 0 };
        if target.is_null() {
            return 0;
        }
        let bytes = frames * block_align;
        std::ptr::copy_nonoverlapping(source.as_ptr(), target, bytes);
        if render.ReleaseBuffer(frames_u32, 0).is_err() {
            return 0;
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(bits: u16, channels: u16, rate: u32, samples: &[u8], extra_chunk: Option<&[u8]>) -> Vec<u8> {
        let block_align = channels * bits / 8;
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        let mut body = Vec::new();
        body.extend_from_slice(b"WAVE");
        if let Some(extra) = extra_chunk {
            body.extend_from_slice(b"LIST");
            body.extend_from_slice(&(extra.len() as u32).to_le_bytes());
            body.extend_from_slice(extra);
            if extra.len() % 2 == 1 {
                body.push(0);
            }
        }
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        body.extend_from_slice(&fmt);
        body.extend_from_slice(b"data");
        body.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        body.extend_from_slice(samples);
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn parses_fmt_and_data_after_odd_padded_chunk() {
        let samples: Vec<u8> = (0..400u16).flat_map(|v| (v as i16).to_le_bytes()).collect();
        let bytes = wav(16, 2, 1000, &samples, Some(b"odd"));
        let parsed = WavTemplate::from_bytes(bytes).expect("valid wav");
        assert_eq!(parsed.channels(), 2);
        assert_eq!(parsed.samples_per_sec(), 1000);
        assert_eq!(parsed.block_align(), 4);
        assert_eq!(parsed.bits_per_sample(), 16);
        assert_eq!(parsed.data(), &samples[..]);
        // 800 bytes / 4 = 200 frames at 1000 Hz = 200 ms.
        assert_eq!(parsed.duration(), Duration::from_millis(200));
    }

    #[test]
    fn rejects_malformed_headers() {
        assert!(WavTemplate::from_bytes(Vec::new()).is_none());
        assert!(WavTemplate::from_bytes(b"RIFF\0\0\0\0WAVX".to_vec()).is_none());
        // data before fmt
        let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
        bytes.extend_from_slice(b"data\x02\0\0\0\x01\x02");
        assert!(WavTemplate::from_bytes(bytes).is_none());
        // chunk overruns the buffer
        let mut good = wav(16, 1, 8000, &[0, 0, 1, 0], None);
        good.truncate(good.len() - 1);
        assert!(WavTemplate::from_bytes(good).is_none());
        // zero channels
        assert!(WavTemplate::from_bytes(wav(16, 0, 8000, &[0, 0], None)).is_none());
    }

    #[test]
    fn scales_16_bit_samples_and_keeps_header() {
        let samples: Vec<u8> = [1000i16, -1000, 32767, -32768].iter().flat_map(|v| v.to_le_bytes()).collect();
        let parsed = WavTemplate::from_bytes(wav(16, 1, 8000, &samples, None)).expect("valid");
        let scaled = parsed.clone_scaled(0.5);
        let header = parsed.bytes().len() - samples.len();
        assert_eq!(&scaled[..header], &parsed.bytes()[..header]);
        let values: Vec<i16> = scaled[header..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(values, vec![500, -500, 16383, -16384]);
        // Near-unity volume returns the bytes untouched; out-of-range volumes clamp.
        assert_eq!(parsed.clone_scaled(0.9995), parsed.bytes());
        assert_eq!(parsed.clone_scaled(2.0), parsed.bytes());
        let silent = parsed.clone_scaled(-1.0);
        assert!(silent[header..].iter().all(|&b| b == 0));
    }

    #[test]
    fn scales_24_and_32_bit_samples() {
        let samples24 = [0xFF, 0xFF, 0x7F, 0x00, 0x00, 0x80]; // 8388607, -8388608
        let parsed = WavTemplate::from_bytes(wav(24, 1, 8000, &samples24, None)).expect("valid");
        let scaled = parsed.clone_scaled(0.25);
        let header = parsed.bytes().len() - samples24.len();
        let read24 = |c: &[u8]| {
            let mut v = i32::from(c[0]) | (i32::from(c[1]) << 8) | (i32::from(c[2]) << 16);
            if v & 0x80_0000 != 0 {
                v |= 0xFF00_0000_u32 as i32;
            }
            v
        };
        let values: Vec<i32> = scaled[header..].chunks_exact(3).map(read24).collect();
        assert_eq!(values, vec![(8_388_607f32 * 0.25) as i32, -2_097_152]);

        let samples32: Vec<u8> = [1_000_000i32, -1_000_000].iter().flat_map(|v| v.to_le_bytes()).collect();
        let parsed = WavTemplate::from_bytes(wav(32, 1, 8000, &samples32, None)).expect("valid");
        let scaled = parsed.clone_scaled(0.5);
        let header = parsed.bytes().len() - samples32.len();
        let values: Vec<i32> =
            scaled[header..].chunks_exact(4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        assert_eq!(values, vec![500_000, -500_000]);
    }

    #[test]
    fn unsupported_depth_is_left_untouched() {
        let parsed = WavTemplate::from_bytes(wav(8, 1, 8000, &[10, 20, 30, 40], None)).expect("valid");
        assert_eq!(parsed.clone_scaled(0.1), parsed.bytes());
    }
}
