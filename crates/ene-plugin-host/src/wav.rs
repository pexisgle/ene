//! WAV encode/decode for plugin audio, backed by the `hound` crate.
//!
//! The plugin IPC contract returns whole audio files (base64 `SpeechResult`
//! payloads), while [`ene_ai::TtsProvider`] consumes PCM `f32` chunks, so the
//! host adapter decodes the WAV bytes itself; the STT direction inverts the
//! same shape (f32 PCM → WAV) so microphone audio can ride the existing
//! `TranscribeAudio` wire contract. The codec accepts the formats the built-in
//! plugins emit (PCM s16/s32 or IEEE float, one or two channels; stereo is
//! downmixed to mono) and rejects anything else as
//! [`AudioProviderError::UnsupportedFormat`].

use ene_ai::AudioProviderError;
use hound::{SampleFormat, WavSpec, WavWriter};
use std::io::Cursor;

/// Cap on the WAV byte size `decode_wav` accepts. 24 kHz s16 mono audio is
/// ~2.9 MB per minute, so 32 MiB covers very long utterances while bounding
/// the allocation a misbehaving plugin (or engine) can force on the host.
///
/// The bound must also fit the IPC frame cap after base64 expansion
/// (`MAX_WAV_BYTES * 4/3 < 64 MiB`); the base64 pre-check in the host adapter
/// is only reachable for payloads that fit on the wire.
pub const MAX_WAV_BYTES: usize = 32 * 1024 * 1024;

/// Decoded PCM audio.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedWav {
    /// Interleaved mono PCM samples normalized to `[-1.0, 1.0]`.
    pub pcm: Vec<f32>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

/// Encodes mono `f32` PCM into a 16-bit PCM RIFF/WAVE byte stream.
///
/// Used by the STT adapter to carry microphone audio over the plugin IPC
/// (whose `TranscribeAudio` request takes a whole audio file, like the TTS
/// direction). Samples are clamped to `[-1.0, 1.0]` before scaling.
///
/// # Errors
///
/// Returns [`AudioProviderError::Provider`] if the WAV header or a sample
/// cannot be written (only possible with a corrupt in-memory writer).
pub fn encode_wav(pcm: &[f32], sample_rate: u32) -> Result<Vec<u8>, AudioProviderError> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut out = Cursor::new(Vec::with_capacity(44 + pcm.len() * 2));
    {
        let mut writer = WavWriter::new(&mut out, spec)
            .map_err(|e| AudioProviderError::Provider(format!("WAV header write failed: {e}")))?;
        for &sample in pcm {
            let scaled = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
            writer.write_sample(scaled).map_err(|e| {
                AudioProviderError::Provider(format!("WAV sample write failed: {e}"))
            })?;
        }
        writer
            .finalize()
            .map_err(|e| AudioProviderError::Provider(format!("WAV finalize failed: {e}")))?;
    }
    Ok(out.into_inner())
}

/// Parses a RIFF/WAVE byte stream into mono PCM `f32` samples.
///
/// # Errors
///
/// Returns [`AudioProviderError::UnsupportedFormat`] when the bytes are not
/// a well-formed WAV file in one of the supported encodings, and
/// [`AudioProviderError::PayloadTooLarge`] when the payload exceeds
/// [`MAX_WAV_BYTES`].
pub fn decode_wav(bytes: &[u8]) -> Result<DecodedWav, AudioProviderError> {
    if bytes.len() > MAX_WAV_BYTES {
        return Err(AudioProviderError::PayloadTooLarge {
            max_bytes: MAX_WAV_BYTES,
            actual: bytes.len(),
        });
    }

    let reader = hound::WavReader::new(Cursor::new(bytes))
        .map_err(|e| AudioProviderError::UnsupportedFormat(format!("invalid WAV header: {e}")))?;
    let spec = reader.spec();
    if spec.sample_rate == 0 {
        return Err(AudioProviderError::UnsupportedFormat(
            "WAV sample rate is zero".to_string(),
        ));
    }
    if !matches!(spec.channels, 1 | 2) {
        return Err(AudioProviderError::UnsupportedFormat(format!(
            "unsupported WAV channel count: {}",
            spec.channels
        )));
    }

    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Int, 16) => reader
            .into_samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| map_decode_error(&e))?
            .into_iter()
            .map(|s| f32::from(s) / f32::from(i16::MAX))
            .collect(),
        (SampleFormat::Int, 32) => reader
            .into_samples::<i32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| map_decode_error(&e))?
            .into_iter()
            .map(|s| s as f32 / i32::MAX as f32)
            .collect(),
        (SampleFormat::Float, 32) => reader
            .into_samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| map_decode_error(&e))?,
        _ => {
            return Err(AudioProviderError::UnsupportedFormat(format!(
                "unsupported WAV encoding: {} at {} bits",
                format_tag(spec.sample_format),
                spec.bits_per_sample
            )));
        }
    };

    let pcm = if spec.channels == 2 {
        samples
            .chunks_exact(2)
            .map(|pair| (pair[0] + pair[1]) * 0.5)
            .collect()
    } else {
        samples
    };

    Ok(DecodedWav {
        pcm,
        sample_rate: spec.sample_rate,
    })
}

fn format_tag(format: SampleFormat) -> &'static str {
    match format {
        SampleFormat::Int => "PCM",
        SampleFormat::Float => "IEEE float",
    }
}

fn map_decode_error(e: &hound::Error) -> AudioProviderError {
    AudioProviderError::UnsupportedFormat(format!("invalid WAV data: {e}"))
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests use expect for concise assertions"
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

    const WAVE_FORMAT_PCM: u16 = 1;
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

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
    fn encode_wav_roundtrips_through_decode() {
        let pcm = vec![0.0, 0.5, -0.5, 1.0, -1.0];
        let bytes = encode_wav(&pcm, 16_000).expect("valid encode");
        let decoded = decode_wav(&bytes).expect("encoded wav decodes");
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.pcm.len(), pcm.len());
        for (actual, expected) in decoded.pcm.iter().zip(&pcm) {
            assert!(
                (actual - expected).abs() < 1e-4,
                "got {actual}, want {expected}"
            );
        }
    }

    #[test]
    fn encode_wav_clamps_out_of_range_samples() {
        let bytes = encode_wav(&[2.0, -2.0], 16_000).expect("valid encode");
        let decoded = decode_wav(&bytes).expect("encoded wav decodes");
        assert!((decoded.pcm[0] - 1.0).abs() < 1e-4);
        assert!((decoded.pcm[1] + 1.0).abs() < 1e-4);
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

    #[test]
    fn rejects_oversized_payloads() {
        let mut wav = build_wav(WAVE_FORMAT_PCM, 1, 24_000, 16, &[0, 1]);
        wav.resize(MAX_WAV_BYTES + 1, 0);
        let err = decode_wav(&wav).expect_err("oversized payload rejected");
        assert!(matches!(err, AudioProviderError::PayloadTooLarge { .. }));
    }
}
