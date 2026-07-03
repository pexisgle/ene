use ene_memory::{ActiveCommitmentPrompt, AffectState};

/// A recent conversation turn available to the recall planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecallTurn<'a> {
    /// Speaker role, typically `"user"` or `"assistant"`.
    pub role: &'a str,
    /// Turn content.
    pub content: &'a str,
}

/// Inputs used to generate a recall plan for the current user turn.
#[derive(Debug, Clone)]
pub struct RecallPlannerInput<'a> {
    /// Current user message.
    pub user_input: &'a str,
    /// Recent raw conversation turns, ordered oldest to newest.
    pub recent_turns: &'a [RecallTurn<'a>],
    /// Active scene or rolling summary, when available.
    pub scene_summary: Option<&'a str>,
    /// Current affect state for the character/user pair.
    pub affect: Option<&'a AffectState>,
    /// Active commitments that should be considered independent of similarity.
    pub commitments: &'a [ActiveCommitmentPrompt],
    /// Character identifier for recall scope.
    pub character_id: &'a str,
    /// Optional user identifier for recall scope.
    pub user_id: Option<&'a str>,
}
