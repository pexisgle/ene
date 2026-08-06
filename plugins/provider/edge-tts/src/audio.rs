//! MP3 → f32 PCM decoding (nanomp3) and WAV (IEEE float, mono) encoding.

use crate::error::EdgeError;

/// Cap on the WAV payload, matching the host-side decode cap in
/// `ene-plugin-host` (`wav::MAX_WAV_BYTES`), so this plugin never emits a
/// payload the host would reject.
pub const MAX_WAV_BYTES: usize = 32 * 1024 * 1024;
/// RIFF/WAVE header size; counts against [`MAX_WAV_BYTES`] so the emitted
/// file never exceeds the host's cap. `hound` writes a 68-byte header for
/// 32-bit IEEE float (12 RIFF + 8+40 extensible fmt + 8 data).
const WAV_HEADER_BYTES: usize = 68;

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
        if pcm.len().saturating_mul(4).saturating_add(WAV_HEADER_BYTES) > MAX_WAV_BYTES {
            return Err(EdgeError::TooLarge { max: MAX_WAV_BYTES });
        }
    }
    let sample_rate =
        sample_rate.ok_or_else(|| EdgeError::Decode("no decodable MP3 frames".to_string()))?;
    Ok(DecodedPcm { pcm, sample_rate })
}

/// Wraps mono f32 PCM in a WAV file (IEEE float, one channel).
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
    if data_len.saturating_add(WAV_HEADER_BYTES) > MAX_WAV_BYTES {
        return Err(EdgeError::TooLarge { max: MAX_WAV_BYTES });
    }

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut out = std::io::Cursor::new(Vec::with_capacity(WAV_HEADER_BYTES + data_len));
    {
        let mut writer = hound::WavWriter::new(&mut out, spec)
            .map_err(|e| EdgeError::Decode(format!("WAV header write failed: {e}")))?;
        for &sample in pcm {
            writer
                .write_sample(sample)
                .map_err(|e| EdgeError::Decode(format!("WAV sample write failed: {e}")))?;
        }
        writer
            .finalize()
            .map_err(|e| EdgeError::Decode(format!("WAV finalize failed: {e}")))?;
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
    fn encodes_mono_f32_wav_header() {
        let pcm = [0.0, 0.5, -0.5, 1.0];
        let wav = encode_wav(&pcm, 24_000).expect("small wav");
        let reader = hound::WavReader::new(std::io::Cursor::new(&wav)).expect("valid wav");
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 24_000);
        assert_eq!(spec.bits_per_sample, 32);
        assert_eq!(spec.sample_format, hound::SampleFormat::Float);
        let samples: Vec<f32> = reader
            .into_samples::<f32>()
            .collect::<Result<_, _>>()
            .expect("samples read");
        assert_eq!(samples, pcm);
    }

    #[test]
    fn rejects_pcm_over_cap() {
        let pcm = vec![0f32; MAX_WAV_BYTES / 4 + 1];
        let err = encode_wav(&pcm, 24_000).expect_err("over cap");
        assert!(matches!(err, EdgeError::TooLarge { .. }));
    }

    #[test]
    fn allows_wav_up_to_the_exact_host_cap() {
        let pcm = vec![0f32; (MAX_WAV_BYTES - WAV_HEADER_BYTES) / 4];
        let wav = encode_wav(&pcm, 24_000).expect("at cap");
        assert_eq!(wav.len(), MAX_WAV_BYTES);
        let pcm = vec![0f32; (MAX_WAV_BYTES - WAV_HEADER_BYTES) / 4 + 1];
        let err = encode_wav(&pcm, 24_000).expect_err("over cap");
        assert!(matches!(err, EdgeError::TooLarge { .. }));
    }
}
