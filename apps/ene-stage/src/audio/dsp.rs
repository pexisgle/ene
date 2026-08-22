//! Mic DSP shared by capture and tests: RMS, resample, echo-aware gate, coalesce.

use ene_api::LISTEN_SAMPLE_RATE;

/// RMS below this is silence for the echo-aware barge-in gate.
pub const SILENCE_RMS: f32 = 0.01;

/// Multiplier on [`SILENCE_RMS`] while local TTS is playing (speaker bleed).
pub const BARGE_IN_ENERGY_FACTOR: f32 = 2.0;

/// Target capture rate after resampling (`ene_api::LISTEN_SAMPLE_RATE`).
pub const CAPTURE_SAMPLE_RATE: u32 = LISTEN_SAMPLE_RATE;

/// ~100 ms at 16 kHz — one listen-stream frame.
pub const LISTEN_FRAME_SAMPLES: usize = 1_600;

/// Carries a fractional input position across callbacks so the output
/// stream stays continuous.
pub struct Resampler {
    ratio: f64,
    pos: f64,
}

impl Resampler {
    #[must_use]
    pub fn new(src_rate: u32) -> Self {
        let ratio = f64::from(src_rate) / f64::from(CAPTURE_SAMPLE_RATE);
        Self {
            ratio: ratio.max(f64::from(f32::EPSILON)),
            pos: 0.0,
        }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        let len = input.len() as f64;
        while self.pos < len {
            let idx_f = self.pos.floor();
            let frac = (self.pos - idx_f) as f32;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "idx_f is non-negative and < input.len()"
            )]
            let idx = idx_f as usize;
            let a = input[idx];
            let b = if idx + 1 < input.len() {
                input[idx + 1]
            } else {
                a
            };
            out.push(a + (b - a) * frac);
            self.pos += self.ratio;
        }
        self.pos -= len;
    }
}

#[must_use]
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|sample| sample * sample).sum();
    (sum / samples.len() as f32).sqrt()
}

/// While TTS plays, drop frames quieter than speaker bleed. Idle frames
/// (including silence) still go to core VAD so utterances can close.
#[must_use]
pub fn should_forward_mic(pcm: &[f32], tts_playing: bool) -> bool {
    if !tts_playing {
        return true;
    }
    rms(pcm) >= SILENCE_RMS * BARGE_IN_ENERGY_FACTOR
}

pub fn push_coalesced(
    pending: &mut Vec<f32>,
    chunk: &[f32],
    frame_samples: usize,
    out: &mut Vec<Vec<f32>>,
) {
    if frame_samples == 0 {
        return;
    }
    pending.extend_from_slice(chunk);
    while pending.len() >= frame_samples {
        let rest = pending.split_off(frame_samples);
        out.push(std::mem::replace(pending, rest));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BARGE_IN_ENERGY_FACTOR, CAPTURE_SAMPLE_RATE, LISTEN_FRAME_SAMPLES, Resampler, SILENCE_RMS,
        push_coalesced, rms, should_forward_mic,
    };

    #[test]
    fn resampler_downsamples_by_ratio() {
        let mut resampler = Resampler::new(48_000);
        let mut out = Vec::new();
        let input = vec![0.0_f32; 4800];
        resampler.process(&input, &mut out);
        assert!((1590..=1610).contains(&out.len()), "got {}", out.len());
    }

    #[test]
    fn resampler_passthrough_at_target_rate() {
        let mut resampler = Resampler::new(CAPTURE_SAMPLE_RATE);
        let mut out = Vec::new();
        let input = vec![0.5_f32; 1600];
        resampler.process(&input, &mut out);
        assert!((1590..=1600).contains(&out.len()), "got {}", out.len());
    }

    #[test]
    fn resampler_empty_input_is_noop() {
        let mut resampler = Resampler::new(48_000);
        let mut out = Vec::new();
        resampler.process(&[], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn resampler_interpolates_forward() {
        let mut resampler = Resampler::new(24_000);
        let mut out = Vec::new();
        let input = vec![0.0_f32, 1.0, 2.0, 3.0];
        resampler.process(&input, &mut out);
        assert!(!out.is_empty());
        assert!((out[0] - 0.0).abs() < 1e-6, "first sample: {}", out[0]);
        assert!(
            out.len() > 1,
            "expected at least 2 samples, got {}",
            out.len()
        );
        assert!((out[1] - 1.5).abs() < 1e-6, "second sample: {}", out[1]);
    }

    #[test]
    fn rms_energy_computes_correctly() {
        assert!((rms(&[0.0; 100]) - 0.0).abs() < 1e-9);
        assert!((rms(&[0.5; 100]) - 0.5).abs() < 1e-6);
        assert!((rms(&[]) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn echo_aware_gate_drops_playback_bleed() {
        let bleed = vec![SILENCE_RMS * 1.2; 320];
        assert!(should_forward_mic(&bleed, false));
        assert!(!should_forward_mic(&bleed, true));
        let speech = vec![SILENCE_RMS * BARGE_IN_ENERGY_FACTOR * 1.1; 320];
        assert!(should_forward_mic(&speech, true));
    }

    #[test]
    fn idle_silence_is_forwarded_for_vad() {
        assert!(should_forward_mic(&[0.0; 160], false));
    }

    #[test]
    fn coalescer_emits_fixed_frames() {
        let mut pending = Vec::new();
        let mut out = Vec::new();
        push_coalesced(&mut pending, &[0.1; 1000], LISTEN_FRAME_SAMPLES, &mut out);
        assert!(out.is_empty());
        push_coalesced(&mut pending, &[0.2; 1000], LISTEN_FRAME_SAMPLES, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), LISTEN_FRAME_SAMPLES);
        assert_eq!(pending.len(), 400);
    }
}
