//! System-audio loopback capture and beat detection (Beat Sync).
//!
//! Opens a loopback (monitor) capture device via `cpal`, runs a real-time
//! FFT over the incoming mono mix, and detects musical beats with an
//! adaptive energy-threshold onset detector on the low-frequency band.
//! Each detected beat is reported through the AI bridge so the runtime
//! broadcasts it on the chat bus as `EneEvent::BeatPulse`.
//!
//! ## Detection algorithm
//!
//! Energy-based onset detection (chosen for robustness and simplicity over
//! autocorrelation or comb-filter tempo trackers at this scope): input is
//! low-passed at ~250 Hz (2nd-order Butterworth) so out-of-band content
//! cannot leak into the analysis band, then every [`FFT_HOP`] samples a
//! Blackman-Harris-windowed 4096-point FFT is computed and the mean
//! magnitude over the kick band (≈20–150 Hz, DC excluded) is the frame
//! energy. An onset fires
//! when the energy exceeds a slow exponential average by a fixed margin,
//! the kick band holds a minimum share of the sub-500 Hz energy (suppresses
//! high-frequency transients), the refractory period since the last onset
//! has elapsed, and the energy clears an absolute floor (silence never
//! triggers). Onset intensity is the normalized overshoot
//! `1 - average/energy`; BPM is `60 / median(inter-onset interval)` over
//! the recent intervals, clamped to a plausible tempo range.
//!
//! ## Platform support
//!
//! `cpal` has no loopback API: on Linux it enumerates `ALSA` / `PipeWire`
//! capture devices, so loopback works where a monitor source is exposed as
//! an input device. Device selection is deliberately narrow — the default
//! microphone is never an implicit candidate, because on ALSA and PipeWire
//! the mic can share the default output device's name. Candidates are, in
//! order: the `desktop.beat_sync.device` override (exact name), and any
//! input device whose name contains "monitor" (the `PulseAudio` /
//! `PipeWire` `<output>.monitor` convention, also exposed through
//! `pipewire-alsa`). When no candidate exists the feature errors at start
//! and stays disabled.
//!
//! Gated behind the `voice` feature; without it the desktop builds a
//! text-only shell and this module is not compiled.

use std::cmp::Ordering as CmpOrdering;
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::{Sample, SampleFormat};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::ai_bridge::AiBridge;

/// FFT window size in samples (~85 ms at 48 kHz).
const FFT_WINDOW: usize = 4096;

/// Samples advanced between FFTs (~21 ms at 48 kHz).
const FFT_HOP: usize = 1024;

/// Low-frequency ("kick") band analyzed for beat onsets, in Hz.
const KICK_BAND_HZ: (f32, f32) = (20.0, 150.0);

/// Upper bound of the reference low band the kick band is compared against.
const LOW_BAND_HZ: f32 = 500.0;

/// Cutoff of the input low-pass filter; just above the kick band so in-band
/// content passes nearly unchanged while out-of-band energy is attenuated
/// by ~40 dB before the FFT.
const LOWPASS_CUTOFF_HZ: f32 = 250.0;

/// Minimum share of the low-band energy the kick band must hold for an
/// onset; suppresses high-frequency transients (hi-hats, clicks) whose
/// spectral leakage into the kick band is small relative to their total.
const KICK_DOMINANCE: f32 = 0.4;

/// Frame energy must exceed this multiple of the running average to fire.
const ONSET_MARGIN: f32 = 1.5;

/// Minimum gap between onsets, in seconds, suppressing double triggers.
const REFRACTORY_SECS: f32 = 0.25;

/// Absolute energy floor; below this the frame counts as silence.
const ENERGY_FLOOR: f32 = 1e-4;

/// Analyses that must seed the background average before onsets can fire,
/// so a continuous tone starting at capture time is not an onset.
const WARMUP_ANALYSES: u8 = 3;

/// How many recent inter-onset intervals the BPM estimate keeps.
const INTERVAL_HISTORY: usize = 8;

/// How many of the most recent intervals the median BPM uses.
const BPM_MEDIAN_WINDOW: usize = 4;

/// Plausible tempo range the BPM estimate is clamped to.
const BPM_RANGE: Range<f32> = 60.0..180.0;

/// How long the capture loop polls the shutdown flag before joining.
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A single detected beat, normalized for the runtime relay.
#[derive(Debug, Clone, Copy)]
pub struct BeatPulse {
    /// Estimated tempo in beats per minute.
    pub bpm: f32,
    /// Normalized onset strength in `[0, 1]`.
    pub intensity: f32,
}

