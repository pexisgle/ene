use super::load::LoadedModel;
use super::map_llama_err;
use ene_ai::error::LlmProviderError;
use llama_cpp_4::context::LlamaContext;
use llama_cpp_4::llama_batch::LlamaBatch;
use llama_cpp_4::model::AddBos;

/// Embed `text` with last-token pooling (Jina v5 / llama.cpp default for
/// retrieval GGUFs), on the caller's cached `ctx` — see
/// `crate::embedding::model::LlamaEmbedModel`, which builds `ctx` once (with
/// `with_embeddings(true)`/`with_pooling_type(Last)`) and reuses it across
/// jobs rather than calling `model.new_context(..)` per request.
pub(crate) fn embed_text(
    loaded: &LoadedModel,
    ctx: &mut LlamaContext<'_>,
    text: &str,
) -> Result<Vec<f32>, LlmProviderError> {
    if text.trim().is_empty() {
        return Err(LlmProviderError::LocalLlm(
            "empty embedding input".to_string(),
        ));
    }

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
}
