use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::PathBuf;
use std::sync::Arc;

use candle_core::quantized::gguf_file;
use candle_core::Device;

use crate::error::AiCoreError;

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
) -> Result<candle_core::Tensor, AiCoreError> {
    let qtensor = ct
        .tensor(reader, name, device)
        .map_err(|e| AiCoreError::EmbeddingError(format!("Tensor {name} not found: {e}")))?;
    qtensor
        .dequantize(device)
        .map_err(|e| AiCoreError::EmbeddingError(format!("Failed to dequantize {name}: {e}")))
}

pub fn load_model(
    gguf_path: &str,
    device: &Device,
) -> Result<(EmbeddingModel, HashMap<String, gguf_file::Value>), AiCoreError> {
    let mut file = std::fs::File::open(gguf_path)
        .map_err(|e| AiCoreError::EmbeddingError(format!("Cannot open GGUF: {e}")))?;
    let ct = gguf_file::Content::read(&mut file)
        .map_err(|e| AiCoreError::EmbeddingError(format!("Failed to read GGUF: {e}")))?;

    let metadata = ct.metadata.clone();

    let md_get = |s: &str| -> Result<&gguf_file::Value, AiCoreError> {
        metadata
            .get(s)
            .ok_or_else(|| AiCoreError::EmbeddingError(format!("Missing metadata: {s}")))
    };

    let num_heads = md_get("qwen3.attention.head_count")?
        .to_u32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("head_count: {e}")))?
        as usize;
    let num_kv_heads = md_get("qwen3.attention.head_count_kv")?
        .to_u32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("head_count_kv: {e}")))?
        as usize;
    let head_dim = md_get("qwen3.attention.key_length")?
        .to_u32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("key_length: {e}")))?
        as usize;
    let num_layers = md_get("qwen3.block_count")?
        .to_u32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("block_count: {e}")))?
        as usize;
    let hidden_size = md_get("qwen3.embedding_length")?
        .to_u32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("embedding_length: {e}")))?
        as usize;
    let max_seq_len = md_get("qwen3.context_length")?
        .to_u32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("context_length: {e}")))?
        as usize;
    let rms_norm_eps = md_get("qwen3.attention.layer_norm_rms_epsilon")?
        .to_f32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("rms_epsilon: {e}")))?;
    let rope_theta = md_get("qwen3.rope.freq_base")?
        .to_f32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("freq_base: {e}")))?
        as f64;
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

pub fn resolve_gguf_paths(
    model_name: &str,
    quantization: &str,
) -> Result<(PathBuf, PathBuf), AiCoreError> {
    let model_dir = crate::paths::models_dir();

    let model_name_owned = model_name.to_string();
    let quant_owned = quantization.to_string();

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async move {
            let api = hf_hub::api::tokio::ApiBuilder::new()
                .with_cache_dir(model_dir)
                .build()
                .map_err(|e| {
                    AiCoreError::EmbeddingError(format!("Failed to create HF API: {e}"))
                })?;

            let repo_id = match model_name_owned.as_str() {
                "jina-embeddings-v5-text-nano" => "jinaai/jina-embeddings-v5-text-nano-retrieval",
                "jina-embeddings-v5-text-small" => "jinaai/jina-embeddings-v5-text-small-retrieval",
                _ => {
                    return Err(AiCoreError::EmbeddingError(format!(
                        "Unknown model: {model_name_owned}"
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
                _ => unreachable!(),
            };

            let gguf_path = repo.get(&gguf_filename).await.map_err(|e| {
                AiCoreError::EmbeddingError(format!("Failed to download GGUF: {e}"))
            })?;

            let tokenizer_path = repo.get("tokenizer.json").await.map_err(|e| {
                AiCoreError::EmbeddingError(format!("Failed to download tokenizer: {e}"))
            })?;

            tracing::info!(
                "[Embedding] GGUF model ready: {} ({} bytes)",
                gguf_path.display(),
                std::fs::metadata(&gguf_path).map(|m| m.len()).unwrap_or(0),
            );

            Ok((gguf_path, tokenizer_path))
        })
    })
}
