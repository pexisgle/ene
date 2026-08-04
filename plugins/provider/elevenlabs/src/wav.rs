//! Minimal RIFF/WAVE encoder for plugin-delivered TTS audio.
//!
//! The host-side TTS adapter decodes plugin audio as WAV (`ene-plugin-host`
//! `wav::decode_wav`), so the raw PCM stream from the API is wrapped in a
//! header here instead of sent headerless.

use ene_plugin::PluginError;

/// Canonical 16-bit mono PCM WAV header size.
const WAV_HEADER_LEN: usize = 44;

/// Wraps raw 16-bit mono PCM in a RIFF/WAVE container.
///
/// `pcm.len()` must fit in `u32`; the client caps payloads at
/// [`crate::client::MAX_PCM_BYTES`], far below that bound.
#[must_use = "the wrapped WAV result must be handled"]
pub fn wrap_pcm(pcm: &[u8], sample_rate: u32) -> Result<Vec<u8>, PluginError> {
    let data_len = pcm.len() as u32;
    let byte_rate = sample_rate.checked_mul(2).ok_or_else(|| {
        PluginError::provider("sample rate is too large for a 16-bit WAV byte-rate field")
    })?;
    let mut wav = Vec::with_capacity(WAV_HEADER_LEN + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
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
    wav.extend_from_slice(pcm);
    Ok(wav)
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
        let pcm = vec![0u8, 1, 2, 3];
        let wav = wrap_pcm(&pcm, 24_000).expect("valid sample rate");
        assert_eq!(wav.len(), WAV_HEADER_LEN + pcm.len());
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]), 36 + 4);
        assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1);
        assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1);
        assert_eq!(
            u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
            24_000
        );
        assert_eq!(
            u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]),
            48_000
        );
        assert_eq!(u16::from_le_bytes([wav[32], wav[33]]), 2);
        assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
        assert_eq!(
            u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]),
            pcm.len() as u32
        );
        assert_eq!(&wav[WAV_HEADER_LEN..], pcm.as_slice());
    }
}
