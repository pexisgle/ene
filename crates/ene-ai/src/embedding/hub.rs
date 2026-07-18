//! `HuggingFace` Hub resolution for Jina v5 retrieval GGUFs.

use super::EneEmbeddingError;
use std::path::PathBuf;

/// Resolves (and downloads if needed) the GGUF path for a known embedding model.
pub fn resolve_gguf_path(
    model_name: &str,
    quantization: &str,
    model_dir: PathBuf,
) -> Result<PathBuf, EneEmbeddingError> {
    let model_name_owned = model_name.to_string();
    let quant_owned = quantization.to_string();

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
            let client = hf_hub::HFClient::builder()
                .cache_dir(model_dir)
                .build()
                .map_err(|e| {
                    EneEmbeddingError::LocalLlm(format!("Failed to create HF client: {e}"))
                })?;

            let (repo_owner, repo_name) = match model_name_owned.as_str() {
                "jina-embeddings-v5-text-nano" => ("jinaai", "jina-embeddings-v5-text-nano-retrieval"),
                "jina-embeddings-v5-text-small" => {
                    ("jinaai", "jina-embeddings-v5-text-small-retrieval")
                }
                _ => {
                    return Err(EneEmbeddingError::LocalLlm(format!(
                        "Unknown model: {model_name_owned}. Supported models: \
                         jina-embeddings-v5-text-nano, jina-embeddings-v5-text-small. \
                         For other GGUFs, call GgufEmbeddingProvider::load with a local path."
                    )));
                }
            };

            let repo = client.model(repo_owner, repo_name);

            let gguf_filename: String = match model_name_owned.as_str() {
                "jina-embeddings-v5-text-small" => match quant_owned.as_str() {
                    "F16" => "v5-small-retrieval-F16.gguf",
                    "Q8_0" => "v5-small-retrieval-Q8_0.gguf",
                    "Q4_K_M" => "v5-small-retrieval-Q4_K_M.gguf",
                    "Q4_K_S" => "v5-small-retrieval-Q4_K_S.gguf",
                    "Q5_K_M" => "v5-small-retrieval-Q5_K_M.gguf",
                    "Q2_K" => "v5-small-retrieval-Q2_K.gguf",
                    "IQ4_XS" => "v5-small-retrieval-IQ4_XS.gguf",
                    _ => {
                        tracing::warn!(
                            "[Embedding] Unknown quantization {quant_owned}, falling back to F16"
                        );
                        "v5-small-retrieval-F16.gguf"
                    }
                }
                .to_string(),
                "jina-embeddings-v5-text-nano" => {
                    format!("v5-nano-retrieval-{quant_owned}.gguf")
                }
                other => {
                    return Err(EneEmbeddingError::LocalLlm(format!(
                        "Internal: model {other:?} passed the repo_id match but not the filename match"
                    )));
                }
            };

            let gguf_path = repo
                .download_file()
                .filename(&gguf_filename)
                .send()
                .await
                .map_err(|e| {
                    EneEmbeddingError::LocalLlm(format!("Failed to download GGUF: {e}"))
                })?;

            tracing::info!(
                "[Embedding] GGUF model ready: {} ({} bytes)",
                gguf_path.display(),
                std::fs::metadata(&gguf_path).map_or(0, |m| m.len()),
            );

            Ok(gguf_path)
        })
    })
}