/// Errors returned when the beat-sync capture thread cannot start.
#[derive(Debug, thiserror::Error)]
pub enum BeatSyncError {
    /// The OS refused to spawn the capture thread.
    #[error("failed to spawn beat sync thread: {0}")]
    Spawn(String),
    /// No loopback candidate device exists (see the module docs).
    #[error("no loopback (monitor) audio device found")]
    NoLoopbackDevice,
    /// The loopback device reported no supported input configuration.
    #[error("loopback device has no supported input configuration: {0}")]
    NoSupportedConfig(String),
    /// The negotiated format is not linear PCM (e.g. raw DSD).
    #[error("unsupported loopback sample format: {0}")]
    UnsupportedFormat(String),
}

/// Handle to the running beat-sync capture thread.
///
/// The `cpal::Stream` is created and owned inside the thread (it is
/// `!Send + !Sync`), so this handle is just a shutdown flag plus a join
/// handle plus a liveness flag, and can live in a bevy resource. Dropping
/// the handle stops the stream and joins the thread.
pub struct BeatSyncHandle {
    join: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    /// False until the capture stream is actually playing; flipped false
    /// again if the stream dies (e.g. device unplug). `is_alive()` consults
    /// it so a dead thread is never reported as running.
    alive: Arc<AtomicBool>,
}

impl BeatSyncHandle {
    /// Stop capture and wait for the thread to exit.
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            drop(join.join());
        }
    }

    /// Whether the capture stream is currently running.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

impl Drop for BeatSyncHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Spawn the beat-sync capture thread.
///
/// `device_name` overrides loopback device selection (the
/// `desktop.beat_sync.device` config value); `None` selects the monitor of
/// the default output device automatically. The loopback device and its
/// input configuration are resolved **synchronously**, so a missing device
/// or unsupported format surfaces as an error here instead of a silently
/// dead thread. Every detected beat is relayed through `ai` into the
/// runtime.
pub fn spawn_beat_sync(
    device_name: Option<String>,
    ai: Arc<AiBridge>,
) -> Result<BeatSyncHandle, BeatSyncError> {
    let (device, config, sample_format) = open_loopback(device_name.as_deref())?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let alive = Arc::new(AtomicBool::new(false));
    let loop_shutdown = Arc::clone(&shutdown);
    let loop_alive = Arc::clone(&alive);
    let join = std::thread::Builder::new()
        .name("ene-beat-sync".to_string())
        .spawn(move || beat_sync_loop(device, config, sample_format, ai, loop_shutdown, loop_alive))
        .map_err(|e| BeatSyncError::Spawn(e.to_string()))?;
    Ok(BeatSyncHandle {
        join: Some(join),
        shutdown,
        alive,
    })
}

fn beat_sync_loop(
    device: cpal::Device,
    config: cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    ai: Arc<AiBridge>,
    shutdown: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
) {
    let src_rate = config.sample_rate;
    let channels = usize::from(config.channels.max(1));
    let mut state = Some(CaptureState {
        detector: BeatDetector::new(src_rate),
        ai,
        channels,
        mono: Vec::new(),
        sample_rate: src_rate,
    });
    let err_alive = Arc::clone(&alive);

    let stream = match device.build_input_stream_raw(
        config,
        sample_format,
        move |data, _info| {
            if let Some(state) = state.as_mut() {
                state.on_data(data);
            }
        },
        move |err| {
            tracing::warn!(
                component = "BeatSync",
                error = %err,
                "loopback stream error; beat sync disabled until re-enabled"
            );
            err_alive.store(false, Ordering::Relaxed);
        },
        Some(Duration::from_secs(5)),
    ) {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!(
                component = "BeatSync",
                error = %e,
                "failed to build loopback stream; beat sync disabled"
            );
            alive.store(false, Ordering::Relaxed);
            return;
        }
    };
    if let Err(e) = stream.play() {
        tracing::warn!(
            component = "BeatSync",
            error = %e,
            "failed to start loopback stream; beat sync disabled"
        );
        alive.store(false, Ordering::Relaxed);
        return;
    }
    alive.store(true, Ordering::Relaxed);

    let device_name = device
        .description()
        .map_or_else(|_| "unknown".to_string(), |d| d.name().to_string());
    tracing::info!(
        component = "BeatSync",
        device = %device_name,
        src_rate,
        channels,
        sample_format = ?sample_format,
        "beat sync capture started"
    );

    while !shutdown.load(Ordering::Relaxed) && alive.load(Ordering::Relaxed) {
        std::thread::sleep(SHUTDOWN_POLL_INTERVAL);
    }
}

