//! Local GGUF embedding via llama-cpp-2 (#171).

mod error;
mod provider;

pub use error::EneEmbeddingError;
pub use provider::GgufEmbeddingProvider;

use crate::resolve::ResolvedLocalModel;

/// Creates a GGUF local embedding provider from a resolved local model entry.
///
/// # Runtime requirement
///
/// The returned provider uses `tokio::task::block_in_place` for the
/// synchronous llama.cpp forward pass. That requires a **multi-thread**
/// tokio runtime.
pub fn create_local_provider(
    local: &ResolvedLocalModel,
) -> Result<Box<dyn crate::EmbeddingProvider>, EneEmbeddingError> {
    let gguf_path = crate::gguf::resolve_local_gguf_path(local).map_err(|e| {
        EneEmbeddingError::LocalLlm(e.to_string())
    })?;
    let gguf_str = gguf_path.to_str().ok_or_else(|| {
        EneEmbeddingError::LocalLlm(format!(
            "GGUF path is not valid UTF-8: {}",
            gguf_path.display()
        ))
    })?;
    let provider = GgufEmbeddingProvider::load_with_acceleration(
        &local.name,
        gguf_str,
        &local.quantization,
        local.acceleration,
    )?;
    Ok(Box::new(provider))
}
