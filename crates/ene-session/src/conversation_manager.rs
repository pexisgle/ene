use crate::Role;
use crate::error::SessionError;
use chrono::{DateTime, Utc};
use ene_memory as summarizer;
use ene_memory::{KeyFact, MemoryStore};
use ene_provider::EmbeddingProvider;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Compute cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

// ... rest of file unchanged ...

/// Represents whether a session should continue or split.
#[derive(Debug)]
pub enum SessionBoundary {
    /// The session should continue without splitting.
    Continue,
    /// The session should be split for the given reason.
    Split(SplitReason),
}

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
    /// Split requested manually by the user.
    Manual,
}

impl std::fmt::Display for SplitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SplitReason::Timeout { elapsed_minutes } => {
                write!(f, "{}分間の無操作により会話を分割", elapsed_minutes)
            }
            SplitReason::TopicChange { similarity } => {
                write!(f, "トピック変更を検出 (類似度: {:.2})", similarity)
            }
            SplitReason::Manual => {
                write!(f, "手動により会話を分割")
            }
        }
    }
}

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
    pub new_session_id: String,
}

/// Handle to a pending split task running in the background.
pub struct PendingSplitTask {
    /// Oneshot receiver for the split result.
    pub rx: oneshot::Receiver<Result<SplitResult, SessionError>>,
}

/// Input parameters for a split task.
pub struct SplitTaskInput {
    /// The embedding of the previous user input.
    pub last_input_embedding: Option<Vec<f32>>,
    /// Timestamp of the last message.
    pub last_message_time: Option<DateTime<Utc>>,
    /// Current conversation turn count.
    pub current_turn_count: usize,
    /// The current user input.
    pub user_input: String,
    /// Session configuration.
    pub session_config: crate::SessionConfig,
    /// The provider used for summarization.
    pub provider: Arc<dyn ene_provider::LlmProvider>,
    /// Conversation history.
    pub history: Vec<(Role, String)>,
    /// Current session ID.
    pub session_id: String,
    /// Character card name.
    pub card_name: String,
    /// User's name.
    pub user_name: String,
    /// Memory store for persistence.
    pub store: Arc<MemoryStore>,
    /// Embedding provider for vector operations.
    pub embedder: Arc<dyn EmbeddingProvider>,
}

/// Checks whether the current session should be split based on timeout or topic change.
pub async fn check_boundary(
    last_input_embedding: Option<&Vec<f32>>,
    last_message_time: Option<DateTime<Utc>>,
    current_turn_count: usize,
    settings: &crate::SessionConfig,
    user_input: &str,
    embedder: &dyn EmbeddingProvider,
) -> SessionBoundary {
    if !settings.auto_session_split {
        return SessionBoundary::Continue;
    }

    if let Some(last_time) = last_message_time {
        let elapsed = Utc::now() - last_time;
        let elapsed_minutes = elapsed.num_minutes().max(0) as u64;
        if elapsed_minutes >= settings.session_timeout_minutes
            && current_turn_count >= settings.min_turns_before_split
        {
            return SessionBoundary::Split(SplitReason::Timeout { elapsed_minutes });
        }
    }

    if let Some(prev_embedding) = last_input_embedding
        && current_turn_count >= settings.min_turns_before_split
    {
        match embedder.embed_query(user_input).await {
            Ok(current_embedding) => {
                let similarity = cosine_similarity(prev_embedding, &current_embedding);
                if similarity < settings.topic_change_threshold {
                    return SessionBoundary::Split(SplitReason::TopicChange { similarity });
                }
            }
            Err(e) => {
                tracing::error!("[Session] Embedding error for boundary check: {}", e);
            }
        }
    }

    SessionBoundary::Continue
}

