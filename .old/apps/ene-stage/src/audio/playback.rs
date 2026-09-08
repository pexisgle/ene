//! Speaker playback via rodio.

use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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
    tts_playing: Arc<AtomicBool>,
    playback_until: Mutex<Option<Instant>>,
}

impl Default for AudioPlayback {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioPlayback {
    pub fn new() -> Self {
        let recent_pcm = Arc::new(Mutex::new(Vec::with_capacity(RECENT_PCM_CAP)));
        let tts_playing = Arc::new(AtomicBool::new(false));
        match DeviceSinkBuilder::open_default_sink() {
            Ok(sink) => {
                let player = Player::connect_new(sink.mixer());
                Self {
                    _sink: Some(sink),
                    player: Some(player),
                    recent_pcm,
                    sample_rate: 48_000,
                    tts_playing,
                    playback_until: Mutex::new(None),
                }
            }
            Err(err) => {
                tracing::warn!(?err, "audio output unavailable; using silent playback");
                Self {
                    _sink: None,
                    player: None,
                    recent_pcm,
                    sample_rate: 48_000,
                    tts_playing,
                    playback_until: Mutex::new(None),
                }
            }
        }
    }

    #[must_use]
    pub fn tts_playing_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.tts_playing)
    }

    pub fn play_pcm(&mut self, samples: &[f32], sample_rate: u32) -> Result<(), AudioError> {
        self.sample_rate = sample_rate.max(1);
        self.note_playback(samples.len(), self.sample_rate);
        {
            let mut recent = self.recent_pcm.lock();
            recent.extend_from_slice(samples);
            if recent.len() > RECENT_PCM_CAP {
                let drain = recent.len() - RECENT_PCM_CAP;
                recent.drain(0..drain);
            }
        }

        if samples.is_empty() {
            return Ok(());
        }

        let Some(player) = self.player.as_ref() else {
            return Ok(());
        };
        let channels = NonZero::new(1)
            .ok_or_else(|| AudioError::Playback("invalid channel count".to_owned()))?;
        let rate = NonZero::new(self.sample_rate)
            .ok_or_else(|| AudioError::Playback("invalid sample rate".to_owned()))?;
        let buffer = SamplesBuffer::new(channels, rate, samples.to_vec());
        player.append(buffer);
        Ok(())
    }

    /// Drop queued PCM and stop the sink immediately (barge-in abort).
    pub fn stop(&mut self) {
        self.recent_pcm.lock().clear();
        if let Some(player) = self.player.as_ref() {
            player.stop();
        }
        *self.playback_until.lock() = None;
        self.tts_playing.store(false, Ordering::Relaxed);
    }

    pub fn tick_playback(&self) {
        let until = self.playback_until.lock();
        if until.is_none_or(|deadline| Instant::now() >= deadline) {
            self.tts_playing.store(false, Ordering::Relaxed);
        }
    }

    #[must_use]
    pub fn is_tts_playing(&self) -> bool {
        self.tick_playback();
        self.tts_playing.load(Ordering::Relaxed)
    }

    fn note_playback(&self, n_samples: usize, sample_rate: u32) {
        if n_samples == 0 {
            *self.playback_until.lock() = None;
            self.tts_playing.store(false, Ordering::Relaxed);
            return;
        }
        let seconds = n_samples as f64 / f64::from(sample_rate.max(1));
        let dur = Duration::from_secs_f64(seconds);
        let mut until = self.playback_until.lock();
        let start = until
            .filter(|deadline| *deadline > Instant::now())
            .unwrap_or_else(Instant::now);
        *until = Some(start + dur);
        self.tts_playing.store(true, Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_idle_with_a_positive_sample_rate() {
        let mut playback = AudioPlayback::new();
        assert!(playback.sample_rate() > 0);
        assert!(!playback.is_tts_playing());
        assert!(!playback.tts_playing_flag().load(Ordering::Relaxed));
        playback.stop();
        assert!(!playback.is_tts_playing());
        assert!(playback.recent_pcm().is_empty());
        playback.note_playback(0, playback.sample_rate());
        assert!(!playback.is_tts_playing());
    }
}
