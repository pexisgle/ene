//! Microphone capture via `cpal`.
//!
//! Opens the default (or configured) input device, converts the raw
//! callback audio to 16 kHz mono `f32`, runs it through a
//! [`VadEngine`], and — on speech end — transcribes the accumulated
//! utterance with an [`SttProvider`] and feeds the transcript into the
//! AI bridge as a normal user turn.
//!
//! **Echo-aware barge-in** (M3): while [`AudioState::tts_playing`] is
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

use crate::ai_bridge::AiBridge;
use crate::events::{AppEvent, AppEventSender};

use super::AudioState;
use ene_ai::{SttProvider, VadEngine, VadEvent};

/// Capture operates at the Silero VAD rate; the STT provider resamples
/// internally if it needs a different rate.
const CAPTURE_SAMPLE_RATE: u32 = 16_000;

/// Multiplier applied to the RMS energy gate while TTS is playing (M3).
/// Speech must exceed this factor above the normal threshold to be
/// considered a barge-in, reducing false triggers from speaker bleed.
const BARGE_IN_ENERGY_FACTOR: f32 = 2.0;

/// RMS energy below which a frame is considered silence for the
/// barge-in gate. Calibrated for 16-bit-equivalent float audio.
const SILENCE_RMS: f32 = 0.01;

/// Errors returned when microphone capture cannot start.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// No usable input device was found.
    #[error("no input audio device available")]
    NoInputDevice,
    /// The device reported no supported input configuration.
    #[error("input device has no supported input configuration: {0}")]
    NoSupportedConfig(String),
    /// `cpal` failed to build the input stream.
    #[error("failed to build input stream: {0}")]
    BuildStream(String),
}

/// Handle to a running microphone capture stream.
///
/// Dropping the handle (or calling [`stop`](Self::stop)) closes the
/// `cpal` stream and clears the `mic_active` flag.
pub struct MicHandle {
    stream: Option<cpal::Stream>,
    mic_active: Arc<AtomicBool>,
    event_tx: AppEventSender,
}

impl MicHandle {
    /// Stop capture and release the input stream.
    pub fn stop(&mut self) {
        if self.stream.take().is_some() {
            self.mic_active.store(false, Ordering::Relaxed);
            let _ = self
                .event_tx
                .send(AppEvent::MicStateChanged { active: false });
        }
    }
}

impl Drop for MicHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Stateful linear resampler from the device rate to [`CAPTURE_SAMPLE_RATE`].
///
/// Carries a fractional input position and the previous buffer's last
/// sample across callbacks so the output stream stays continuous.
struct Resampler {
    /// Input samples consumed per output sample (`src_rate / dst_rate`).
    ratio: f64,
    /// Fractional input position carried across buffers.
    pos: f64,
    /// Last sample of the previous buffer (interpolation anchor).
    prev: f32,
}

impl Resampler {
    fn new(src_rate: u32) -> Self {
        let ratio = f64::from(src_rate) / f64::from(CAPTURE_SAMPLE_RATE);
        Self {
            ratio: ratio.max(f64::from(f32::EPSILON)),
            pos: 0.0,
            prev: 0.0,
        }
    }

    /// Resample `input` (mono, device rate) into 16 kHz, appending to `out`.
    ///
    /// Interpolates between `input[idx]` and `input[idx + 1]` at `frac`
    /// (standard convention, L9). When `idx + 1` is out of bounds the
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
        self.prev = *input.last().unwrap_or(&self.prev);
    }
}

/// Per-callback capture state moved into the `cpal` data closure.
struct CaptureState {
    resampler: Resampler,
    /// Resampled audio waiting to be framed into VAD frames.
    resampled: Vec<f32>,
    /// Accumulated speech PCM for the current utterance.
    speech: Vec<f32>,
    /// Whether the VAD currently considers the user to be speaking.
    speaking: bool,
    /// Whether the previous callback was suppressed (self-voice).
    suppressed: bool,
    /// Consecutive VAD inference failures; escalated after a threshold.
    vad_errors: u32,
    vad: Box<dyn VadEngine>,
    stt: Arc<dyn SttProvider>,
    ai: Arc<AiBridge>,
    tts_playing: Arc<AtomicBool>,
    tokio: tokio::runtime::Handle,
    channels: usize,
}

