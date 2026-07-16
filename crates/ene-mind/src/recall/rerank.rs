//! Optional memory reranking after hybrid search and MMR diversification.
//!
//! Downstream recall execution can call [`MemoryRerankPipeline::rerank`] after
//! `MemoryStore::search`, [`MemoryDiversifyPipeline::diversify`],
//! and before [`RecallResultMapper::map`].
//!
//! LLM relevance scores affect **candidate order only**. Each [`ScoredMemory`]'s
//! [`MemoryScoreBreakdown::total`](ene_store::MemoryScoreBreakdown::total) remains
//! the hybrid-search score so explainable recall reasons stay aligned with search
//! semantics (#77).

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use ene_ai::{LlmMessage, LlmProvider, UserMessagePart};
use ene_store::ScoredMemory;
use thiserror::Error;

use crate::config::MindMemoryConfig;

/// Telemetry outcome for a rerank pipeline invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RerankOutcome {
    Skipped,
    Completed,
    Fallback,
}

impl RerankOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::Completed => "completed",
            Self::Fallback => "fallback",
        }
    }
}

/// Why rerank was intentionally skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RerankSkipReason {
    Disabled,
    SingleCandidate,
    NoProvider,
}

impl RerankSkipReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::SingleCandidate => "single_candidate",
            Self::NoProvider => "no_provider",
        }
    }
}

/// Input for a memory rerank request.
#[derive(Debug, Clone)]
pub struct MemoryRerankInput<'a> {
    /// Natural-language recall question from the current turn.
    pub recall_question: &'a str,
    /// Hybrid-search candidates in their current score order.
    pub candidates: Vec<ScoredMemory>,
}

/// Runtime options controlling rerank behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryRerankOptions {
    /// Whether reranking is enabled.
    pub enabled: bool,
    /// Maximum number of top candidates to send to the reranker.
    pub candidate_limit: usize,
    /// Timeout in seconds for the rerank LLM call.
    pub timeout_secs: u64,
}

impl MemoryRerankOptions {
    /// Build options from mind memory config.
    #[must_use]
    pub fn from_config(config: &MindMemoryConfig) -> Self {
        Self {
            enabled: config.rerank_enabled,
            candidate_limit: config.rerank_candidate_limit.max(1),
            timeout_secs: config.rerank_timeout_secs.max(1),
        }
    }
}

/// Errors raised during LLM memory reranking.
#[derive(Debug, Error)]
pub enum MemoryRerankError {
    /// The rerank LLM call exceeded the configured timeout.
    #[error("memory rerank timed out after {0}s")]
    TimedOut(u64),
    /// The LLM provider returned an error.
    #[error("memory rerank provider error: {0}")]
    Provider(String),
    /// The LLM response could not be parsed or validated.
    #[error("memory rerank response invalid: {0}")]
    InvalidResponse(String),
}

/// Trait for optional memory reranking strategies.
#[async_trait]
pub trait MemoryReranker: Send + Sync {
    /// Reorder candidates by relevance to the recall question.
    ///
    /// Implementations must preserve the candidate set and hybrid scores;
    /// only order may change.
    async fn rerank(&self, input: MemoryRerankInput<'_>) -> Vec<ScoredMemory>;
}

/// Default passthrough reranker: returns candidates unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct PassthroughMemoryReranker;

#[async_trait]
impl MemoryReranker for PassthroughMemoryReranker {
    async fn rerank(&self, input: MemoryRerankInput<'_>) -> Vec<ScoredMemory> {
        input.candidates
    }
}

/// LLM-based structured memory reranker.
pub struct LlmMemoryReranker {
    provider: Arc<dyn LlmProvider>,
    timeout_secs: u64,
}

impl LlmMemoryReranker {
    /// Create a new LLM reranker with the given provider and timeout budget.
    #[must_use]
    pub fn new(provider: Arc<dyn LlmProvider>, timeout_secs: u64) -> Self {
        Self {
            provider,
            timeout_secs: timeout_secs.max(1),
        }
    }

