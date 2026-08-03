//! Performance cues for chat presentation.
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
    /// LLM marker / expression proposal.
    Llm,
    /// Previous expression held due to hysteresis.
    Hysteresis,
    /// Fallback to neutral or nearest supported expression.
    Fallback,
}

impl CueSource {
    /// Stable debug / log label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Affect => "affect",
            Self::Llm => "llm",
            Self::Hysteresis => "hysteresis",
            Self::Fallback => "fallback",
        }
    }
}

impl From<ExpressionSource> for CueSource {
    fn from(value: ExpressionSource) -> Self {
        match value {
            ExpressionSource::AffectFallback => Self::Affect,
            ExpressionSource::Llm => Self::Llm,
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

/// Default expression weight when a cue omits `weight`.
pub const DEFAULT_EXPRESSION_WEIGHT: f32 = 1.0;
/// Default expression hold when a cue omits `hold` (seconds).
pub const DEFAULT_EXPRESSION_HOLD_SECS: f64 = 4.0;

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
    /// Character offset of the marker in the turn's clean (marker-stripped)
    /// spoken text. Set when the cue came from a streamed `<|perf:…|>` marker;
    /// lets the TTS pipeline attribute the cue to the sentence whose text
    /// range contains it, so hosts can switch the expression while speaking.
    pub text_offset: Option<usize>,
}

impl PerformanceCue {
    /// Creates a cue from an expression or emotion name.
    pub fn expression(name: impl Into<String>) -> Self {
        Self {
            kind: PerfKind::Expression,
            name: name.into(),
            weight: None,
            hold_secs: None,
            motion_layer: None,
            text_offset: None,
        }
    }

    /// Creates an expression cue with explicit weight and hold.
    pub fn expression_with(name: impl Into<String>, weight: f32, hold_secs: f64) -> Self {
        Self {
            kind: PerfKind::Expression,
            name: name.into(),
            weight: Some(weight.clamp(0.0, 1.0)),
            hold_secs: Some(hold_secs.max(0.0)),
            motion_layer: None,
            text_offset: None,
        }
    }

    /// Creates a motion cue.
    pub fn motion(name: impl Into<String>, layer: Option<MotionLayer>) -> Self {
        Self {
            kind: PerfKind::Motion,
            name: name.into(),
            weight: None,
            hold_secs: None,
            motion_layer: layer,
            text_offset: None,
        }
    }

    /// Creates a look-at cue.
    pub fn look_at(target: impl Into<String>) -> Self {
        Self {
            kind: PerfKind::LookAt,
            name: target.into(),
            weight: None,
            hold_secs: None,
            motion_layer: None,
            text_offset: None,
        }
    }

    /// Creates a cancel cue.
    pub fn cancel(scope: impl Into<String>) -> Self {
        Self {
            kind: PerfKind::Cancel,
            name: scope.into(),
            weight: None,
            hold_secs: None,
            motion_layer: None,
            text_offset: None,
        }
    }

    /// Records the marker's character offset in the clean spoken text.
    #[must_use]
    pub fn with_text_offset(mut self, offset: usize) -> Self {
        self.text_offset = Some(offset);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_text_offset_records_position() {
        let cue = PerformanceCue::expression("happy").with_text_offset(42);
        assert_eq!(cue.text_offset, Some(42));
        assert_eq!(cue.name, "happy");
    }

    #[test]
    fn constructors_start_without_text_offset() {
        assert_eq!(PerformanceCue::expression("happy").text_offset, None);
        assert_eq!(PerformanceCue::motion("wave", None).text_offset, None);
    }

    #[test]
    fn with_text_offset_is_pure() {
        let base = PerformanceCue::expression("sad");
        let moved = base.clone().with_text_offset(7);
        assert_eq!(base.text_offset, None);
        assert_eq!(moved.text_offset, Some(7));
    }
}
