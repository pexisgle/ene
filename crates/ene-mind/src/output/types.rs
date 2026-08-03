//! Types for the Expression Arbiter.

use std::time::Duration;

use ene_config::ResolvedExpression;
use ene_core::AffectState;

/// Source of the resolved expression decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpressionSource {
    /// LLM proposal is canonical when present.
    Llm,
    /// Affect mapping used when no LLM proposal was available.
    AffectFallback,
    /// Previous expression held due to hysteresis.
    HysteresisHold,
    /// Fallback to neutral or nearest supported expression.
    FallbackNeutral,
}

impl ExpressionSource {
    /// Debug string for event emission.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Llm => "llm",
            Self::AffectFallback => "affect",
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
    /// True when `llm_proposal` came from an explicit streamed `[expr:...]`
    /// marker; classifier hints pass `false`.
    pub explicit_proposal: bool,
    /// Previous resolved expression name.
    pub previous_expression: &'a str,
    /// Elapsed time since the last expression change.
    pub elapsed_since_change: Option<Duration>,
    /// Current irritation level spike overrides hysteresis when true.
    pub irritation_spike: bool,
}

/// Resolved expression output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpressionDecision {
    /// Normalized expression name.
    pub expression: String,
    /// Human-readable reason for tracing / UX.
    pub reason: String,
    /// How the expression was chosen.
    pub source: ExpressionSource,
}
