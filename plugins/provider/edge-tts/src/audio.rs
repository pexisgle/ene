//! MP3 → f32 PCM decoding (nanomp3) and WAV (IEEE float, mono) encoding.

use crate::error::EdgeError;

/// Cap on the WAV payload, matching the host-side decode cap in
/// `ene-plugin-host` (`wav::MAX_WAV_BYTES`), so this plugin never emits a
/// payload the host would reject.
pub const MAX_WAV_BYTES: usize = 32 * 1024 * 1024;

/// Cap on accumulated MP3 bytes per synthesis request; the 48 kbps Edge
/// stream produces ~6 KB/s, so this is far above any legitimate request
/// while bounding what a misbehaving server can make us buffer.
pub const MAX_MP3_BYTES: usize = 16 * 1024 * 1024;

/// Decoded mono PCM.
pub struct DecodedPcm {
    /// Samples normalized to `[-1.0, 1.0]`.
    pub pcm: Vec<f32>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

/// Decodes an MP3 stream to mono f32 PCM.
///
/// # Errors
///
/// Returns [`EdgeError::Decode`] when no MP3 frame decodes, the sample rate
/// changes mid-stream, or the decoded audio exceeds [`MAX_WAV_BYTES`].
pub fn decode_mp3(mp3: &[u8]) -> Result<DecodedPcm, EdgeError> {
    let mut decoder = nanomp3::Decoder::new();
    let mut frame = [0f32; nanomp3::MAX_SAMPLES_PER_FRAME];
    let mut pcm: Vec<f32> = Vec::new();
    let mut sample_rate = None;
    let mut consumed = 0usize;
    while consumed < mp3.len() {
        let (used, info) = decoder.decode(&mp3[consumed..], &mut frame);
        if used == 0 {
            break;
        }
        consumed += used;
        let Some(info) = info else {
            continue;
        };
        if info.samples_produced == 0 {
            continue;
        }
        let rate = *sample_rate.get_or_insert(info.sample_rate);
        if rate != info.sample_rate {
            return Err(EdgeError::Decode(format!(
                "sample rate changed mid-stream: {rate} -> {}",
                info.sample_rate
            )));
        }
        // nanomp3 writes one channel per `samples_produced` for mono and
        // interleaved L/R pairs for stereo (minimp3 semantics).
        match info.channels {
            nanomp3::Channels::Mono => pcm.extend_from_slice(&frame[..info.samples_produced]),
            nanomp3::Channels::Stereo => {
                for pair in frame[..info.samples_produced * 2].chunks_exact(2) {
                    pcm.push((pair[0] + pair[1]) * 0.5);
                }
            }
        }
        if pcm.len() * 4 > MAX_WAV_BYTES {
            return Err(EdgeError::TooLarge { max: MAX_WAV_BYTES });
        }
    }
    let sample_rate =
        sample_rate.ok_or_else(|| EdgeError::Decode("no decodable MP3 frames".to_string()))?;
    Ok(DecodedPcm { pcm, sample_rate })
}

/// Wraps mono f32 PCM in a WAV file (16-byte `fmt` chunk, IEEE float, one
/// channel).
///
/// # Errors
///
/// Returns [`EdgeError::TooLarge`] when the PCM would exceed
/// [`MAX_WAV_BYTES`].
pub fn encode_wav(pcm: &[f32], sample_rate: u32) -> Result<Vec<u8>, EdgeError> {
    let data_len = pcm
        .len()
        .checked_mul(4)
        .ok_or(EdgeError::TooLarge { max: MAX_WAV_BYTES })?;
    if data_len > MAX_WAV_BYTES {
        return Err(EdgeError::TooLarge { max: MAX_WAV_BYTES });
    }
    let mut wav = Vec::with_capacity(44 + data_len);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(
        &u32::try_from(36 + data_len)
            .map_err(|_| EdgeError::TooLarge { max: MAX_WAV_BYTES })?
            .to_le_bytes(),
    );
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&3u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&sample_rate.saturating_mul(4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&32u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(
        &u32::try_from(data_len)
            .map_err(|_| EdgeError::TooLarge { max: MAX_WAV_BYTES })?
            .to_le_bytes(),
    );
    for sample in pcm {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(wav)
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit tests use expect/unwrap for concise assertions"
)]
mod tests {
    use super::*;

    #[test]
    fn encodes_mono_f32_wav_header() {
        let pcm = [0.0, 0.5, -0.5, 1.0];
        let wav = encode_wav(&pcm, 24_000).expect("small wav");
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(wav[16..20].try_into().unwrap()), 16);
        assert_eq!(u16::from_le_bytes(wav[20..22].try_into().unwrap()), 3);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 24_000);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 32);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 16);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 16);
    }

    #[test]
    fn rejects_pcm_over_cap() {
        let pcm = vec![0f32; MAX_WAV_BYTES / 4 + 1];
        let err = encode_wav(&pcm, 24_000).expect_err("over cap");
        assert!(matches!(err, EdgeError::TooLarge { .. }));
    }
}
