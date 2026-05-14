mod quantized;

use std::time::Instant;
use async_trait::async_trait;
use async_openai::types::embeddings::CreateEmbeddingRequestArgs;

use crate::client::build_openai_client;
use crate::error::AiCoreError;

pub use quantized::{GgufEmbeddingProvider, resolve_gguf_paths};

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AiCoreError>;
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, AiCoreError>;
    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
}

pub struct ApiEmbeddingProvider {
    client: async_openai::Client<async_openai::config::OpenAIConfig>,
    model: String,
    dims: usize,
}

impl ApiEmbeddingProvider {
    pub fn new(base_url: &str, api_key: &str, model: &str, dims: usize) -> Self {
        Self {
            client: build_openai_client(base_url, api_key),
            model: model.to_string(),
            dims,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for ApiEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AiCoreError> {
        if text.trim().is_empty() {
            return Ok(vec![0.0; self.dims]);
        }

        let start = Instant::now();
        let request = CreateEmbeddingRequestArgs::default()
            .model(&self.model)
            .input(text)
            .build()
            .map_err(|e| AiCoreError::EmbeddingError(format!("Failed to build embedding request: {e}")))?;

        let response = self.client.embeddings().create(request).await
            .map_err(|e| AiCoreError::EmbeddingError(format!("Embedding API call failed: {e}")))?;
        let elapsed = start.elapsed();

        tracing::debug!(
            "[Embedding] API({}) {} chars → {:.2}ms",
            self.model,
            text.len(),
            elapsed.as_secs_f64() * 1000.0,
        );

        let embedding = response.data.into_iter().next()
            .ok_or_else(|| AiCoreError::EmbeddingError("Empty embedding response".to_string()))?;

        Ok(embedding.embedding)
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, AiCoreError> {
        self.embed(text).await
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
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

pub fn create_embedding_provider(
    provider_type: crate::config::EmbeddingProviderType,
    model: &str,
    base_url: &str,
    api_key: &str,
    dimensions: usize,
    quantization: Option<&str>,
) -> Result<Box<dyn EmbeddingProvider>, AiCoreError> {
    match provider_type {
        crate::config::EmbeddingProviderType::Local => {
            let quant = quantization.unwrap_or("F16");
            let (gguf_path, tokenizer_path) = resolve_gguf_paths(model, quant)?;
            let max_length = 8192;
            let provider = GgufEmbeddingProvider::load(
                model,
                gguf_path.to_str().unwrap_or(""),
                tokenizer_path.to_str().unwrap_or(""),
                max_length,
                quant,
            )?;
            Ok(Box::new(provider))
        }
        crate::config::EmbeddingProviderType::Api => {
            let provider = ApiEmbeddingProvider::new(base_url, api_key, model, dimensions);
            Ok(Box::new(provider))
        }
    }
}
