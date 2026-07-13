//! Performance cues for chat presentation (#126).
//!
//! Lives in `ene-mind` so `ene-vrm` does not depend on mind/runtime.
//! Runtime re-exports these types for host apps. `CueSource::Host` is
//! intentionally absent until an explicit `perform` API exists.

use super::types::ExpressionSource;

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
    pub fn as_str(self) -> &'static str {
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

/// A single presentation cue (expression / emote name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceCue {
    /// Normalized cue name (e.g. `happy`, `neutral`).
    pub name: String,
}

impl PerformanceCue {
    /// Creates a cue from an expression or emotion name.
    #[must_use]
    pub fn expression(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}
