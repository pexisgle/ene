use async_trait::async_trait;
use std::sync::Arc;

use crate::message::{LlmMessage, UserMessagePart};
use crate::traits::{
    EmbeddingError, EmbeddingKind, EmbeddingProvider, LlmProvider, cosine_similarity,
};

/// Wraps a primary embedder with optional LLM-backed `HyDE` and rerank.
///
/// When an LLM is set, `hyde()` delegates to it to produce a
/// hypothetical tool-invocation document and `rerank()`
/// uses LLM scoring. Otherwise both fall back to the default
/// cosine-similarity paths.
///
/// The per-task LLM is a separate `Arc<dyn LlmProvider>`
/// rather than a `(provider, model_name)` pair: a model name
/// alone would not actually override the model on the wire,
/// because the underlying provider's `chat_completion` is
/// configured at construction. Separate provider instances
/// guarantee the model is honored.
pub struct HybridRerankProvider {
    embedder: Arc<dyn EmbeddingProvider>,
    /// LLM used for `HyDE` document generation. When `None`,
    /// `hyde()` returns the query unchanged.
    hyde_llm: Option<Arc<dyn LlmProvider>>,
    /// LLM used for `rerank()`. When `None`, falls back to
    /// cosine-similarity scoring against the embedder.
    rerank_llm: Option<Arc<dyn LlmProvider>>,
}

impl HybridRerankProvider {
    /// Creates a wrapper that uses `embedder` for all embedding operations.
    /// `HyDE` and rerank use default cosine-similarity fallbacks unless
    /// LLM providers are set.
    pub fn new(embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            embedder,
            hyde_llm: None,
            rerank_llm: None,
        }
    }

    /// Attaches LLM providers for `HyDE` and rerank. Pass `None`
    /// for either to keep the default fallback path for that
    /// task. The two providers are separate so a builder can
    /// route `HyDE` to a small fast model and rerank to a
    /// larger / more accurate one.
    pub fn with_llm(
        mut self,
        hyde_llm: Option<Arc<dyn LlmProvider>>,
        rerank_llm: Option<Arc<dyn LlmProvider>>,
    ) -> Self {
        self.hyde_llm = hyde_llm;
        self.rerank_llm = rerank_llm;
        self
    }
}

#[async_trait]
impl EmbeddingProvider for HybridRerankProvider {
    async fn embed(&self, text: &str, kind: EmbeddingKind) -> Result<Vec<f32>, EmbeddingError> {
        self.embedder.embed(text, kind).await
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        self.embedder.embed_query(text).await
    }

    async fn embed_batch(
        &self,
        items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        self.embedder.embed_batch(items).await
    }

    async fn hyde(&self, query: &str) -> Result<String, EmbeddingError> {
        let llm = match &self.hyde_llm {
            Some(llm) => llm,
            None => return Ok(query.to_string()),
        };

        let messages = vec![
            LlmMessage::System {
                content: "You are an assistant that writes hypothetical tool-invocation documents. \
                          Given a user query, describe in one sentence what tool would help and how \
                          it would be called. Keep it under 200 characters."
                    .to_string(),
            },
            LlmMessage::User {
                parts: vec![UserMessagePart::Text {
                    text: query.to_string(),
                }],
            },
        ];

        llm.chat_completion(&messages, None)
            .await
            .map_err(|e| EmbeddingError::Provider(e.to_string()))
    }

    async fn rerank(
        &self,
        query: &str,
        candidates: &[ene_tool_proto::ToolSpec],
    ) -> Result<Vec<f32>, EmbeddingError> {
        let llm = if let Some(llm) = &self.rerank_llm {
            llm
        } else {
            // Fall back to default cosine-similarity rerank.
            let query_emb = self.embed_query(query).await?;
            let mut scores = Vec::with_capacity(candidates.len());
            for spec in candidates {
                let text = format!("{} {}", spec.summary, spec.description);
                let emb = self.embed(&text, EmbeddingKind::Description).await?;
                scores.push(cosine_similarity(&query_emb, &emb));
            }
            return Ok(scores);
        };

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Build a prompt that asks the LLM to score each candidate 0..1.
        let cand_text: Vec<String> = candidates
            .iter()
            .enumerate()
            .map(|(i, s)| {
                format!(
                    "{}. Tool `{}`: {} — {}",
                    i, s.name, s.summary, s.description,
                )
            })
            .collect();

        let system = format!(
            "You are a relevance scorer. For each candidate tool listed below, \
             assign a score from 0.0 to 1.0 indicating how relevant it is to \
             the user query. Reply ONLY with a JSON array of scores in the \
             same order as the candidates, e.g. [0.8, 0.1, 0.4]. Do not include \
             any other text.\n\nCandidates:\n{}",
            cand_text.join("\n"),
        );

        let messages = vec![
            LlmMessage::System { content: system },
            LlmMessage::User {
                parts: vec![UserMessagePart::Text {
                    text: query.to_string(),
                }],
            },
        ];

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "scores": {
                    "type": "array",
                    "items": { "type": "number", "minimum": 0.0, "maximum": 1.0 }
                }
            },
            "required": ["scores"]
        });

        let raw = llm
            .chat_completion(&messages, Some(schema))
            .await
            .map_err(|e| EmbeddingError::Provider(e.to_string()))?;

        // Parse the response. The previous implementation
        // silently returned `vec![0.0; candidates.len()]`
        // when the LLM produced a bare array (no `scores`
        // object) or a malformed object, which would have
        // the caller rank every candidate at exactly zero
        // — an indistinguishable "all-irrelevant" answer.
        // Surface a typed error instead.
        let parsed: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
            EmbeddingError::Provider(format!(
                "rerank response was not valid JSON: {e}; raw response: {raw}"
            ))
        })?;
        let arr = parsed
            .get("scores")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                EmbeddingError::Provider(format!(
                    "rerank response missing `scores` array; got: {parsed}"
                ))
            })?;
        if arr.len() != candidates.len() {
            return Err(EmbeddingError::Provider(format!(
                "rerank response had {} scores, expected {} (one per candidate)",
                arr.len(),
                candidates.len()
            )));
        }
        let scores: Vec<f32> = arr
            .iter()
            .map(|v| {
                v.as_f64()
                    .ok_or_else(|| {
                        EmbeddingError::Provider(format!(
                            "rerank response contained a non-numeric score: {v}"
                        ))
                    })
                    .map(|f| f as f32)
            })
            .collect::<Result<Vec<f32>, _>>()?;
        Ok(scores)
    }

    fn dimensions(&self) -> usize {
        self.embedder.dimensions()
    }

    fn model_name(&self) -> &str {
        self.embedder.model_name()
    }
}
