//! Minimal RIFF/WAVE encoder for plugin-delivered TTS audio.
//!
//! Kokoro produces `f32` PCM; the host adapter decodes plugin audio as WAV
//! (`ene-plugin-host` `wav::decode_wav`), so samples are converted to 16-bit
//! mono PCM and wrapped in a header — the same wire shape the openai-tts
//! plugin emits.

use ene_plugin::PluginError;

/// Canonical 16-bit mono PCM WAV header size.
const WAV_HEADER_LEN: usize = 44;

/// Wraps normalized `f32` PCM in a 16-bit mono RIFF/WAVE container.
///
/// Samples are clamped to `[-1.0, 1.0]` and scaled to `i16` range; the
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
    let riff_len = u32::try_from(u64::from(data_len) + 36)
        .map_err(|_| PluginError::provider("PCM buffer too large for a WAV RIFF chunk"))?;
    let byte_rate = sample_rate.checked_mul(2).ok_or_else(|| {
        PluginError::provider("sample rate is too large for a 16-bit WAV byte-rate field")
    })?;

    let mut wav = Vec::with_capacity(WAV_HEADER_LEN + pcm.len() * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&riff_len.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for &sample in pcm {
        let scaled = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        wav.extend_from_slice(&scaled.to_le_bytes());
    }
    Ok(wav)
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;

    fn u16_at(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    }

    fn u32_at(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }

    #[test]
    fn wraps_pcm_with_canonical_header() {
        let pcm = vec![0.0, 0.5, 1.0, -1.0, 2.0, -2.0];
        let wav = encode_wav(&pcm, 24_000).expect("valid sample rate");
        assert_eq!(wav.len(), WAV_HEADER_LEN + pcm.len() * 2);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32_at(&wav, 4), 36 + pcm.len() as u32 * 2);
        assert_eq!(u16_at(&wav, 20), 1);
        assert_eq!(u16_at(&wav, 22), 1);
        assert_eq!(u32_at(&wav, 24), 24_000);
        assert_eq!(u32_at(&wav, 28), 48_000);
        assert_eq!(u16_at(&wav, 32), 2);
        assert_eq!(u16_at(&wav, 34), 16);
        assert_eq!(u32_at(&wav, 40), pcm.len() as u32 * 2);
    }

    #[test]
    fn samples_are_clamped_and_scaled_to_i16() {
        let pcm = vec![0.0, 0.5, 1.0, -1.0, 2.0, -2.0];
        let wav = encode_wav(&pcm, 24_000).expect("valid sample rate");
        let samples: Vec<i16> = wav[WAV_HEADER_LEN..]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(samples, vec![0, 16384, 32767, -32767, 32767, -32767]);
    }

    #[test]
    fn empty_pcm_yields_header_only_wav() {
        let wav = encode_wav(&[], 24_000).expect("empty PCM encodes");
        assert_eq!(wav.len(), WAV_HEADER_LEN);
        assert_eq!(u32_at(&wav, 40), 0);
    }
}
