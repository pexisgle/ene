//! Rolling context compression (#79).

use std::sync::Arc;

use ene_ai::LlmProvider;
use ene_config::PromptLibrary;
use ene_core::{MemoryPort, NewMemorySpan};
use tokio::sync::oneshot;
use tokio::time::{Duration, timeout};

use crate::config::ContextConfig;
use crate::error::CognitionError;
use crate::lifecycle::HistoryEntry;

/// Compression hierarchy levels stored in `memory_spans.compression_level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum CompressionLevel {
    /// Scene-level summary of recent raw turns.
    Scene = 0,
    /// Chapter summary aggregating multiple scenes.
    Chapter = 1,
    /// Arc summary aggregating multiple chapters.
    Arc = 2,
}

impl CompressionLevel {
    /// Database value for this level.
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// Parse from database value.
    pub const fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Scene),
            1 => Some(Self::Chapter),
            2 => Some(Self::Arc),
            _ => None,
        }
    }
}

/// Minimum number of history messages that must be eligible before auto-compression runs.
pub const MIN_MESSAGES_TO_COMPRESS: usize = 4;

/// Why compression was triggered.
#[derive(Debug, Clone, PartialEq)]
pub enum CompressionReason {
    /// Turn count exceeded the scene threshold.
    TurnThreshold {
        /// Number of turns in session.
        turn_count: usize,
    },
    /// Window pressure: the retained history's estimated token count reached
    /// the configured ceiling (#368). This is the token-based replacement for
    /// the former message-count ratio heuristic.
    ContextPressure {
        /// Estimated tokens of the retained history when the trigger fired.
        history_tokens: usize,
    },
    /// Manual trigger from CLI or API.
    Manual,
}

/// Active scene summary for prompt injection.
#[derive(Debug, Clone)]
pub struct ActiveSceneSummary {
    /// Summary text for the prompt packet.
    pub text: String,
    /// Span database id when known.
    pub span_id: Option<i64>,
    /// Compression level of this summary.
    pub level: CompressionLevel,
}

/// Input for a background compression task.
#[derive(Debug, Clone)]
pub struct CompressionTaskInput {
    /// Session identifier.
    pub session_id: String,
    /// Character name for summarization.
    pub character_name: String,
    /// User display name.
    pub user_name: String,
    /// Turns to compress into a scene span.
    pub turns: Vec<HistoryEntry>,
    /// Turn index range start (inclusive).
    pub turn_start: i32,
    /// Turn index range end (inclusive).
    pub turn_end: i32,
    /// Target compression level.
    pub level: CompressionLevel,
    /// Context configuration snapshot.
    pub config: ContextConfig,
    /// Number of *leading* history messages the compressed span occupies (#368).
    /// `None` trims to the configured recent window on apply; `Some(n)` drops
    /// the `n` leading messages (used by retroactive topic-boundary
    /// compression to keep the boundary turn onward).
    pub drop_leading: Option<usize>,
}

/// Result of a completed compression task.
#[derive(Debug, Clone)]
pub struct CompressionResult {
    /// Session that was compressed.
    pub session_id: String,
    /// Inserted or updated span id.
    pub span_id: i64,
    /// Generated summary text (empty when summarization failed).
    pub summary: String,
    /// Compression level written.
    pub level: CompressionLevel,
    /// Number of *leading* history messages the compressed span occupied (#368).
    /// `None` for window-pressure/manual compression, which trims to the
    /// configured recent window; `Some(n)` for retroactive topic-boundary
    /// compression, which drops the `n` leading messages (the pre-boundary
    /// span) and keeps the boundary turn onward.
    pub drop_leading: Option<usize>,
}

/// Whether a compression result produced a usable summary for prompt injection.
pub fn compression_has_usable_summary(result: &CompressionResult) -> bool {
    !result.summary.trim().is_empty()
}

/// Handle for an in-flight compression task.
pub struct PendingCompressionTask {
    receiver: oneshot::Receiver<Result<CompressionResult, CognitionError>>,
}

/// Execute compression synchronously (for manual triggers and tests).
pub async fn execute_compression(
    store: Arc<dyn MemoryPort>,
    provider: Arc<dyn LlmProvider>,
    input: CompressionTaskInput,
) -> Result<CompressionResult, CognitionError> {
    run_compression(store, provider, input).await
}

