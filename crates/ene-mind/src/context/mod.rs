//! Context budget management and rolling compression (#79, #80, #81).

mod budget;
mod compression;
mod tokens;

pub use budget::{
    BudgetMeta, ContextBudget, PackInput, PackedPrompt, pack_prompt, validate_context_config,
};
pub use compression::{
    ActiveSceneSummary, CompressionLevel, CompressionReason, CompressionResult,
    CompressionTaskInput, MIN_MESSAGES_TO_COMPRESS, PendingCompressionTask,
    compression_has_usable_summary, evaluate_compression_trigger, execute_compression,
    load_active_scene_summary, maybe_roll_up_chapter, poll_compression_result,
    spawn_compression_task,
};
pub use tokens::{estimate_tokens, tokens_to_chars, truncate_to_tokens};

use std::sync::Arc;

use ene_ai::LlmProvider;
use ene_store::MemoryStore;

use crate::config::ContextConfig;
use crate::error::CognitionError;
use crate::lifecycle::{HistoryEntry, TurnContext};

/// Context budget manager and rolling compression orchestrator.
#[derive(Default)]
pub struct ContextManager {
    pending_compression: Option<PendingCompressionTask>,
}

impl ContextManager {
    /// Validate context configuration at startup.
    pub fn validate_config(config: &ContextConfig) -> Result<(), CognitionError> {
        validate_context_config(config)
    }

    /// Evaluate whether compression should be triggered for the current session state.
    pub fn evaluate_compression_trigger(
        config: &ContextConfig,
        turn_count: usize,
        history_len: usize,
    ) -> Option<CompressionReason> {
        evaluate_compression_trigger(config, turn_count, history_len)
    }

    /// Load the active scene summary for prompt injection.
    pub async fn load_scene_summary(
        ctx: TurnContext<'_>,
    ) -> Result<Option<ActiveSceneSummary>, CognitionError> {
        let store = ctx.store.ok_or_else(|| {
            CognitionError::MissingProvider("Memory store required for scene summary".into())
        })?;
        load_active_scene_summary(store, ctx.session_id).await
    }

    /// Whether a background compression task is in flight.
    pub fn has_pending(&self) -> bool {
        self.pending_compression.is_some()
    }

    /// Evaluate trigger conditions and spawn a background compression task if warranted.
    ///
    /// Does nothing if a task is already pending or thresholds are not met.
    pub fn check_and_trigger(
        &mut self,
        config: &ContextConfig,
        turn_count: usize,
        history: &[HistoryEntry],
        session_id: &str,
        character_name: &str,
        user_name: &str,
        store: Arc<MemoryStore>,
        provider: Arc<dyn LlmProvider>,
    ) {
        if self.pending_compression.is_some() {
            return;
        }
        if evaluate_compression_trigger(config, turn_count, history.len()).is_none() {
            return;
        }

        let recent_cap = config.recent_turns.saturating_mul(2).max(2);
        if history.len() <= recent_cap {
            return;
        }
        let compress_count = history.len().saturating_sub(recent_cap);
        if compress_count < MIN_MESSAGES_TO_COMPRESS {
            return;
        }
        let turns: Vec<HistoryEntry> = history[..compress_count].to_vec();
        let turn_end = i32::try_from(turn_count).unwrap_or(i32::MAX);
        let compress_msg_count = i32::try_from(compress_count / 2).unwrap_or(i32::MAX).max(1);
        let turn_start = (turn_end - compress_msg_count).max(0);

        let input = CompressionTaskInput {
            session_id: session_id.to_string(),
            character_name: character_name.to_string(),
            user_name: user_name.to_string(),
            turns,
            turn_start,
            turn_end,
            level: CompressionLevel::Scene,
            config: config.clone(),
        };
        spawn_compression_task(&mut self.pending_compression, store, provider, input);
    }

    /// Poll the pending compression task; returns `Some(result)` when complete.
    pub fn poll_pending(&mut self) -> Option<Result<CompressionResult, CognitionError>> {
        poll_compression_result(&mut self.pending_compression)
    }

    /// Execute compression synchronously (for manual triggers).
    pub async fn execute_manual(
        store: Arc<MemoryStore>,
        provider: Arc<dyn LlmProvider>,
        input: CompressionTaskInput,
    ) -> Result<CompressionResult, CognitionError> {
        execute_compression(store, provider, input).await
    }

    /// Spawn a background chapter rollup task if scene span thresholds are exceeded.
    pub fn spawn_chapter_rollup(
        store: Arc<MemoryStore>,
        provider: Arc<dyn LlmProvider>,
        session_id: String,
        character_name: String,
        user_name: String,
        config: ContextConfig,
    ) {
        tokio::spawn(async move {
            if let Err(error) = maybe_roll_up_chapter(
                store.as_ref(),
                provider,
                &session_id,
                &character_name,
                &user_name,
                &config,
            )
            .await
            {
                tracing::warn!(
                    component = "ContextCompression",
                    error = %error,
                    "Chapter rollup failed"
                );
            }
        });
    }
}
