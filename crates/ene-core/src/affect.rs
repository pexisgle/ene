//! Affect state domain model (PAD pleasure/arousal/dominance emotion state).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Discrete emotional label with intensity.
///
/// Paired with [`AffectState`] to provide both dimensional
/// (valence/arousal/dominance) and categorical affect representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscreteEmotion {
    /// Emotion label (e.g., "joy", "sadness", "anger", "fear", "surprise", "neutral").
    pub label: String,
    /// Intensity of the emotion (0.0–1.0).
    pub intensity: f32,
}

impl DiscreteEmotion {
    /// Create a new discrete emotion with intensity clamped to [0.0, 1.0].
    ///
    /// NaN and infinite inputs are clamped to 0.0.
    pub fn new(label: impl Into<String>, intensity: f32) -> Self {
        let intensity = if intensity.is_finite() {
            intensity.clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            label: label.into(),
            intensity,
        }
    }
}

/// Persistent affective / emotional state tracking three PAD dimensions
/// (Pleasure–Arousal–Dominance), relationship metrics, and optional discrete
/// emotion intensities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AffectState {
    /// Character identifier.
    pub character_id: String,
    /// User identifier (empty for character-global state).
    #[serde(default)]
    pub user_id: String,
    /// Pleasure–displeasure (-1.0 ..= 1.0).
    pub valence: f32,
    /// Excitement–calm (-1.0 ..= 1.0).
    pub arousal: f32,
    /// Control–submission (-1.0 ..= 1.0).
    pub dominance: f32,
    /// Trust toward the user (-1.0 ..= 1.0).
    pub trust: f32,
    /// Affinity / liking toward the user (-1.0 ..= 1.0).
    pub affinity: f32,
    /// Irritation / annoyance level (0.0 ..= 1.0).
    pub irritation: f32,
    /// Curiosity / interest level (0.0 ..= 1.0).
    pub curiosity: f32,
    /// Fatigue / energy depletion (0.0 ..= 1.0).
    pub fatigue: f32,
    /// Human-readable mood label (e.g. "cheerful", "anxious").
    #[serde(default)]
    pub mood_label: String,
    /// Natural-language description of the last expression/behaviour.
    #[serde(default)]
    pub last_expression: String,
    /// Discrete emotion intensities (joy, sadness, etc.).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discrete_emotions: Vec<DiscreteEmotion>,
    /// Last persistence timestamp (used for time-based decay).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

/// Serialized post-turn LLM classifier proposal waiting for next-turn merge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingAffectProposal {
    /// Character identifier.
    pub character_id: String,
    /// User identifier (empty when global).
    pub user_id: String,
    /// Turn index that produced this proposal.
    pub source_turn_id: i64,
    /// Detected user emotion label.
    pub user_emotion: String,
    /// Detected user intent label.
    pub user_intent: String,
    /// Estimated valence after the conversation (-1.0 ..= 1.0).
    pub valence: f32,
    /// Estimated arousal after the conversation (-1.0 ..= 1.0).
    pub arousal: f32,
    /// Estimated irritation after the conversation (0.0 ..= 1.0).
    pub irritation: f32,
    /// Estimated affinity after the conversation (-1.0 ..= 1.0).
    pub affinity: f32,
    /// Suggested expression name.
    pub recommended_expression: String,
    /// Proposal confidence in `[0.0, 1.0]`.
    pub confidence: f32,
    /// Human-readable reason from the classifier.
    pub reason: String,
    /// Creation timestamp for observability.
    pub created_at: DateTime<Utc>,
}

impl AffectState {
    /// Create a neutral affect state.
    pub fn neutral(character_id: impl Into<String>) -> Self {
        Self {
            character_id: character_id.into(),
            user_id: String::new(),
            valence: 0.0,
            arousal: 0.0,
            dominance: 0.0,
            trust: 0.0,
            affinity: 0.0,
            irritation: 0.0,
            curiosity: 0.0,
            fatigue: 0.0,
            mood_label: String::new(),
            last_expression: String::new(),
            discrete_emotions: Vec::new(),
            updated_at: None,
        }
    }

    /// Clamp all PAD and metric values to their valid ranges.
    ///
    /// NaN and infinite inputs are replaced with 0.0.
    pub fn clamp(&mut self) {
        const fn clamp_finite(v: &mut f32, min: f32, max: f32) {
            if v.is_finite() {
                *v = v.clamp(min, max);
            } else {
                *v = 0.0;
            }
        }
        clamp_finite(&mut self.valence, -1.0, 1.0);
        clamp_finite(&mut self.arousal, -1.0, 1.0);
        clamp_finite(&mut self.dominance, -1.0, 1.0);
        clamp_finite(&mut self.trust, -1.0, 1.0);
        clamp_finite(&mut self.affinity, -1.0, 1.0);
        clamp_finite(&mut self.irritation, 0.0, 1.0);
        clamp_finite(&mut self.curiosity, 0.0, 1.0);
        clamp_finite(&mut self.fatigue, 0.0, 1.0);
        for emo in &mut self.discrete_emotions {
            if emo.intensity.is_finite() {
                emo.intensity = emo.intensity.clamp(0.0, 1.0);
            } else {
                emo.intensity = 0.0;
            }
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
            user_id: String::new(),
            valence: 0.3,
            arousal: -0.1,
            dominance: 0.5,
            trust: 0.4,
            affinity: 0.6,
            irritation: 0.1,
            curiosity: 0.8,
            fatigue: 0.2,
            mood_label: "cheerful".into(),
            last_expression: "smiling warmly".into(),
            discrete_emotions: vec![
                DiscreteEmotion::new("joy", 0.7),
                DiscreteEmotion::new("surprise", 0.2),
            ],
            updated_at: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: AffectState = serde_json::from_str(&json).unwrap();
        assert!((state.valence - back.valence).abs() < f32::EPSILON);
        assert!((state.arousal - back.arousal).abs() < f32::EPSILON);
        assert!((state.dominance - back.dominance).abs() < f32::EPSILON);
        assert_eq!(state.discrete_emotions.len(), back.discrete_emotions.len());
        let state_joy = state
            .discrete_emotions
            .first()
            .expect("fixture has joy emotion");
        let back_joy = back
            .discrete_emotions
            .first()
            .expect("round-tripped fixture has joy emotion");
        assert_eq!(state_joy.label, back_joy.label);
        assert!((state_joy.intensity - back_joy.intensity).abs() < f32::EPSILON);
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
