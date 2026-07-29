//! Audio-driven viseme (lip-sync) analysis.
//!
//! This module turns a stream of PCM audio samples into the five
//! mouth-shape blend-shape weights used by the VRM procedural
//! lip-sync targets (`aa`, `ih`, `ou`, `ee`, `oh` — see
//! [`crate::expression_override::MOUTH_TARGET_NAMES`]).
//!
//! The analysis is pure digital signal processing with no audio
//! I/O, which keeps `ene-vrm` a rendering-only crate:
//!
//! - **RMS amplitude** drives the overall mouth openness, so silence
//!   collapses every weight toward zero.
//! - **Zero-crossing rate** discriminates rounded vowels (low rate)
//!   from spread vowels (high rate).
//! - A small **FFT** splits each frame into frequency bands so the
//!   analyzer can tell open / low-mid vowels (`aa`, `oh`) apart from
//!   high-frequency spread vowels (`ih`, `ee`).
//!
//! The raw per-frame estimates are run through an asymmetric
//! exponential moving average (fast attack, slower release) to
//! suppress jitter while still tracking speech.
//!
//! Feed samples with [`VisemeAnalyzer::push_pcm`], then call
//! [`VisemeAnalyzer::analyze`] once per render frame and hand the
//! result to [`crate::expression::ExpressionLayer::apply_viseme_weights`].

use std::collections::VecDeque;
use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

/// Analysis frame length in seconds (20 ms).
const FRAME_SECONDS: f32 = 0.020;
/// Minimum FFT window length, regardless of sample rate.
const MIN_WINDOW: usize = 64;
/// Fewer buffered samples than this is treated as "no signal yet".
const MIN_SAMPLES: usize = 4;
/// RMS below this is considered silence (noise gate).
const NOISE_GATE: f32 = 0.005;
/// RMS at which the mouth is considered fully open.
const AMP_FULL: f32 = 0.2;
/// Zero-crossing rate below this counts as "low".
const ZCR_LOW: f32 = 0.04;
/// Zero-crossing rate above this counts as "high".
const ZCR_HIGH: f32 = 0.2;
/// EMA attack coefficient (moving toward a louder target).
const ATTACK: f32 = 0.6;
/// EMA release coefficient (moving toward a quieter target).
const RELEASE: f32 = 0.35;
/// Energy / sum floor used to avoid division by zero.
const EPS: f32 = 1e-6;

/// Upper edge of the low band (Hz). Low band is `[0, 350)`.
const BAND_LOW_MAX: f32 = 350.0;
/// Upper edge of the low-mid band (Hz). Low-mid band is `[350, 900)`.
const BAND_LOW_MID_MAX: f32 = 900.0;
/// Upper edge of the mid band (Hz). Mid band is `[900, 1800)`.
const BAND_MID_MAX: f32 = 1800.0;
/// Upper edge of the mid-high band (Hz). Mid-high is `[1800, 3200)`,
/// everything above is the high band.
const BAND_MID_HIGH_MAX: f32 = 3200.0;

/// Per-frame mouth-shape weights for the five VRM procedural
/// lip-sync targets.
///
/// Every field is in `[0, 1]`. The default is all zeros (mouth
/// closed).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VisemeWeights {
    /// Open mouth ("aa" as in "father").
    pub aa: f32,
    /// Spread / smile ("ih" as in "bit").
    pub ih: f32,
    /// Rounded ("ou" as in "boot").
    pub ou: f32,
    /// Wide ("ee" as in "beet").
    pub ee: f32,
    /// Small round ("oh" as in "boat").
    pub oh: f32,
}

/// Streaming PCM-to-viseme analyzer.
///
/// Feed audio with [`push_pcm`](Self::push_pcm) and read the smoothed
/// mouth shape with [`analyze`](Self::analyze) once per render frame.
/// The analyzer keeps an internal ring buffer sized to a single
/// analysis window (about 20 ms at the configured sample rate) and
/// performs no audio I/O — the host is responsible for capturing and
/// scheduling the samples.
pub struct VisemeAnalyzer {
    sample_rate: u32,
    window_size: usize,
    buffer: VecDeque<f32>,
    smoothed: VisemeWeights,
    fft: Arc<dyn Fft<f32>>,
    frame: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl VisemeAnalyzer {
    /// Build an analyzer for the given `sample_rate` (Hz).
    ///
    /// The analysis window is sized to roughly 20 ms of audio and
    /// rounded up to the next power of two so the FFT stays cheap.
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        let raw = (sample_rate as f32 * FRAME_SECONDS).ceil() as usize;
        let window_size = raw.max(MIN_WINDOW).next_power_of_two();
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(window_size);
        // Size scratch to at least what the FFT algorithm requires.
        // `get_inplace_scratch_len()` reports the true requirement;
        // we take the max with `window_size` as a safety floor.
        let scratch_len = window_size.max(fft.get_inplace_scratch_len());
        Self {
            sample_rate,
            window_size,
            buffer: VecDeque::with_capacity(window_size),
            smoothed: VisemeWeights::default(),
            fft,
            frame: vec![Complex::new(0.0, 0.0); window_size],
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
        }
    }

