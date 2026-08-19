//! Voice capture/playback hub for the stage client.

#[cfg(feature = "voice")]
mod capture;
#[cfg(feature = "voice")]
mod playback;

#[cfg(feature = "voice")]
pub use capture::MicCapture;
#[cfg(feature = "voice")]
pub use playback::AudioPlayback;

use ene_vrm::viseme::VisemeWeights;

/// Central audio IO facade (mic in, speaker out, viseme tap).
pub struct AudioHub {
    #[cfg(feature = "voice")]
    capture: MicCapture,
    #[cfg(feature = "voice")]
    playback: AudioPlayback,
}

impl AudioHub {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Poll mic chunks and forward them to the viseme analyzer path.
    pub fn poll_mic_chunks(&mut self) -> Vec<Vec<f32>> {
        #[cfg(feature = "voice")]
        {
            let mut out = Vec::new();
            while let Some(chunk) = self.capture.try_recv() {
                out.push(chunk);
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
    pub fn analyze_visemes(&mut self, analyzer: &mut ene_vrm::viseme::VisemeAnalyzer) -> VisemeWeights {
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
            Self {
                capture: MicCapture::new(None),
                playback: AudioPlayback::new(),
            }
        }
        #[cfg(not(feature = "voice"))]
        {
            Self {}
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
    pub fn new(_energy_threshold: Option<f32>) -> Self {
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
