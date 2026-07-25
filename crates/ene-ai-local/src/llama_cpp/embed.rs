//! Embedding forward pass via llama-cpp-2.

use super::backend::with_backend;
use super::load::LoadedModel;
use super::map_llama_err;
use ene_ai::error::LlmProviderError;
use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::AddBos;

/// Embed `text` with last-token pooling (Jina v5 / llama.cpp default for retrieval GGUFs).
pub(crate) fn embed_text(loaded: &LoadedModel, text: &str) -> Result<Vec<f32>, LlmProviderError> {
    if text.trim().is_empty() {
        return Err(LlmProviderError::LocalLlm(
            "empty embedding input".to_string(),
        ));
    }

    with_backend(|backend| {
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(Some(loaded.context_size))
            .with_embeddings(true)
            .with_pooling_type(LlamaPoolingType::Last);
        let mut ctx = loaded
            .model
            .new_context(backend, ctx_params)
            .map_err(|e| map_llama_err("failed to create embedding context", e))?;

        let tokens = loaded
            .model
            .str_to_token(text, AddBos::Always)
            .map_err(|e| map_llama_err("tokenize embedding input", e))?;
        if tokens.is_empty() {
            return Err(LlmProviderError::LocalLlm(
                "embedding input tokenized to empty sequence".to_string(),
            ));
        }

        let batch_capacity = tokens.len().saturating_add(64).max(512);
        let mut batch = LlamaBatch::new(batch_capacity, 1);
        for (i, token) in (0_i32..).zip(tokens.iter().copied()) {
            batch
                .add(token, i, &[0], true)
                .map_err(|e| map_llama_err("batch.add embedding", e))?;
        }

        ctx.decode(&mut batch)
            .map_err(|e| map_llama_err("decode embedding", e))?;

        let emb = ctx
            .embeddings_seq_ith(0)
            .map_err(|e| map_llama_err("embeddings_seq_ith", e))?;
        Ok(emb.to_vec())
    })
}