/// Embeds all conversation history messages (User + Assistant) individually and merges via max-pooling
///
/// - Includes Assistant messages: captures new topics/words introduced by the AI,
///   improving semantic matching accuracy with subsequent search queries
/// - Max-pooling: adopts the strongest feature per dimension. Prevents dilution
///   by zero-information messages like greetings. For example, a dimension that
///   strongly responded to "Python async" will not be overwritten by the low
///   activation value of "hello."
/// - Each message is embedded individually, avoiding model max_length limits
pub async fn embed_session_messages(
    embedder: &dyn EmbeddingProvider,
    history: &[(Role, String)],
) -> Result<Vec<f32>, SessionError> {
    let dims = embedder.dimensions();
    let messages: Vec<&str> = history
        .iter()
        .filter_map(|(role, content)| {
            if matches!(role, Role::User | Role::Assistant) && !content.trim().is_empty() {
                Some(content.as_str())
            } else {
                None
            }
        })
        .collect();

    if messages.is_empty() {
        return Ok(vec![0.0; dims]);
    }

    let mut all_embeddings: Vec<Vec<f32>> = Vec::with_capacity(messages.len());
    for content in &messages {
        match embedder.embed(content).await {
            Ok(emb) => all_embeddings.push(emb),
            Err(e) => {
                tracing::warn!("[Session] Failed to embed message (skipping): {}", e);
            }
        }
    }

    if all_embeddings.is_empty() {
        return Ok(vec![0.0; dims]);
    }

    let mut max_pooled = all_embeddings[0].clone();
    for emb in all_embeddings.iter().skip(1) {
        for (m, &v) in max_pooled.iter_mut().zip(emb.iter()) {
            if v > *m {
                *m = v;
            }
        }
    }

    let norm: f32 = max_pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in max_pooled.iter_mut() {
            *x /= norm;
        }
    }

    Ok(max_pooled)
}

/// Executes a session split: summarizes conversation, saves to memory, and generates a new session ID.
pub async fn execute_split(
    history: &[(Role, String)],
    session_id: &str,
    card_name: &str,
    user_name: &str,
    store: &Arc<MemoryStore>,
    embedder: &Arc<dyn EmbeddingProvider>,
    provider: &dyn ene_provider::LlmProvider,
    reason: SplitReason,
) -> Result<SplitResult, SessionError> {
    let ended_at = Utc::now();

    for (role, content) in history {
        let role_str = match role {
            Role::User => "user",
            Role::Assistant => "assistant",
            _ => "system",
        };
        if let Err(e) = store.insert_log(session_id, card_name, role_str, content) {
            tracing::error!("[Session] Failed to save log: {}", e);
        }
    }

    let existing_facts = store.get_all_keyfacts(card_name).unwrap_or_default();

    let provider_messages: Vec<ene_provider::LlmMessage> = history
        .iter()
        .map(|(role, content)| match role {
            Role::User => ene_provider::LlmMessage::User {
                parts: vec![ene_provider::UserMessagePart::Text {
                    text: content.clone(),
                }],
            },
            Role::Assistant => ene_provider::LlmMessage::Assistant {
                content: Some(content.clone()),
                tool_calls: None,
            },
            Role::System => ene_provider::LlmMessage::System {
                content: content.clone(),
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

    let summary_embedding = embed_session_messages(embedder.as_ref(), history).await?;

    store.insert_summary(
        session_id,
        card_name,
        &summary_result.summary,
        &summary_result.key_facts,
        &summary_embedding,
        ended_at,
    )?;

    let new_session_id = generate_session_id();

    Ok(SplitResult {
        reason,
        summary: summary_result.summary,
        key_facts: summary_result.key_facts,
        new_session_id,
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
            &session_config,
            &user_input,
            embedder.as_ref(),
        )
        .await;

        let result = match boundary {
            SessionBoundary::Split(reason) => {
                execute_split(
                    &history,
                    &session_id,
                    &card_name,
                    &user_name,
                    &store,
                    &embedder,
                    provider.as_ref(),
                    reason,
                )
                .await
            }
            SessionBoundary::Continue => Err(SessionError::SplitNotNeeded),
        };

        let _ = tx.send(result);
    });

    *pending_split = Some(PendingSplitTask { rx });
}

/// Polls for the result of a pending split task without blocking.
pub fn poll_split_result(
    pending_split: &mut Option<PendingSplitTask>,
) -> Option<Result<SplitResult, SessionError>> {
    let mut task = pending_split.take()?;

    match task.rx.try_recv() {
        Ok(result) => Some(result),
        Err(oneshot::error::TryRecvError::Empty) => {
            *pending_split = Some(task);
            None
        }
        Err(oneshot::error::TryRecvError::Closed) => Some(Err(SessionError::ChannelClosed)),
    }
}

/// Generates a unique session identifier.
pub fn generate_session_id() -> String {
    format!("session_{}", uuid::Uuid::new_v4())
}
