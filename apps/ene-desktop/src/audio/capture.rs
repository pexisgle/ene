//! Microphone capture via `cpal`.
//!
//! Opens the default (or configured) input device, converts the raw
//! callback audio to 16 kHz mono `f32`, and uses RMS energy to barge in
//! while TTS is playing. Speech-to-text lives in `ene-core`; this process
//! only watches the mic for interruption.
//!
//! **Echo-aware barge-in**: while [`AudioState::tts_playing`] is
//! set the callback raises the VAD energy gate instead of hard-muting,
//! so loud user speech can still trigger a barge-in event that cancels
//! the current TTS turn. Full acoustic echo cancellation is not
//! implemented; the elevated threshold is a pragmatic approximation.
//!
//! Gated behind the `voice` feature; without it the desktop builds a
//! text-only shell and this module is not compiled.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::core_session::CoreSession;
use crate::events::{AppEvent, AppEventSender};

use super::AudioState;

/// Capture operates at the Silero VAD rate; the STT provider resamples
/// internally if it needs a different rate.
const CAPTURE_SAMPLE_RATE: u32 = 16_000;

/// Multiplier applied to the RMS energy gate while TTS is playing.
/// Speech must exceed this factor above the normal threshold to be
/// considered a barge-in, reducing false triggers from speaker bleed.
const BARGE_IN_ENERGY_FACTOR: f32 = 2.0;

/// RMS energy below which a frame is considered silence for the
/// barge-in gate. Calibrated for 16-bit-equivalent float audio.
const SILENCE_RMS: f32 = 0.01;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("no input audio device available")]
    NoInputDevice,
    #[error("input device has no supported input configuration: {0}")]
    NoSupportedConfig(String),
    #[error("failed to build input stream: {0}")]
    BuildStream(String),
}

/// Dropping the handle (or calling [`stop`](Self::stop)) closes the
/// `cpal` stream and clears the `mic_active` flag.
pub struct MicHandle {
    stream: Option<cpal::Stream>,
    mic_active: Arc<AtomicBool>,
    event_tx: AppEventSender,
}

impl MicHandle {
    pub fn stop(&mut self) {
        if self.stream.take().is_some() {
            self.mic_active.store(false, Ordering::Relaxed);
            drop(
                self.event_tx
                    .send(AppEvent::MicStateChanged { active: false }),
            );
        }
    }
}

impl Drop for MicHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Carries a fractional input position across callbacks so the output
/// stream stays continuous.
struct Resampler {
    ratio: f64,
    pos: f64,
}

impl Resampler {
    fn new(src_rate: u32) -> Self {
        let ratio = f64::from(src_rate) / f64::from(CAPTURE_SAMPLE_RATE);
        Self {
            ratio: ratio.max(f64::from(f32::EPSILON)),
            pos: 0.0,
        }
    }

