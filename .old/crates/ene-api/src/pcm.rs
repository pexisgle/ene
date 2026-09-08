//! Packed PCM for the listen WebSocket. JSON `f32` arrays stay on `POST /listen`.

use crate::error::ApiError;

/// Little-endian signed 16-bit mono encoding used on `listen/stream`.
pub const PCM_S16LE: &str = "pcm_s16le";

/// Sample rate stage resamples to before streaming (Silero / STT convention).
pub const LISTEN_SAMPLE_RATE: u32 = 16_000;

/// Pack mono `f32` (`-1.0..=1.0`) as little-endian `i16`.
#[must_use]
pub fn encode_pcm_s16le(pcm: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len().saturating_mul(2));
    for sample in pcm {
        let clamped = sample.clamp(-1.0, 1.0);
        let scaled = (clamped * 32_767.0).round();
        let int = if scaled >= 32_767.0 {
            i16::MAX
        } else if scaled <= -32_768.0 {
            i16::MIN
        } else {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "value is clamped to the i16 range"
            )]
            {
                scaled as i16
            }
        };
        out.extend_from_slice(&int.to_le_bytes());
    }
    out
}

/// Unpack little-endian `i16` mono into `f32` (`-1.0..=1.0`).
///
/// # Errors
///
/// Returns [`ApiError::Codec`] when `bytes` is not an even length.
pub fn decode_pcm_s16le(bytes: &[u8]) -> Result<Vec<f32>, ApiError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(ApiError::Codec(
            "pcm_s16le length must be a multiple of 2".to_owned(),
        ));
    }
    let mut pcm = Vec::with_capacity(bytes.len() / 2);
    let (chunks, _) = bytes.as_chunks::<2>();
    for chunk in chunks {
        let int = i16::from_le_bytes(*chunk);
        pcm.push(f32::from(int) / 32_768.0);
    }
    Ok(pcm)
}

#[cfg(test)]
mod tests {
    use super::{decode_pcm_s16le, encode_pcm_s16le};

    #[test]
    fn roundtrip_preserves_silence_and_peak() {
        let src = [0.0_f32, 0.5, -0.5, 1.0, -1.0];
        let encoded = encode_pcm_s16le(&src);
        assert_eq!(encoded.len(), src.len() * 2);
        let decoded = decode_pcm_s16le(&encoded).expect("even length");
        assert_eq!(decoded.len(), src.len());
        for (got, want) in decoded.iter().zip(src.iter()) {
            assert!((got - want).abs() < 0.001, "got {got} want {want}");
        }
    }

    #[test]
    fn odd_length_is_codec_error() {
        let err = decode_pcm_s16le(&[0, 1, 2]).expect_err("odd");
        assert_eq!(err.error_class(), "codec");
    }

    #[test]
    fn empty_pcm_roundtrips_to_empty_bytes() {
        assert!(encode_pcm_s16le(&[]).is_empty());
        assert_eq!(decode_pcm_s16le(&[]).expect("empty"), Vec::<f32>::new());
    }
}