    /// Score and reorder candidates, returning an error when the LLM call fails.
    pub async fn rerank_candidates(
        &self,
        recall_question: &str,
        candidates: Vec<ScoredMemory>,
    ) -> Result<Vec<ScoredMemory>, MemoryRerankError> {
        if candidates.len() <= 1 {
            return Ok(candidates);
        }

        let messages = build_rerank_messages(recall_question, &candidates);
        let schema = rerank_schema();

        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            self.provider.chat_completion(&messages, Some(schema)),
        )
        .await
        .map_err(|_| MemoryRerankError::TimedOut(self.timeout_secs))?
        .map_err(|e| MemoryRerankError::Provider(e.to_string()))?;

        let scores = parse_rerank_scores(&raw, candidates.len())?;
        Ok(reorder_by_scores(&candidates, &scores))
    }
}

#[async_trait]
impl MemoryReranker for LlmMemoryReranker {
    async fn rerank(&self, input: MemoryRerankInput<'_>) -> Vec<ScoredMemory> {
        let fallback = input.candidates.clone();
        match self
            .rerank_candidates(input.recall_question, input.candidates)
            .await
        {
            Ok(reordered) => reordered,
            Err(error) => {
                tracing::warn!(
                    component = "MemoryRerank",
                    outcome = RerankOutcome::Fallback.as_str(),
                    error = %error,
                    "LLM rerank failed; using hybrid search order"
                );
                fallback
            }
        }
    }
}

/// Pipeline that selects passthrough or LLM rerank based on config and availability.
pub struct MemoryRerankPipeline {
    llm: Option<Arc<dyn LlmProvider>>,
}

impl MemoryRerankPipeline {
    /// Create a pipeline. Pass `None` for LLM to always passthrough even when enabled.
    #[must_use]
    pub fn new(llm: Option<Arc<dyn LlmProvider>>) -> Self {
        Self { llm }
    }

    /// Rerank candidates according to config, with fallback on failure.
    ///
    /// Only the top [`MemoryRerankOptions::candidate_limit`] candidates are sent
    /// to the LLM. Any remaining tail candidates keep their original hybrid order
    /// and are appended after the reranked head.
    pub async fn rerank(
        &self,
        recall_question: &str,
        candidates: Vec<ScoredMemory>,
        options: MemoryRerankOptions,
    ) -> Vec<ScoredMemory> {
        let start = Instant::now();
        let candidate_count = candidates.len();

        if !options.enabled {
            tracing::debug!(
                component = "MemoryRerank",
                outcome = RerankOutcome::Skipped.as_str(),
                skip_reason = RerankSkipReason::Disabled.as_str(),
                enabled = options.enabled,
                candidate_count,
                elapsed_ms = start.elapsed().as_millis() as u64,
                "Rerank skipped"
            );
            return candidates;
        }

        if candidates.len() <= 1 {
            tracing::debug!(
                component = "MemoryRerank",
                outcome = RerankOutcome::Skipped.as_str(),
                skip_reason = RerankSkipReason::SingleCandidate.as_str(),
                enabled = options.enabled,
                candidate_count,
                elapsed_ms = start.elapsed().as_millis() as u64,
                "Rerank skipped"
            );
            return candidates;
        }

        let limit = options.candidate_limit.min(candidates.len());
        let tail_count = candidates.len().saturating_sub(limit);
        let mut rerank_pool = candidates;
        let tail = rerank_pool.split_off(limit);
        let fallback_pool = rerank_pool.clone();
        let reranked_count = rerank_pool.len();

        let Some(provider) = self.llm.clone() else {
            tracing::debug!(
                component = "MemoryRerank",
                outcome = RerankOutcome::Skipped.as_str(),
                skip_reason = RerankSkipReason::NoProvider.as_str(),
                enabled = options.enabled,
                candidate_count,
                reranked_count,
                tail_count,
                elapsed_ms = start.elapsed().as_millis() as u64,
                "Rerank enabled but no LLM provider configured"
            );
            let mut result = rerank_pool;
            result.extend(tail);
            return result;
        };

        let llm = LlmMemoryReranker::new(provider, options.timeout_secs);
        match llm.rerank_candidates(recall_question, rerank_pool).await {
            Ok(mut reordered) => {
                tracing::debug!(
                    component = "MemoryRerank",
                    outcome = RerankOutcome::Completed.as_str(),
                    enabled = options.enabled,
                    candidate_count,
                    reranked_count = reordered.len(),
                    tail_count,
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "Rerank completed"
                );
                reordered.extend(tail);
                reordered
            }
            Err(error) => {
                tracing::warn!(
                    component = "MemoryRerank",
                    outcome = RerankOutcome::Fallback.as_str(),
                    enabled = options.enabled,
                    candidate_count,
                    reranked_count,
                    tail_count,
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    error = %error,
                    "Rerank failed; using hybrid search order"
                );
                let mut result = fallback_pool;
                result.extend(tail);
                result
            }
        }
    }
}