/// Spawn a background compression task.
pub fn spawn_compression_task(
    pending: &mut Option<PendingCompressionTask>,
    store: Arc<dyn MemoryPort>,
    provider: Arc<dyn LlmProvider>,
    input: CompressionTaskInput,
) {
    let (tx, rx) = oneshot::channel();
    *pending = Some(PendingCompressionTask { receiver: rx });

    tokio::spawn(async move {
        let result = run_compression(store, provider, input).await;
        drop(tx.send(result));
    });
}

/// Poll a pending compression task; returns `Some(result)` when complete.
pub fn poll_compression_result(
    pending: &mut Option<PendingCompressionTask>,
) -> Option<Result<CompressionResult, CognitionError>> {
    let task = pending.as_mut()?;
    match task.receiver.try_recv() {
        Ok(result) => {
            *pending = None;
            Some(result)
        }
        Err(oneshot::error::TryRecvError::Empty) => None,
        Err(oneshot::error::TryRecvError::Closed) => {
            *pending = None;
            Some(Err(CognitionError::Other(
                "Compression task channel closed".into(),
            )))
        }
    }
}

/// Evaluate whether window-pressure compression should run for the current
/// turn (#368).
///
/// This is the *secondary* trigger — a token-based safety net for topics that
/// run long without a detected boundary. The *primary* trigger is the
/// retroactive topic-boundary compression planned by
/// [`plan_retroactive_compression`]. The window-pressure term compares the
/// retained history's estimated token count against
/// [`ContextConfig::context_pressure_tokens`] (replacing the former
/// message-count ratio heuristic, which ignored token cost entirely); the
/// turn-count term is retained as a coarse backstop.
pub fn evaluate_compression_trigger(
    config: &ContextConfig,
    turn_count: usize,
    history_tokens: usize,
) -> Option<CompressionReason> {
    if turn_count >= config.scene_turn_threshold {
        return Some(CompressionReason::TurnThreshold { turn_count });
    }
    if history_tokens >= config.context_pressure_tokens {
        return Some(CompressionReason::ContextPressure { history_tokens });
    }
    None
}

/// A planned retroactive compression: the span of history *before* a detected
/// topic boundary, to be summarized into a single scene span (#368).
///
/// The boundary turn itself is the first turn of the *new* topic, so it is
/// retained; everything strictly before it is compressed. The resulting
/// history is "previous-topic summary" (injected as the scene section at
/// composition time) plus "the boundary turn onward".
#[derive(Debug, Clone)]
pub struct RetroactiveCompressionPlan {
    /// History entries before the boundary, to be summarized.
    pub turns: Vec<HistoryEntry>,
    /// Turn index range start (inclusive) of the compressed span.
    pub turn_start: i32,
    /// Turn index range end (inclusive) of the compressed span.
    pub turn_end: i32,
    /// Number of *leading* history messages the span occupies. Once the summary
    /// is usable, exactly this many leading messages are dropped, leaving the
    /// boundary turn onward. Expressed as a leading-drop count (not an absolute
    /// keep count) so it stays correct even if the summary completes after
    /// later turns have appended to history.
    pub drop_leading: usize,
}

/// Plan a retroactive compression for a detected topic boundary (#368).
///
/// `history` is the committed history *including* the boundary turn (the last
/// user/assistant exchange), and `turn_count` is the session's completed turn
/// count. The boundary turn opens the new topic, so the compressible span is
/// everything before the final exchange. Returns `None` when there is no
/// pre-boundary span worth compressing (fewer than [`MIN_MESSAGES_TO_COMPRESS`]
/// messages before the boundary), so a boundary on the first topic is a no-op.
pub fn plan_retroactive_compression(
    history: &[HistoryEntry],
    turn_count: usize,
) -> Option<RetroactiveCompressionPlan> {
    // The boundary turn is the final exchange; everything before it is the
    // previous topic. Guard against a trailing partial (user-only) exchange.
    let has_final_exchange = history.len() >= 2
        && matches!(
            history[history.len() - 2].role,
            ene_ai::Role::User | ene_ai::Role::System
        )
        && matches!(history[history.len() - 1].role, ene_ai::Role::Assistant);
    let boundary_start = if has_final_exchange {
        history.len() - 2
    } else {
        history.len().saturating_sub(1)
    };
    if boundary_start < MIN_MESSAGES_TO_COMPRESS {
        return None;
    }

    let turns = history[..boundary_start].to_vec();
    let turn_end = i32::try_from(turn_count.saturating_sub(1)).unwrap_or(i32::MAX);
    let span_turns = i32::try_from(boundary_start / 2).unwrap_or(i32::MAX).max(1);
    let turn_start = (turn_end - span_turns).max(0);

    Some(RetroactiveCompressionPlan {
        turns,
        turn_start,
        turn_end,
        drop_leading: boundary_start,
    })
}

