//! Hybrid HyDE / rerank helpers wrapping a primary embedder with optional LLMs.
//!
//! These are pipeline helpers — not `EmbeddingProvider` trait methods.

use std::sync::Arc;

use ene_ai::{
    EmbeddingError, EmbeddingKind, EmbeddingProvider, LlmMessage, LlmProvider, UserMessagePart,
    cosine_similarity, embed, embed_query,
};

/// Wraps a primary embedder with optional LLM-backed `HyDE` and rerank.
///
/// Embedding operations delegate to the inner provider. `HyDE` and rerank are
/// inherent methods / free functions — they are not part of
/// [`EmbeddingProvider`].
pub struct HybridRerankProvider {
    embedder: Arc<dyn EmbeddingProvider>,
    /// LLM used for `HyDE` document generation. When `None`,
    /// [`hyde_document`] returns the query unchanged.
    hyde_llm: Option<Arc<dyn LlmProvider>>,
    /// LLM used for [`rerank_tool_specs`]. When `None`, falls back to
    /// cosine-similarity scoring against the embedder.
    rerank_llm: Option<Arc<dyn LlmProvider>>,
}

impl HybridRerankProvider {
    /// Creates a wrapper that uses `embedder` for all embedding operations.
    pub fn new(embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            embedder,
            hyde_llm: None,
            rerank_llm: None,
        }
    }

    /// Attaches LLM providers for `HyDE` and rerank. Pass `None`
    /// for either to keep the default fallback path for that task.
    #[must_use]
    pub fn with_llm(
        mut self,
        hyde_llm: Option<Arc<dyn LlmProvider>>,
        rerank_llm: Option<Arc<dyn LlmProvider>>,
    ) -> Self {
        self.hyde_llm = hyde_llm;
        self.rerank_llm = rerank_llm;
        self
    }

    /// Returns true when an LLM-backed reranker is configured.
    pub fn has_reranker(&self) -> bool {
        self.rerank_llm.is_some()
    }

    /// Generate a hypothetical document for retrieval (`HyDE`).
    pub async fn hyde(&self, query: &str) -> Result<String, EmbeddingError> {
        hyde_document(self.hyde_llm.as_deref(), query).await
    }

    /// Rerank tool candidates by relevance to `query`.
    pub async fn rerank(
        &self,
        query: &str,
        candidates: &[ene_tool_proto::ToolSpec],
    ) -> Result<Vec<f32>, EmbeddingError> {
        rerank_tool_specs(
            self.embedder.as_ref(),
            self.rerank_llm.as_deref(),
            query,
            candidates,
        )
        .await
    }

    /// Access the inner embedder.
    pub fn embedder(&self) -> &Arc<dyn EmbeddingProvider> {
        &self.embedder
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for HybridRerankProvider {
    async fn embed_batch(
        &self,
        items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        self.embedder.embed_batch(items).await
    }

    fn dimensions(&self) -> usize {
        self.embedder.dimensions()
    }

    fn model_name(&self) -> &str {
        self.embedder.model_name()
    }
}

/// Generate a hypothetical document for retrieval.
///
/// When `llm` is `None`, echoes `query` unchanged (cheap local default).
pub async fn hyde_document(
    llm: Option<&dyn LlmProvider>,
    query: &str,
) -> Result<String, EmbeddingError> {
    let Some(llm) = llm else {
        return Ok(query.to_string());
    };

    let messages = vec![
        LlmMessage::System {
            content: "You are an assistant that writes a short hypothetical passage that would \
                      be highly relevant for retrieving information about the user's query. \
                      Write one or two sentences of content that might appear in a matching \
                      document. Keep it under 200 characters."
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

/// Rerank tool candidates. Uses `rerank_llm` when present; otherwise cosine
/// similarity between the query embedding and each candidate's description.
pub async fn rerank_tool_specs(
    embedder: &dyn EmbeddingProvider,
    rerank_llm: Option<&dyn LlmProvider>,
    query: &str,
    candidates: &[ene_tool_proto::ToolSpec],
) -> Result<Vec<f32>, EmbeddingError> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let Some(llm) = rerank_llm else {
        let query_emb = embed_query(embedder, query).await?;
        let texts: Vec<String> = candidates
            .iter()
            .map(|spec| spec.description.clone())
            .collect();
        let batch_items: Vec<(&str, EmbeddingKind)> = texts
            .iter()
            .map(|t| (t.as_str(), EmbeddingKind::Description))
            .collect();
        let embeddings = embedder.embed_batch(&batch_items).await?;
        if embeddings.len() != candidates.len() {
            return Err(EmbeddingError::DimensionMismatch(format!(
                "rerank embed_batch returned {} vectors for {} candidates",
                embeddings.len(),
                candidates.len()
            )));
        }
        let mut scores = Vec::with_capacity(embeddings.len());
        for emb in embeddings {
            if emb.len() != query_emb.len() {
                return Err(EmbeddingError::DimensionMismatch(format!(
                    "rerank candidate dims {} != query dims {}",
                    emb.len(),
                    query_emb.len()
                )));
            }
            scores.push(cosine_similarity(&query_emb, &emb));
        }
        return Ok(scores);
    };

    let cand_text: Vec<String> = candidates
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. Tool `{}`: {}", i, s.name, s.description))
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
    arr.iter()
        .map(|v| {
            v.as_f64()
                .ok_or_else(|| {
                    EmbeddingError::Provider(format!(
                        "rerank response contained a non-numeric score: {v}"
                    ))
                })
                .map(|f| f as f32)
        })
        .collect()
}

/// Convenience: embed a single text through a hybrid provider's inner embedder.
pub async fn hybrid_embed(
    provider: &HybridRerankProvider,
    text: &str,
    kind: EmbeddingKind,
) -> Result<Vec<f32>, EmbeddingError> {
    embed(provider.embedder.as_ref(), text, kind).await
}