/// Open the loopback capture device and its default input configuration.
fn open_loopback(
    configured: Option<&str>,
) -> Result<(cpal::Device, cpal::StreamConfig, cpal::SampleFormat), BeatSyncError> {
    let host = cpal::default_host();
    let device =
        find_loopback_device(&host, configured).ok_or(BeatSyncError::NoLoopbackDevice)?;
    let supported = device
        .default_input_config()
        .map_err(|e| BeatSyncError::NoSupportedConfig(e.to_string()))?;
    let sample_format = supported.sample_format();
    if !is_linear_pcm(sample_format) {
        return Err(BeatSyncError::UnsupportedFormat(format!(
            "{sample_format:?}"
        )));
    }
    let config = supported.config();
    Ok((device, config, sample_format))
}

/// Pick a loopback input device.
///
/// Selection order: the configured override (exact name), then any input
/// whose name contains "monitor". A bare name-contains-output match is
/// intentionally absent — on ALSA, PulseAudio, and PipeWire the microphone
/// can share the output device's name, so only the monitor convention can
/// identify a loopback implicitly.
fn find_loopback_device(host: &cpal::Host, configured: Option<&str>) -> Option<cpal::Device> {
    let inputs: Vec<(String, cpal::Device)> = host
        .input_devices()
        .ok()?
        .filter_map(|d| {
            d.description()
                .ok()
                .map(|desc| (desc.name().to_string(), d))
        })
        .collect();
    let picked = pick_loopback_device(configured, inputs.iter().map(|(name, _)| name.as_str()))?;
    inputs
        .into_iter()
        .find(|(name, _)| name == &picked)
        .map(|(_, device)| device)
}

/// Pure matching core of [`find_loopback_device`], unit-testable.
fn pick_loopback_device<'a>(
    configured: Option<&str>,
    input_names: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let names: Vec<&str> = input_names.into_iter().collect();
    if let Some(configured) = configured {
        return names
            .iter()
            .find(|name| **name == configured)
            .map(|name| name.to_string());
    }
    names
        .into_iter()
        .find(|name| name.to_lowercase().contains("monitor"))
        .map(str::to_string)
}

/// Whether `format` is linear PCM the detector can decode.
fn is_linear_pcm(format: cpal::SampleFormat) -> bool {
    matches!(
        format,
        SampleFormat::F32
            | SampleFormat::F64
            | SampleFormat::I8
            | SampleFormat::I16
            | SampleFormat::I24
            | SampleFormat::I32
            | SampleFormat::I64
            | SampleFormat::U8
            | SampleFormat::U16
            | SampleFormat::U24
            | SampleFormat::U32
            | SampleFormat::U64
    )
}

/// Per-callback capture state moved into the `cpal` data closure.
struct CaptureState {
    detector: BeatDetector,
    ai: Arc<AiBridge>,
    channels: usize,
    /// Reused mono scratch buffer; avoids per-callback allocations.
    mono: Vec<f32>,
    sample_rate: u32,
}

impl CaptureState {
    fn on_data(&mut self, data: &cpal::Data) {
        match data.sample_format() {
            SampleFormat::F32 => self.decode::<f32>(data),
            SampleFormat::F64 => self.decode::<f64>(data),
            SampleFormat::I8 => self.decode::<i8>(data),
            SampleFormat::I16 => self.decode::<i16>(data),
            SampleFormat::I24 => self.decode::<cpal::I24>(data),
            SampleFormat::I32 => self.decode::<i32>(data),
            SampleFormat::I64 => self.decode::<i64>(data),
            SampleFormat::U8 => self.decode::<u8>(data),
            SampleFormat::U16 => self.decode::<u16>(data),
            SampleFormat::U24 => self.decode::<cpal::U24>(data),
            SampleFormat::U32 => self.decode::<u32>(data),
            SampleFormat::U64 => self.decode::<u64>(data),
            // Raw bitstream formats (DSD) and future formats; the open
            // path already rejects them, this arm is defense in depth.
            _ => return,
        }
        if let Some(pulse) = self.detector.process(&self.mono, self.sample_rate) {
            tracing::debug!(
                component = "BeatSync",
                bpm = pulse.bpm,
                intensity = pulse.intensity,
                "beat detected"
            );
            self.ai.report_beat_pulse(pulse.bpm, pulse.intensity);
        }
    }

