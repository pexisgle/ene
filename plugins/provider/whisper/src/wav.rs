//! Minimal RIFF/WAVE decoder for the audio files the host sends over the
//! `TranscribeAudio` wire contract.
//!
//! The host adapter encodes microphone PCM as 16-bit mono WAV (the format
//! this plugin declares), so the decoder only needs the shapes the encoder
//! can produce — plus a few defensive rejects.

use ene_plugin::PluginError;

/// Decoded PCM audio.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedWav {
    /// Mono PCM samples normalized to `[-1.0, 1.0]`.
    pub pcm: Vec<f32>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

/// Parses a RIFF/WAVE byte stream into mono PCM `f32` samples.
///
/// # Errors
///
/// Returns a provider error when the bytes are not a well-formed WAV file in
/// one of the supported encodings (PCM s16/s32 or IEEE float).
pub fn decode_wav(bytes: &[u8]) -> Result<DecodedWav, PluginError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(PluginError::provider(
            "STT audio is not a RIFF/WAVE stream".to_string(),
        ));
    }
    let mut fmt: Option<FmtChunk> = None;
    let mut data: Option<&[u8]> = None;
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = u32_at(bytes, offset + 4)? as usize;
        let chunk_data = offset + 8;
        let chunk_end = chunk_data + chunk_size;
        if chunk_end > bytes.len() {
            return Err(PluginError::provider("truncated WAV chunk".to_string()));
        }
        match chunk_id {
            b"fmt " if fmt.is_none() => {
                fmt = Some(parse_fmt_chunk(&bytes[chunk_data..chunk_end])?);
            }
            b"data" if data.is_none() => {
                data = Some(&bytes[chunk_data..chunk_end]);
            }
            _ => {}
        }
        offset = chunk_end + (chunk_size & 1);
    }
    let fmt =
        fmt.ok_or_else(|| PluginError::provider("WAV stream has no fmt chunk".to_string()))?;
    let data =
        data.ok_or_else(|| PluginError::provider("WAV stream has no data chunk".to_string()))?;
    if fmt.sample_rate == 0 {
        return Err(PluginError::provider("WAV sample rate is zero".to_string()));
    }
    Ok(DecodedWav {
        pcm: decode_samples(data, fmt)?,
        sample_rate: fmt.sample_rate,
    })
}

#[derive(Debug, Clone, Copy)]
struct FmtChunk {
    encoding: u16,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

fn parse_fmt_chunk(bytes: &[u8]) -> Result<FmtChunk, PluginError> {
    if bytes.len() < 16 {
        return Err(PluginError::provider("truncated WAV fmt chunk".to_string()));
    }
    let fmt = FmtChunk {
        encoding: u16_at(bytes, 0),
        channels: u16_at(bytes, 2),
        sample_rate: u32_at(bytes, 4)?,
        bits_per_sample: u16_at(bytes, 14),
    };
    match fmt.encoding {
        WAVE_FORMAT_PCM if matches!(fmt.bits_per_sample, 16 | 32) => {}
        WAVE_FORMAT_IEEE_FLOAT if fmt.bits_per_sample == 32 => {}
        _ => {
            return Err(PluginError::provider(format!(
                "unsupported WAV encoding: format {} at {} bits",
                fmt.encoding, fmt.bits_per_sample
            )));
        }
    }
    if !matches!(fmt.channels, 1 | 2) {
        return Err(PluginError::provider(format!(
            "unsupported WAV channel count: {}",
            fmt.channels
        )));
    }
    Ok(fmt)
}

fn decode_samples(data: &[u8], fmt: FmtChunk) -> Result<Vec<f32>, PluginError> {
    let bytes_per_sample = usize::from(fmt.bits_per_sample / 8);
    let frame_size = bytes_per_sample * usize::from(fmt.channels);
    if frame_size == 0 {
        return Err(PluginError::provider(
            "invalid WAV sample width".to_string(),
        ));
    }
    let frames = data.len() / frame_size;
    let mut pcm = Vec::with_capacity(frames);
    for frame in data.chunks_exact(frame_size) {
        let mut mixed = 0.0f32;
        for channel in 0..usize::from(fmt.channels) {
            let sample = &frame[channel * bytes_per_sample..(channel + 1) * bytes_per_sample];
            mixed += match (fmt.encoding, fmt.bits_per_sample) {
                (WAVE_FORMAT_PCM, 16) => {
                    f32::from(i16::from_le_bytes([sample[0], sample[1]])) / f32::from(i16::MAX)
                }
                (WAVE_FORMAT_PCM, 32) => {
                    i32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]) as f32
                        / i32::MAX as f32
                }
                (WAVE_FORMAT_IEEE_FLOAT, 32) => {
                    f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]])
                }
                _ => unreachable!("encoding validated by parse_fmt_chunk"),
            };
        }
        pcm.push(mixed / f32::from(fmt.channels));
    }
    Ok(pcm)
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, PluginError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| PluginError::provider("truncated WAV header".to_string()))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// A valid 16 kHz mono s16 WAV fixture (4 zero samples), shared with the
/// plugin-level tests.
#[cfg(test)]
pub(crate) fn decode_wav_test_fixture() -> Vec<u8> {
    build_wav_for_tests(16_000, &[0, 0, 0, 0])
}

#[cfg(test)]
fn build_wav_for_tests(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&WAVE_FORMAT_PCM.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect/unwrap for concise assertions"
)]
mod tests {
    use super::*;

    fn build_wav(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
        build_wav_for_tests(sample_rate, samples)
    }

    #[test]
    fn decodes_mono_s16_pcm() {
        let wav = build_wav(16_000, &[0, 16_384, -16_384, i16::MAX]);
        let decoded = decode_wav(&wav).expect("valid s16 wav");
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.pcm.len(), 4);
        assert!(decoded.pcm[0].abs() < 1e-4);
        assert!((decoded.pcm[1] - 0.5).abs() < 1e-4);
        assert!((decoded.pcm[2] + 0.5).abs() < 1e-4);
        assert!((decoded.pcm[3] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn rejects_non_riff_bytes() {
        let err = decode_wav(b"not a wav file at all").expect_err("invalid magic");
        assert!(err.to_string().contains("RIFF/WAVE"));
    }

    #[test]
    fn rejects_truncated_data_chunk() {
        let mut wav = build_wav(16_000, &[0, 1, 2]);
        wav.truncate(wav.len() - 2);
        let err = decode_wav(&wav).expect_err("truncated");
        assert!(err.to_string().contains("truncated"));
    }
}
