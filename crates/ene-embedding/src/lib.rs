//! # ene-embedding
//!
//! Vector embedding providers for the ene AI character platform.
//!
//! ## Providers
//!
//! - [`ApiEmbeddingProvider`] — OpenAI-compatible remote API embedding (e.g., OpenRouter, any OpenAI-compatible endpoint)
//! - [`GgufEmbeddingProvider`] — Local GPU-free inference via Candle with GGUF-quantized models (e.g., jina-embeddings-v5-text-small)
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use ene_embedding::{create_embedding_provider, EmbeddingConfig, EmbeddingProviderType};
//! use ene_embedding::EmbeddingProvider;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let config = EmbeddingConfig {
//!     provider_type: EmbeddingProviderType::Local,
//!     model: "jina-embeddings-v5-text-small".into(),
//!     ..Default::default()
//! };
//!
//! let provider = create_embedding_provider(
//!     config.provider_type, &config.model, "", "", 0, None,
//!     std::path::PathBuf::from("./models"),
//! )?;
//!
//! let embedding = tokio::runtime::Runtime::new()?
//!     .block_on(provider.embed_query("Hello, world!"))?;
//! println!("Dimensions: {}", embedding.len());
//! # Ok(())
//! # }
//! ```
//!
//! ## Utility Functions
//!
//! - [`cosine_similarity`] — Compute cosine similarity between two embedding vectors
//! - [`create_embedding_provider`] — Factory function dispatching on [`EmbeddingProviderType`]
#![warn(missing_docs)]

pub mod client;
pub mod config;
pub mod error;
mod quantized;

pub use config::{EmbeddingConfig, EmbeddingProviderType};

use async_openai::types::embeddings::CreateEmbeddingRequestArgs;
use async_trait::async_trait;
use std::time::Instant;

use crate::client::build_openai_client;
use crate::error::EmbeddingError;

pub use quantized::{GgufEmbeddingProvider, resolve_gguf_paths};

/// Trait for generating vector embeddings from text.
///
/// Implementors provide a unified interface for both local and remote embedding backends.
/// Implementations include [`ApiEmbeddingProvider`] for OpenAI-compatible APIs and
/// [`GgufEmbeddingProvider`] for local GPU-free inference.
///
/// # Required Methods
///
/// - [`embed`](EmbeddingProvider::embed) — Generate an embedding for any text (used for indexing)
/// - [`embed_query`](EmbeddingProvider::embed_query) — Generate an embedding optimized for query/search text
/// - [`dimensions`](EmbeddingProvider::dimensions) — Dimensionality of the embedding vectors
/// - [`model_name`](EmbeddingProvider::model_name) — Human-readable model identifier
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;
    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
}

/// OpenAI-compatible API embedding provider.
///
/// Sends text to a remote embedding endpoint (OpenAI, OpenRouter, or any compatible server).
/// Empty input returns a zero vector of `dims` elements.
pub struct ApiEmbeddingProvider {
    client: async_openai::Client<async_openai::config::OpenAIConfig>,
    model: String,
    dims: usize,
}

impl ApiEmbeddingProvider {
    /// Creates a new API-based embedding provider.
    ///
    /// * `base_url` — Base URL for the embedding API endpoint
    /// * `api_key` — API key (may be empty for local endpoints)
    /// * `model` — Model name (e.g., `"text-embedding-3-small"`)
    /// * `dims` — Expected embedding dimensions
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
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        if text.trim().is_empty() {
            return Ok(vec![0.0; self.dims]);
        }

        let start = Instant::now();
        let request = CreateEmbeddingRequestArgs::default()
            .model(&self.model)
            .input(text)
            .build()
            .map_err(|e| {
                EmbeddingError::EmbeddingError(format!("Failed to build embedding request: {e}"))
            })?;

        let response = self
            .client
            .embeddings()
            .create(request)
            .await
            .map_err(|e| {
                EmbeddingError::EmbeddingError(format!("Embedding API call failed: {e}"))
            })?;
        let elapsed = start.elapsed();

        tracing::debug!(
            "[Embedding] API({}) {} chars → {:.2}ms",
            self.model,
            text.len(),
            elapsed.as_secs_f64() * 1000.0,
        );

        let embedding = response.data.into_iter().next().ok_or_else(|| {
            EmbeddingError::EmbeddingError("Empty embedding response".to_string())
        })?;

        Ok(embedding.embedding)
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        self.embed(text).await
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

/// Computes the cosine similarity between two embedding vectors.
///
/// Returns a value in `[-1.0, 1.0]`. Returns `0.0` if the vectors have different lengths,
/// are empty, or either has zero norm.
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

/// Factory function that creates the appropriate [`EmbeddingProvider`] based on the
/// [`EmbeddingProviderType`] configuration.
///
/// * `Local` — Loads a GGUF-quantized model via [`GgufEmbeddingProvider`]
/// * `Api` — Creates an [`ApiEmbeddingProvider`] for OpenAI-compatible endpoints
///
/// Returns a boxed trait object suitable for dynamic dispatch.
pub fn create_embedding_provider(
    provider_type: crate::EmbeddingProviderType,
    model: &str,
    base_url: &str,
    api_key: &str,
    dimensions: usize,
    quantization: Option<&str>,
    model_dir: std::path::PathBuf,
) -> Result<Box<dyn EmbeddingProvider>, EmbeddingError> {
    match provider_type {
        crate::EmbeddingProviderType::Local => {
            let quant = quantization.unwrap_or("F16");
            let (gguf_path, tokenizer_path) = resolve_gguf_paths(model, quant, model_dir)?;
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
        crate::EmbeddingProviderType::Api => {
            let provider = ApiEmbeddingProvider::new(base_url, api_key, model, dimensions);
            Ok(Box::new(provider))
        }
    }
}
