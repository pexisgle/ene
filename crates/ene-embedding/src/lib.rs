//! # ene-embedding
//!
//! Local GGUF quantized vector embedding provider using Candle/GGUF for the ene AI character platform.

#![warn(missing_docs)]

#[cfg(target_os = "macos")]
extern crate accelerate_src;

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
///
/// # Runtime requirement
///
/// The returned provider's forward pass uses
/// `tokio::task::block_in_place` to call into Candle, which
/// is synchronous and CPU-bound. `block_in_place` requires
/// a **multi-thread tokio runtime**; it panics on a
/// `current_thread` runtime or outside any runtime.
///
/// Pass a `multi_thread` runtime when constructing your own
/// `Runtime::new()`:
///
/// ```ignore
/// // CORRECT
/// let rt = tokio::runtime::Builder::new_multi_thread()
///     .enable_all()
///     .build()?;
/// // INCORRECT — panics inside `embed_query`:
/// let rt = tokio::runtime::Builder::new_current_thread()
///     .enable_all()
///     .build()?;
/// ```
///
/// The `#[tokio::main]` macro on a `fn main()` uses the
/// multi-thread flavor by default, so plain
/// `#[tokio::main] async fn main()` is the simplest
/// correct setup.
pub fn create_local_provider(
    model: &str,
    quantization: &str,
    model_dir: PathBuf,
) -> Result<Box<dyn ene_provider::EmbeddingProvider>, EneEmbeddingError> {
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