    fn decode<T: cpal::SizedSample>(&mut self, data: &cpal::Data)
    where
        f32: cpal::FromSample<T>,
    {
        decode_to_mono::<T>(data, self.channels, &mut self.mono);
    }
}

/// Decode a cpal data buffer into mono `f32`, reusing the caller's scratch.
fn decode_to_mono<T: cpal::SizedSample>(
    data: &cpal::Data,
    channels: usize,
    mono: &mut Vec<f32>,
) where
    f32: cpal::FromSample<T>,
{
    mono.clear();
    let Some(slice) = data.as_slice::<T>() else {
        return;
    };
    if channels <= 1 {
        mono.extend(slice.iter().map(|&s| f32::from_sample(s)));
    } else {
        mono.extend(slice.chunks(channels).map(|frame| {
            frame
                .iter()
                .map(|&s| f32::from_sample(s))
                .sum::<f32>()
                / channels as f32
        }));
    }
}

/// Real-time FFT + energy-onset beat detector.
///
/// Sample-rate agnostic: the kick band and FFT bins are derived from the
/// rate passed to [`new`](Self::new). Feed mono PCM in arbitrary chunk
/// sizes; analysis runs every [`FFT_HOP`] samples once the window fills.
pub struct BeatDetector {
    lowpass: LowPass,
    ring: VecDeque<f32>,
    window: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    window_weights: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    band: Range<usize>,
    low_band: Range<usize>,
    avg_energy: f32,
    bpm: f32,
    intervals: VecDeque<f32>,
    last_onset_at: f64,
    warmup: u8,
    samples_processed: u64,
    next_analysis_at: u64,
}

impl BeatDetector {
    /// Build a detector for audio at `sample_rate` Hz.
    pub fn new(sample_rate: u32) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_WINDOW);
        let scratch_len = FFT_WINDOW.max(fft.get_inplace_scratch_len());
        let band = kick_band_bins(sample_rate);
        let low_band = low_band_bins(sample_rate);
        Self {
            lowpass: LowPass::butterworth(LOWPASS_CUTOFF_HZ, sample_rate as f32),
            ring: VecDeque::with_capacity(FFT_WINDOW),
            window: vec![Complex::default(); FFT_WINDOW],
            scratch: vec![Complex::default(); scratch_len],
            window_weights: (0..FFT_WINDOW)
                .map(|i| {
                    let x = i as f32 / (FFT_WINDOW - 1) as f32;
                    // 4-term Blackman-Harris: -92 dB sidelobes keep
                    // out-of-band leakage below the detection floor.
                    0.35875 - 0.48829 * (std::f32::consts::TAU * x).cos()
                        + 0.14128 * (std::f32::consts::TAU * 2.0 * x).cos()
                        - 0.01168 * (std::f32::consts::TAU * 3.0 * x).cos()
                })
                .collect(),
            fft,
            band,
            low_band,
            avg_energy: 0.0,
            bpm: 120.0,
            intervals: VecDeque::with_capacity(INTERVAL_HISTORY),
            last_onset_at: 0.0,
            warmup: 0,
            samples_processed: 0,
            next_analysis_at: 0,
        }
    }

    /// Feed mono PCM and return a detected beat, if any.
    pub fn process(&mut self, samples: &[f32], sample_rate: u32) -> Option<BeatPulse> {
        let mut pulse = None;
        for &sample in samples {
            let filtered = self.lowpass.process(sample);
            if self.ring.len() == FFT_WINDOW {
                self.ring.pop_front();
            }
            self.ring.push_back(filtered);
            self.samples_processed += 1;
            if self.samples_processed >= self.next_analysis_at && self.ring.len() == FFT_WINDOW {
                self.next_analysis_at += FFT_HOP as u64;
                if let Some(detected) = self.analyze(sample_rate) {
                    pulse = Some(detected);
                }
            }
        }
        pulse
    }

    fn analyze(&mut self, sample_rate: u32) -> Option<BeatPulse> {
        for (i, sample) in self.ring.iter().enumerate() {
            self.window[i] = Complex::new(*sample * self.window_weights[i], 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.window, &mut self.scratch);
        let energy = self.band_energy(&self.band);
        let now = self.samples_processed as f64 / f64::from(sample_rate);
        self.warmup = self.warmup.saturating_add(1);
        if energy <= ENERGY_FLOOR {
            return None;
        }
        if energy < KICK_DOMINANCE * self.band_energy(&self.low_band) {
            self.avg_energy += 0.1 * (energy - self.avg_energy);
            return None;
        }
        if self.warmup >= WARMUP_ANALYSES
            && energy > self.avg_energy * ONSET_MARGIN
            && now - self.last_onset_at >= f64::from(REFRACTORY_SECS)
        {
            let intensity = if self.avg_energy > 0.0 {
                (1.0 - self.avg_energy / energy).clamp(0.0, 1.0)
            } else {
                1.0
            };
            if self.last_onset_at > 0.0 {
                let interval = (now - self.last_onset_at) as f32;
                self.intervals.push_back(interval);
                while self.intervals.len() > INTERVAL_HISTORY {
                    self.intervals.pop_front();
                }
                let window_start = self.intervals.len().saturating_sub(BPM_MEDIAN_WINDOW);
                let recent: Vec<f32> = self.intervals.iter().copied().skip(window_start).collect();
                if recent.len() >= 2 {
                    self.bpm = (60.0 / median(&recent)).clamp(BPM_RANGE.start, BPM_RANGE.end);
                }
            }
            self.last_onset_at = now;
            return Some(BeatPulse {
                bpm: self.bpm,
                intensity,
            });
        }

        // Update the background average only after the onset comparison, so
        // an onset is judged against the pre-onset baseline.
        self.avg_energy += 0.1 * (energy - self.avg_energy);
        None
    }

    fn band_energy(&self, band: &Range<usize>) -> f32 {
        let sum: f32 = self.window[band.clone()].iter().map(|c| c.norm()).sum();
        sum / band.len() as f32
    }
}

