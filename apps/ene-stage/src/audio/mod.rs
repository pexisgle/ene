//! Voice capture/playback hub for the stage client.

mod dsp;
mod stream;

#[cfg(feature = "voice")]
mod capture;
#[cfg(feature = "voice")]
mod playback;

#[cfg(feature = "voice")]
pub use capture::MicCapture;
#[cfg(feature = "voice")]
pub use playback::AudioPlayback;

pub use dsp::{
    BARGE_IN_ENERGY_FACTOR, CAPTURE_SAMPLE_RATE, LISTEN_FRAME_SAMPLES, SILENCE_RMS,
    should_forward_mic,
};
pub use stream::run_listen_stream;

use ene_vrm::viseme::VisemeWeights;

use self::dsp::push_coalesced;

/// Central audio IO facade (mic in, speaker out, viseme tap).
pub struct AudioHub {
    #[cfg(feature = "voice")]
    capture: MicCapture,
    #[cfg(feature = "voice")]
    playback: AudioPlayback,
    pending: Vec<f32>,
}

impl AudioHub {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Coalesced 16 kHz frames for the listen stream (~100 ms each).
    pub fn poll_mic_batches(&mut self) -> Vec<Vec<f32>> {
        #[cfg(feature = "voice")]
        {
            self.playback.tick_playback();
            let tts = self.playback.is_tts_playing();
            if tts {
                self.pending.clear();
            }
            let mut out = Vec::new();
            while let Some(chunk) = self.capture.try_recv() {
                if !should_forward_mic(&chunk, tts) {
                    continue;
                }
                if tts {
                    out.push(chunk);
                    continue;
                }
                push_coalesced(&mut self.pending, &chunk, LISTEN_FRAME_SAMPLES, &mut out);
            }
            out
        }
        #[cfg(not(feature = "voice"))]
        {
            Vec::new()
        }
    }

    /// Play mono/stereo-interleaved PCM at `sample_rate`.
    pub fn play_pcm(&mut self, samples: &[f32], sample_rate: u32) -> Result<(), AudioError> {
        #[cfg(feature = "voice")]
        {
            self.playback.play_pcm(samples, sample_rate)
        }
        #[cfg(not(feature = "voice"))]
        {
            let _ = (samples, sample_rate);
            Ok(())
        }
    }

    /// Stop playback and clear the viseme PCM buffer (no-op without `voice`).
    pub fn stop(&mut self) {
        #[cfg(feature = "voice")]
        {
            self.playback.stop();
        }
    }

    /// Recent playback PCM for lip-sync analysis.
    #[must_use]
    pub fn playback_pcm(&self) -> Vec<f32> {
        #[cfg(feature = "voice")]
        {
            self.playback.recent_pcm()
        }
        #[cfg(not(feature = "voice"))]
        {
            Vec::new()
        }
    }

    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        #[cfg(feature = "voice")]
        {
            self.playback.sample_rate()
        }
        #[cfg(not(feature = "voice"))]
        {
            48_000
        }
    }

    /// Whether local TTS playback is still queued (echo-aware barge-in).
    #[must_use]
    pub fn is_tts_playing(&self) -> bool {
        #[cfg(feature = "voice")]
        {
            self.playback.is_tts_playing()
        }
        #[cfg(not(feature = "voice"))]
        {
            false
        }
    }

    /// Whether mic energy exceeded the barge-in threshold.
    #[must_use]
    pub fn mic_barge_in(&self) -> bool {
        #[cfg(feature = "voice")]
        {
            self.capture.barge_in_active()
        }
        #[cfg(not(feature = "voice"))]
        {
            false
        }
    }

    /// Analyze playback audio into viseme weights (no-op without `voice`).
    pub fn analyze_visemes(
        &mut self,
        analyzer: &mut ene_vrm::viseme::VisemeAnalyzer,
    ) -> VisemeWeights {
        #[cfg(feature = "voice")]
        {
            let pcm = self.playback.recent_pcm();
            analyzer.push_pcm(&pcm);
            analyzer.analyze()
        }
        #[cfg(not(feature = "voice"))]
        {
            let _ = analyzer;
            VisemeWeights::default()
        }
    }
}

impl Default for AudioHub {
    fn default() -> Self {
        #[cfg(feature = "voice")]
        {
            let playback = AudioPlayback::new();
            let capture = MicCapture::new(None, playback.tts_playing_flag());
            Self {
                capture,
                playback,
                pending: Vec::new(),
            }
        }
        #[cfg(not(feature = "voice"))]
        {
            Self {
                pending: Vec::new(),
            }
        }
    }
}

/// Stub mic capture when the `voice` feature is disabled.
#[cfg(not(feature = "voice"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct MicCapture;

#[cfg(not(feature = "voice"))]
impl MicCapture {
    #[must_use]
    pub fn new(
        _energy_threshold: Option<f32>,
        _tts_playing: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self
    }

    pub fn try_recv(&self) -> Option<Vec<f32>> {
        None
    }

    #[must_use]
    pub fn barge_in_active(&self) -> bool {
        false
    }
}

/// Stub playback when the `voice` feature is disabled.
#[cfg(not(feature = "voice"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct AudioPlayback;

#[cfg(not(feature = "voice"))]
impl AudioPlayback {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    pub fn play_pcm(&mut self, _samples: &[f32], _sample_rate: u32) -> Result<(), AudioError> {
        Ok(())
    }

    pub fn stop(&mut self) {}

    #[must_use]
    pub fn recent_pcm(&self) -> Vec<f32> {
        Vec::new()
    }

    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        48_000
    }
}

/// Audio subsystem errors.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[cfg(feature = "voice")]
    #[error("audio device: {0}")]
    Device(String),
    #[error("playback failed: {0}")]
    Playback(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_clears_recent_playback_pcm() {
        let mut hub = AudioHub::new();
        hub.play_pcm(&[0.25, -0.5, 0.75], 16_000)
            .expect("enqueue pcm");
        #[cfg(feature = "voice")]
        assert!(!hub.playback_pcm().is_empty());
        hub.stop();
        assert!(hub.playback_pcm().is_empty());
    }
}
