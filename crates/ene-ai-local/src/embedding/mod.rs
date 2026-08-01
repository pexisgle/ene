//! Local GGUF embedding via llama-cpp-4.

mod error;
mod model;
mod provider;

pub use error::EneEmbeddingError;
pub use provider::GgufEmbeddingProvider;

use ene_ai::resolve::ResolvedLocalModel;

/// Creates a GGUF local embedding provider from a resolved local model entry.
///
/// The returned provider runs inference on a dedicated worker thread owned
/// by an `ene_infer::EngineHandle` (see `model::LlamaEmbedModel`) — no
/// `tokio::task::block_in_place`/`spawn_blocking`, so this has no particular
/// runtime-flavor requirement beyond whatever the rest of the workspace
/// already needs.
pub fn create_local_provider(
    local: &ResolvedLocalModel,
) -> Result<Box<dyn ene_ai::EmbeddingProvider>, EneEmbeddingError> {
    let gguf_path = crate::gguf::resolve_local_gguf_path(local)
        .map_err(|e| EneEmbeddingError::LocalLlm(e.to_string()))?;
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
