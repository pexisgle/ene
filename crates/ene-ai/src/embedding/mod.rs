//! Local GGUF embedding via llama-cpp-2 (#171).

mod error;
mod hub;
mod provider;

pub use error::EneEmbeddingError;
pub use hub::resolve_gguf_path;
pub use provider::GgufEmbeddingProvider;

use std::path::PathBuf;

/// Creates a GGUF local embedding provider.
///
/// * `model` — Model name (e.g., `"jina-embeddings-v5-text-small"`)
/// * `quantization` — Quantization format (e.g., `"F16"`, `"Q4_K_M"`)
/// * `model_dir` — Directory where GGUF models are stored / cached
///
/// # Runtime requirement
///
/// The returned provider uses `tokio::task::block_in_place` for the
/// synchronous llama.cpp forward pass. That requires a **multi-thread**
/// tokio runtime.
pub fn create_local_provider(
    model: &str,
    quantization: &str,
    model_dir: PathBuf,
) -> Result<Box<dyn crate::EmbeddingProvider>, EneEmbeddingError> {
    let gguf_path = resolve_gguf_path(model, quantization, model_dir)?;
    let gguf_str = gguf_path.to_str().ok_or_else(|| {
        EneEmbeddingError::LocalLlm(format!(
            "GGUF path is not valid UTF-8: {}",
            gguf_path.display()
        ))
    })?;
    let provider = GgufEmbeddingProvider::load(model, gguf_str, quantization)?;
    Ok(Box::new(provider))
}

/// Compatibility alias: older callers expected `(gguf, tokenizer)` paths.
/// Tokenizer is now embedded in the GGUF; the second path is empty.
pub fn resolve_gguf_paths(
    model_name: &str,
    quantization: &str,
    model_dir: PathBuf,
) -> Result<(PathBuf, PathBuf), EneEmbeddingError> {
    let gguf = resolve_gguf_path(model_name, quantization, model_dir)?;
    Ok((gguf, PathBuf::new()))
}
