use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::PathBuf;
use std::sync::Arc;

use candle_core::Device;
use candle_core::quantized::gguf_file;

use crate::error::{EmbeddingError, EneEmbeddingError};

use super::attention::AttentionBlock;
use super::layer::LayerBlock;
use super::mlp::MlpBlock;
use super::model::EmbeddingModel;
use super::rotary::RotaryEmbedding;

fn load_tensor<R: Read + Seek>(
    ct: &gguf_file::Content,
    reader: &mut R,
    name: &str,
    device: &Device,
) -> Result<candle_core::Tensor, EmbeddingError> {
    let qtensor = ct
        .tensor(reader, name, device)
        .map_err(super::candle_err(&format!("Tensor {name} not found")))?;
    qtensor
        .dequantize(device)
        .map_err(super::candle_err(&format!("Failed to dequantize {name}")))
}

pub fn load_model(
    gguf_path: &str,
    device: &Device,
) -> Result<(EmbeddingModel, HashMap<String, gguf_file::Value>), EmbeddingError> {
    let mut file = std::fs::File::open(gguf_path).map_err(super::candle_err("Cannot open GGUF"))?;
    let ct =
        gguf_file::Content::read(&mut file).map_err(super::candle_err("Failed to read GGUF"))?;

    let metadata = ct.metadata.clone();

    let md_get = |s: &str| -> Result<&gguf_file::Value, EmbeddingError> {
        metadata
            .get(s)
            .ok_or_else(|| EneEmbeddingError::CandleError(format!("Missing metadata: {s}")))
    };

    let num_heads = md_get("qwen3.attention.head_count")?
        .to_u32()
        .map_err(super::candle_err("head_count"))? as usize;
    let num_kv_heads = md_get("qwen3.attention.head_count_kv")?
        .to_u32()
        .map_err(super::candle_err("head_count_kv"))? as usize;
    let head_dim = md_get("qwen3.attention.key_length")?
        .to_u32()
        .map_err(super::candle_err("key_length"))? as usize;
    let num_layers = md_get("qwen3.block_count")?
        .to_u32()
        .map_err(super::candle_err("block_count"))? as usize;
    let hidden_size = md_get("qwen3.embedding_length")?
        .to_u32()
        .map_err(super::candle_err("embedding_length"))? as usize;
    let max_seq_len = md_get("qwen3.context_length")?
        .to_u32()
        .map_err(super::candle_err("context_length"))? as usize;
    let rms_norm_eps = md_get("qwen3.attention.layer_norm_rms_epsilon")?
        .to_f32()
        .map_err(super::candle_err("rms_epsilon"))?;
    let rope_theta = f64::from(
        md_get("qwen3.rope.freq_base")?
            .to_f32()
            .map_err(super::candle_err("freq_base"))?,
    );
    let num_kv_groups = num_heads / num_kv_heads;

    tracing::info!(
        "[Embedding] GGUF config: {} layers, {} heads ({} kv), {} dim, {} head_dim",
        num_layers,
        num_heads,
        num_kv_heads,
        hidden_size,
        head_dim,
    );

    let embed_weight = load_tensor(&ct, &mut file, "token_embd.weight", device)?;
    let embed_tokens = candle_nn::Embedding::new(embed_weight, hidden_size);

    let rotary_emb = Arc::new(RotaryEmbedding::new(
        head_dim,
        max_seq_len,
        rope_theta,
        device,
    )?);

    let mut layers = Vec::with_capacity(num_layers);
    for i in 0..num_layers {
        let prefix = format!("blk.{i}");

        let q_norm_weight = load_tensor(
            &ct,
            &mut file,
            &format!("{prefix}.attn_q_norm.weight"),
            device,
        )?;
        let k_norm_weight = load_tensor(
            &ct,
            &mut file,
            &format!("{prefix}.attn_k_norm.weight"),
            device,
        )?;

        let attn = AttentionBlock {
            q_proj: candle_nn::Linear::new(
                load_tensor(&ct, &mut file, &format!("{prefix}.attn_q.weight"), device)?,
                None,
            ),
            k_proj: candle_nn::Linear::new(
                load_tensor(&ct, &mut file, &format!("{prefix}.attn_k.weight"), device)?,
                None,
            ),
            v_proj: candle_nn::Linear::new(
                load_tensor(&ct, &mut file, &format!("{prefix}.attn_v.weight"), device)?,
                None,
            ),
            o_proj: candle_nn::Linear::new(
                load_tensor(
                    &ct,
                    &mut file,
                    &format!("{prefix}.attn_output.weight"),
                    device,
                )?,
                None,
            ),
            q_norm_weight,
            k_norm_weight,
            q_norm_eps: rms_norm_eps,
            k_norm_eps: rms_norm_eps,
            num_heads,
            num_kv_heads,
            num_kv_groups,
            head_dim,
            rotary_emb: rotary_emb.clone(),
        };

        let mlp = MlpBlock {
            gate_proj: candle_nn::Linear::new(
                load_tensor(&ct, &mut file, &format!("{prefix}.ffn_gate.weight"), device)?,
                None,
            ),
            up_proj: candle_nn::Linear::new(
                load_tensor(&ct, &mut file, &format!("{prefix}.ffn_up.weight"), device)?,
                None,
            ),
            down_proj: candle_nn::Linear::new(
                load_tensor(&ct, &mut file, &format!("{prefix}.ffn_down.weight"), device)?,
                None,
            ),
        };

        layers.push(LayerBlock {
            self_attn: attn,
            mlp,
            ln1_weight: load_tensor(
                &ct,
                &mut file,
                &format!("{prefix}.attn_norm.weight"),
                device,
            )?,
            ln2_weight: load_tensor(&ct, &mut file, &format!("{prefix}.ffn_norm.weight"), device)?,
            ln_eps: rms_norm_eps,
        });
    }

    let norm_weight = load_tensor(&ct, &mut file, "output_norm.weight", device)?;

    let model = EmbeddingModel {
        embed_tokens,
        layers,
        norm_weight,
        norm_eps: rms_norm_eps,
        hidden_size,
    };

    tracing::info!(
        "[Embedding] GGUF model loaded: {} layers, {} hidden, {} heads",
        num_layers,
        hidden_size,
        num_heads,
    );

    Ok((model, metadata))
}