impl CaptureState {
    /// Handle one raw callback buffer of interleaved device audio.
    fn on_data(&mut self, data: &cpal::Data) {
        let tts_active = self.tts_playing.load(Ordering::Relaxed);

        // Echo-aware barge-in (M3): while TTS is playing, apply an
        // elevated energy gate instead of hard-muting. If the frame
        // energy exceeds the barge-in threshold, feed it to VAD so
        // genuine user speech can cancel the current turn. Otherwise
        // discard the frame and keep the VAD state clean.
        if tts_active {
            if !self.suppressed {
                self.suppressed = true;
                self.vad.reset();
                self.speech.clear();
                self.speaking = false;
                self.resampled.clear();
            }
            // Still decode and resample so we can measure energy.
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
            self.resampler.process(&mono, &mut self.resampled);
            // Energy gate: only process frames that are loud enough to
            // plausibly be user speech over the speaker output.
            let rms = rms_energy(&self.resampled);
            if rms < SILENCE_RMS * BARGE_IN_ENERGY_FACTOR {
                self.resampled.clear();
                return;
            }
            // Loud frame during TTS: run VAD and emit barge-in if speech
            // is detected.
            self.drain_vad_frames();
            return;
        }
        self.suppressed = false;

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

        // Downmix to mono.
        let mono: Vec<f32> = if self.channels <= 1 {
            interleaved
        } else {
            let ch = self.channels;
            interleaved
                .chunks(ch)
                .map(|frame| frame.iter().sum::<f32>() / ch as f32)
                .collect()
        };

        self.resampler.process(&mono, &mut self.resampled);
        self.drain_vad_frames();
    }

    /// Consume buffered resampled audio in engine-sized frames.
    fn drain_vad_frames(&mut self) {
        let frame_size = self.vad.frame_size();
        while self.resampled.len() >= frame_size {
            let frame: Vec<f32> = self.resampled.drain(0..frame_size).collect();
            match self.vad.process_chunk(&frame) {
                Ok(event) => {
                    self.vad_errors = 0;
                    self.on_vad_event(event, &frame);
                }
                Err(e) => {
                    self.vad_errors += 1;
                    tracing::warn!(
                        component = "MicCapture",
                        error = %e,
                        consecutive = self.vad_errors,
                        "VAD process_chunk failed"
                    );
                    if self.vad_errors >= 10 {
                        tracing::error!(
                            component = "MicCapture",
                            "VAD failed {} consecutive times; resetting engine",
                            self.vad_errors
                        );
                        self.vad.reset();
                        self.vad_errors = 0;
                    }
                }
            }
        }
    }

    /// React to a VAD event, accumulating speech and dispatching on end.
    fn on_vad_event(&mut self, event: VadEvent, frame: &[f32]) {
        match event {
            VadEvent::SpeechStart => {
                self.speaking = true;
                self.speech.clear();
                self.speech.extend_from_slice(frame);
                // Barge-in: if TTS is playing and we detect speech start,
                // cancel the current turn so the user can speak (M3).
                if self.tts_playing.load(Ordering::Relaxed) {
                    tracing::info!(
                        component = "MicCapture",
                        "barge-in: speech detected during TTS playback; cancelling turn"
                    );
                    self.ai.cancel();
                }
            }
            VadEvent::SpeechContinue => {
                if self.speaking {
                    self.speech.extend_from_slice(frame);
                }
            }
            VadEvent::SpeechEnd => {
                if self.speaking {
                    self.speech.extend_from_slice(frame);
                    self.speaking = false;
                    let utterance = std::mem::take(&mut self.speech);
                    self.dispatch_utterance(utterance);
                }
            }
            VadEvent::Silence => {}
        }
    }

