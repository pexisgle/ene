//! Local GGUF quantized vector embedding provider using Candle.

mod error;
mod quantized;

use std::path::PathBuf;

pub use error::EneEmbeddingError;
pub use quantized::{GgufEmbeddingProvider, resolve_gguf_paths};

/// Creates a GGUF local embedding provider.
///
/// * `model` — Model name (e.g., `"jina-embeddings-v5-text-small"`)
/// * `quantization` — Quantization format (e.g., `"F16"`, `"Q4_K_M"`)
/// * `model_dir` — Directory where GGUF models are stored
///
/// # Runtime requirement
///
/// The returned provider's forward pass uses
/// `tokio::task::block_in_place` to call into Candle, which
/// is synchronous and CPU-bound. `block_in_place` requires
/// a **multi-thread tokio runtime**; it panics on a
/// `current_thread` runtime or outside any runtime.
pub fn create_local_provider(
    model: &str,
    quantization: &str,
    model_dir: PathBuf,
) -> Result<Box<dyn crate::EmbeddingProvider>, EneEmbeddingError> {
    let (gguf_path, tokenizer_path) = resolve_gguf_paths(model, quantization, model_dir)?;
    let max_length = 8192;
    let gguf_str = gguf_path.to_str().ok_or_else(|| {
        EneEmbeddingError::CandleError(format!(
            "GGUF path is not valid UTF-8: {}",
            gguf_path.display()
        ))
    })?;
    let tokenizer_str = tokenizer_path.to_str().ok_or_else(|| {
        EneEmbeddingError::CandleError(format!(
            "tokenizer path is not valid UTF-8: {}",
            tokenizer_path.display()
        ))
    })?;
    let provider =
        GgufEmbeddingProvider::load(model, gguf_str, tokenizer_str, max_length, quantization)?;
    Ok(Box::new(provider))
}
