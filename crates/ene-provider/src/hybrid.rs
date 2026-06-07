use async_trait::async_trait;
use std::sync::Arc;

use crate::message::{LlmMessage, UserMessagePart};
use crate::traits::{
    EmbeddingError, EmbeddingKind, EmbeddingProvider, LlmProvider, cosine_similarity,
};

/// Wraps a primary embedder with optional LLM-backed `HyDE` and rerank.
///
/// When `llm` is set, `hyde()` delegates to the LLM to produce a hypothetical
/// tool-invocation document and `rerank()` uses LLM scoring. Otherwise both
/// fall back to the default cosine-similarity paths.
pub struct HybridRerankProvider {
    embedder: Arc<dyn EmbeddingProvider>,
    llm: Option<Arc<dyn LlmProvider>>,
    hyde_model: Option<String>,
    rerank_model: Option<String>,
}

impl HybridRerankProvider {
    /// Creates a wrapper that uses `embedder` for all embedding operations.
    /// `HyDE` and rerank use default cosine-similarity fallbacks unless
    /// LLM models are set.
    pub fn new(embedder: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            embedder,
            llm: None,
            hyde_model: None,
            rerank_model: None,
        }
    }

    /// Attaches an LLM provider and optional model overrides for `HyDE` / rerank.
    pub fn with_llm(
        mut self,
        llm: Arc<dyn LlmProvider>,
        hyde_model: Option<String>,
        rerank_model: Option<String>,
    ) -> Self {
        self.llm = Some(llm);
        self.hyde_model = hyde_model;
        self.rerank_model = rerank_model;
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
        let llm = match &self.llm {
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
            .map_err(EmbeddingError::Provider)
    }

    async fn rerank(
        &self,
        query: &str,
        candidates: &[ene_tool_proto::ToolSpec],
    ) -> Result<Vec<f32>, EmbeddingError> {
        let llm = if let Some(llm) = &self.llm {
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
            .map_err(EmbeddingError::Provider)?;

        // Parse JSON response.
        let parsed: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| EmbeddingError::Provider(format!("rerank JSON parse: {e}")))?;
        let scores: Vec<f32> = parsed["scores"].as_array().map_or_else(
            || vec![0.0; candidates.len()],
            |arr| {
                arr.iter()
                    .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                    .collect()
            },
        );
        Ok(scores)
    }

    fn dimensions(&self) -> usize {
        self.embedder.dimensions()
    }

    fn model_name(&self) -> &str {
        self.embedder.model_name()
    }
}
