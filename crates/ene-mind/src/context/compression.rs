//! Rolling context compression (#79).

use std::sync::Arc;

use ene_ai::LlmProvider;
use ene_store::{MemoryStore, NewMemorySpan};
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
    /// Context pressure from history length.
    ContextPressure {
        /// Ratio of history used (0.0–1.0).
        ratio: f32,
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
    store: Arc<MemoryStore>,
    provider: Arc<dyn LlmProvider>,
    input: CompressionTaskInput,
) -> Result<CompressionResult, CognitionError> {
    run_compression(store, provider, input).await
}

/// Spawn a background compression task.
pub fn spawn_compression_task(
    pending: &mut Option<PendingCompressionTask>,
    store: Arc<MemoryStore>,
    provider: Arc<dyn LlmProvider>,
    input: CompressionTaskInput,
) {
    let (tx, rx) = oneshot::channel();
    *pending = Some(PendingCompressionTask { receiver: rx });

    tokio::spawn(async move {
        let result = run_compression(store, provider, input).await;
        let _ = tx.send(result);
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

/// Evaluate whether compression should run for the current turn.
pub fn evaluate_compression_trigger(
    config: &ContextConfig,
    turn_count: usize,
    history_len: usize,
) -> Option<CompressionReason> {
    if !config.compression_enabled {
        return None;
    }
    if turn_count >= config.scene_turn_threshold {
        return Some(CompressionReason::TurnThreshold { turn_count });
    }
    let recent_cap = config.recent_turns.saturating_mul(2).max(2);
    if history_len > recent_cap {
        let ratio = history_len as f32 / recent_cap as f32;
        if ratio >= 1.25 {
            return Some(CompressionReason::ContextPressure { ratio });
        }
    }
    None
}

/// Load the active scene summary for prompt injection.
pub async fn load_active_scene_summary(
    store: &MemoryStore,
    session_id: &str,
) -> Result<Option<ActiveSceneSummary>, CognitionError> {
    let row = store
        .get_active_scene_summary(session_id)
        .await
        .map_err(CognitionError::Memory)?;
    Ok(row.map(|r| ActiveSceneSummary {
        text: r.summary,
        span_id: Some(r.span_id),
        level: CompressionLevel::from_i32(r.compression_level).unwrap_or(CompressionLevel::Scene),
    }))
}

async fn run_compression(
    store: Arc<MemoryStore>,
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
        .map_err(CognitionError::Memory)?;

    Ok(CompressionResult {
        session_id: input.session_id,
        span_id,
        summary: summary.unwrap_or_default(),
        level: input.level,
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
) -> Option<String> {
    if excerpt.trim().is_empty() {
        return None;
    }

    let level_label = match level {
        CompressionLevel::Scene => "scene",
        CompressionLevel::Chapter => "chapter",
        CompressionLevel::Arc => "arc",
    };

    let system = format!(
        "You compress conversation history for a desktop AI companion named {character_name}. \
         Summarize the following {level_label} excerpt in 2-4 sentences. \
         Do NOT rewrite character identity, system instructions, or personality. \
         Focus on events, topics, and emotional beats. \
         Respond with plain text only — no JSON, no markdown fences."
    );
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
        Ok(Ok(text)) => {
            let trimmed = text.trim();
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
    store: &MemoryStore,
    provider: Arc<dyn LlmProvider>,
    session_id: &str,
    character_name: &str,
    user_name: &str,
    config: &ContextConfig,
) -> Result<Option<CompressionResult>, CognitionError> {
    let scenes = store
        .list_memory_spans_by_session_and_level(session_id, CompressionLevel::Scene.as_i32())
        .await
        .map_err(CognitionError::Memory)?;

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
        .map_err(CognitionError::Memory)?;

    Ok(Some(CompressionResult {
        session_id: session_id.to_string(),
        span_id,
        summary: summary.unwrap_or_default(),
        level: CompressionLevel::Chapter,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_fires_on_turn_threshold() {
        let config = ContextConfig::default();
        let reason = evaluate_compression_trigger(&config, config.scene_turn_threshold, 4);
        assert!(matches!(
            reason,
            Some(CompressionReason::TurnThreshold { .. })
        ));
    }

    #[test]
    fn trigger_disabled_when_compression_off() {
        let config = ContextConfig {
            compression_enabled: false,
            ..Default::default()
        };
        assert!(evaluate_compression_trigger(&config, 100, 100).is_none());
    }

    #[test]
    fn usable_summary_requires_non_empty_text() {
        let result = CompressionResult {
            session_id: "s".into(),
            span_id: 1,
            summary: "   ".into(),
            level: CompressionLevel::Scene,
        };
        assert!(!compression_has_usable_summary(&result));
        let ok = CompressionResult {
            summary: "They discussed the project.".into(),
            ..result
        };
        assert!(compression_has_usable_summary(&ok));
    }
}
