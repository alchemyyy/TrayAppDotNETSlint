//! Endpoint default format: WAVEFORMATEXTENSIBLE blob build/parse (`AudioDevice.BuildFormatBlob`
//! / `ParseFormatBlob`) and the mmsys.cpl-identical KS format probe
//! (`EnumerateSupportedFormats` / `QueryKsAudioDataRanges` / `ProbeFormat`).

use std::collections::BTreeSet;
use std::ffi::c_void;

use vtadn_model::snapshot::Format;
use windows::Win32::Media::Audio::{IMMDevice, IPartsList};
use windows::Win32::Media::KernelStreaming::{IKsFormatSupport, KSDATAFORMAT};
use windows::Win32::System::Com::CLSCTX_INPROC_SERVER;
use windows_core::{BOOL, GUID, Interface};

/// `WAVE_FORMAT_EXTENSIBLE`.
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// cbSize of the EXTENSIBLE tail.
const EXTENSIBLE_CB_SIZE: u16 = 22;
/// Size of the WAVEFORMATEXTENSIBLE blob.
pub const WFX_BLOB_SIZE: usize = 40;
/// Size of the KSDATAFORMAT header.
const KS_DATA_FORMAT_HEADER_SIZE: usize = 64;
/// KSDATAFORMAT header + WAVEFORMATEXTENSIBLE.
const KS_FORMAT_PROBE_SIZE: usize = KS_DATA_FORMAT_HEADER_SIZE + WFX_BLOB_SIZE;

const KSDATAFORMAT_TYPE_AUDIO: GUID = GUID::from_u128(0x73647561_0000_0010_8000_00aa00389b71);
const KSDATAFORMAT_SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const KSDATAFORMAT_SPECIFIER_WAVEFORMATEX: GUID = GUID::from_u128(0x05589f81_c356_11ce_bf01_00aa0055595a);

/// Probe rates for 16-bit candidates.
pub const RATES_16: [u32; 11] = [8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000];
/// Probe rates for 24-bit candidates (mmsys.cpl never offers 24-bit below 44.1 kHz).
pub const RATES_24: [u32; 6] = [44100, 48000, 88200, 96000, 176400, 192000];

/// In-memory layout of a GUID (`Guid.ToByteArray`).
fn guid_bytes(guid: &GUID) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&guid.data1.to_le_bytes());
    out[4..6].copy_from_slice(&guid.data2.to_le_bytes());
    out[6..8].copy_from_slice(&guid.data3.to_le_bytes());
    out[8..16].copy_from_slice(&guid.data4);
    out
}

/// `KSAUDIO_SPEAKER_*` mask for a channel count.
pub fn channel_mask(channels: u16) -> u32 {
    match channels {
        1 => 0x4,
        2 => 0x3,
        4 => 0x33,
        6 => 0x3F,
        8 => 0xFF,
        n if n >= 32 => 0,
        n => (1u32 << n) - 1,
    }
}

/// `BuildFormatBlob`: 40-byte little-endian WAVEFORMATEXTENSIBLE with the PCM subformat.
pub fn build_format_blob(channels: u16, valid_bits: u16, container_bits: u16, rate: u32) -> [u8; WFX_BLOB_SIZE] {
    let block_align = channels.wrapping_mul(container_bits / 8);
    let avg_bytes_per_sec = rate.wrapping_mul(u32::from(block_align));
    let mut blob = [0u8; WFX_BLOB_SIZE];
    blob[0..2].copy_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
    blob[2..4].copy_from_slice(&channels.to_le_bytes());
    blob[4..8].copy_from_slice(&rate.to_le_bytes());
    blob[8..12].copy_from_slice(&avg_bytes_per_sec.to_le_bytes());
    blob[12..14].copy_from_slice(&block_align.to_le_bytes());
    blob[14..16].copy_from_slice(&container_bits.to_le_bytes());
    blob[16..18].copy_from_slice(&EXTENSIBLE_CB_SIZE.to_le_bytes());
    blob[18..20].copy_from_slice(&valid_bits.to_le_bytes());
    blob[20..24].copy_from_slice(&channel_mask(channels).to_le_bytes());
    blob[24..40].copy_from_slice(&guid_bytes(&KSDATAFORMAT_SUBTYPE_PCM));
    blob
}

