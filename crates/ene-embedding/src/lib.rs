//! # ene-embedding
//!
//! Local GGUF quantized vector embedding provider using Candle/GGUF for the ene AI character platform.

#![warn(missing_docs)]

/// Embedding-related error types.
pub mod error;
mod quantized;

use std::path::PathBuf;

/// Publicly export `EneEmbeddingError`.
pub use error::EneEmbeddingError;
/// Local GGUF-quantized embedding provider and path resolution.
pub use quantized::{GgufEmbeddingProvider, resolve_gguf_paths};

/// Creates a GGUF local embedding provider.
///
/// * `model` — Model name (e.g., `"jina-embeddings-v5-text-small"`)
/// * `quantization` — Quantization format (e.g., `"F16"`, `"Q4_K_M"`)
/// * `model_dir` — Directory where GGUF models are stored
pub fn create_local_provider(
    model: &str,
    quantization: &str,
    model_dir: PathBuf,
) -> Result<Box<dyn ene_provider::EmbeddingProvider>, EneEmbeddingError> {
    let (gguf_path, tokenizer_path) = resolve_gguf_paths(model, quantization, model_dir)?;
    let max_length = 8192;
    let provider = GgufEmbeddingProvider::load(
        model,
        gguf_path.to_str().unwrap_or(""),
        tokenizer_path.to_str().unwrap_or(""),
        max_length,
        quantization,
    )?;
    Ok(Box::new(provider))
}
