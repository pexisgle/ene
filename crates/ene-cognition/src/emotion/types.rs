//! Input/output types for the Emotion Engine (#86).

use std::time::Duration;

use ene_memory::AffectState;

/// Optional LLM affect classifier proposal (#88).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AffectProposal {
    /// Detected user emotion label.
    pub user_emotion: String,
    /// Detected user intent label.
    pub user_intent: String,
    /// Proposed valence delta.
    pub valence_delta: f32,
    /// Proposed arousal delta.
    pub arousal_delta: f32,
    /// Proposed irritation delta.
    pub irritation_delta: f32,
    /// Proposed affinity delta.
    pub affinity_delta: f32,
    /// Suggested expression name from the classifier.
    pub recommended_expression: String,
    /// Classifier confidence in `[0.0, 1.0]`.
    pub confidence: f32,
    /// Human-readable reason from the classifier.
    pub reason: String,
}

/// Per-field delta applied during a turn update.
#[derive(Debug, Clone, PartialEq)]
pub struct AffectDelta {
    /// Field name (e.g. `valence`, `irritation`).
    pub field: &'static str,
    /// Signed change applied.
    pub delta: f32,
}

/// Human-readable reason for an affect change.
#[derive(Debug, Clone, PartialEq)]
pub struct AffectUpdateReason {
    /// Short category label (e.g. `decay`, `gratitude`, `classifier`).
    pub category: &'static str,
    /// Detail message for tracing.
    pub detail: String,
    /// Field deltas attributed to this reason.
    pub deltas: Vec<AffectDelta>,
}

/// Input for a single turn affect update.
pub struct TurnAffectInput<'a> {
    /// Mutable affect state to update in place.
    pub state: &'a mut AffectState,
    /// Current user message text.
    pub user_message: &'a str,
    /// Elapsed time since the last persisted update.
    pub elapsed_since_update: Duration,
    /// Number of recent conversation turns (for fatigue heuristic).
    pub recent_turn_count: usize,
    /// Optional LLM classifier proposal (advisory only).
    pub classifier_proposal: Option<AffectProposal>,
    /// Minimum confidence to apply classifier deltas.
    pub classifier_min_confidence: f32,
    /// When true, skip deterministic appraisal (LLM-only mode).
    pub llm_only: bool,
}

impl<'a> TurnAffectInput<'a> {
    /// Attach a classifier proposal to this input.
    #[must_use]
    pub fn with_proposal(mut self, proposal: AffectProposal) -> Self {
        self.classifier_proposal = Some(proposal);
        self
    }
}

/// Result of a turn affect update.
#[derive(Debug, Clone)]
pub struct AffectUpdateResult {
    /// Recomputed mood label after update.
    pub mood_label: String,
    /// Traceable update reasons for debugging.
    pub reasons: Vec<AffectUpdateReason>,
}