/// Resolves GGUF weight and tokenizer file paths for a given model.
///
/// Downloads the model from `HuggingFace` Hub if not already cached in `model_dir`.
pub fn resolve_gguf_paths(
    model_name: &str,
    quantization: &str,
    model_dir: PathBuf,
) -> Result<(PathBuf, PathBuf), EmbeddingError> {
    let model_name_owned = model_name.to_string();
    let quant_owned = quantization.to_string();

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
            let api = hf_hub::api::tokio::ApiBuilder::new()
                .with_cache_dir(model_dir)
                .build()
                .map_err(super::candle_err("Failed to create HF API"))?;

            let repo_id = match model_name_owned.as_str() {
                "jina-embeddings-v5-text-nano" => "jinaai/jina-embeddings-v5-text-nano-retrieval",
                "jina-embeddings-v5-text-small" => "jinaai/jina-embeddings-v5-text-small-retrieval",
                _ => {
                    return Err(EneEmbeddingError::CandleError(format!(
                        "Unknown model: {model_name_owned}. Supported models: \
                         jina-embeddings-v5-text-nano, jina-embeddings-v5-text-small. \
                         Note: the local GGUF loader is qwen3-metadata-keyed (qwen3.attention.head_count, \
                         qwen3.embedding_length, ...), so other architectures cannot be loaded via \
                         create_local_provider even if their weights are present in model_dir. \
                         For Qwen3-Embedding-0.6B or other architectures, load the GgufEmbeddingProvider \
                         directly with a pre-existing local path."
                    )));
                }
            };

            let repo = api.model(repo_id.to_string());

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
                    return Err(EneEmbeddingError::CandleError(format!(
                        "Internal: model {other:?} passed the repo_id match but not the \
                         filename match. This should be unreachable; please file a bug."
                    )));
                }
            };

            let gguf_path = repo
                .get(&gguf_filename)
                .await
                .map_err(super::candle_err("Failed to download GGUF"))?;

            let tokenizer_path = repo
                .get("tokenizer.json")
                .await
                .map_err(super::candle_err("Failed to download tokenizer"))?;

            tracing::info!(
                "[Embedding] GGUF model ready: {} ({} bytes)",
                gguf_path.display(),
                std::fs::metadata(&gguf_path).map_or(0, |m| m.len()),
            );

            Ok((gguf_path, tokenizer_path))
        })
    })
}
