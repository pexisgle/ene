//! Performance cues for chat presentation (#126, #128).
//!
//! Lives in `ene-mind` so `ene-vrm` does not depend on mind/runtime.
//! Runtime re-exports these types for host apps. `CueSource::Host` is
//! intentionally absent until an explicit `perform` API exists.

use super::types::ExpressionSource;
pub use ene_config::MotionLayer;

/// Origin of a [`PerformanceCue`] batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueSource {
    /// Mapped from current affect state (PAD dimensions).
    Affect,
    /// LLM marker token used as an advisory hint.
    LlmAdvisory,
    /// LLM marker token treated as a direct command.
    LlmCommand,
    /// Previous expression held due to hysteresis.
    Hysteresis,
    /// Fallback to neutral or nearest supported expression.
    Fallback,
}

impl CueSource {
    /// Stable debug / log label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Affect => "affect",
            Self::LlmAdvisory => "llm_advisory",
            Self::LlmCommand => "llm_command",
            Self::Hysteresis => "hysteresis",
            Self::Fallback => "fallback",
        }
    }
}

impl From<ExpressionSource> for CueSource {
    fn from(value: ExpressionSource) -> Self {
        match value {
            ExpressionSource::AffectMapping => Self::Affect,
            ExpressionSource::LlmAdvisory => Self::LlmAdvisory,
            ExpressionSource::LlmCommand => Self::LlmCommand,
            ExpressionSource::HysteresisHold => Self::Hysteresis,
            ExpressionSource::FallbackNeutral => Self::Fallback,
        }
    }
}

/// Kind of a [`PerformanceCue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PerfKind {
    /// Expression / emotion blend-shape.
    Expression,
    /// Motion / animation playback.
    Motion,
    /// Look-at target directive.
    LookAt,
    /// Cancel a running expression or motion.
    Cancel,
}

/// A single presentation cue (expression / motion / look-at / cancel).
#[derive(Debug, Clone, PartialEq)]
pub struct PerformanceCue {
    /// Cue kind.
    pub kind: PerfKind,
    /// Cue name (expression name, motion name, look-at target, or cancel scope).
    pub name: String,
    /// Target expression weight `[0, 1]` (Expression only).
    pub weight: Option<f32>,
    /// Hold duration in seconds (Expression only).
    pub hold_secs: Option<f64>,
    /// Motion body layer (Motion only).
    pub motion_layer: Option<MotionLayer>,
}

impl PerformanceCue {
    /// Creates a cue from an expression or emotion name.
    #[must_use]
    pub fn expression(name: impl Into<String>) -> Self {
        Self {
            kind: PerfKind::Expression,
            name: name.into(),
            weight: None,
            hold_secs: None,
            motion_layer: None,
        }
    }

    /// Creates an expression cue with explicit weight and hold.
    #[must_use]
    pub fn expression_with(name: impl Into<String>, weight: f32, hold_secs: f64) -> Self {
        Self {
            kind: PerfKind::Expression,
            name: name.into(),
            weight: Some(weight.clamp(0.0, 1.0)),
            hold_secs: Some(hold_secs.max(0.0)),
            motion_layer: None,
        }
    }

    /// Creates a motion cue.
    #[must_use]
    pub fn motion(name: impl Into<String>, layer: Option<MotionLayer>) -> Self {
        Self {
            kind: PerfKind::Motion,
            name: name.into(),
            weight: None,
            hold_secs: None,
            motion_layer: layer,
        }
    }

    /// Creates a look-at cue.
    #[must_use]
    pub fn look_at(target: impl Into<String>) -> Self {
        Self {
            kind: PerfKind::LookAt,
            name: target.into(),
            weight: None,
            hold_secs: None,
            motion_layer: None,
        }
    }

    /// Creates a cancel cue.
    #[must_use]
    pub fn cancel(scope: impl Into<String>) -> Self {
        Self {
            kind: PerfKind::Cancel,
            name: scope.into(),
            weight: None,
            hold_secs: None,
            motion_layer: None,
        }
    }
}