/// `SetDeviceFormat`: 24-bit is written as 24-in-32; every other depth as container == valid.
pub fn container_bits(bits: u16) -> u16 {
    if bits == 24 { 32 } else { bits }
}

fn u16_at(blob: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([blob[at], blob[at + 1]])
}

/// `ParseFormatBlob`: needs 18 bytes; EXTENSIBLE blobs report wValidBitsPerSample when non-zero.
pub fn parse_format_blob(blob: &[u8]) -> Option<Format> {
    if blob.len() < 18 {
        return None;
    }
    let tag = u16_at(blob, 0);
    let channels = u16_at(blob, 2);
    let rate = u32::from_le_bytes([blob[4], blob[5], blob[6], blob[7]]);
    let mut bits = u16_at(blob, 14);
    if tag == WAVE_FORMAT_EXTENSIBLE && blob.len() >= 22 {
        let valid = u16_at(blob, 18);
        if valid > 0 {
            bits = valid;
        }
    }
    Some(Format { channels, bits, rate })
}

/// Channel candidates: the mmsys.cpl layouts plus the current count (sorted, deduplicated).
pub fn channel_candidates(current_channels: u16) -> BTreeSet<u16> {
    [1, 2, 4, 6, 8, current_channels].into_iter().collect()
}

/// Current channel count for the probe: u16 at offset 2 of an 18+ byte blob, else 2.
pub fn current_channels(blob: Option<&[u8]>) -> u16 {
    match blob {
        Some(b) if b.len() >= 18 => u16_at(b, 2),
        _ => 2,
    }
}

/// The 64-byte KSDATAFORMAT header (audio / PCM / WAVEFORMATEX specifier) with `FormatSize`.
fn ks_header(format_size: u32, out: &mut [u8]) {
    out[..KS_DATA_FORMAT_HEADER_SIZE].fill(0);
    out[0..4].copy_from_slice(&format_size.to_le_bytes());
    out[16..32].copy_from_slice(&guid_bytes(&KSDATAFORMAT_TYPE_AUDIO));
    out[32..48].copy_from_slice(&guid_bytes(&KSDATAFORMAT_SUBTYPE_PCM));
    out[48..64].copy_from_slice(&guid_bytes(&KSDATAFORMAT_SPECIFIER_WAVEFORMATEX));
}

/// 8-byte aligned buffer for KS structures.
#[repr(C, align(8))]
struct KsBuffer<const N: usize>([u8; N]);

mod ffi {
    #![allow(non_snake_case)]

    use std::ffi::c_void;

    use windows_core::{HRESULT, IUnknown, IUnknown_Vtbl};

    /// Private audio-engine filter interface mmsys.cpl activates on the endpoint
    /// (`IID_AudioEnginePartFilter`); slot 3 returns an `IPartsList` of engine topology parts that
    /// accept a KSDATAFORMAT.
    #[windows_core::interface("2b0711de-dab7-4610-a16f-d3383749b220")]
    pub unsafe trait IAudioEnginePartFilter: IUnknown {
        pub fn FilterByDataFormat(
            &self,
            format: *const u8,
            size: u32,
            reserved: *const c_void,
            parts: *mut *mut c_void,
        ) -> HRESULT;
    }
}

use ffi::IAudioEnginePartFilter;

