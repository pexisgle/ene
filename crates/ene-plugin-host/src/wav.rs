//! Minimal RIFF/WAVE decoder for plugin-delivered TTS audio.
//!
//! The plugin IPC contract returns whole audio files (base64 `SpeechResult`
//! payloads), while [`ene_ai::TtsProvider`] consumes PCM `f32` chunks, so the
//! host adapter decodes the WAV bytes itself. Only the formats VOICEVOX /
//! Aivis Speech engines emit are supported: PCM s16/s32 or IEEE float, one or
//! two channels (stereo is downmixed to mono), any sample rate. Anything
//! else is rejected as [`AudioProviderError::UnsupportedFormat`].

use ene_ai::AudioProviderError;

/// Decoded PCM audio.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedWav {
    /// Interleaved mono PCM samples normalized to `[-1.0, 1.0]`.
    pub pcm: Vec<f32>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

/// Parses a RIFF/WAVE byte stream into mono PCM `f32` samples.
///
/// # Errors
///
/// Returns [`AudioProviderError::UnsupportedFormat`] when the bytes are not
/// a well-formed WAV file in one of the supported encodings.
pub fn decode_wav(bytes: &[u8]) -> Result<DecodedWav, AudioProviderError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(AudioProviderError::UnsupportedFormat(
            "not a RIFF/WAVE stream".to_string(),
        ));
    }

    let mut fmt: Option<FmtChunk> = None;
    let mut data: Option<&[u8]> = None;
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = u32_at(bytes, offset + 4)? as usize;
        let chunk_data = offset + 8;
        let chunk_end = chunk_data.checked_add(chunk_size).ok_or_else(|| {
            AudioProviderError::UnsupportedFormat("WAV chunk size overflow".to_string())
        })?;
        if chunk_end > bytes.len() {
            return Err(AudioProviderError::UnsupportedFormat(
                "truncated WAV chunk".to_string(),
            ));
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

    let fmt = fmt.ok_or_else(|| {
        AudioProviderError::UnsupportedFormat("WAV stream has no fmt chunk".to_string())
    })?;
    let data = data.ok_or_else(|| {
        AudioProviderError::UnsupportedFormat("WAV stream has no data chunk".to_string())
    })?;
    if fmt.sample_rate == 0 {
        return Err(AudioProviderError::UnsupportedFormat(
            "WAV sample rate is zero".to_string(),
        ));
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

/// PCM encoding tags from the WAV `fmt` chunk.
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

fn parse_fmt_chunk(bytes: &[u8]) -> Result<FmtChunk, AudioProviderError> {
    if bytes.len() < 16 {
        return Err(AudioProviderError::UnsupportedFormat(
            "truncated WAV fmt chunk".to_string(),
        ));
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
            return Err(AudioProviderError::UnsupportedFormat(format!(
                "unsupported WAV encoding: format {} at {} bits",
                fmt.encoding, fmt.bits_per_sample
            )));
        }
    }
    if !matches!(fmt.channels, 1 | 2) {
        return Err(AudioProviderError::UnsupportedFormat(format!(
            "unsupported WAV channel count: {}",
            fmt.channels
        )));
    }
    Ok(fmt)
}

fn decode_samples(data: &[u8], fmt: FmtChunk) -> Result<Vec<f32>, AudioProviderError> {
    let bytes_per_sample = usize::from(fmt.bits_per_sample / 8);
    let frame_size = bytes_per_sample * usize::from(fmt.channels);
    if frame_size == 0 {
        return Err(AudioProviderError::UnsupportedFormat(
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

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, AudioProviderError> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| AudioProviderError::UnsupportedFormat("WAV offset overflow".to_string()))?;
    let slice = bytes
        .get(offset..end)
        .ok_or_else(|| AudioProviderError::UnsupportedFormat("truncated WAV header".to_string()))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests use expect/unwrap for concise assertions"
)]
mod tests {
    use super::*;

    /// Builds a WAV byte stream for the given mono/stereo interleaved samples.
    fn build_wav(
        encoding: u16,
        channels: u16,
        sample_rate: u32,
        bits_per_sample: u16,
        samples: &[i32],
    ) -> Vec<u8> {
        let bytes_per_sample = usize::from(bits_per_sample / 8);
        let mut data = Vec::with_capacity(samples.len() * bytes_per_sample);
        for sample in samples {
            match bits_per_sample {
                16 => data.extend_from_slice(&(*sample as i16).to_le_bytes()),
                32 if encoding == WAVE_FORMAT_PCM => {
                    data.extend_from_slice(&sample.to_le_bytes());
                }
                32 => data.extend_from_slice(&(*sample as f32).to_le_bytes()),
                _ => panic!("unsupported test bits"),
            }
        }
        let fmt_len = 16u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&fmt_len.to_le_bytes());
        bytes.extend_from_slice(&encoding.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(
            &(sample_rate * u32::from(bits_per_sample / 8) * u32::from(channels)).to_le_bytes(),
        );
        bytes.extend_from_slice(&((bits_per_sample / 8) * channels).to_le_bytes());
        bytes.extend_from_slice(&bits_per_sample.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes
    }

    #[test]
    fn decodes_mono_s16_pcm() {
        let wav = build_wav(
            WAVE_FORMAT_PCM,
            1,
            24_000,
            16,
            &[0, 16_384, -16_384, 32_767],
        );
        let decoded = decode_wav(&wav).expect("valid s16 wav");
        assert_eq!(decoded.sample_rate, 24_000);
        assert_eq!(decoded.pcm.len(), 4);
        assert!(decoded.pcm[0].abs() < 1e-4);
        assert!((decoded.pcm[1] - 0.5).abs() < 1e-4);
        assert!((decoded.pcm[2] + 0.5).abs() < 1e-4);
        assert!((decoded.pcm[3] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn decodes_mono_s32_pcm() {
        let wav = build_wav(
            WAVE_FORMAT_PCM,
            1,
            24_000,
            32,
            &[i32::MAX / 2, i32::MIN / 2],
        );
        let decoded = decode_wav(&wav).expect("valid s32 wav");
        assert!((decoded.pcm[0] - 0.5).abs() < 1e-6);
        assert!((decoded.pcm[1] + 0.5).abs() < 1e-6);
    }

    #[test]
    fn decodes_mono_f32_pcm() {
        let wav = build_wav(WAVE_FORMAT_IEEE_FLOAT, 1, 48_000, 32, &[0, -1, 1]);
        let decoded = decode_wav(&wav).expect("valid f32 wav");
        assert_eq!(decoded.sample_rate, 48_000);
        for (actual, expected) in decoded.pcm.iter().zip([0.0, -1.0, 1.0]) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn downmixes_stereo_to_mono() {
        let wav = build_wav(WAVE_FORMAT_PCM, 2, 24_000, 16, &[0, 0, 16_384, -16_384]);
        let decoded = decode_wav(&wav).expect("valid stereo wav");
        assert_eq!(decoded.pcm.len(), 2);
        assert!(decoded.pcm[0].abs() < 1e-4);
        assert!((decoded.pcm[1] - 0.0).abs() < 1e-4);
    }

    #[test]
    fn rejects_non_riff_bytes() {
        let err = decode_wav(b"not a wav file at all").expect_err("invalid magic");
        assert!(matches!(err, AudioProviderError::UnsupportedFormat(_)));
    }

    #[test]
    fn rejects_truncated_data_chunk() {
        let mut wav = build_wav(WAVE_FORMAT_PCM, 1, 24_000, 16, &[0, 1, 2]);
        wav.truncate(wav.len() - 2);
        let err = decode_wav(&wav).expect_err("truncated data");
        assert!(matches!(err, AudioProviderError::UnsupportedFormat(_)));
    }

    #[test]
    fn rejects_unsupported_encoding() {
        let wav = build_wav(6, 1, 24_000, 16, &[0, 1]);
        let err = decode_wav(&wav).expect_err("alaw is unsupported");
        assert!(matches!(err, AudioProviderError::UnsupportedFormat(_)));
    }

    #[test]
    fn rejects_unsupported_channel_count() {
        let wav = build_wav(WAVE_FORMAT_PCM, 3, 24_000, 16, &[0, 1, 2]);
        let err = decode_wav(&wav).expect_err("3 channels unsupported");
        assert!(matches!(err, AudioProviderError::UnsupportedFormat(_)));
    }

    #[test]
    fn accepts_empty_data_chunk() {
        let wav = build_wav(WAVE_FORMAT_PCM, 1, 24_000, 16, &[]);
        let decoded = decode_wav(&wav).expect("empty data is valid");
        assert!(decoded.pcm.is_empty());
    }
}