/// Load the active scene summary for prompt injection.
pub async fn load_active_scene_summary(
    store: &dyn MemoryPort,
    session_id: &str,
) -> Result<Option<ActiveSceneSummary>, CognitionError> {
    let row = store
        .get_active_scene_summary(session_id)
        .await
        .map_err(CognitionError::MemoryPort)?;
    Ok(row.map(|r| ActiveSceneSummary {
        text: r.summary,
        span_id: Some(r.span_id),
        level: CompressionLevel::from_i32(r.compression_level).unwrap_or(CompressionLevel::Scene),
    }))
}

async fn run_compression(
    store: Arc<dyn MemoryPort>,
    provider: Arc<dyn LlmProvider>,
    input: CompressionTaskInput,
) -> Result<CompressionResult, CognitionError> {
    let raw_excerpt = render_turn_excerpt(&input.turns);
    let summary = summarize_span(
        provider.as_ref(),
        &input.character_name,
        &input.user_name,
        &raw_excerpt,
        input.level,
        input.config.compression_timeout_secs,
        &input.config.compression_language,
    )
    .await;

    let span = NewMemorySpan {
        session_id: input.session_id.clone(),
        turn_start: input.turn_start,
        turn_end: input.turn_end,
        raw_excerpt: Some(raw_excerpt),
        compressed_summary: summary.clone(),
        compression_level: input.level.as_i32(),
    };

    let span_id = store
        .insert_memory_span(&span)
        .await
        .map_err(CognitionError::MemoryPort)?;

    Ok(CompressionResult {
        session_id: input.session_id,
        span_id,
        summary: summary.unwrap_or_default(),
        level: input.level,
        drop_leading: input.drop_leading,
    })
}