/// The probe chain of `QueryKsAudioDataRanges`: engine part filter → parts list → first part that
/// activates `IKsFormatSupport` → `IsFormatSupported` per candidate. None on any failure.
pub fn query_supported_formats(device: &IMMDevice, current_channels: u16, label: &str) -> Option<Vec<Format>> {
    // SAFETY: COM activation on a device owned by this (MTA) thread.
    let filter: IAudioEnginePartFilter = match unsafe { device.Activate(CLSCTX_INPROC_SERVER, None) } {
        Ok(filter) => filter,
        Err(error) => {
            log::info!("QueryKsAudioDataRanges({label}): Activate(IID_AudioEnginePartFilter) failed: {error}");
            return None;
        }
    };

    let mut header = KsBuffer([0u8; KS_DATA_FORMAT_HEADER_SIZE]);
    ks_header(KS_DATA_FORMAT_HEADER_SIZE as u32, &mut header.0);
    let mut parts_raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: the header buffer is valid for 64 bytes; the out pointer receives an owned reference.
    let hr = unsafe {
        filter.FilterByDataFormat(header.0.as_ptr(), header.0.len() as u32, std::ptr::null(), &mut parts_raw)
    };
    if hr.is_err() || parts_raw.is_null() {
        log::info!("QueryKsAudioDataRanges({label}): filter->vtable[3] hr=0x{:08X}", hr.0);
        return None;
    }
    // SAFETY: slot 3 returned an owned `IPartsList` pointer (GetCount slot 3, GetPart slot 4).
    let parts = unsafe { IPartsList::from_raw(parts_raw) };
    // SAFETY: plain COM call.
    let count = match unsafe { parts.GetCount() } {
        Ok(count) if count > 0 => count,
        other => {
            log::info!("QueryKsAudioDataRanges({label}): enum->GetCount {other:?}");
            return None;
        }
    };

    let mut format_support: Option<IKsFormatSupport> = None;
    for index in 0..count {
        // SAFETY: plain COM call.
        let Ok(part) = (unsafe { parts.GetPart(index) }) else { continue };
        let mut raw: *mut c_void = std::ptr::null_mut();
        // SAFETY: `IPart::Activate` (slot 13) writes an owned interface pointer for the IID.
        let activated = unsafe { part.Activate(CLSCTX_INPROC_SERVER.0, &IKsFormatSupport::IID, Some(&mut raw)) };
        if activated.is_ok() && !raw.is_null() {
            log::debug!("QueryKsAudioDataRanges({label}): part[{index}] -> IKsFormatSupport");
            // SAFETY: Activate succeeded for IKsFormatSupport's IID; we own the reference.
            format_support = Some(unsafe { IKsFormatSupport::from_raw(raw) });
            break;
        }
    }
    let format_support = format_support?;

    let mut accepted = BTreeSet::new();
    let mut probed = 0u32;
    for channels in channel_candidates(current_channels) {
        if channels < 1 {
            continue;
        }
        for rate in RATES_16 {
            probed += 1;
            if probe_format(&format_support, channels, 16, 16, rate) {
                accepted.insert(Format { channels, bits: 16, rate });
            }
        }
        for rate in RATES_24 {
            probed += 1;
            // 24-in-32 (most render endpoints) or packed 24-in-24 (most USB drivers).
            if probe_format(&format_support, channels, 24, 32, rate)
                || probe_format(&format_support, channels, 24, 24, rate)
            {
                accepted.insert(Format { channels, bits: 24, rate });
            }
        }
    }
    log::info!("QueryKsAudioDataRanges({label}): probed={probed} accepted={}", accepted.len());
    Some(accepted.into_iter().collect())
}