/// Second-order Butterworth low-pass (RBJ cookbook), applied per sample
/// before the FFT so out-of-band leakage is attenuated ~40 dB at one octave
/// above the cutoff and keeps falling — the deterministic rejection of
/// high-frequency content that thresholds alone cannot achieve.
struct LowPass {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl LowPass {
    fn butterworth(cutoff_hz: f32, sample_rate: f32) -> Self {
        let w0 = std::f32::consts::TAU * cutoff_hz / sample_rate;
        let (sin_w, cos_w) = w0.sin_cos();
        let alpha = sin_w / (2.0_f32).sqrt();
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - cos_w) / (2.0 * a0),
            b1: (1.0 - cos_w) / a0,
            b2: (1.0 - cos_w) / (2.0 * a0),
            a1: (-2.0 * cos_w) / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// FFT bins covered by [`KICK_BAND_HZ`] at `sample_rate`, DC excluded.
fn kick_band_bins(sample_rate: u32) -> Range<usize> {
    let bin_width = sample_rate as f32 / FFT_WINDOW as f32;
    let start = ((KICK_BAND_HZ.0 / bin_width).ceil() as usize).max(1);
    let end = ((KICK_BAND_HZ.1 / bin_width).floor() as usize).max(start + 1);
    start..end
}

/// FFT bins covered by 20 Hz up to [`LOW_BAND_HZ`] at `sample_rate`.
fn low_band_bins(sample_rate: u32) -> Range<usize> {
    let bin_width = sample_rate as f32 / FFT_WINDOW as f32;
    let start = ((KICK_BAND_HZ.0 / bin_width).ceil() as usize).max(1);
    let end = ((LOW_BAND_HZ / bin_width).floor() as usize).max(start + 1);
    start..end
}

/// Median of `values` (len ≥ 2); NaN inputs sort to the end and are ignored
/// by the midpoint unless every value is NaN.
fn median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(CmpOrdering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        f32::midpoint(sorted[mid - 1], sorted[mid])
    } else {
        sorted[mid]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: u32 = 48_000;

    /// A 100 Hz burst track at 120 BPM: 100 ms bursts every 500 ms.
    fn synthetic_beat_track(seconds: usize) -> Vec<f32> {
        let total = seconds * SAMPLE_RATE as usize;
        (0..total)
            .map(|i| {
                let t = i as f64 / f64::from(SAMPLE_RATE);
                if t % 0.5 < 0.1 {
                    0.5 * (std::f64::consts::TAU * 100.0 * t).sin() as f32
                } else {
                    0.0
                }
            })
            .collect()
    }

    #[test]
    fn detector_finds_synthetic_beats_and_bpm() {
        let mut detector = BeatDetector::new(SAMPLE_RATE);
        let track = synthetic_beat_track(5);
        let mut pulses = Vec::new();
        for chunk in track.chunks(SAMPLE_RATE as usize / 10) {
            if let Some(pulse) = detector.process(chunk, SAMPLE_RATE) {
                pulses.push(pulse);
            }
        }
        // 10 bursts in 5 s; the first needs the 85 ms window to fill.
        assert!(
            pulses.len() >= 7,
            "expected ~9 pulses, got {}",
            pulses.len()
        );
        assert!(
            (110.0..=130.0).contains(&pulses.last().expect("pulses non-empty").bpm),
            "bpm should converge near 120, got {:?}",
            pulses.last()
        );
        for pulse in &pulses {
            assert!((0.0..=1.0).contains(&pulse.intensity));
        }
    }

    #[test]
    fn detector_stays_silent_on_silence() {
        let mut detector = BeatDetector::new(SAMPLE_RATE);
        let silence = vec![0.0f32; SAMPLE_RATE as usize];
        assert!(detector.process(&silence, SAMPLE_RATE).is_none());
    }

    #[test]
    fn detector_ignores_high_frequency_energy() {
        let mut detector = BeatDetector::new(SAMPLE_RATE);
        // 4 kHz tone is far outside the kick band.
        let tone: Vec<f32> = (0..SAMPLE_RATE as usize)
            .map(|i| {
                0.5 * (std::f64::consts::TAU * 4000.0 * i as f64 / f64::from(SAMPLE_RATE)).sin()
                    as f32
            })
            .collect();
        assert!(detector.process(&tone, SAMPLE_RATE).is_none());
    }

    #[test]
    fn configured_override_wins() {
        let names = ["hw:0", "alsa_output.pci-1.analog-stereo.monitor"];
        assert_eq!(
            pick_loopback_device(Some("hw:0"), names),
            Some("hw:0".to_string())
        );
    }

    #[test]
    fn pulseaudio_monitor_name_is_picked() {
        let names = ["hw:0", "alsa_output.pci-1.analog-stereo.monitor"];
        assert_eq!(
            pick_loopback_device(None, names),
            Some("alsa_output.pci-1.analog-stereo.monitor".to_string())
        );
    }

    #[test]
    fn alsa_shared_default_name_never_picks_the_mic() {
        // ALSA exposes default input and output as the same "Default Audio
        // Device" PCM; without a monitor-named candidate there is no
        // loopback, and the mic must not be selected.
        let names = ["Default Audio Device", "HD-Audio Generic, USB Audio"];
        assert_eq!(
            pick_loopback_device(None, names),
            None
        );
    }

    #[test]
    fn pipewire_monitor_port_is_picked_over_shared_names() {
        // On PipeWire the sink and the mic can share the "Default Audio
        // Device" description; the monitor port (also exposed through
        // pipewire-alsa) is the only unambiguous loopback candidate.
        let names = ["Default Audio Device", "alsa_output.pci-1.analog-stereo.monitor"];
        assert_eq!(
            pick_loopback_device(None, names),
            Some("alsa_output.pci-1.analog-stereo.monitor".to_string())
        );
    }

    #[test]
    fn no_monitor_means_no_device() {
        assert_eq!(
            pick_loopback_device(None, ["Built-in Mic", "hdmi-capture"]),
            None
        );
    }

    #[test]
    fn linear_pcm_formats_are_accepted() {
        for format in [
            SampleFormat::F32,
            SampleFormat::F64,
            SampleFormat::I8,
            SampleFormat::I16,
            SampleFormat::I24,
            SampleFormat::I32,
            SampleFormat::I64,
            SampleFormat::U8,
            SampleFormat::U16,
            SampleFormat::U24,
            SampleFormat::U32,
            SampleFormat::U64,
        ] {
            assert!(is_linear_pcm(format), "{format:?} must be linear PCM");
        }
    }

    #[test]
    fn dsd_formats_are_rejected() {
        for format in [
            SampleFormat::DsdU8,
            SampleFormat::DsdU16,
            SampleFormat::DsdU32,
        ] {
            assert!(!is_linear_pcm(format), "{format:?} must be rejected");
        }
    }
}
