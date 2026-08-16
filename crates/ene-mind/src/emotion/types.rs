use std::time::Duration;

use ene_card::AffectBaseline;
use ene_core::AffectState;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AffectProposal {
    pub user_emotion: String,
    pub user_intent: String,
    /// Estimated valence after the conversation (-1.0 ..= 1.0).
    pub valence: f32,
    /// Estimated arousal after the conversation (-1.0 ..= 1.0).
    pub arousal: f32,
    /// Estimated irritation after the conversation (0.0 ..= 1.0).
    pub irritation: f32,
    /// Estimated affinity after the conversation (-1.0 ..= 1.0).
    pub affinity: f32,
    pub recommended_expression: String,
    /// Classifier confidence in `[0.0, 1.0]`.
    pub confidence: f32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AffectDelta {
    /// Field name (e.g. `valence`, `irritation`).
    pub field: &'static str,
    /// Signed change applied.
    pub delta: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AffectUpdateReason {
    /// Short category label (e.g. `decay`, `fatigue`, `classifier`).
    pub category: &'static str,
    /// Detail message for tracing.
    pub detail: String,
    pub deltas: Vec<AffectDelta>,
}

pub struct TurnAffectInput<'a> {
    pub state: &'a mut AffectState,
    /// Elapsed time since the last persisted update.
    pub elapsed_since_update: Duration,
    /// Number of recent conversation turns (for fatigue heuristic).
    pub recent_turn_count: usize,
    /// Resting affect that decay converges toward; all zeros when undefined.
    pub baseline: AffectBaseline,
    /// Optional LLM classifier proposal (advisory only).
    pub classifier_proposal: Option<AffectProposal>,
    /// Minimum confidence to blend classifier absolute estimates.
    pub classifier_min_confidence: f32,
}

impl TurnAffectInput<'_> {
    #[must_use]
    pub fn with_proposal(mut self, proposal: AffectProposal) -> Self {
        self.classifier_proposal = Some(proposal);
        self
    }
}

#[derive(Debug, Clone)]
pub struct AffectUpdateResult {
    pub mood_label: String,
    pub reasons: Vec<AffectUpdateReason>,
}
