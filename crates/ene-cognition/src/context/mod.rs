//! Context budget management and rolling compression (#79, #80, #81).

mod budget;
mod compression;
mod tokens;

pub use budget::{
    BudgetMeta, ContextBudget, PackInput, PackedPrompt, pack_prompt, validate_context_config,
};
pub use compression::{
    ActiveSceneSummary, CompressionLevel, CompressionReason, CompressionResult,
    CompressionTaskInput, PendingCompressionTask, MIN_MESSAGES_TO_COMPRESS,
    compression_has_usable_summary, evaluate_compression_trigger,
    execute_compression, load_active_scene_summary, maybe_roll_up_chapter,
    poll_compression_result, spawn_compression_task,
};
pub use tokens::{estimate_tokens, tokens_to_chars, truncate_to_tokens};

use crate::config::ContextConfig;
use crate::error::CognitionError;
use crate::lifecycle::TurnContext;

/// Context budget manager and rolling compression orchestrator.
#[derive(Debug, Default, Clone, Copy)]
pub struct ContextManager;

impl ContextManager {
    /// Validate context configuration at startup.
    pub fn validate_config(config: &ContextConfig) -> Result<(), CognitionError> {
        validate_context_config(config)
    }

    /// Evaluate whether compression should be triggered for the current session state.
    #[must_use]
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
            CognitionError::Other("Memory store required for scene summary".into())
        })?;
        load_active_scene_summary(store, ctx.session_id).await
    }
}