    /// Interpolates between `input[idx]` and `input[idx + 1]` at `frac`
    /// (standard convention). When `idx + 1` is out of bounds the
    /// output is clamped to `input[idx]`.
    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        let len = input.len() as f64;
        while self.pos < len {
            let idx_f = self.pos.floor();
            let frac = (self.pos - idx_f) as f32;
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

/// Per-callback capture state moved into the `cpal` data closure.
struct CaptureState {
    resampler: Resampler,
    ai: Arc<CoreSession>,
    tts_playing: Arc<AtomicBool>,
    barged: bool,
    channels: usize,
    utterance: Vec<f32>,
    silence_frames: u32,
}

impl CaptureState {
    fn on_data(&mut self, data: &cpal::Data) {
        let interleaved: Vec<f32> = match data.sample_format() {
            SampleFormat::F32 => data.as_slice::<f32>().unwrap_or_default().to_vec(),
            SampleFormat::I16 => data
                .as_slice::<i16>()
                .unwrap_or_default()
                .iter()
                .map(|&v| f32::from(v) / 32768.0)
                .collect(),
            SampleFormat::U16 => data
                .as_slice::<u16>()
                .unwrap_or_default()
                .iter()
                .map(|&v| (f32::from(v) - 32768.0) / 32768.0)
                .collect(),
            _ => return,
        };
        let mono: Vec<f32> = if self.channels <= 1 {
            interleaved
        } else {
            let ch = self.channels;
            interleaved
                .chunks(ch)
                .map(|frame| frame.iter().sum::<f32>() / ch as f32)
                .collect()
        };
        let mut resampled = Vec::new();
        self.resampler.process(&mono, &mut resampled);
        let rms = rms_energy(&resampled);
        let tts_active = self.tts_playing.load(Ordering::Relaxed);
        if tts_active {
            self.utterance.clear();
            self.silence_frames = 0;
            if rms < SILENCE_RMS * BARGE_IN_ENERGY_FACTOR {
                return;
            }
            if self.barged {
                return;
            }
            self.barged = true;
            tracing::info!(
                component = "MicCapture",
                rms,
                "barge-in: energy during TTS playback; cancelling turn"
            );
            self.ai.barge_in();
            self.ai.cancel();
            return;
        }
        self.barged = false;
        if rms >= SILENCE_RMS {
            self.utterance.extend_from_slice(&resampled);
            self.silence_frames = 0;
            return;
        }
        if self.utterance.is_empty() {
            return;
        }
        self.silence_frames = self.silence_frames.saturating_add(1);
        if self.silence_frames < 8 {
            self.utterance.extend_from_slice(&resampled);
            return;
        }
        let pcm = std::mem::take(&mut self.utterance);
        self.silence_frames = 0;
        if pcm.len() >= 3_200 {
            self.ai.listen(pcm, CAPTURE_SAMPLE_RATE);
        }
    }
}

fn rms_energy(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

pub fn start_mic_capture(
    audio_state: &AudioState,
    ai: Arc<CoreSession>,
    event_tx: AppEventSender,
) -> Result<MicHandle, CaptureError> {
    let host = cpal::default_host();

    let device = audio_state
        .mic_device
        .as_deref()
        .and_then(|name| select_device_by_name(&host, name))
        .or_else(|| host.default_input_device())
        .ok_or(CaptureError::NoInputDevice)?;

    let supported = device
        .default_input_config()
        .map_err(|e| CaptureError::NoSupportedConfig(e.to_string()))?;
    let config = supported.config();
    let sample_format = supported.sample_format();
    let src_rate = config.sample_rate;
    let channels = usize::from(config.channels.max(1));

    let state = CaptureState {
        resampler: Resampler::new(src_rate),
        ai,
        tts_playing: Arc::clone(&audio_state.tts_playing),
        barged: false,
        channels,
        utterance: Vec::new(),
        silence_frames: 0,
    };
    let mut state = Some(state);

    // Clone state for the error callback so it can clear `mic_active`
    // and emit a disconnect event.
    let err_mic_active = Arc::clone(&audio_state.mic_active);
    let err_event_tx = event_tx.clone();

    let stream = device
        .build_input_stream_raw(
            config,
            sample_format,
            move |data, _info| {
                if let Some(state) = state.as_mut() {
                    state.on_data(data);
                }
            },
            move |err| {
                tracing::warn!(component = "MicCapture", error = %err, "input stream error");
                // Device disconnect recovery: clear the active flag
                // and notify the UI so the mic indicator resets.
                err_mic_active.store(false, Ordering::Relaxed);
                drop(err_event_tx.send(AppEvent::MicStateChanged { active: false }));
            },
            Some(Duration::from_secs(5)),
        )
        .map_err(|e| CaptureError::BuildStream(e.to_string()))?;

    stream
        .play()
        .map_err(|e| CaptureError::BuildStream(e.to_string()))?;

    audio_state.mic_active.store(true, Ordering::Relaxed);
    drop(event_tx.send(AppEvent::MicStateChanged { active: true }));
    tracing::info!(
        component = "MicCapture",
        device = %device.description().map_or_else(|_| "unknown".to_string(), |d| d.name().to_string()),
        src_rate,
        channels,
        sample_format = ?sample_format,
        "microphone capture started"
    );

    Ok(MicHandle {
        stream: Some(stream),
        mic_active: Arc::clone(&audio_state.mic_active),
        event_tx,
    })
}

fn select_device_by_name(host: &cpal::Host, name: &str) -> Option<cpal::Device> {
    let devices = host.input_devices().ok()?;
    devices
        .filter_map(|d| {
            d.description()
                .ok()
                .map(|desc| (desc.name().to_string(), d))
        })
        .find(|(n, _)| n == name)
        .map(|(_, d)| d)
}

/// Used by the settings UI to offer a microphone picker. The list is a
/// snapshot; devices may be added or removed between enumeration and capture.
pub fn list_input_device_names() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.description().ok())
                .map(|desc| desc.name().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
const fn supported_format(format: SampleFormat) -> bool {
    matches!(
        format,
        SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_downsamples_by_ratio() {
        // 48 kHz -> 16 kHz is a 3:1 ratio.
        let mut r = Resampler::new(48_000);
        let mut out = Vec::new();
        let input = vec![0.0f32; 4800]; // 100 ms at 48 kHz
        r.process(&input, &mut out);
        // ~1600 samples at 16 kHz (allow off-by-one for the carry).
        assert!((1590..=1610).contains(&out.len()), "got {}", out.len());
    }

    #[test]
    fn resampler_passthrough_at_target_rate() {
        let mut r = Resampler::new(CAPTURE_SAMPLE_RATE);
        let mut out = Vec::new();
        let input = vec![0.5f32; 1600];
        r.process(&input, &mut out);
        assert!((1590..=1600).contains(&out.len()), "got {}", out.len());
    }

    #[test]
    fn resampler_empty_input_is_noop() {
        let mut r = Resampler::new(48_000);
        let mut out = Vec::new();
        r.process(&[], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn resampler_interpolates_forward() {
        // With ratio 1.5 (24 kHz -> 16 kHz), output positions are
        // 0, 1.5, 3.0 — the second sample interpolates between
        // input[1] and input[2] at frac=0.5.
        let mut r = Resampler::new(24_000);
        let mut out = Vec::new();
        let input = vec![0.0f32, 1.0, 2.0, 3.0];
        r.process(&input, &mut out);
        // First output: pos=0, idx=0, frac=0 -> input[0] = 0.0.
        assert!(!out.is_empty());
        assert!((out[0] - 0.0).abs() < 1e-6, "first sample: {}", out[0]);
        // Second output: pos=1.5, idx=1, frac=0.5 -> lerp(input[1], input[2], 0.5) = 1.5.
        assert!(
            out.len() > 1,
            "expected at least 2 samples, got {}",
            out.len()
        );
        assert!((out[1] - 1.5).abs() < 1e-6, "second sample: {}", out[1]);
    }

    #[test]
    fn rms_energy_computes_correctly() {
        let silence = vec![0.0f32; 100];
        assert!((rms_energy(&silence) - 0.0).abs() < 1e-9);
        let dc = vec![0.5f32; 100];
        assert!((rms_energy(&dc) - 0.5).abs() < 1e-6);
        assert!((rms_energy(&[]) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn capture_supports_common_formats() {
        assert!(supported_format(SampleFormat::F32));
        assert!(supported_format(SampleFormat::I16));
        assert!(supported_format(SampleFormat::U16));
    }
}
