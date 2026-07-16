use super::error::EneSessionError;
use super::types::{CardName, SessionId};
use crate::summarizer;
use chrono::{DateTime, Utc};
use ene_ai::EmbeddingProvider;
use ene_ai::{Role, cosine_similarity};
use ene_store::{KeyFact, MemoryStore};
use std::sync::Arc;
use tokio::sync::oneshot;

// ── Split detection ──────────────────────────────────────────────────────────

/// Reasons for a session split.
#[derive(Debug, Clone)]
pub enum SplitReason {
    /// Split due to inactivity timeout.
    Timeout {
        /// Minutes elapsed since the last message.
        elapsed_minutes: u64,
    },
    /// Split due to topic change detection.
    TopicChange {
        /// Cosine similarity between consecutive user message embeddings.
        similarity: f32,
    },
    /// Split due to context length pressure.
    ContextPressure {
        /// Proportion of history used (0.0–1.0).
        context_ratio: f32,
    },
    /// Split due to a high composite score across multiple factors.
    Composite {
        /// The computed split score (0.0–1.0+).
        score: f32,
    },
    /// Split requested manually by the user.
    Manual,
}

impl std::fmt::Display for SplitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout { elapsed_minutes } => {
                write!(
                    f,
                    "Session split due to {elapsed_minutes} minutes of inactivity"
                )
            }
            Self::TopicChange { similarity } => {
                write!(
                    f,
                    "Session split due to topic change (similarity: {similarity:.2})"
                )
            }
            Self::ContextPressure { context_ratio } => {
                write!(
                    f,
                    "Session split due to context pressure ({:.0}% full)",
                    context_ratio * 100.0
                )
            }
            Self::Composite { score } => {
                write!(f, "Session split with composite score {score:.2}")
            }
            Self::Manual => write!(f, "Session split manually"),
        }
    }
}

/// Whether a session should continue or split.
#[derive(Debug)]
pub enum SessionBoundary {
    /// The session should continue without splitting.
    Continue,
    /// The session should be split for the given reason.
    Split(SplitReason),
}

// ── Composite split scoring ──────────────────────────────────────────────────

/// Decomposed components of a split score for diagnostics.
#[derive(Debug, Clone)]
pub struct SplitScore {
    /// Combined weighted score (0.0–1.0+). Split triggered when ≥ threshold.
    pub total: f32,
    /// Contribution from the time-elapsed factor.
    pub time_component: f32,
    /// Contribution from the topic-distance factor (`1 − similarity`).
    pub topic_component: f32,
    /// Contribution from context-length pressure.
    pub context_component: f32,
    /// Contribution from the turn-count factor.
    pub turn_component: f32,
}

/// Computes a composite split score from several independent signals.
///
/// Each factor is normalised to [0, 1] before being weighted:
///
/// | Factor | 0 → no pressure | 1 → maximum pressure |
/// |--------|-----------------|----------------------|
/// | **Time** | No time has passed | `timeout_minutes` elapsed |
/// | **Topic** | Identical topic | Completely different topic |
/// | **Context** | Empty history | History at `max_history_turns` |
/// | **Turn count** | 0 turns | 100 turns (soft cap) |
///
/// When `topic_similarity` is `None` (no embedder), the topic component is 0.
#[must_use]
pub fn compute_split_score(
    time_elapsed_minutes: f64,
    topic_similarity: Option<f32>,
    context_ratio: f32,
    turn_count: usize,
    config: &super::SessionConfig,
) -> SplitScore {
    let weights = &config.split_weights;

    // Time factor: linearly approaches 1.0 at timeout_minutes; capped at 1.0.
    let time_factor = if config.timeout_minutes == 0 {
        0.0f32
    } else {
        (time_elapsed_minutes as f32 / config.timeout_minutes as f32).min(1.0)
    };

    // Topic-distance factor: 1 − cosine_similarity (higher = more different).
    let topic_factor = match topic_similarity {
        Some(sim) => (1.0 - sim).clamp(0.0, 1.0),
        None => 0.0,
    };

    // Context-pressure factor: proportion of max history used.
    let context_factor = context_ratio.clamp(0.0, 1.0);

    // Turn-count factor: soft cap at 100 turns (configurable via future field).
    let turn_factor = (turn_count as f32 / 100.0).min(1.0);

    let time_component = time_factor * weights.time;
    let topic_component = topic_factor * weights.topic;
    let context_component = context_factor * weights.context;
    let turn_component = turn_factor * weights.turn_count;

    SplitScore {
        total: time_component + topic_component + context_component + turn_component,
        time_component,
        topic_component,
        context_component,
        turn_component,
    }
}

