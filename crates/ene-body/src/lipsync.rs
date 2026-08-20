use crate::queue::Viseme;

/// Frame of mouth weights matching `ene-vrm` viseme targets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VisemeWeights {
    pub aa: f32,
    pub ih: f32,
    pub ou: f32,
    pub ee: f32,
    pub oh: f32,
}

impl VisemeWeights {
    #[must_use]
    pub fn amplitude(self) -> f32 {
        self.aa
            .max(self.ih)
            .max(self.ou)
            .max(self.ee)
            .max(self.oh)
            .clamp(0.0, 1.0)
    }

    #[must_use]
    pub fn dominant(self) -> Option<Viseme> {
        let pairs = [
            (self.aa, Viseme::Aa),
            (self.ih, Viseme::Ih),
            (self.ou, Viseme::Ou),
            (self.ee, Viseme::Ee),
            (self.oh, Viseme::Oh),
        ];
        let (amp, viseme) = pairs
            .into_iter()
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))?;
        (amp > 0.02).then_some(viseme)
    }
}

/// RMS / zero-crossing lip-sync. Same five targets as `ene_vrm::viseme`.
#[derive(Debug, Default)]
pub struct LipSyncAnalyzer {
    ema: f32,
}

impl LipSyncAnalyzer {
    #[must_use]
    pub fn analyze(pcm: &[f32]) -> VisemeWeights {
        Self::default().push(pcm)
    }

    pub fn push(&mut self, pcm: &[f32]) -> VisemeWeights {
        if pcm.is_empty() {
            self.ema *= 0.5;
            return VisemeWeights::default();
        }
        let mut sum = 0.0f32;
        let mut zc = 0u32;
        let mut prev = 0.0f32;
        for (i, sample) in pcm.iter().copied().enumerate() {
            sum += sample * sample;
            if i > 0 && prev.signum() != sample.signum() && sample.abs() > 0.001 {
                zc += 1;
            }
            prev = sample;
        }
        let rms = (sum / pcm.len() as f32).sqrt();
        let zcr = zc as f32 / pcm.len() as f32;
        let attack = if rms > self.ema { 0.6 } else { 0.35 };
        self.ema = self.ema + (rms - self.ema) * attack;
        if self.ema < 0.005 {
            return VisemeWeights::default();
        }
        let open = (self.ema / 0.2).clamp(0.0, 1.0);
        if zcr > 0.2 {
            VisemeWeights {
                ee: open * 0.8,
                ih: open * 0.4,
                ..VisemeWeights::default()
            }
        } else if zcr < 0.04 {
            VisemeWeights {
                ou: open * 0.7,
                oh: open * 0.5,
                ..VisemeWeights::default()
            }
        } else {
            VisemeWeights {
                aa: open,
                oh: open * 0.3,
                ..VisemeWeights::default()
            }
        }
    }
}
