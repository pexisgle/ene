use serde::{Deserialize, Serialize};

/// Discrete emotional label with intensity.
///
/// Paired with `AffectState` to provide both dimensional (valence/arousal/dominance)
/// and categorical (joy, sadness, etc.) affect representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscreteEmotion {
    /// Emotion label (e.g., "joy", "sadness", "anger", "fear", "surprise", "neutral").
    pub label: String,
    /// Intensity of the emotion (0.0–1.0).
    pub intensity: f32,
}

impl DiscreteEmotion {
    /// Create a new discrete emotion with intensity clamped to [0.0, 1.0].
    #[must_use]
    pub fn new(label: impl Into<String>, intensity: f32) -> Self {
        Self {
            label: label.into(),
            intensity: intensity.clamp(0.0, 1.0),
        }
    }
}

/// Persistent affective / emotional state.
///
/// Tracks three PAD dimensions (Pleasure–Arousal–Dominance) and optional
/// discrete emotion intensities. The engine updates this every turn and
/// persists it so it survives restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AffectState {
    /// Character identifier.
    pub character_id: String,
    /// Pleasure–displeasure (-1.0 ..= 1.0).
    pub valence: f32,
    /// Excitement–calm (-1.0 ..= 1.0).
    pub arousal: f32,
    /// Control–submission (-1.0 ..= 1.0).
    pub dominance: f32,
    /// Discrete emotion intensities (joy, sadness, etc.).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discrete_emotions: Vec<DiscreteEmotion>,
}

impl AffectState {
    /// Create a neutral affect state.
    #[must_use]
    pub fn neutral(character_id: impl Into<String>) -> Self {
        Self {
            character_id: character_id.into(),
            valence: 0.0,
            arousal: 0.0,
            dominance: 0.0,
            discrete_emotions: Vec::new(),
        }
    }

    /// Clamp all PAD values to their valid ranges.
    pub fn clamp(&mut self) {
        self.valence = self.valence.clamp(-1.0, 1.0);
        self.arousal = self.arousal.clamp(-1.0, 1.0);
        self.dominance = self.dominance.clamp(-1.0, 1.0);
        for emo in &mut self.discrete_emotions {
            emo.intensity = emo.intensity.clamp(0.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affect_state_serde_roundtrip() {
        let state = AffectState {
            character_id: "ene".into(),
            valence: 0.3,
            arousal: -0.1,
            dominance: 0.5,
            discrete_emotions: vec![
                DiscreteEmotion::new("joy", 0.7),
                DiscreteEmotion::new("surprise", 0.2),
            ],
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: AffectState = serde_json::from_str(&json).unwrap();
        assert!((state.valence - back.valence).abs() < f32::EPSILON);
        assert!((state.arousal - back.arousal).abs() < f32::EPSILON);
        assert!((state.dominance - back.dominance).abs() < f32::EPSILON);
        assert_eq!(state.discrete_emotions.len(), back.discrete_emotions.len());
        assert_eq!(
            state.discrete_emotions[0].label,
            back.discrete_emotions[0].label
        );
        assert!(
            (state.discrete_emotions[0].intensity - back.discrete_emotions[0].intensity).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn neutral_state_clamps() {
        let mut state = AffectState::neutral("test");
        state.valence = 2.0;
        state.arousal = -5.0;
        state.dominance = 100.0;
        state.clamp();
        assert!((state.valence - 1.0).abs() < f32::EPSILON);
        assert!((state.arousal + 1.0).abs() < f32::EPSILON);
        assert!((state.dominance - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn discrete_emotion_clamps() {
        let emo = DiscreteEmotion::new("joy", 1.5);
        assert!((emo.intensity - 1.0).abs() < f32::EPSILON);
        let emo2 = DiscreteEmotion::new("sadness", -0.5);
        assert!((emo2.intensity - 0.0).abs() < f32::EPSILON);
    }
}