    /// Transcribe a completed utterance and run it as a user turn.
    fn dispatch_utterance(&self, pcm: Vec<f32>) {
        if pcm.is_empty() {
            return;
        }
        let stt = Arc::clone(&self.stt);
        let ai = Arc::clone(&self.ai);
        self.tokio.spawn(async move {
            match stt.transcribe(&pcm, CAPTURE_SAMPLE_RATE).await {
                Ok(result) => {
                    let text = result.text.trim().to_string();
                    if text.is_empty() {
                        tracing::debug!(
                            component = "MicCapture",
                            "STT returned empty transcript; ignoring"
                        );
                        return;
                    }
                    tracing::info!(
                        component = "MicCapture",
                        chars = text.chars().count(),
                        "transcribed speech; running turn"
                    );
                    ai.run(text);
                }
                Err(e) => {
                    tracing::warn!(component = "MicCapture", error = %e, "STT transcription failed");
                }
            }
        });
    }
}

/// Root-mean-square energy of a PCM buffer.
fn rms_energy(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Start microphone capture, returning a [`MicHandle`] that keeps the
/// stream alive.
///
/// # Errors
///
/// Returns [`CaptureError`] when no input device is available, the
/// device has no supported input configuration, or `cpal` cannot build
/// the stream.
pub fn start_mic_capture(
    audio_state: &AudioState,
    stt: Arc<dyn SttProvider>,
    vad: Box<dyn VadEngine>,
    ai: Arc<AiBridge>,
    tokio: tokio::runtime::Handle,
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
    let src_rate = config.sample_rate.0;
    let channels = usize::from(config.channels.max(1));

    let state = CaptureState {
        resampler: Resampler::new(src_rate),
        resampled: Vec::new(),
        speech: Vec::new(),
        speaking: false,
        suppressed: false,
        vad_errors: 0,
        vad,
        stt,
        ai,
        tts_playing: Arc::clone(&audio_state.tts_playing),
        tokio,
        channels,
    };
    let mut state = Some(state);

    // Clone state for the error callback so it can clear `mic_active`
    // and emit a disconnect event (M1).
    let err_mic_active = Arc::clone(&audio_state.mic_active);
    let err_event_tx = event_tx.clone();

    let stream = device
        .build_input_stream_raw(
            &config,
            sample_format,
            move |data, _info| {
                if let Some(state) = state.as_mut() {
                    state.on_data(data);
                }
            },
            move |err| {
                tracing::warn!(component = "MicCapture", error = %err, "input stream error");
                // Device disconnect recovery (M1): clear the active flag
                // and notify the UI so the mic indicator resets.
                err_mic_active.store(false, Ordering::Relaxed);
                let _ = err_event_tx.send(AppEvent::MicStateChanged { active: false });
            },
            Some(Duration::from_secs(5)),
        )
        .map_err(|e| CaptureError::BuildStream(e.to_string()))?;

    stream
        .play()
        .map_err(|e| CaptureError::BuildStream(e.to_string()))?;

    audio_state.mic_active.store(true, Ordering::Relaxed);
    let _ = event_tx.send(AppEvent::MicStateChanged { active: true });
    tracing::info!(
        component = "MicCapture",
        device = %device.name().unwrap_or_else(|_| "unknown".to_string()),
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

/// Find an input device whose name matches `name` (case-sensitive).
fn select_device_by_name(host: &cpal::Host, name: &str) -> Option<cpal::Device> {
    let devices = host.input_devices().ok()?;
    devices
        .filter_map(|d| d.name().ok().map(|n| (n, d)))
        .find(|(n, _)| n == name)
        .map(|(_, d)| d)
}

/// Whether a given [`SampleFormat`] is one the capture path can decode.
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
        // L9: verify interpolation uses input[idx] and input[idx+1].
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
