use serde::{Deserialize, Serialize};

/// Discrete emotion cue from the soul (no PAD numbers).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmotionCue {
    pub label: String,
    pub intensity: f32,
}

/// Performance-protocol command consumed by a body adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PerformanceCommand {
    Expression {
        label: String,
        intensity: f32,
        duration_ms: Option<u32>,
    },
    Motion {
        name: String,
        layer: MotionLayer,
        intensity: Option<f32>,
    },
    LookAt {
        target: LookTarget,
        weight: f32,
    },
    LipSync {
        amplitude: f32,
        viseme: Option<Viseme>,
    },
    Posture {
        pose: Posture,
        blend: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionLayer {
    Base,
    Overlay,
    OneShot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LookTarget {
    User,
    Away,
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Posture {
    Relax,
    Alert,
    Thinking,
}

/// Mouth-shape names matching `ene-vrm` viseme targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Viseme {
    Aa,
    Ih,
    Ou,
    Ee,
    Oh,
}

impl Viseme {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Aa => "aa",
            Self::Ih => "ih",
            Self::Ou => "ou",
            Self::Ee => "ee",
            Self::Oh => "oh",
        }
    }
}

/// Coarse vitality for client-side autonomy (blink / breath / gaze).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vitality {
    Exhausted,
    Tired,
    Neutral,
    Lively,
    Wired,
}

impl Vitality {
    #[must_use]
    pub fn from_arousal_fatigue(arousal: f32, fatigue: f32) -> Self {
        if fatigue >= 0.75 {
            return Self::Exhausted;
        }
        if fatigue >= 0.45 || arousal < -0.4 {
            return Self::Tired;
        }
        if arousal > 0.55 {
            return Self::Wired;
        }
        if arousal > 0.2 {
            return Self::Lively;
        }
        Self::Neutral
    }
}
