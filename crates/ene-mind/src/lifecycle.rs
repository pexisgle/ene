//! Turn lifecycle input/output types for cognitive runtime integration (#100).

use std::sync::Arc;

use ene_ai::{EmbeddingProvider, LlmMessage, LlmProvider, Role};
use ene_config::CharacterCardV3;
use ene_store::{ActiveCommitmentPrompt, AffectState, MemoryStore};

use crate::character::StyleExample;
use crate::config::MindConfig;
use crate::memory_writer::candidate::{ToolResultSummary, TurnInput};
use crate::recall::{RecallPlan, RecallTurn, RecalledMemory};

/// A single history entry for prompt composition.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// Speaker role.
    pub role: Role,
    /// Message text.
    pub content: String,
}

impl HistoryEntry {
    /// Stable string label for recall / logging (`user`, `assistant`, `system`).
    pub const fn role_label(&self) -> &'static str {
        match self.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
        }
    }
}

/// Input context for a single conversation turn.
pub struct TurnContext<'a> {
    /// Cognitive runtime configuration.
    pub config: &'a MindConfig,
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
    /// LLM provider for memory extraction and other cognitive calls.
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
    /// Classifier expression hint when confidence meets threshold (#88).
    pub classifier_expression_hint: Option<String>,
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

/// Optional prefetched inputs for [`crate::CognitionEngine::compose_prompt_packet`].
///
/// When a field is `None`, compose fetches it internally (test / legacy callers).
/// When `Some`, the prefetched value is used and the internal fetch is skipped.
#[derive(Debug, Clone, Default)]
pub struct ComposePrefetch {
    /// Prefetched style examples (`Some(vec![])` means none selected).
    pub style_examples: Option<Vec<StyleExample>>,
    /// Prefetched scene summary text (`Some(None)` means no active scene).
    pub scene_summary: Option<Option<String>>,
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

/// Owned turn input for deferred post-turn memory writing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OwnedTurnInput {
    /// The user's message text.
    pub user_message: String,
    /// The assistant's response (if available).
    pub assistant_message: Option<String>,
    /// Tool call results from this turn.
    pub tool_results: Vec<ToolResultSummary>,
}

/// Owned post-turn input for deferred memory writing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OwnedPostTurnInput {
    /// Turn extraction input.
    pub turn: OwnedTurnInput,
    /// Affect state after the turn.
    pub affect: AffectState,
    /// Character identifier.
    pub character_id: String,
    /// User identifier.
    pub user_id: String,
}

impl OwnedPostTurnInput {
    /// Borrows as a [`PostTurnInput`] for synchronous memory-writer calls.
    pub fn as_borrowed(&self) -> PostTurnInput<'_> {
        PostTurnInput {
            turn: TurnInput {
                user_message: &self.turn.user_message,
                assistant_message: self.turn.assistant_message.as_deref(),
                tool_results: &self.turn.tool_results,
            },
            affect: self.affect.clone(),
            character_id: &self.character_id,
            user_id: &self.user_id,
        }
    }
}

impl TurnContext<'_> {
    /// Build recall turns from history using configured recent turn limit.
    pub fn recent_recall_turns(&self) -> Vec<RecallTurn<'_>> {
        let limit = self.config.context.recent_turns.max(1);
        self.history
            .iter()
            .rev()
            .take(limit.saturating_mul(2))
            .rev()
            .map(|entry| RecallTurn {
                role: entry.role_label(),
                content: entry.content.as_str(),
            })
            .collect()
    }
}
