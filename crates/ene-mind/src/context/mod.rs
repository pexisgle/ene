//! Context budget management and rolling compression.

mod budget;
mod compression;
mod tokens;

pub use budget::{
    BudgetMeta, ContextBudget, PackInput, PackedPrompt, estimate_history_tokens, pack_prompt,
};
pub use compression::{
    ActiveSceneSummary, CompressionLevel, CompressionReason, CompressionResult,
    CompressionTaskInput, MIN_MESSAGES_TO_COMPRESS, PendingCompressionTask,
    RetroactiveCompressionPlan, compression_has_usable_summary, evaluate_compression_trigger,
    execute_compression, load_active_scene_summary, maybe_roll_up_chapter,
    plan_retroactive_compression, poll_compression_result, spawn_compression_task,
};
pub use tokens::{
    estimate_tokens, estimate_tokens_language_aware, tokens_to_chars, truncate_to_tokens,
};

use std::sync::Arc;

use ene_ai::LlmProvider;
use ene_core::MemoryPort;

use crate::config::ContextConfig;
use crate::error::CognitionError;
use crate::lifecycle::{HistoryEntry, TurnContext};

/// Context budget manager and rolling compression orchestrator.
#[derive(Default)]
pub struct ContextManager {
    pending_compression: Option<PendingCompressionTask>,
}

impl ContextManager {
    /// Evaluate whether window-pressure compression should be triggered for the
    /// current session state (token-based).
    pub fn evaluate_compression_trigger(
        config: &ContextConfig,
        turn_count: usize,
        history_tokens: usize,
    ) -> Option<CompressionReason> {
        evaluate_compression_trigger(config, turn_count, history_tokens)
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

    /// Evaluate the window-pressure trigger and spawn a background compression
    /// task if warranted.
    ///
    /// This is the *secondary* trigger — a safety net for a topic that runs
    /// long without a detected boundary (the *primary* trigger is
    /// [`Self::check_and_trigger_retroactive`]). It compresses the oldest
    /// messages, keeping the recent window. Does nothing if a task is already
    /// pending or thresholds are not met.
    pub fn check_and_trigger(
        &mut self,
        config: &ContextConfig,
        turn_count: usize,
        history: &[HistoryEntry],
        session_id: &str,
        character_name: &str,
        user_name: &str,
        store: Arc<dyn MemoryPort>,
        provider: Arc<dyn LlmProvider>,
    ) {
        if self.pending_compression.is_some() {
            return;
        }
        let history_tokens = estimate_history_tokens(history);
        if evaluate_compression_trigger(config, turn_count, history_tokens).is_none() {
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

        self.spawn_scene_compression(
            config,
            session_id,
            character_name,
            user_name,
            turns,
            turn_start,
            turn_end,
            None,
            store,
            provider,
        );
    }

    /// Plan and spawn a retroactive compression for a detected topic boundary.
    ///
    /// Called from the deferred post-turn slot once the topic-boundary detector
    /// signals a boundary. The span *before* the boundary (the previous topic)
    /// is summarized into a single scene span; the boundary turn onward is
    /// retained, so the next turn sees "previous-topic summary + recent".
    /// Does nothing if a task is already pending or there is no pre-boundary
    /// span worth compressing.
    pub fn check_and_trigger_retroactive(
        &mut self,
        config: &ContextConfig,
        turn_count: usize,
        history: &[HistoryEntry],
        boundary_score: f32,
        session_id: &str,
        character_name: &str,
        user_name: &str,
        store: Arc<dyn MemoryPort>,
        provider: Arc<dyn LlmProvider>,
    ) {
        if self.pending_compression.is_some() {
            return;
        }
        let Some(plan) = plan_retroactive_compression(history, turn_count) else {
            return;
        };
        tracing::info!(
            component = "ContextCompression",
            session_id,
            score = boundary_score,
            messages = plan.turns.len(),
            turn_start = plan.turn_start,
            turn_end = plan.turn_end,
            "Topic boundary detected; compressing pre-boundary span"
        );
        self.spawn_scene_compression(
            config,
            session_id,
            character_name,
            user_name,
            plan.turns,
            plan.turn_start,
            plan.turn_end,
            Some(plan.drop_leading),
            store,
            provider,
        );
    }

    /// Build a scene-level [`CompressionTaskInput`] and spawn it as the pending
    /// task. Callers must have already checked [`Self::has_pending`].
    fn spawn_scene_compression(
        &mut self,
        config: &ContextConfig,
        session_id: &str,
        character_name: &str,
        user_name: &str,
        turns: Vec<HistoryEntry>,
        turn_start: i32,
        turn_end: i32,
        drop_leading: Option<usize>,
        store: Arc<dyn MemoryPort>,
        provider: Arc<dyn LlmProvider>,
    ) {
        let input = CompressionTaskInput {
            session_id: session_id.to_string(),
            character_name: character_name.to_string(),
            user_name: user_name.to_string(),
            turns,
            turn_start,
            turn_end,
            level: CompressionLevel::Scene,
            config: config.clone(),
            drop_leading,
        };
        spawn_compression_task(&mut self.pending_compression, store, provider, input);
    }

    /// Poll the pending compression task; returns `Some(result)` when complete.
    pub fn poll_pending(&mut self) -> Option<Result<CompressionResult, CognitionError>> {
        poll_compression_result(&mut self.pending_compression)
    }

    /// Execute compression synchronously (for manual triggers).
    pub async fn execute_manual(
        store: Arc<dyn MemoryPort>,
        provider: Arc<dyn LlmProvider>,
        input: CompressionTaskInput,
    ) -> Result<CompressionResult, CognitionError> {
        execute_compression(store, provider, input).await
    }

    /// Spawn a background chapter rollup task if scene span thresholds are exceeded.
    pub fn spawn_chapter_rollup(
        store: Arc<dyn MemoryPort>,
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
