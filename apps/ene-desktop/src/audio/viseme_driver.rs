//! Audio-driven viseme (lip-sync) driver.
//!
//! Wraps [`ene_vrm::VisemeAnalyzer`] (pure DSP, no audio I/O) so the
//! playback task can feed synthesized PCM while the render loop reads
//! the smoothed mouth-shape weights once per frame. The two sides meet
//! through the `parking_lot::Mutex` inside [`super::VisemeState`].

use ene_vrm::viseme::{VisemeAnalyzer, VisemeWeights};

/// Streaming PCM-to-viseme driver.
///
/// Feed audio with [`feed_pcm`](Self::feed_pcm) from the playback task
/// and call [`analyze_weights`](Self::analyze_weights) once per render
/// frame, handing the result to
/// [`ene_vrm::expression::ExpressionLayer::apply_viseme_weights`].
#[derive(Default)]
pub struct VisemeDriver {
    /// Analyzer sized to the most recently observed sample rate.
    /// `None` until the first PCM chunk arrives.
    analyzer: Option<VisemeAnalyzer>,
    sample_rate: u32,
}

impl VisemeDriver {
    /// Append a chunk of mono PCM (`[-1.0, 1.0]`) at `sample_rate` Hz.
    ///
    /// Rebuilds the internal analyzer when the sample rate changes
    /// (e.g. switching TTS providers mid-session).
    pub fn feed_pcm(&mut self, pcm: &[f32], sample_rate: u32) {
        if pcm.is_empty() {
            return;
        }
        if self.analyzer.is_none() || self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.analyzer = Some(VisemeAnalyzer::new(sample_rate));
        }
        if let Some(analyzer) = self.analyzer.as_mut() {
            analyzer.push_pcm(pcm);
        }
    }

    /// Analyze the buffered audio and return the smoothed mouth-shape
    /// weights, or `None` before any PCM has been fed.
    ///
    /// Intended to be called once per render frame.
    pub fn analyze_weights(&mut self) -> Option<VisemeWeights> {
        self.analyzer.as_mut().map(VisemeAnalyzer::analyze)
    }

    pub fn reset(&mut self) {
        if let Some(analyzer) = self.analyzer.as_mut() {
            analyzer.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, amp: f32, count: usize, rate: u32) -> Vec<f32> {
        (0..count)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn no_weights_before_feed() {
        let mut driver = VisemeDriver::default();
        assert!(driver.analyze_weights().is_none());
    }

    #[test]
    fn feed_produces_weights() {
        let mut driver = VisemeDriver::default();
        driver.feed_pcm(&sine(440.0, 0.5, 4096, 24_000), 24_000);
        assert!(driver.analyze_weights().is_some());
    }

    #[test]
    fn empty_feed_is_ignored() {
        let mut driver = VisemeDriver::default();
        driver.feed_pcm(&[], 24_000);
        assert!(driver.analyze_weights().is_none());
    }

    #[test]
    fn reset_clears_state() {
        let mut driver = VisemeDriver::default();
        driver.feed_pcm(&sine(440.0, 0.5, 4096, 24_000), 24_000);
        driver.reset();
        let weights = driver.analyze_weights();
        assert!(weights.is_some_and(|w| w.aa + w.ih + w.ou + w.ee + w.oh == 0.0));
    }
}
