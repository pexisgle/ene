//! Turn lifecycle input/output types for cognitive runtime integration (#100).

use std::sync::Arc;

use ene_config::CharacterCardV3;
use ene_memory::{ActiveCommitmentPrompt, AffectState, MemoryStore};
use ene_provider::{EmbeddingProvider, LlmMessage, LlmProvider};

use crate::config::CognitionConfig;
use crate::memory_writer::candidate::TurnInput;
use crate::recall::{RecallPlan, RecallTurn, RecalledMemory};

/// A single history entry for prompt composition.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// Speaker role label (`user`, `assistant`, `system`).
    pub role: String,
    /// Message text.
    pub content: String,
}

/// Input context for a single conversation turn.
pub struct TurnContext<'a> {
    /// Cognitive runtime configuration.
    pub config: &'a CognitionConfig,
    /// Character card for identity kernel compilation.
    pub card: &'a CharacterCardV3,
    /// Character / card identifier.
    pub character_id: &'a str,
    /// User display name.
    pub user_name: &'a str,
    /// Session identifier for logging.
    pub session_id: &'a str,
    /// Current user message.
    pub user_input: &'a str,
    /// Recent conversation history.
    pub history: &'a [HistoryEntry],
    /// Memory store (optional when memory disabled).
    pub store: Option<&'a MemoryStore>,
    /// Query embedding for recall (optional).
    pub query_embedding: Option<&'a [f32]>,
    /// Embedding provider for model name.
    pub embedder: Option<&'a Arc<dyn EmbeddingProvider>>,
    /// LLM provider for optional reranking.
    pub llm_provider: Option<Arc<dyn LlmProvider>>,
    /// Expression PHI block (emotion protocol + card post-history instructions).
    pub post_history_block: Option<&'a str>,
}

/// Output of pre-turn analysis and recall planning.
#[derive(Debug, Clone)]
pub struct PreTurnOutput {
    /// Generated recall plan.
    pub recall_plan: RecallPlan,
    /// Current affect state loaded or defaulted.
    pub affect: AffectState,
    /// Recalled typed memories for prompt injection.
    pub recalled: Vec<RecalledMemory>,
    /// Active commitments for prompt injection.
    pub commitments: Vec<ActiveCommitmentPrompt>,
}

/// Metadata about a composed prompt packet.
#[derive(Debug, Clone, Default)]
pub struct PromptPacketMeta {
    /// Whether the identity kernel section was included.
    pub identity_kernel_included: bool,
    /// Number of style examples injected.
    pub style_example_count: usize,
    /// Number of recalled memories injected.
    pub recalled_memory_count: usize,
    /// Whether the post-history PHI block was included.
    pub post_history_included: bool,
    /// Whether an active scene summary was included.
    pub scene_summary_included: bool,
    /// Sections dropped by the budget manager.
    pub dropped_sections: Vec<crate::prompt_packet::PromptSectionKind>,
    /// Approximate packed token count.
    pub packed_tokens: usize,
}

/// Result of prompt packet composition.
#[derive(Debug, Clone)]
pub struct ComposedPrompt {
    /// LLM messages ready for streaming.
    pub messages: Vec<LlmMessage>,
    /// Composition metadata for tracing/tests.
    pub meta: PromptPacketMeta,
}

/// Post-turn input for memory writing and affect persistence.
pub struct PostTurnInput<'a> {
    /// Turn extraction input.
    pub turn: TurnInput<'a>,
    /// Affect state after the turn.
    pub affect: AffectState,
    /// Character identifier.
    pub character_id: &'a str,
    /// User identifier.
    pub user_id: &'a str,
}

impl TurnContext<'_> {
    /// Build recall turns from history using configured recent turn limit.
    #[must_use]
    pub fn recent_recall_turns(&self) -> Vec<RecallTurn<'_>> {
        let limit = self.config.context.recent_turns.max(1);
        self.history
            .iter()
            .rev()
            .take(limit.saturating_mul(2))
            .rev()
            .map(|entry| RecallTurn {
                role: entry.role.as_str(),
                content: entry.content.as_str(),
            })
            .collect()
    }
}