    /// Number of PCM samples held in one analysis window.
    ///
    /// Feeding at least this many samples guarantees the analyzer
    /// sees a full frame; feeding more simply discards the oldest
    /// samples.
    #[must_use]
    pub fn window_size(&self) -> usize {
        self.window_size
    }

    /// Append PCM samples (mono, `[-1, 1]`) to the internal ring
    /// buffer.
    ///
    /// Samples beyond the analysis window are dropped oldest-first,
    /// so the buffer always holds the most recent
    /// [`window_size`](Self::window_size) samples.
    pub fn push_pcm(&mut self, pcm: &[f32]) {
        self.buffer.extend(pcm.iter().copied());
        let excess = self.buffer.len().saturating_sub(self.window_size);
        if excess > 0 {
            self.buffer.drain(0..excess);
        }
    }

    /// Analyze the buffered audio and return the smoothed mouth-shape
    /// weights.
    ///
    /// Intended to be called once per render frame. With too few
    /// buffered samples the weights simply decay toward zero.
    pub fn analyze(&mut self) -> VisemeWeights {
        let count = self.buffer.len();
        if count < MIN_SAMPLES {
            self.smoothed = smooth_weights(self.smoothed, VisemeWeights::default());
            return self.smoothed;
        }

        let mut sum_sq = 0.0f32;
        let mut crossings = 0usize;
        let mut prev = 0.0f32;
        for (idx, &sample) in self.buffer.iter().enumerate() {
            sum_sq += sample * sample;
            if idx > 0 && ((prev < 0.0) != (sample < 0.0)) {
                crossings += 1;
            }
            prev = sample;
        }
        let rms = (sum_sq / count as f32).sqrt();
        let zcr = crossings as f32 / count.saturating_sub(1).max(1) as f32;

        let window_len = self.window_size;
        for idx in 0..window_len {
            let sample = if idx < count { self.buffer[idx] } else { 0.0 };
            self.frame[idx] = Complex::new(sample * hann(idx, window_len), 0.0);
        }

        self.fft
            .process_with_scratch(&mut self.frame, &mut self.scratch);

        let bands = band_fractions(&self.frame, self.sample_rate, window_len);
        let target = target_weights(rms, zcr, bands);
        self.smoothed = smooth_weights(self.smoothed, target);
        self.smoothed
    }

    /// Clear the ring buffer and reset the smoothed weights to zero.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.smoothed = VisemeWeights::default();
    }
}

/// Hann window coefficient for sample `idx` of a `len`-sample frame.
fn hann(idx: usize, len: usize) -> f32 {
    if len <= 1 {
        return 1.0;
    }
    let phase = 2.0 * std::f32::consts::PI * idx as f32 / (len - 1) as f32;
    0.5 - 0.5 * phase.cos()
}

/// Split the FFT magnitude spectrum into five normalized frequency
/// bands: `[low, low-mid, mid, mid-high, high]`. The returned
/// fractions sum to `1` (or are all zero for a silent frame).
fn band_fractions(frame: &[Complex<f32>], sample_rate: u32, len: usize) -> [f32; 5] {
    let rate = sample_rate.max(1) as f32;
    let mut energy = [0.0f32; 5];
    let last_bin = len / 2;
    for (bin, coeff) in frame.iter().enumerate().take(last_bin + 1) {
        let magnitude = coeff.re * coeff.re + coeff.im * coeff.im;
        let freq = bin as f32 * rate / len as f32;
        let band = if freq < BAND_LOW_MAX {
            0
        } else if freq < BAND_LOW_MID_MAX {
            1
        } else if freq < BAND_MID_MAX {
            2
        } else if freq < BAND_MID_HIGH_MAX {
            3
        } else {
            4
        };
        energy[band] += magnitude;
    }
    let total: f32 = energy.iter().sum();
    if total > EPS {
        for value in &mut energy {
            *value /= total;
        }
    }
    energy
}

/// Map the per-frame DSP features (RMS, zero-crossing rate, band
/// fractions) onto raw, un-smoothed viseme weights.
fn target_weights(rms: f32, zcr: f32, bands: [f32; 5]) -> VisemeWeights {
    // Overall mouth openness from amplitude; silence collapses to 0.
    let openness = smoothstep(NOISE_GATE, AMP_FULL, rms);
    let zcr_high = smoothstep(ZCR_LOW, ZCR_HIGH, zcr);
    let amp_high = smoothstep(0.5, 0.9, openness);
    let amp_low = 1.0 - smoothstep(0.1, 0.4, openness);
    let amp_mid = smoothstep(0.2, 0.45, openness) * (1.0 - smoothstep(0.6, 0.85, openness));

    let e_low = bands[0];
    let e_low_mid = bands[1];
    let e_mid_high = bands[3];
    let e_high = bands[4];

    // Per-vowel scores combine amplitude / zero-crossing character
    // with the spectral band that best identifies each vowel.
    let open_score = e_low_mid * amp_high; // open mouth, low-mid energy
    let smile_score = e_high * zcr_high; // smile, high frequency
    let round_score = e_low * amp_low; // round, low frequency, quiet
    let wide_score = e_mid_high * zcr_high; // wide, mid-high frequency
    let small_round_score = e_low_mid * amp_mid; // small round, mid amplitude

    let dist = distribute([
        open_score,
        smile_score,
        round_score,
        wide_score,
        small_round_score,
    ]);

    VisemeWeights {
        aa: openness * dist[0],
        ih: openness * dist[1],
        ou: openness * dist[2],
        ee: openness * dist[3],
        oh: openness * dist[4],
    }
}