/// Checks whether the current session should be split based on the composite
/// scoring function and the configured threshold.
///
/// Returns `SessionBoundary::Continue` when:
/// - `auto_split` is disabled, or
/// - fewer than `min_turns_before_split` turns have occurred, or
/// - the composite score is below the configured threshold.
///
/// Part of the hard-split-only strategy (see the crate-level docs); `ene-runtime` skips this
/// check entirely in favor of `crate::context::evaluate_compression_trigger` whenever
/// compression is enabled.
pub async fn check_boundary(
    last_input_embedding: Option<&Vec<f32>>,
    last_message_time: Option<DateTime<Utc>>,
    current_turn_count: usize,
    history_len: usize,
    settings: &super::SessionConfig,
    user_input: &str,
    embedder: &dyn EmbeddingProvider,
) -> SessionBoundary {
    if !settings.auto_split {
        return SessionBoundary::Continue;
    }

    // Always require a minimum number of turns to avoid splitting trivially short sessions.
    if current_turn_count < settings.min_turns_before_split {
        return SessionBoundary::Continue;
    }

    // Time elapsed since the last message (0 if no prior message).
    let elapsed_minutes =
        last_message_time.map_or(0.0, |t| (Utc::now() - t).num_minutes().max(0) as f64);

    // Compute topic similarity by embedding the current user input and comparing
    // with the previous turn's embedding. Skip if no embedder or prior embedding.
    let topic_similarity: Option<f32> = if let Some(prev_embedding) = last_input_embedding {
        match ene_ai::embed_query(embedder, user_input).await {
            Ok(current_embedding) => Some(cosine_similarity(prev_embedding, &current_embedding)),
            Err(e) => {
                tracing::warn!(component = "Session", error = %e, "Embedding error during boundary check");
                None
            }
        }
    } else {
        None
    };

    // Context-pressure ratio: how full is the in-memory history?
    let max_history = settings.max_history_turns * 2; // each turn = 2 messages
    let context_ratio = if max_history == 0 {
        0.0f32
    } else {
        (history_len as f32 / max_history as f32).min(1.0)
    };

    let score = compute_split_score(
        elapsed_minutes,
        topic_similarity,
        context_ratio,
        current_turn_count,
        settings,
    );

    tracing::debug!(
        "[Session] Split score: {:.3} (time={:.3}, topic={:.3}, ctx={:.3}, turns={:.3}) threshold={:.3}",
        score.total,
        score.time_component,
        score.topic_component,
        score.context_component,
        score.turn_component,
        settings.split_weights.threshold
    );

    if score.total >= settings.split_weights.threshold {
        // Choose the most dominant reason for user-facing messaging.
        let reason = dominant_reason(&score, elapsed_minutes, topic_similarity);
        SessionBoundary::Split(reason)
    } else {
        SessionBoundary::Continue
    }
}

/// Returns the split reason whose component contributed most to the total score.
fn dominant_reason(
    score: &SplitScore,
    elapsed_minutes: f64,
    topic_similarity: Option<f32>,
) -> SplitReason {
    let max_component = score
        .time_component
        .max(score.topic_component)
        .max(score.context_component)
        .max(score.turn_component);

    if (score.time_component - max_component).abs() < f32::EPSILON {
        SplitReason::Timeout {
            elapsed_minutes: elapsed_minutes as u64,
        }
    } else if (score.topic_component - max_component).abs() < f32::EPSILON {
        SplitReason::TopicChange {
            similarity: topic_similarity.unwrap_or(0.0),
        }
    } else if (score.context_component - max_component).abs() < f32::EPSILON {
        SplitReason::ContextPressure {
            context_ratio: score.context_component / score.context_component.max(f32::EPSILON),
        }
    } else {
        SplitReason::Composite { score: score.total }
    }
}

// ── Session embedding ────────────────────────────────────────────────────────

// ── Split execution ──────────────────────────────────────────────────────────

/// The result of a session split operation.
#[derive(Debug, Clone)]
pub struct SplitResult {
    /// The reason the split was triggered.
    pub reason: SplitReason,
    /// The generated conversation summary.
    pub summary: String,
    /// Extracted key facts from the conversation.
    pub key_facts: Vec<KeyFact>,
    /// The ID of the new session.
    pub new_session_id: SessionId,
    /// Number of conversation-history entries that were in the snapshot
    /// passed to the summarizer. The actor uses this as a boundary when
    /// applying the split: history entries before index `snapshot_len - 1`
    /// are discarded, and entries at and after that index are retained so
    /// any user/assistant messages that arrived while the split was
    /// running are preserved in the live history.
    pub snapshot_len: usize,
}

