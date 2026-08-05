//! Raw 16-bit little-endian PCM helpers.
//!
//! The API's `pcm_*` response formats are headerless streams of mono s16
//! samples. The plugin wire contract carries WAV bytes, so the production
//! path only needs stream-integrity validation: an odd trailing byte means
//! the upstream stream was truncated mid-sample. The full s16 → `f32`
//! conversion is exercised by tests and kept for a future streaming path;
//! converting the whole payload just to validate parity would waste ~2x the
//! audio size per request.

use ene_plugin::{PluginError, ProviderErrorKind};

/// Rejects byte streams that end mid-sample.
///
/// # Errors
///
/// Returns a typed [`Truncated`](ProviderErrorKind::Truncated) provider
/// error when the byte count is odd (the stream ends mid-sample).
pub fn validate_pcm(bytes: &[u8]) -> Result<(), PluginError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(PluginError::provider_typed(
            ProviderErrorKind::Truncated,
            format!(
                "PCM response ends mid-sample: {} bytes is not a multiple of 2",
                bytes.len()
            ),
        ));
    }
    Ok(())
}

/// Converts 2-byte little-endian s16 samples into `f32` values.
///
/// # Errors
///
/// Returns a typed [`Truncated`](ProviderErrorKind::Truncated) provider
/// error when the byte count is odd (the stream ends mid-sample).
#[cfg(test)]
pub fn samples_from_bytes(bytes: &[u8]) -> Result<Vec<f32>, PluginError> {
    validate_pcm(bytes)?;
    // 32768.0 (not 32767.0) keeps the mapping symmetric, so the full-scale
    // negative sample stays inside the documented [-1.0, 1.0] range.
    Ok(bytes
        .chunks_exact(2)
        .map(|sample| f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32_768.0)
        .collect())
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;

    fn to_bytes(samples: &[i16]) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect()
    }

    #[test]
    fn converts_s16_samples_to_f32() {
        let samples = samples_from_bytes(&to_bytes(&[0, 16_384, -16_384, 32_767, -32_768]))
            .expect("even byte count converts");
        assert_eq!(samples.len(), 5);
        assert!(samples[0].abs() < 1e-6);
        assert!((samples[1] - 0.5).abs() < 1e-6);
        assert!((samples[2] + 0.5).abs() < 1e-6);
        assert!((samples[3] - (32_767.0 / 32_768.0)).abs() < 1e-6);
        assert!((samples[4] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn empty_input_converts_to_empty_samples() {
        assert!(
            samples_from_bytes(&[])
                .expect("empty input converts")
                .is_empty()
        );
    }

    #[test]
    fn odd_byte_count_is_typed_truncated() {
        let err = samples_from_bytes(&[0, 0, 1]).expect_err("odd length rejected");
        assert_eq!(
            err.provider_error_kind(),
            Some(ProviderErrorKind::Truncated)
        );
        assert!(err.to_string().contains("mid-sample"));
    }
}