/// Build LLM messages for memory reranking.
///
/// Only the recall question and candidate `content` strings are included.
#[must_use]
pub fn build_rerank_messages(
    recall_question: &str,
    candidates: &[ScoredMemory],
) -> Vec<LlmMessage> {
    let candidate_lines: Vec<String> = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| format!("{}. {}", index, candidate.item.content))
        .collect();

    let system = format!(
        "You are a relevance scorer for memory recall. For each candidate memory listed below, \
         assign a score from 0.0 to 1.0 indicating how relevant it is to the recall question. \
         Reply ONLY with a JSON object containing a `scores` array in the same order as the \
         candidates.\n\nCandidates:\n{}",
        candidate_lines.join("\n")
    );

    vec![
        LlmMessage::System { content: system },
        LlmMessage::User {
            parts: vec![UserMessagePart::Text {
                text: recall_question.to_string(),
            }],
        },
    ]
}

fn rerank_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "scores": {
                "type": "array",
                "items": { "type": "number", "minimum": 0.0, "maximum": 1.0 }
            }
        },
        "required": ["scores"],
        "additionalProperties": false
    })
}

fn parse_rerank_scores(raw: &str, expected_len: usize) -> Result<Vec<f32>, MemoryRerankError> {
    let response_len = raw.len();
    let parsed: serde_json::Value = serde_json::from_str(raw).map_err(|error| {
        MemoryRerankError::InvalidResponse(format!(
            "response was not valid JSON: {error} (response_len={response_len})"
        ))
    })?;

    let arr = parsed
        .get("scores")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            MemoryRerankError::InvalidResponse(format!(
                "response missing `scores` array (response_len={response_len})"
            ))
        })?;

    if arr.len() != expected_len {
        return Err(MemoryRerankError::InvalidResponse(format!(
            "response had {} scores, expected {expected_len} (response_len={response_len})",
            arr.len()
        )));
    }

    arr.iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_f64().ok_or_else(|| {
                MemoryRerankError::InvalidResponse(format!(
                    "response contained a non-numeric score at index {index} (response_len={response_len})"
                ))
            }).map(|score| score as f32)
        })
        .collect()
}

