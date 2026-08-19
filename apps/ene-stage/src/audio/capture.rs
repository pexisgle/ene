//! Microphone capture via cpal.

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use crossbeam_channel::{Receiver, unbounded};
use parking_lot::Mutex;

use super::AudioError;

/// Default RMS threshold for barge-in detection.
const DEFAULT_BARGE_IN_THRESHOLD: f32 = 0.02;

/// Streaming microphone input that forwards `f32` PCM chunks.
pub struct MicCapture {
    _stream: Option<Stream>,
    rx: Receiver<Vec<f32>>,
    barge_in: Arc<Mutex<bool>>,
}

impl MicCapture {
    /// Open the default input device. `energy_threshold` overrides the barge-in RMS gate.
    pub fn new(energy_threshold: Option<f32>) -> Self {
        let threshold = energy_threshold.unwrap_or(DEFAULT_BARGE_IN_THRESHOLD);
        let (tx, rx) = unbounded();
        let barge_in = Arc::new(Mutex::new(false));

        match open_stream(tx, Arc::clone(&barge_in), threshold) {
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

    /// Non-blocking receive of the next PCM chunk.
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
) -> Result<Stream, AudioError> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| AudioError::Device("no input device".to_owned()))?;
    let config = device
        .default_input_config()
        .map_err(|err| AudioError::Device(err.to_string()))?;
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(&device, stream_config, tx, barge_in, threshold)?,
        SampleFormat::I16 => build_stream::<i16>(&device, stream_config, tx, barge_in, threshold)?,
        SampleFormat::U16 => build_stream::<u16>(&device, stream_config, tx, barge_in, threshold)?,
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

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    tx: crossbeam_channel::Sender<Vec<f32>>,
    barge_in: Arc<Mutex<bool>>,
    threshold: f32,
) -> Result<Stream, AudioError>
where
    T: cpal::Sample + cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels.max(1));
    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let mono: Vec<f32> = data
                    .chunks(channels)
                    .map(|frame| {
                        let sum: f32 = frame
                            .iter()
                            .map(|s| cpal::Sample::to_sample::<f32>(*s))
                            .sum();
                        sum / channels as f32
                    })
                    .collect();
                let energy = rms(&mono);
                *barge_in.lock() = energy >= threshold;
                if tx.send(mono).is_err() {
                    // Mic consumer dropped.
                }
            },
            move |err| tracing::warn!(?err, "mic input stream error"),
            None,
        )
        .map_err(|err| AudioError::Device(err.to_string()))?;
    Ok(stream)
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}
