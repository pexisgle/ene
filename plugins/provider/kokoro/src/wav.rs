//! WAV encoding for plugin-delivered TTS audio, backed by `hound`.
//!
//! Kokoro produces `f32` PCM; the host adapter decodes plugin audio as WAV
//! (`ene-plugin-host` `wav::decode_wav`), so samples are converted to 16-bit
//! mono PCM and wrapped in a header — the same wire shape the openai-tts
//! plugin emits.

use ene_plugin::PluginError;
use hound::{SampleFormat, WavSpec, WavWriter};
use std::io::Cursor;

/// Wraps normalized `f32` PCM in a 16-bit mono RIFF/WAVE container.
///
/// Samples are clamped to `[-1.0, 1.0]` and rounded to `i16` range; the
/// resulting byte length must fit the RIFF `u32` size fields.
///
/// # Errors
///
/// Returns a provider error when the PCM buffer or the byte rate is too
/// large for the WAV header fields.
pub fn encode_wav(pcm: &[f32], sample_rate: u32) -> Result<Vec<u8>, PluginError> {
    let data_len = u32::try_from(
        pcm.len()
            .checked_mul(2)
            .ok_or_else(|| PluginError::provider("PCM buffer too large for a WAV data chunk"))?,
    )
    .map_err(|_| PluginError::provider("PCM buffer too large for a WAV data chunk"))?;
    sample_rate.checked_mul(2).ok_or_else(|| {
        PluginError::provider("sample rate is too large for a 16-bit WAV byte-rate field")
    })?;

    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut out = Cursor::new(Vec::with_capacity(44 + data_len as usize));
    {
        let mut writer = WavWriter::new(&mut out, spec)
            .map_err(|e| PluginError::provider(format!("WAV header write failed: {e}")))?;
        for &sample in pcm {
            let scaled = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
            writer
                .write_sample(scaled)
                .map_err(|e| PluginError::provider(format!("WAV sample write failed: {e}")))?;
        }
        writer
            .finalize()
            .map_err(|e| PluginError::provider(format!("WAV finalize failed: {e}")))?;
    }
    Ok(out.into_inner())
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;

    #[test]
    fn wraps_pcm_with_canonical_header() {
        let pcm = vec![0.0, 0.5, 1.0, -1.0, 2.0, -2.0];
        let wav = encode_wav(&pcm, 24_000).expect("valid sample rate");
        let reader = hound::WavReader::new(Cursor::new(&wav)).expect("valid wav");
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 24_000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, SampleFormat::Int);
        let samples: Vec<i16> = reader
            .into_samples::<i16>()
            .collect::<Result<_, _>>()
            .expect("samples read");
        assert_eq!(samples, vec![0, 16384, 32767, -32767, 32767, -32767]);
    }

    #[test]
    fn empty_pcm_yields_header_only_wav() {
        let wav = encode_wav(&[], 24_000).expect("empty PCM encodes");
        assert_eq!(wav.len(), 44);
    }
}
