//! Types for the Expression Arbiter (#89).

use std::time::Duration;

use ene_config::ResolvedExpression;
use ene_store::AffectState;

/// Source of the resolved expression decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionSource {
    /// Mapped from current affect state (PAD dimensions).
    AffectMapping,
    /// LLM token used as advisory hint.
    LlmAdvisory,
    /// LLM token treated as a direct command.
    LlmCommand,
    /// Previous expression held due to hysteresis.
    HysteresisHold,
    /// Fallback to neutral or nearest supported expression.
    FallbackNeutral,
}

impl ExpressionSource {
    /// Debug string for event emission (#91).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AffectMapping => "affect",
            Self::LlmAdvisory => "llm_advisory",
            Self::LlmCommand => "llm_command",
            Self::HysteresisHold => "hysteresis",
            Self::FallbackNeutral => "fallback",
        }
    }
}

/// Input for expression resolution.
pub struct ExpressionInput<'a> {
    /// Current affect state after pre-turn update.
    pub affect: &'a AffectState,
    /// Available expressions from the character card.
    pub available: &'a [ResolvedExpression],
    /// Optional LLM expression proposal from streamed tokens.
    pub llm_proposal: Option<&'a str>,
    /// Previous resolved expression name.
    pub previous_expression: &'a str,
    /// Elapsed time since the last expression change.
    pub elapsed_since_change: Option<Duration>,
    /// Assistant response text (lightweight sentiment hints).
    pub response_text: &'a str,
    /// Current irritation level spike overrides hysteresis when true.
    pub irritation_spike: bool,
}

/// Resolved expression output.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpressionDecision {
    /// Normalized expression name.
    pub expression: String,
    /// Human-readable reason for tracing / UX.
    pub reason: String,
    /// How the expression was chosen.
    pub source: ExpressionSource,
}