/// Handle to a pending split task running in the background.
pub struct PendingSplitTask {
    /// Oneshot receiver for the split result.
    pub(crate) rx: oneshot::Receiver<Result<SplitResult, EneSessionError>>,
}

/// Input parameters for a split task.
pub struct SplitTaskInput {
    /// The embedding of the previous user input.
    pub last_input_embedding: Option<Vec<f32>>,
    /// Timestamp of the last message.
    pub last_message_time: Option<DateTime<Utc>>,
    /// Current conversation turn count.
    pub current_turn_count: usize,
    /// Current in-memory history length (number of individual messages, not turns).
    pub history_len: usize,
    /// The current user input.
    pub user_input: String,
    /// Session configuration.
    pub session_config: super::SessionConfig,
    /// The provider used for summarization.
    pub provider: Arc<dyn ene_ai::LlmProvider>,
    /// Conversation history.
    pub history: Vec<crate::lifecycle::HistoryEntry>,
    /// Current session ID.
    pub session_id: SessionId,
    /// Character card name.
    pub card_name: CardName,
    /// User's name.
    pub user_name: String,
    /// Memory store for persistence.
    pub store: Arc<MemoryStore>,
    /// Embedding provider for vector operations.
    pub embedder: Arc<dyn EmbeddingProvider>,
}

/// Executes a session split: summarizes conversation, saves to memory, and
/// generates a new session ID.
///
/// This is the **hard-split** context-management strategy — prefer enabling
/// `ene-mind`'s `MindConfig::context.compression_enabled` where possible, which trims
/// history gradually without minting a new session ID. See the crate-level docs for details.
pub async fn execute_split(
    history: &[crate::lifecycle::HistoryEntry],
    session_id: &str,
    card_name: &str,
    user_name: &str,
    store: &Arc<MemoryStore>,
    provider: &dyn ene_ai::LlmProvider,
    reason: SplitReason,
) -> Result<SplitResult, EneSessionError> {
    let history_clone = history.to_vec();
    let session_id_clone = session_id.to_string();
    let card_name_clone = card_name.to_string();

    let existing_facts = Vec::new();
    for entry in &history_clone {
        if let Err(e) = store
            .insert_log(
                &session_id_clone,
                &card_name_clone,
                entry.role_label(),
                &entry.content,
            )
            .await
        {
            tracing::error!(component = "Session", error = %e, "Failed to save log");
        }
    }
    // Legacy keyfacts are retired from the product path (#125); summarizer
    // runs without prior keyfact context. Prefer compression / scene spans.

    let provider_messages: Vec<ene_ai::LlmMessage> = history
        .iter()
        .map(|entry| match entry.role {
            Role::User => ene_ai::LlmMessage::User {
                parts: vec![ene_ai::UserMessagePart::Text {
                    text: entry.content.clone(),
                }],
            },
            Role::Assistant => ene_ai::LlmMessage::Assistant {
                content: Some(entry.content.clone()),
                tool_calls: None,
            },
            Role::System => ene_ai::LlmMessage::System {
                content: entry.content.clone(),
            },
        })
        .collect();

    let summary_result = summarizer::summarize_conversation(
        provider,
        &provider_messages,
        card_name,
        user_name,
        &existing_facts,
    )
    .await?;

    let new_session_id = generate_session_id();
    let snapshot_len = history.len();

    Ok(SplitResult {
        reason,
        summary: summary_result.summary,
        key_facts: summary_result.key_facts,
        new_session_id,
        snapshot_len,
    })
}

/// Spawns an async task that checks the session boundary and executes a split if needed.
pub fn spawn_split_task(pending_split: &mut Option<PendingSplitTask>, input: SplitTaskInput) {
    if pending_split.is_some() {
        return;
    }

    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let SplitTaskInput {
            last_input_embedding,
            last_message_time,
            current_turn_count,
            history_len,
            user_input,
            session_config,
            provider,
            history,
            session_id,
            card_name,
            user_name,
            store,
            embedder,
        } = input;

        let boundary = check_boundary(
            last_input_embedding.as_ref(),
            last_message_time,
            current_turn_count,
            history_len,
            &session_config,
            &user_input,
            embedder.as_ref(),
        )
        .await;

        let result = match boundary {
            SessionBoundary::Split(reason) => {
                execute_split(
                    &history,
                    session_id.as_str(),
                    card_name.as_str(),
                    &user_name,
                    &store,
                    provider.as_ref(),
                    reason,
                )
                .await
            }
            SessionBoundary::Continue => Err(EneSessionError::SplitNotNeeded),
        };

        let _ = tx.send(result);
    });

    *pending_split = Some(PendingSplitTask { rx });
}

