//! Microphone capture via cpal.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use crossbeam_channel::{Receiver, unbounded};
use parking_lot::Mutex;

use super::AudioError;
use super::dsp::{Resampler, SILENCE_RMS, rms, should_forward_mic};

/// Default RMS flag used by [`MicCapture::barge_in_active`].
const DEFAULT_BARGE_IN_THRESHOLD: f32 = 0.02;

/// Streaming microphone input that forwards 16 kHz mono `f32` chunks.
pub struct MicCapture {
    _stream: Option<Stream>,
    rx: Receiver<Vec<f32>>,
    barge_in: Arc<Mutex<bool>>,
}

impl MicCapture {
    /// Open the default input device. `energy_threshold` overrides the barge-in RMS gate.
    pub fn new(energy_threshold: Option<f32>, tts_playing: Arc<AtomicBool>) -> Self {
        Self::new_with_device(energy_threshold, tts_playing, None)
    }

    /// Open the configured input device, falling back to the system default when it is absent.
    pub fn new_with_device(
        energy_threshold: Option<f32>,
        tts_playing: Arc<AtomicBool>,
        device_name: Option<&str>,
    ) -> Self {
        let threshold = energy_threshold.unwrap_or(DEFAULT_BARGE_IN_THRESHOLD);
        let (tx, rx) = unbounded();
        let barge_in = Arc::new(Mutex::new(false));

        match open_stream(
            tx,
            Arc::clone(&barge_in),
            threshold,
            tts_playing,
            device_name,
        ) {
            Ok(stream) => Self {
                _stream: Some(stream),
                rx,
                barge_in,
            },
            Err(err) => {
                tracing::warn!(?err, "mic capture unavailable; using silent stub");
                Self {
                    _stream: None,
                    rx,
                    barge_in,
                }
            }
        }
    }

    /// Non-blocking receive of the next 16 kHz PCM chunk.
    pub fn try_recv(&self) -> Option<Vec<f32>> {
        self.rx.try_recv().ok()
    }

    #[must_use]
    pub fn barge_in_active(&self) -> bool {
        *self.barge_in.lock()
    }
}

fn open_stream(
    tx: crossbeam_channel::Sender<Vec<f32>>,
    barge_in: Arc<Mutex<bool>>,
    threshold: f32,
    tts_playing: Arc<AtomicBool>,
    device_name: Option<&str>,
) -> Result<Stream, AudioError> {
    let host = cpal::default_host();
    let device = device_name
        .and_then(|name| select_device_by_name(&host, name))
        .or_else(|| host.default_input_device())
        .ok_or_else(|| AudioError::Device("no input device".to_owned()))?;
    let config = device
        .default_input_config()
        .map_err(|err| AudioError::Device(err.to_string()))?;
    let sample_format = config.sample_format();
    let src_rate = config.sample_rate();
    let stream_config: cpal::StreamConfig = config.into();

    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(
            &device,
            stream_config,
            src_rate,
            tx,
            barge_in,
            threshold,
            tts_playing,
        )?,
        SampleFormat::I16 => build_stream::<i16>(
            &device,
            stream_config,
            src_rate,
            tx,
            barge_in,
            threshold,
            tts_playing,
        )?,
        SampleFormat::U16 => build_stream::<u16>(
            &device,
            stream_config,
            src_rate,
            tx,
            barge_in,
            threshold,
            tts_playing,
        )?,
        other => {
            return Err(AudioError::Device(format!(
                "unsupported sample format: {other:?}"
            )));
        }
    };
    stream
        .play()
        .map_err(|err| AudioError::Device(err.to_string()))?;
    Ok(stream)
}

fn select_device_by_name(host: &cpal::Host, name: &str) -> Option<cpal::Device> {
    host.input_devices().ok()?.find(|device| {
        device
            .description()
            .is_ok_and(|description| description.name() == name)
    })
}

/// Snapshot the names of currently available input devices for the settings UI.
pub fn list_input_device_names() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = host
        .input_devices()
        .map(|devices| {
            devices
                .filter_map(|device| device.description().ok())
                .map(|description| description.name().to_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    src_rate: u32,
    tx: crossbeam_channel::Sender<Vec<f32>>,
    barge_in: Arc<Mutex<bool>>,
    threshold: f32,
    tts_playing: Arc<AtomicBool>,
) -> Result<Stream, AudioError>
where
    T: cpal::Sample + cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels.max(1));
    let mut resampler = Resampler::new(src_rate);
    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mono: Vec<f32> = data
                    .chunks(channels)
                    .map(|frame| {
                        let sum: f32 = frame
                            .iter()
                            .map(|sample| cpal::Sample::to_sample::<f32>(*sample))
                            .sum();
                        sum / channels as f32
                    })
                    .collect();
                let mut at_16k = Vec::new();
                resampler.process(&mono, &mut at_16k);
                if at_16k.is_empty() {
                    return;
                }
                let energy = rms(&at_16k);
                *barge_in.lock() = energy >= threshold.max(SILENCE_RMS);
                let playing = tts_playing.load(Ordering::Relaxed);
                if !should_forward_mic(&at_16k, playing) {
                    return;
                }
                if tx.send(at_16k).is_err() {
                    // Mic consumer dropped.
                }
            },
            move |err| tracing::warn!(?err, "mic input stream error"),
            None,
        )
        .map_err(|err| AudioError::Device(err.to_string()))?;
    Ok(stream)
}