fn render_turn_excerpt(turns: &[HistoryEntry]) -> String {
    turns
        .iter()
        .map(|t| format!("{}: {}", t.role_label(), t.content))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn summarize_span(
    provider: &dyn LlmProvider,
    character_name: &str,
    user_name: &str,
    excerpt: &str,
    level: CompressionLevel,
    timeout_secs: u64,
    language: &str,
) -> Option<String> {
    if excerpt.trim().is_empty() {
        return None;
    }

    let level_label = match level {
        CompressionLevel::Scene => "scene",
        CompressionLevel::Chapter => "chapter",
        CompressionLevel::Arc => "arc",
    };

    let prompts = PromptLibrary::load(language);
    let system = prompts
        .compression()
        .render_system(character_name, level_label);
    let user = format!("User: {user_name}\n\nConversation excerpt:\n{excerpt}");

    let messages = vec![
        ene_ai::LlmMessage::System { content: system },
        ene_ai::LlmMessage::User {
            parts: vec![ene_ai::UserMessagePart::Text { text: user }],
        },
    ];

    let duration = Duration::from_secs(timeout_secs.max(5));
    let result = timeout(duration, provider.chat_completion(&messages, None)).await;

    match result {
        Ok(Ok(completion)) => {
            let trimmed = completion.text.trim();
            if trimmed.is_empty() || trimmed == excerpt.trim() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Ok(Err(error)) => {
            tracing::warn!(
                component = "ContextCompression",
                error = %error,
                "LLM compression summarization failed"
            );
            None
        }
        Err(_) => {
            tracing::warn!(
                component = "ContextCompression",
                timeout_secs,
                "LLM compression summarization timed out"
            );
            None
        }
    }
}

/// Roll up scene spans into a chapter summary when thresholds are exceeded.
pub async fn maybe_roll_up_chapter(
    store: &dyn MemoryPort,
    provider: Arc<dyn LlmProvider>,
    session_id: &str,
    character_name: &str,
    user_name: &str,
    config: &ContextConfig,
) -> Result<Option<CompressionResult>, CognitionError> {
    let scenes = store
        .list_memory_spans_by_session_and_level(session_id, CompressionLevel::Scene.as_i32())
        .await
        .map_err(CognitionError::MemoryPort)?;

    if scenes.len() < config.chapter_span_threshold {
        return Ok(None);
    }

    let summaries: Vec<String> = scenes
        .iter()
        .filter_map(|s| s.compressed_summary.clone())
        .collect();
    if summaries.is_empty() {
        return Ok(None);
    }

    let excerpt = summaries.join("\n\n");
    let summary = summarize_span(
        provider.as_ref(),
        character_name,
        user_name,
        &excerpt,
        CompressionLevel::Chapter,
        config.compression_timeout_secs,
        &config.compression_language,
    )
    .await;

    let turn_start = scenes.first().map_or(0, |s| s.turn_start);
    let turn_end = scenes.last().map_or(0, |s| s.turn_end);
    let span = NewMemorySpan {
        session_id: session_id.to_string(),
        turn_start,
        turn_end,
        raw_excerpt: Some(excerpt),
        compressed_summary: summary.clone(),
        compression_level: CompressionLevel::Chapter.as_i32(),
    };
    let span_id = store
        .insert_memory_span(&span)
        .await
        .map_err(CognitionError::MemoryPort)?;

    Ok(Some(CompressionResult {
        session_id: session_id.to_string(),
        span_id,
        summary: summary.unwrap_or_default(),
        level: CompressionLevel::Chapter,
        drop_leading: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(role: ene_ai::Role, content: &str) -> HistoryEntry {
        HistoryEntry {
            role,
            content: content.to_string(),
        }
    }

    /// An alternating user/assistant history of `turns` exchanges.
    fn exchanges(turns: usize) -> Vec<HistoryEntry> {
        let mut history = Vec::with_capacity(turns * 2);
        for i in 0..turns {
            history.push(entry(ene_ai::Role::User, &format!("user {i}")));
            history.push(entry(ene_ai::Role::Assistant, &format!("assistant {i}")));
        }
        history
    }

    #[test]
    fn trigger_fires_on_turn_threshold() {
        let config = ContextConfig::default();
        let reason = evaluate_compression_trigger(&config, config.scene_turn_threshold, 0);
        assert!(matches!(
            reason,
            Some(CompressionReason::TurnThreshold { .. })
        ));
    }

    #[test]
    fn trigger_fires_on_token_pressure() {
        let config = ContextConfig {
            scene_turn_threshold: 999,
            context_pressure_tokens: 100,
            ..ContextConfig::default()
        };
        let reason = evaluate_compression_trigger(&config, 1, 100);
        assert!(matches!(
            reason,
            Some(CompressionReason::ContextPressure {
                history_tokens: 100
            })
        ));
        // Below the token ceiling and the turn threshold: no trigger.
        assert!(evaluate_compression_trigger(&config, 1, 99).is_none());
    }

    #[test]
    fn retroactive_plan_compresses_span_before_boundary_turn() {
        // Three completed exchanges; the last is the boundary turn. The span
        // before it (exchanges 0 and 1) is the previous topic.
        let history = exchanges(3);
        let plan = plan_retroactive_compression(&history, 3).expect("span before boundary");

        assert_eq!(plan.turns.len(), 4);
        assert_eq!(plan.drop_leading, 4, "drops the two pre-boundary exchanges");
        assert_eq!(plan.turn_start, 0);
        assert_eq!(plan.turn_end, 2);
        // The compressed span is exactly the pre-boundary prefix. `HistoryEntry`
        // intentionally does not derive `PartialEq`, so compare field by field.
        assert_eq!(plan.turns.len(), history[..4].len());
        for (got, want) in plan.turns.iter().zip(&history[..4]) {
            assert_eq!(got.role_label(), want.role_label());
            assert_eq!(got.content, want.content);
        }
    }

    #[test]
    fn retroactive_plan_is_none_when_topic_too_short() {
        // Only one exchange before the boundary turn: below the floor.
        let history = exchanges(2);
        assert!(plan_retroactive_compression(&history, 2).is_none());
        // A boundary on the very first topic has nothing before it.
        let first = exchanges(1);
        assert!(plan_retroactive_compression(&first, 1).is_none());
    }

    #[test]
    fn retroactive_plan_handles_trailing_partial_exchange() {
        // A trailing user-only message (mid-turn snapshot) is not treated as
        // the boundary exchange; the span still ends before the last complete
        // exchange.
        let mut history = exchanges(3);
        history.push(entry(ene_ai::Role::User, "partial"));
        let plan = plan_retroactive_compression(&history, 3).expect("span before boundary");
        // boundary_start = len - 1 (partial); compress everything before it.
        assert_eq!(plan.drop_leading, 6);
        assert_eq!(plan.turns.len(), 6);
    }

    #[test]
    fn usable_summary_requires_non_empty_text() {
        let result = CompressionResult {
            session_id: "s".into(),
            span_id: 1,
            summary: "   ".into(),
            level: CompressionLevel::Scene,
            drop_leading: None,
        };
        assert!(!compression_has_usable_summary(&result));
        let ok = CompressionResult {
            summary: "They discussed the project.".into(),
            ..result
        };
        assert!(compression_has_usable_summary(&ok));
    }

    /// A summarizer stub that always produces a fixed, non-empty summary so
    /// the compression result is usable and the span is persisted.
    struct SummaryLlm;

    #[async_trait::async_trait]
    impl ene_ai::LlmProvider for SummaryLlm {
        fn name(&self) -> &str {
            "summary-llm"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[ene_ai::LlmMessage],
            _tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn tokio_stream::Stream<
                            Item = Result<ene_ai::LlmResponseChunk, ene_ai::LlmProviderError>,
                        > + Send,
                >,
            >,
            ene_ai::LlmProviderError,
        > {
            Err(ene_ai::LlmProviderError::Provider(
                "not used in compression tests".into(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[ene_ai::LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<ene_ai::LlmCompletion, ene_ai::LlmProviderError> {
            Ok(ene_ai::LlmCompletion::text_only(
                "summary of the previous topic".into(),
            ))
        }
    }

    #[tokio::test]
    async fn retroactive_compression_persists_pre_boundary_span() {
        let store = std::sync::Arc::new(ene_store::MemoryStore::open_in_memory(4).await.unwrap());
        let provider: std::sync::Arc<dyn ene_ai::LlmProvider> = std::sync::Arc::new(SummaryLlm);
        let mut manager = crate::context::ContextManager::default();
        let config = ContextConfig::default();
        let history = exchanges(3);

        manager.check_and_trigger_retroactive(
            &config,
            3,
            &history,
            0.9,
            "sess-retro",
            "Ene",
            "user",
            store.clone(),
            provider,
        );
        assert!(manager.has_pending(), "boundary should spawn a compression");

        let result = loop {
            if let Some(polled) = manager.poll_pending() {
                break polled.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert!(compression_has_usable_summary(&result));
        assert_eq!(result.level, CompressionLevel::Scene);

        let spans = store
            .list_memory_spans_by_session_and_level("sess-retro", CompressionLevel::Scene.as_i32())
            .await
            .unwrap();
        assert_eq!(spans.len(), 1, "one topic = one summary span");
        let span = &spans[0];
        assert_eq!(span.turn_start, 0);
        assert_eq!(span.turn_end, 2);
        assert_eq!(
            span.compressed_summary.as_deref(),
            Some("summary of the previous topic")
        );
    }

    #[tokio::test]
    async fn no_boundary_leaves_history_and_store_untouched() {
        let store = std::sync::Arc::new(ene_store::MemoryStore::open_in_memory(4).await.unwrap());
        let provider: std::sync::Arc<dyn ene_ai::LlmProvider> = std::sync::Arc::new(SummaryLlm);
        let mut manager = crate::context::ContextManager::default();
        let config = ContextConfig::default();
        // A boundary on the first topic has no pre-boundary span to compress.
        let history = exchanges(1);

        manager.check_and_trigger_retroactive(
            &config,
            1,
            &history,
            0.9,
            "sess-none",
            "Ene",
            "user",
            store.clone(),
            provider,
        );
        assert!(
            !manager.has_pending(),
            "no pre-boundary span means no compression task"
        );

        let spans = store
            .list_memory_spans_by_session_and_level("sess-none", CompressionLevel::Scene.as_i32())
            .await
            .unwrap();
        assert!(spans.is_empty(), "history must be unchanged");
    }
}