/// Polls for the result of a pending split task without blocking.
pub fn poll_split_result(
    pending_split: &mut Option<PendingSplitTask>,
) -> Option<Result<SplitResult, EneSessionError>> {
    let mut task = pending_split.take()?;

    match task.rx.try_recv() {
        Ok(result) => Some(result),
        Err(oneshot::error::TryRecvError::Empty) => {
            *pending_split = Some(task);
            None
        }
        Err(oneshot::error::TryRecvError::Closed) => Some(Err(EneSessionError::ChannelClosed)),
    }
}

/// Generates a unique session identifier.
#[must_use]
pub fn generate_session_id() -> SessionId {
    SessionId::from(format!("session_{}", uuid::Uuid::new_v4()))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "test asserts exact float equality")]
mod tests {
    use super::*;
    use crate::session::{SessionConfig, SplitScoreWeights};

    fn test_config() -> SessionConfig {
        SessionConfig {
            auto_split: true,
            timeout_minutes: 30,
            max_history_turns: 20,
            min_turns_before_split: 3,
            split_weights: SplitScoreWeights {
                time: 0.35,
                topic: 0.40,
                context: 0.20,
                turn_count: 0.05,
                threshold: 0.65,
            },
            ..Default::default()
        }
    }

    #[test]
    fn score_zero_for_fresh_session() {
        let cfg = test_config();
        let score = compute_split_score(0.0, None, 0.0, 0, &cfg);
        assert_eq!(score.total, 0.0);
    }

    #[test]
    fn score_time_only_reaches_threshold() {
        let mut cfg = test_config();
        // With only time weight, to hit 0.65 we need elapsed >= timeout * (0.65 / 0.35) ≈ 55 min.
        // At exactly timeout (30 min), time_factor = 1.0, time_component = 0.35 < 0.65.
        cfg.split_weights = SplitScoreWeights {
            time: 1.0,
            topic: 0.0,
            context: 0.0,
            turn_count: 0.0,
            threshold: 0.65,
        };
        let score = compute_split_score(30.0, None, 0.0, 0, &cfg);
        assert!(
            score.total >= cfg.split_weights.threshold,
            "time_only score {:.3} should be >= threshold {:.3}",
            score.total,
            cfg.split_weights.threshold
        );
    }

    #[test]
    fn score_fully_different_topic_contributes() {
        let cfg = test_config();
        // similarity = 0.0 → topic_factor = 1.0, topic_component = 0.40
        let score = compute_split_score(0.0, Some(0.0), 0.0, 0, &cfg);
        assert!(
            (score.topic_component - 0.40).abs() < 1e-6,
            "topic_component should be 0.40 but was {}",
            score.topic_component
        );
    }

    #[test]
    fn score_full_context_pressure() {
        let cfg = test_config();
        let score = compute_split_score(0.0, None, 1.0, 0, &cfg);
        assert!(
            (score.context_component - 0.20).abs() < 1e-6,
            "context_component should be 0.20 but was {}",
            score.context_component
        );
    }

    #[test]
    fn composite_high_on_all_factors() {
        let cfg = test_config();
        // All factors at maximum pressure → score = sum of all weights = 1.0
        let score = compute_split_score(30.0, Some(0.0), 1.0, 100, &cfg);
        assert!(
            score.total >= cfg.split_weights.threshold,
            "all-max score {:.3} should exceed threshold {:.3}",
            score.total,
            cfg.split_weights.threshold
        );
    }

    #[test]
    fn score_capped_at_factor_bounds() {
        let cfg = test_config();
        // Elapsed 1000 min, similarity -1 (impossible, clamps to 1 distance),
        // context > 1 (clamps), turns > 100 (clamps)
        let score = compute_split_score(1000.0, Some(-1.0), 5.0, 1000, &cfg);
        // time_factor = 1.0, topic_factor = 1.0, context_factor = 1.0, turn = 1.0
        let expected = 0.35 + 0.40 + 0.20 + 0.05;
        assert!(
            (score.total - expected).abs() < 1e-5,
            "capped score should be {expected:.3} but was {:.3}",
            score.total
        );
    }
}
