//! Turn lifecycle input/output types for cognitive runtime integration.

use std::sync::Arc;

use ene_ai::{EmbeddingProvider, LlmMessage, LlmProvider, Role};
use ene_config::CharacterCardV3;
use ene_core::{ActiveCommitmentPrompt, AffectState, MemoryPort};

use crate::character::StyleExample;
use crate::config::MindConfig;
use crate::memory_writer::candidate::{ToolResultSummary, TurnInput};
use crate::recall::{RecallPlan, RecallTurn, RecalledMemory};
use crate::session::InterruptedState;

/// Builds the system-prompt note describing a previously interrupted response.
///
/// Injected into the next turn so the model can naturally resume or acknowledge
/// the interruption instead of restarting from scratch.
#[must_use]
pub fn interruption_note(state: &InterruptedState) -> String {
    format!(
        "The previous response was interrupted mid-sentence. The assistant had spoken up to: '{}'. The assistant should naturally resume or acknowledge the interruption.",
        state.partial_text
    )
}

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
    /// Index of the greeting this session started with (`0` = `first_mes`,
    /// `i+1` = `alternate_greetings[i]`); `None` when no greeting was chosen.
    /// Drives `@@is_greeting` lorebook gating.
    pub greeting_index: Option<u32>,
    /// Memory store (optional when memory disabled).
    pub store: Option<&'a dyn MemoryPort>,
    /// Query embedding for recall (optional).
    pub query_embedding: Option<&'a [f32]>,
    /// Embedding provider for model name.
    pub embedder: Option<&'a Arc<dyn EmbeddingProvider>>,
    /// LLM provider for memory extraction and other cognitive calls.
    pub llm_provider: Option<Arc<dyn LlmProvider>>,
    /// Prompt tokens the model's context window leaves for this turn.
    ///
    /// Scales window-relative budgets such as the Identity Kernel.
    /// `None` when the window is unknown (tests / legacy callers), in which
    /// case a conservative default window is assumed.
    pub available_window: Option<usize>,
    /// Expression PHI block (emotion protocol + card post-history instructions).
    pub post_history_block: Option<&'a str>,
    /// Whether a rolling-compression task is in flight and its summary has not
    /// yet been applied to the session history.
    ///
    /// Prompt packing reads this to synchronously detach the oldest span from
    /// the prompt-visible history while the summary is in flight — see
    /// [`crate::context::PackInput::compression_pending`] for the contract.
    /// `false` in tests / legacy callers, which keeps the pre-existing packing
    /// behavior.
    pub compression_pending: bool,
    /// Optional override for the prompt packing budget (in tokens).
    ///
    /// When `None` (production), the budget is derived from the model's
    /// effective context window. Tests set this to inject a
    /// deterministic budget and force drop/trim behaviour without depending on
    /// the global AI config or a live provider.
    pub packing_budget_override: Option<usize>,
    /// Raw topic hint from the proactive decision, used to recall memories
    /// relevant to what the generation turn will talk about.
    ///
    /// `Some` only on proactive generation turns; `None` for ordinary turns
    /// and tests. The recall is lexical-only (`Query::embedding = None`) so
    /// the proactive path never blocks on an embedder.
    pub proactive_topic: Option<&'a str>,
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
    /// Classifier expression hint when confidence meets threshold.
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
    /// Oldest history messages *detached* from the prompt while a compression
    /// summary was pending: the `[0, n)` leading range was dropped from
    /// the prompt-visible history synchronously, without waiting for the
    /// summary. Like `dropped_sections` and the packing trim, this is a
    /// prompt-copy-only operation — nothing is removed from the session/DB
    /// history; the detached messages stay in the log until a later
    /// compression covers them.
    pub history_messages_detached: usize,
    /// Approximate packed token count.
    pub packed_tokens: usize,
    /// IDs of recalled memories that actually made it into the packed prompt.
    ///
    /// Subset of the recalled set that survived budget trimming. The recall
    /// access bump is gated on this list: only memories actually
    /// composed into the packed prompt get their `access_count` /
    /// `last_accessed_at` updated, so ranking high in search but being dropped
    /// by the budget no longer reinforces a memory.
    pub injected_memory_ids: Vec<i64>,
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
    /// Context note about a previously interrupted response, injected into the
    /// system prompt so the model can acknowledge or resume it.
    /// `Some(None)` means the caller checked and there is no pending interruption.
    pub interruption_note: Option<Option<String>>,
}

/// Post-turn input for memory writing and affect persistence.
pub struct PostTurnInput<'a> {
    /// Turn extraction input.
    pub turn: TurnInput<'a>,
    /// Affect state after the turn (borrowed to avoid cloning in the
    /// deferred/owned path, see [`OwnedPostTurnInput::as_borrowed`]).
    pub affect: &'a AffectState,
    /// Character identifier.
    pub character_id: &'a str,
    /// User identifier.
    pub user_id: &'a str,
    /// Source turn that produced this post-turn input, when available.
    ///
    /// Persisted on deferred memory candidates so the approval UI can point
    /// back at the conversation. `None` for retried writes (old payloads)
    /// and tests.
    pub source_turn: Option<&'a str>,
    /// Whether the turn was interrupted (barge-in / cancel) before completion.
    pub interrupted: bool,
    /// The partial assistant text produced before interruption, if any.
    pub spoken_text: Option<&'a str>,
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
    /// Source turn that produced this post-turn input, when available.
    ///
    /// `#[serde(default)]` keeps pending-write payloads persisted before this
    /// field existed deserializable on retry.
    #[serde(default)]
    pub source_turn: Option<String>,
    /// Whether the turn was interrupted (barge-in / cancel) before completion.
    #[serde(default)]
    pub interrupted: bool,
    /// The partial assistant text produced before interruption, if any.
    #[serde(default)]
    pub spoken_text: Option<String>,
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
            affect: &self.affect,
            character_id: &self.character_id,
            user_id: &self.user_id,
            source_turn: self.source_turn.as_deref(),
            interrupted: self.interrupted,
            spoken_text: self.spoken_text.as_deref(),
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
