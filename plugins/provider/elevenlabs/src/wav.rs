//! WAV wrapping for plugin-delivered TTS audio, backed by `hound`.
//!
//! The host-side TTS adapter decodes plugin audio as WAV (`ene-plugin-host`
//! `wav::decode_wav`), so the raw PCM stream from the API is wrapped in a
//! header here instead of sent headerless.

use ene_plugin::PluginError;
use hound::{SampleFormat, WavSpec, WavWriter};
use std::io::Cursor;

/// Wraps raw 16-bit mono PCM (little-endian, interleaved for stereo) in a
/// RIFF/WAVE container.
///
/// `pcm.len()` must fit in `u32`; the client caps payloads at
/// [`crate::client::MAX_PCM_BYTES`], far below that bound.
#[must_use = "the wrapped WAV result must be handled"]
pub fn wrap_pcm(pcm: &[u8], sample_rate: u32) -> Result<Vec<u8>, PluginError> {
    let data_len = pcm.len() as u32;
    sample_rate.checked_mul(2).ok_or_else(|| {
        PluginError::provider("sample rate is too large for a 16-bit WAV byte-rate field")
    })?;

    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut out = Cursor::new(Vec::with_capacity(44 + pcm.len()));
    {
        let mut writer = WavWriter::new(&mut out, spec)
            .map_err(|e| PluginError::provider(format!("WAV header write failed: {e}")))?;
        for pair in pcm.chunks_exact(2) {
            let sample = i16::from_le_bytes([pair[0], pair[1]]);
            writer
                .write_sample(sample)
                .map_err(|e| PluginError::provider(format!("WAV sample write failed: {e}")))?;
        }
        writer
            .finalize()
            .map_err(|e| PluginError::provider(format!("WAV finalize failed: {e}")))?;
    }
    let wav = out.into_inner();
    debug_assert_eq!(wav.len(), 44 + data_len as usize);
    Ok(wav)
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;
    use hound::SampleFormat;

    #[test]
    fn wraps_pcm_with_canonical_header() {
        let pcm = vec![0u8, 1, 2, 3];
        let wav = wrap_pcm(&pcm, 24_000).expect("valid sample rate");
        assert_eq!(wav.len(), 44 + pcm.len());
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
        assert_eq!(samples, vec![256, 770]);
    }
}