/// `ProbeFormat`: 104-byte KSDATAFORMAT_WAVEFORMATEX → `IKsFormatSupport::IsFormatSupported`.
fn probe_format(format_support: &IKsFormatSupport, channels: u16, valid_bits: u16, container: u16, rate: u32) -> bool {
    let mut buffer = KsBuffer([0u8; KS_FORMAT_PROBE_SIZE]);
    ks_header(KS_FORMAT_PROBE_SIZE as u32, &mut buffer.0);
    buffer.0[KS_DATA_FORMAT_HEADER_SIZE..].copy_from_slice(&build_format_blob(channels, valid_bits, container, rate));
    let mut supported = BOOL(0);
    // SAFETY: the buffer is a valid, 8-aligned 104-byte KSDATAFORMAT + WAVEFORMATEXTENSIBLE.
    let result = unsafe {
        format_support.IsFormatSupported(
            buffer.0.as_mut_ptr().cast::<KSDATAFORMAT>(),
            KS_FORMAT_PROBE_SIZE as u32,
            &mut supported,
        )
    };
    result.is_ok() && supported.as_bool()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_layout_matches_build_format_blob() {
        let blob = build_format_blob(2, 24, 32, 48000);
        assert_eq!(u16_at(&blob, 0), 0xFFFE);
        assert_eq!(u16_at(&blob, 2), 2);
        assert_eq!(u32::from_le_bytes(blob[4..8].try_into().unwrap()), 48000);
        assert_eq!(u32::from_le_bytes(blob[8..12].try_into().unwrap()), 48000 * 8);
        assert_eq!(u16_at(&blob, 12), 8);
        assert_eq!(u16_at(&blob, 14), 32);
        assert_eq!(u16_at(&blob, 16), 22);
        assert_eq!(u16_at(&blob, 18), 24);
        assert_eq!(u32::from_le_bytes(blob[20..24].try_into().unwrap()), 0x3);
        assert_eq!(&blob[24..40], &[0x01, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
    }

    #[test]
    fn channel_masks() {
        assert_eq!(channel_mask(1), 0x4);
        assert_eq!(channel_mask(2), 0x3);
        assert_eq!(channel_mask(3), 0x7);
        assert_eq!(channel_mask(4), 0x33);
        assert_eq!(channel_mask(5), 0x1F);
        assert_eq!(channel_mask(6), 0x3F);
        assert_eq!(channel_mask(8), 0xFF);
        assert_eq!(channel_mask(31), 0x7FFF_FFFF);
        assert_eq!(channel_mask(32), 0);
        assert_eq!(channel_mask(40), 0);
    }

    #[test]
    fn parse_round_trips_and_prefers_valid_bits() {
        let blob = build_format_blob(6, 24, 32, 96000);
        assert_eq!(parse_format_blob(&blob), Some(Format { channels: 6, bits: 24, rate: 96000 }));

        // EXTENSIBLE with valid bits 0 keeps the container size.
        let mut zero_valid = build_format_blob(2, 0, 16, 44100);
        zero_valid[18] = 0;
        assert_eq!(parse_format_blob(&zero_valid), Some(Format { channels: 2, bits: 16, rate: 44100 }));

        // Plain WAVEFORMATEX (PCM tag 1, 18 bytes) reports wBitsPerSample.
        let mut pcm = [0u8; 18];
        pcm[0] = 1;
        pcm[2] = 1;
        pcm[4..8].copy_from_slice(&22050u32.to_le_bytes());
        pcm[14] = 16;
        assert_eq!(parse_format_blob(&pcm), Some(Format { channels: 1, bits: 16, rate: 22050 }));

        // EXTENSIBLE tag but shorter than 22 bytes: wBitsPerSample.
        let short = &build_format_blob(2, 24, 32, 48000)[..20];
        assert_eq!(parse_format_blob(short), Some(Format { channels: 2, bits: 32, rate: 48000 }));

        assert_eq!(parse_format_blob(&[0u8; 17]), None);
    }

    #[test]
    fn container_bits_for_24_is_32() {
        assert_eq!(container_bits(24), 32);
        assert_eq!(container_bits(16), 16);
        assert_eq!(container_bits(32), 32);
    }

    #[test]
    fn probe_candidates() {
        assert_eq!(channel_candidates(2).into_iter().collect::<Vec<_>>(), vec![1, 2, 4, 6, 8]);
        assert_eq!(channel_candidates(3).into_iter().collect::<Vec<_>>(), vec![1, 2, 3, 4, 6, 8]);
        assert_eq!(current_channels(None), 2);
        assert_eq!(current_channels(Some(&[0u8; 10])), 2);
        assert_eq!(current_channels(Some(&build_format_blob(6, 16, 16, 48000))), 6);
        // 5 channels * (11 + 6) = 85 candidate probes for a stereo device (24-bit may try twice).
        assert_eq!(channel_candidates(2).len() * (RATES_16.len() + RATES_24.len()), 85);
    }

    #[test]
    fn ks_header_layout() {
        let mut header = [0xAAu8; 64];
        ks_header(104, &mut header);
        assert_eq!(u32::from_le_bytes(header[0..4].try_into().unwrap()), 104);
        assert!(header[4..16].iter().all(|b| *b == 0));
        assert_eq!(&header[16..20], &[0x61, 0x75, 0x64, 0x73]);
        assert_eq!(&header[32..36], &[0x01, 0, 0, 0]);
        assert_eq!(&header[48..52], &[0x81, 0x9F, 0x58, 0x05]);
    }

    #[test]
    fn sorted_by_channels_bits_rate() {
        let mut formats = [
            Format { channels: 2, bits: 24, rate: 44100 },
            Format { channels: 1, bits: 16, rate: 48000 },
            Format { channels: 2, bits: 16, rate: 96000 },
            Format { channels: 2, bits: 16, rate: 44100 },
        ];
        formats.sort();
        assert_eq!(
            formats,
            [
                Format { channels: 1, bits: 16, rate: 48000 },
                Format { channels: 2, bits: 16, rate: 44100 },
                Format { channels: 2, bits: 16, rate: 96000 },
                Format { channels: 2, bits: 24, rate: 44100 },
            ]
        );
    }
}
