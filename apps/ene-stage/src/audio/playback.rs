//! Speaker playback via rodio.

use std::num::NonZero;
use std::sync::Arc;

use parking_lot::Mutex;
use rodio::buffer::SamplesBuffer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player};

use super::AudioError;

const RECENT_PCM_CAP: usize = 48_000;

/// Local speaker playback with a rolling PCM buffer for viseme analysis.
pub struct AudioPlayback {
    _sink: Option<MixerDeviceSink>,
    player: Option<Player>,
    recent_pcm: Arc<Mutex<Vec<f32>>>,
    sample_rate: u32,
}

impl AudioPlayback {
    pub fn new() -> Self {
        let recent_pcm = Arc::new(Mutex::new(Vec::with_capacity(RECENT_PCM_CAP)));
        match DeviceSinkBuilder::open_default_sink() {
            Ok(sink) => {
                let player = Player::connect_new(sink.mixer());
                Self {
                    _sink: Some(sink),
                    player: Some(player),
                    recent_pcm,
                    sample_rate: 48_000,
                }
            }
            Err(err) => {
                tracing::warn!(?err, "audio output unavailable; using silent playback");
                Self {
                    _sink: None,
                    player: None,
                    recent_pcm,
                    sample_rate: 48_000,
                }
            }
        }
    }

    pub fn play_pcm(&mut self, samples: &[f32], sample_rate: u32) -> Result<(), AudioError> {
        self.sample_rate = sample_rate.max(1);
        {
            let mut recent = self.recent_pcm.lock();
            recent.extend_from_slice(samples);
            if recent.len() > RECENT_PCM_CAP {
                let drain = recent.len() - RECENT_PCM_CAP;
                recent.drain(0..drain);
            }
        }

        let Some(player) = self.player.as_ref() else {
            return Ok(());
        };
        let channels = NonZero::new(1).ok_or_else(|| {
            AudioError::Playback("invalid channel count".to_owned())
        })?;
        let rate = NonZero::new(self.sample_rate).ok_or_else(|| {
            AudioError::Playback("invalid sample rate".to_owned())
        })?;
        let buffer = SamplesBuffer::new(channels, rate, samples.to_vec());
        player.append(buffer);
        Ok(())
    }

    #[must_use]
    pub fn recent_pcm(&self) -> Vec<f32> {
        self.recent_pcm.lock().clone()
    }

    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