/// Reorder candidates by descending LLM score while preserving hybrid breakdowns.
///
/// Ties break on the original hybrid-search index so ordering stays stable.
fn reorder_by_scores(candidates: &[ScoredMemory], scores: &[f32]) -> Vec<ScoredMemory> {
    let mut indexed: Vec<(usize, f32)> = scores.iter().copied().enumerate().collect();
    indexed.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });

    indexed
        .into_iter()
        .map(|(index, _)| candidates[index].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ene_ai::LlmProviderError;
    use ene_store::{
        AffectAnnotation, MemoryConfidence, MemoryItem, MemoryKind, MemorySalience,
        MemoryScoreBreakdown, MemorySource, MemoryStatus,
    };
    use std::pin::Pin;
    use tokio_stream::Stream;

    type StreamResult =
        Pin<Box<dyn Stream<Item = Result<ene_ai::LlmResponseChunk, LlmProviderError>> + Send>>;

    fn sample_item(id: i64, title: &str, content: &str, total: f32) -> ScoredMemory {
        ScoredMemory {
            item: MemoryItem {
                id: Some(id),
                scope: ene_store::MemoryScope::Character,
                character_id: "ene".into(),
                user_id: String::new(),
                kind: MemoryKind::Semantic,
                title: title.into(),
                content: content.into(),
                source: MemorySource::Conversation,
                source_ref: None,
                confidence: MemoryConfidence::default(),
                salience: MemorySalience::default(),
                affect: AffectAnnotation::default(),
                relationship_impact: 0.0,
                access_count: 0,
                last_accessed_at: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                faded_at: None,
                commitment_id: None,
            },
            breakdown: MemoryScoreBreakdown {
                vector_similarity: total,
                lexical_score: 0.0,
                recency_score: 0.0,
                salience: 0.0,
                confidence: 0.0,
                emotional_match: 0.0,
                relationship: 0.0,
                access_boost: 0.0,
                contradiction_penalty: 0.0,
                stale_penalty: 0.0,
                commitment_boost: 0.0,
                total,
            },
            sources: vec![ene_store::MemoryCandidateSource::Vector],
        }
    }

    struct MockRerankProvider {
        response: String,
    }

    #[async_trait::async_trait]
    impl LlmProvider for MockRerankProvider {
        fn name(&self) -> &'static str {
            "mock-rerank"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_tool_proto::ToolSpec],
        ) -> Result<StreamResult, LlmProviderError> {
            Err(LlmProviderError::Provider(
                "mock: stream not implemented".to_string(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<String, LlmProviderError> {
            Ok(self.response.clone())
        }
    }

    struct SlowRerankProvider;

    #[async_trait::async_trait]
    impl LlmProvider for SlowRerankProvider {
        fn name(&self) -> &'static str {
            "slow-rerank"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_tool_proto::ToolSpec],
        ) -> Result<StreamResult, LlmProviderError> {
            Err(LlmProviderError::Provider(
                "slow: stream not implemented".to_string(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<String, LlmProviderError> {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Ok(r#"{"scores":[0.5,0.5]}"#.to_string())
        }
    }

    struct ErrorRerankProvider;

    #[async_trait::async_trait]
    impl LlmProvider for ErrorRerankProvider {
        fn name(&self) -> &'static str {
            "error-rerank"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[LlmMessage],
            _tools: &[ene_tool_proto::ToolSpec],
        ) -> Result<StreamResult, LlmProviderError> {
            Err(LlmProviderError::Provider(
                "error: stream not implemented".to_string(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<String, LlmProviderError> {
            Err(LlmProviderError::Provider("provider down".to_string()))
        }
    }

    #[tokio::test]
    async fn passthrough_preserves_order_and_scores() {
        let candidates = vec![
            sample_item(1, "first", "content-a", 0.9),
            sample_item(2, "second", "content-b", 0.5),
        ];
        let original_totals: Vec<f32> = candidates
            .iter()
            .map(|candidate| candidate.breakdown.total)
            .collect();

        let reranker = PassthroughMemoryReranker;
        let result = reranker
            .rerank(MemoryRerankInput {
                recall_question: "What do you know about Rust?",
                candidates: candidates.clone(),
            })
            .await;

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].item.id, Some(1));
        assert_eq!(result[1].item.id, Some(2));
        assert_eq!(
            result
                .iter()
                .map(|candidate| candidate.breakdown.total)
                .collect::<Vec<_>>(),
            original_totals
        );
    }

    #[tokio::test]
    async fn disabled_pipeline_preserves_hybrid_order() {
        let candidates = vec![
            sample_item(1, "first", "content-a", 0.9),
            sample_item(2, "second", "content-b", 0.5),
        ];
        let pipeline = MemoryRerankPipeline::new(Some(Arc::new(MockRerankProvider {
            response: r#"{"scores":[0.1,0.9]}"#.to_string(),
        })));
        let options = MemoryRerankOptions {
            enabled: false,
            candidate_limit: 16,
            timeout_secs: 10,
        };

        let result = pipeline
            .rerank("What do you know about Rust?", candidates.clone(), options)
            .await;

        assert_eq!(result[0].item.id, Some(1));
        assert_eq!(result[1].item.id, Some(2));
    }

    #[tokio::test]
    async fn enabled_pipeline_reorders_with_mock_llm() {
        let candidates = vec![
            sample_item(1, "first", "content-a", 0.9),
            sample_item(2, "second", "content-b", 0.5),
        ];
        let pipeline = MemoryRerankPipeline::new(Some(Arc::new(MockRerankProvider {
            response: r#"{"scores":[0.1,0.9]}"#.to_string(),
        })));
        let options = MemoryRerankOptions {
            enabled: true,
            candidate_limit: 16,
            timeout_secs: 10,
        };

        let result = pipeline
            .rerank("What do you know about Rust?", candidates, options)
            .await;

        assert_eq!(result[0].item.id, Some(2));
        assert_eq!(result[1].item.id, Some(1));
        assert!((result[0].breakdown.total - 0.5).abs() < f32::EPSILON);
        assert!((result[1].breakdown.total - 0.9).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn pipeline_falls_back_on_provider_error() {
        let candidates = vec![
            sample_item(1, "first", "content-a", 0.9),
            sample_item(2, "second", "content-b", 0.5),
        ];
        let pipeline = MemoryRerankPipeline::new(Some(Arc::new(ErrorRerankProvider)));
        let options = MemoryRerankOptions {
            enabled: true,
            candidate_limit: 16,
            timeout_secs: 10,
        };

        let result = pipeline
            .rerank("What do you know about Rust?", candidates.clone(), options)
            .await;

        assert_eq!(result[0].item.id, Some(1));
        assert_eq!(result[1].item.id, Some(2));
    }

    #[tokio::test]
    async fn pipeline_falls_back_on_timeout() {
        let candidates = vec![
            sample_item(1, "first", "content-a", 0.9),
            sample_item(2, "second", "content-b", 0.5),
        ];
        let pipeline = MemoryRerankPipeline::new(Some(Arc::new(SlowRerankProvider)));
        let options = MemoryRerankOptions {
            enabled: true,
            candidate_limit: 16,
            timeout_secs: 1,
        };

        let result = pipeline
            .rerank("What do you know about Rust?", candidates.clone(), options)
            .await;

        assert_eq!(result[0].item.id, Some(1));
        assert_eq!(result[1].item.id, Some(2));
    }

    #[test]
    fn parse_rerank_scores_rejects_mismatched_lengths() {
        let error = parse_rerank_scores(r#"{"scores":[0.5]}"#, 2).expect_err("length mismatch");
        assert!(matches!(error, MemoryRerankError::InvalidResponse(_)));
    }

    #[test]
    fn parse_rerank_scores_rejects_invalid_json() {
        let error = parse_rerank_scores("not json", 1).expect_err("invalid json");
        assert!(matches!(error, MemoryRerankError::InvalidResponse(_)));
    }

    #[test]
    fn prompt_contains_only_question_and_candidate_content() {
        let candidates = vec![
            sample_item(1, "secret-title", "visible-content-a", 0.9),
            sample_item(2, "another-title", "visible-content-b", 0.5),
        ];
        let messages = build_rerank_messages("What do you know about Rust?", &candidates);
        let serialized = serde_json::to_string(&messages).expect("serialize messages");

        assert!(serialized.contains("visible-content-a"));
        assert!(serialized.contains("visible-content-b"));
        assert!(serialized.contains("What do you know about Rust?"));
        assert!(!serialized.contains("secret-title"));
        assert!(!serialized.contains("another-title"));
    }

    #[test]
    fn options_from_config_applies_defaults() {
        let config = MindMemoryConfig {
            rerank_enabled: true,
            rerank_candidate_limit: 0,
            rerank_timeout_secs: 0,
            ..MindMemoryConfig::default()
        };
        let options = MemoryRerankOptions::from_config(&config);
        assert!(options.enabled);
        assert_eq!(options.candidate_limit, 1);
        assert_eq!(options.timeout_secs, 1);
    }

    #[test]
    fn parse_rerank_scores_accepts_valid_payload() {
        let scores = parse_rerank_scores(r#"{"scores":[0.2,0.8]}"#, 2).expect("valid payload");
        assert_eq!(scores, vec![0.2, 0.8]);
    }

    #[test]
    fn parse_rerank_scores_rejects_missing_scores_key() {
        let error = parse_rerank_scores(r#"{"other":[]}"#, 1).expect_err("missing scores");
        assert!(matches!(error, MemoryRerankError::InvalidResponse(_)));
    }

    #[test]
    fn parse_rerank_scores_rejects_non_numeric_score() {
        let error = parse_rerank_scores(r#"{"scores":["x"]}"#, 1).expect_err("non numeric");
        assert!(matches!(error, MemoryRerankError::InvalidResponse(_)));
    }

    #[test]
    fn parse_error_does_not_leak_full_raw_response() {
        let sentinel = "SECRET_MEMORY_CONTENT_SHOULD_NOT_APPEAR_IN_ERROR";
        let raw = format!(r"not json but contains {sentinel}");
        let error = parse_rerank_scores(&raw, 1).expect_err("invalid json");
        let message = error.to_string();
        assert!(!message.contains(sentinel));
        assert!(message.contains("response_len="));
    }

    #[test]
    fn reorder_by_scores_sorts_descending() {
        let candidates = vec![
            sample_item(1, "first", "content-a", 0.9),
            sample_item(2, "second", "content-b", 0.5),
        ];
        let reordered = reorder_by_scores(&candidates, &[0.1, 0.9]);
        assert_eq!(reordered[0].item.id, Some(2));
        assert_eq!(reordered[1].item.id, Some(1));
    }

    #[test]
    fn reorder_by_scores_breaks_ties_by_original_index() {
        let candidates = vec![
            sample_item(1, "first", "content-a", 0.9),
            sample_item(2, "second", "content-b", 0.5),
        ];
        let reordered = reorder_by_scores(&candidates, &[0.5, 0.5]);
        assert_eq!(reordered[0].item.id, Some(1));
        assert_eq!(reordered[1].item.id, Some(2));
    }

    #[tokio::test]
    async fn pipeline_skips_single_candidate_without_llm_call() {
        let candidates = vec![sample_item(1, "first", "content-a", 0.9)];
        let pipeline = MemoryRerankPipeline::new(Some(Arc::new(MockRerankProvider {
            response: r#"{"scores":[0.1]}"#.to_string(),
        })));
        let options = MemoryRerankOptions {
            enabled: true,
            candidate_limit: 16,
            timeout_secs: 10,
        };

        let result = pipeline
            .rerank("What do you know about Rust?", candidates.clone(), options)
            .await;

        assert_eq!(result[0].item.id, Some(1));
    }

    #[tokio::test]
    async fn pipeline_no_provider_when_enabled_preserves_order() {
        let candidates = vec![
            sample_item(1, "first", "content-a", 0.9),
            sample_item(2, "second", "content-b", 0.5),
        ];
        let pipeline = MemoryRerankPipeline::new(None);
        let options = MemoryRerankOptions {
            enabled: true,
            candidate_limit: 16,
            timeout_secs: 10,
        };

        let result = pipeline
            .rerank("What do you know about Rust?", candidates.clone(), options)
            .await;

        assert_eq!(result[0].item.id, Some(1));
        assert_eq!(result[1].item.id, Some(2));
    }

    #[tokio::test]
    async fn pipeline_respects_candidate_limit_tail() {
        let candidates = vec![
            sample_item(1, "first", "content-a", 0.9),
            sample_item(2, "second", "content-b", 0.8),
            sample_item(3, "third", "content-c", 0.7),
        ];
        let pipeline = MemoryRerankPipeline::new(Some(Arc::new(MockRerankProvider {
            response: r#"{"scores":[0.1,0.9]}"#.to_string(),
        })));
        let options = MemoryRerankOptions {
            enabled: true,
            candidate_limit: 2,
            timeout_secs: 10,
        };

        let result = pipeline
            .rerank("What do you know about Rust?", candidates, options)
            .await;

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].item.id, Some(2));
        assert_eq!(result[1].item.id, Some(1));
        assert_eq!(result[2].item.id, Some(3));
    }

    #[tokio::test]
    async fn pipeline_falls_back_on_invalid_llm_response() {
        let candidates = vec![
            sample_item(1, "first", "content-a", 0.9),
            sample_item(2, "second", "content-b", 0.5),
        ];
        let pipeline = MemoryRerankPipeline::new(Some(Arc::new(MockRerankProvider {
            response: "not json".to_string(),
        })));
        let options = MemoryRerankOptions {
            enabled: true,
            candidate_limit: 16,
            timeout_secs: 10,
        };

        let result = pipeline
            .rerank("What do you know about Rust?", candidates.clone(), options)
            .await;

        assert_eq!(result[0].item.id, Some(1));
        assert_eq!(result[1].item.id, Some(2));
    }

    #[tokio::test]
    async fn llm_reranker_trait_falls_back_on_invalid_response() {
        let candidates = vec![
            sample_item(1, "first", "content-a", 0.9),
            sample_item(2, "second", "content-b", 0.5),
        ];
        let reranker = LlmMemoryReranker::new(
            Arc::new(MockRerankProvider {
                response: "not json".to_string(),
            }),
            10,
        );

        let result = reranker
            .rerank(MemoryRerankInput {
                recall_question: "What do you know about Rust?",
                candidates: candidates.clone(),
            })
            .await;

        assert_eq!(result[0].item.id, Some(1));
        assert_eq!(result[1].item.id, Some(2));
    }
}