/// Normalize raw per-vowel scores so they sum to one. When there is
/// no spectral evidence the scores are spread evenly, keeping the
/// mouth neutral rather than lopsided.
fn distribute(scores: [f32; 5]) -> [f32; 5] {
    let total: f32 = scores.iter().sum();
    if total > EPS {
        scores.map(|s| s / total)
    } else {
        [0.2; 5]
    }
}

/// Hermite smoothstep between `edge0` and `edge1`, clamped to
/// `[0, 1]`.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let span = edge1 - edge0;
    if span.abs() <= EPS {
        return 0.0;
    }
    let t = ((x - edge0) / span).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Apply the asymmetric EMA (fast attack, slower release) per field.
fn smooth_weights(current: VisemeWeights, target: VisemeWeights) -> VisemeWeights {
    VisemeWeights {
        aa: smooth_field(current.aa, target.aa),
        ih: smooth_field(current.ih, target.ih),
        ou: smooth_field(current.ou, target.ou),
        ee: smooth_field(current.ee, target.ee),
        oh: smooth_field(current.oh, target.oh),
    }
}

/// Move one weight field toward its target, attacking faster than it
/// releases to keep the lips responsive yet jitter-free.
fn smooth_field(current: f32, target: f32) -> f32 {
    let alpha = if target > current { ATTACK } else { RELEASE };
    current + alpha * (target - current)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 16_000;

    fn sine(freq: f32, amp: f32, count: usize) -> Vec<f32> {
        (0..count)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SR as f32).sin())
            .collect()
    }

    fn total(w: VisemeWeights) -> f32 {
        w.aa + w.ih + w.ou + w.ee + w.oh
    }

    #[test]
    fn default_weights_are_zero() {
        assert_eq!(
            VisemeWeights::default(),
            VisemeWeights {
                aa: 0.0,
                ih: 0.0,
                ou: 0.0,
                ee: 0.0,
                oh: 0.0,
            }
        );
    }

    #[test]
    fn empty_buffer_yields_zero_weights() {
        let mut analyzer = VisemeAnalyzer::new(SR);
        assert_eq!(analyzer.analyze(), VisemeWeights::default());
    }

    #[test]
    fn silence_yields_zero_weights() {
        let mut analyzer = VisemeAnalyzer::new(SR);
        analyzer.push_pcm(&vec![0.0; 4096]);
        assert_eq!(analyzer.analyze(), VisemeWeights::default());
    }

    #[test]
    fn loud_signal_yields_nonzero_weights() {
        let mut analyzer = VisemeAnalyzer::new(SR);
        analyzer.push_pcm(&sine(440.0, 0.5, 4096));
        let w = analyzer.analyze();
        assert!(total(w) > 0.0, "expected non-zero weights, got {w:?}");
        assert!(
            w.aa > 0.0,
            "a 440 Hz tone should drive the open 'aa' shape, got {w:?}"
        );
    }

    #[test]
    fn weights_decay_toward_zero_after_silence() {
        let mut analyzer = VisemeAnalyzer::new(SR);
        analyzer.push_pcm(&sine(440.0, 0.5, 4096));
        let peak = analyzer.analyze();
        assert!(total(peak) > 0.0);

        analyzer.push_pcm(&vec![0.0; 4096]);
        let decayed = analyzer.analyze();
        assert!(
            total(decayed) < total(peak),
            "weights should decay once the signal goes silent"
        );
        assert!(total(decayed) > 0.0, "release is gradual, not instant");

        let more = analyzer.analyze();
        assert!(
            total(more) < total(decayed),
            "continued silence should keep decaying the weights"
        );
    }

    #[test]
    fn reset_clears_buffer_and_weights() {
        let mut analyzer = VisemeAnalyzer::new(SR);
        analyzer.push_pcm(&sine(440.0, 0.5, 4096));
        let _ = analyzer.analyze();
        analyzer.reset();
        assert_eq!(analyzer.analyze(), VisemeWeights::default());
    }

    #[test]
    fn window_size_is_power_of_two_and_at_least_minimum() {
        let analyzer = VisemeAnalyzer::new(SR);
        let window = analyzer.window_size();
        assert!(window >= MIN_WINDOW);
        assert!(window.is_power_of_two());
    }
}
