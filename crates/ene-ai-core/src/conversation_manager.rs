use crate::config::AiSettings;
use crate::embedding::{EmbeddingProvider, cosine_similarity};
use crate::error::AiCoreError;
use crate::memory::store::{KeyFact, MemoryStore};
use crate::summarizer;
use async_openai::types::chat::Role;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Debug)]
pub enum SessionBoundary {
    Continue,
    Split(SplitReason),
}

#[derive(Debug, Clone)]
pub enum SplitReason {
    Timeout { elapsed_minutes: u64 },
    TopicChange { similarity: f32 },
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
        }
    }
}

#[derive(Debug, Clone)]
pub struct SplitResult {
    pub reason: SplitReason,
    pub summary: String,
    pub key_facts: Vec<KeyFact>,
    pub new_session_id: String,
}

pub struct PendingSplitTask {
    pub rx: oneshot::Receiver<Result<SplitResult, AiCoreError>>,
}

/// セッション分割タスクの入力パラメータ
pub struct SplitTaskInput {
    pub last_input_embedding: Option<Vec<f32>>,
    pub last_message_time: Option<DateTime<Utc>>,
    pub current_turn_count: usize,
    pub user_input: String,
    pub settings: AiSettings,
    pub history: Vec<(Role, String)>,
    pub session_id: String,
    pub card_name: String,
    pub user_name: String,
    pub store: Arc<MemoryStore>,
    pub embedder: Arc<dyn EmbeddingProvider>,
}

pub async fn check_boundary(
    last_input_embedding: Option<&Vec<f32>>,
    last_message_time: Option<DateTime<Utc>>,
    current_turn_count: usize,
    settings: &AiSettings,
    user_input: &str,
    embedder: &dyn EmbeddingProvider,
) -> SessionBoundary {
    if !settings.memory.auto_session_split {
        return SessionBoundary::Continue;
    }

    if let Some(last_time) = last_message_time {
        let elapsed = Utc::now() - last_time;
        let elapsed_minutes = elapsed.num_minutes().max(0) as u64;
        if elapsed_minutes >= settings.memory.session_timeout_minutes
            && current_turn_count >= settings.memory.min_turns_before_split
        {
            return SessionBoundary::Split(SplitReason::Timeout { elapsed_minutes });
        }
    }

    if let Some(prev_embedding) = last_input_embedding
        && current_turn_count >= settings.memory.min_turns_before_split
    {
        match embedder.embed_query(user_input).await {
            Ok(current_embedding) => {
                let similarity = cosine_similarity(prev_embedding, &current_embedding);
                if similarity < settings.memory.topic_change_threshold {
                    return SessionBoundary::Split(SplitReason::TopicChange { similarity });
                }
            }
            Err(e) => {
                eprintln!("[Session] Embedding error for boundary check: {}", e);
            }
        }
    }

    SessionBoundary::Continue
}

/// 会話履歴の全メッセージ（User + Assistant）を個別にembedし、max-pooling で統合する
///
/// - Assistant メッセージも含める: AIが発した新しい話題・単語も捕捉し、
///   後続の検索クエリとの意味的マッチング精度を向上させる
/// - Max-pooling: 各次元で最も強い特徴を採用。挨拶などの情報量ゼロのメッセージに
///   意味が希釈されない。たとえば「Python非同期」に強く反応した次元は、
///   「こんにちは」の低い活性化値で上書きされない。
/// - 各メッセージは個別にembed → モデルのmax_length制限に引っかからない
pub async fn embed_session_messages(
    embedder: &dyn EmbeddingProvider,
    history: &[(Role, String)],
) -> Result<Vec<f32>, AiCoreError> {
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

pub async fn execute_split(
    history: &[(Role, String)],
    session_id: &str,
    card_name: &str,
    user_name: &str,
    store: &Arc<MemoryStore>,
    embedder: &Arc<dyn EmbeddingProvider>,
    settings: &AiSettings,
    reason: SplitReason,
) -> Result<SplitResult, AiCoreError> {
    let ended_at = Utc::now();

    for (role, content) in history {
        let role_str = match role {
            Role::User => "user",
            Role::Assistant => "assistant",
            _ => "system",
        };
        if let Err(e) = store.insert_log(session_id, card_name, role_str, content) {
            eprintln!("[Session] Failed to save log: {}", e);
        }
    }

    let existing_facts = store.get_all_keyfacts(card_name).unwrap_or_default();

    let summary_result = summarizer::summarize_conversation(
        settings,
        history,
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
            settings,
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
            &settings,
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
                    &settings,
                    reason,
                )
                .await
            }
            SessionBoundary::Continue => Err(AiCoreError::SplitNotNeeded),
        };

        let _ = tx.send(result);
    });

    *pending_split = Some(PendingSplitTask { rx });
}

pub fn poll_split_result(
    pending_split: &mut Option<PendingSplitTask>,
) -> Option<Result<SplitResult, AiCoreError>> {
    let mut task = pending_split.take()?;

    match task.rx.try_recv() {
        Ok(result) => Some(result),
        Err(oneshot::error::TryRecvError::Empty) => {
            *pending_split = Some(task);
            None
        }
        Err(oneshot::error::TryRecvError::Closed) => Some(Err(AiCoreError::ChannelClosed)),
    }
}

pub fn generate_session_id() -> String {
    format!("session_{}", uuid::Uuid::new_v4())
}
