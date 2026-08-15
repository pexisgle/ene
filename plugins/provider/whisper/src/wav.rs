//! WAV decoding for the audio files the host sends over the
//! `TranscribeAudio` wire contract, backed by `hound`.
//!
//! The host adapter encodes microphone PCM as 16-bit mono WAV (the format
//! this plugin declares), but the decoder also accepts the other formats the
//! host's own decoder produces (s32 PCM, IEEE float) plus a few defensive
//! rejects.

use ene_plugin::PluginError;
use hound::SampleFormat;
use std::io::Cursor;

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
    let reader = hound::WavReader::new(Cursor::new(bytes))
        .map_err(|e| PluginError::provider(format!("STT audio is not a RIFF/WAVE stream: {e}")))?;
    let spec = reader.spec();
    if spec.sample_rate == 0 {
        return Err(PluginError::provider("WAV sample rate is zero".to_string()));
    }
    if !matches!(spec.channels, 1 | 2) {
        return Err(PluginError::provider(format!(
            "unsupported WAV channel count: {}",
            spec.channels
        )));
    }

    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Int, 16) => reader
            .into_samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PluginError::provider(format!("truncated WAV stream: {e}")))?
            .into_iter()
            .map(|s| f32::from(s) / f32::from(i16::MAX))
            .collect(),
        (SampleFormat::Int, 32) => reader
            .into_samples::<i32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PluginError::provider(format!("truncated WAV stream: {e}")))?
            .into_iter()
            .map(|s| s as f32 / i32::MAX as f32)
            .collect(),
        (SampleFormat::Float, 32) => reader
            .into_samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| PluginError::provider(format!("truncated WAV stream: {e}")))?,
        _ => {
            return Err(PluginError::provider(format!(
                "unsupported WAV encoding: {} at {} bits",
                match spec.sample_format {
                    SampleFormat::Int => "PCM",
                    SampleFormat::Float => "IEEE float",
                },
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

/// A valid 16 kHz mono s16 WAV fixture (4 zero samples), shared with the
/// plugin-level tests.
#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test fixture uses expect for concise assertions"
)]
pub(crate) fn decode_wav_test_fixture() -> Vec<u8> {
    use hound::{WavSpec, WavWriter};

    let spec = WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut out = Cursor::new(Vec::new());
    let mut writer = WavWriter::new(&mut out, spec).expect("valid spec");
    for _ in 0..4 {
        writer.write_sample(0i16).expect("write sample");
    }
    writer.finalize().expect("finalize");
    out.into_inner()
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;
    use hound::{WavSpec, WavWriter};

    fn build_wav(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
        let spec = WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut out = Cursor::new(Vec::new());
        let mut writer = WavWriter::new(&mut out, spec).expect("valid spec");
        for &sample in samples {
            writer.write_sample(sample).expect("write sample");
        }
        writer.finalize().expect("finalize");
        out.into_inner()
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
