use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use candle_core::quantized::gguf_file;
use candle_core::{DType, Device, Module, Tensor};
use candle_nn::ops;
use candle_nn::rotary_emb::rope;
use tokenizers::Tokenizer;

use crate::error::AiCoreError;

fn rope_precomputed(x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<Tensor, AiCoreError> {
    rope(x, cos, sin).map_err(|e| AiCoreError::EmbeddingError(format!("RoPE failed: {e}")))
}

fn repeat_kv(x: &Tensor, n_rep: usize) -> Result<Tensor, AiCoreError> {
    if n_rep == 1 {
        return Ok(x.clone());
    }
    let (_b, _n_kv_heads, _seq_len, _head_dim) = x
        .dims4()
        .map_err(|e| AiCoreError::EmbeddingError(format!("repeat_kv dims4 failed: {e}")))?;
    let expanded = x
        .unsqueeze(2)
        .map_err(|e| AiCoreError::EmbeddingError(format!("repeat_kv unsqueeze failed: {e}")))?
        .expand((_b, _n_kv_heads, n_rep, _seq_len, _head_dim))
        .map_err(|e| AiCoreError::EmbeddingError(format!("repeat_kv expand failed: {e}")))?
        .reshape((_b, _n_kv_heads * n_rep, _seq_len, _head_dim))
        .map_err(|e| AiCoreError::EmbeddingError(format!("repeat_kv reshape failed: {e}")))?;
    Ok(expanded)
}

struct RotaryEmbedding {
    cos: Tensor,
    sin: Tensor,
}

impl RotaryEmbedding {
    fn new(
        head_dim: usize,
        max_position_embeddings: usize,
        rope_theta: f64,
        device: &Device,
    ) -> Result<Self, AiCoreError> {
        let inv_freq: Vec<f32> = (0..head_dim)
            .step_by(2)
            .map(|i| 1.0_f32 / rope_theta.powf(i as f64 / head_dim as f64) as f32)
            .collect();
        let inv_freq_len = inv_freq.len();
        let inv_freq = Tensor::from_vec(inv_freq, (1, inv_freq_len), device)
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE inv_freq: {e}")))?;
        let t = Tensor::arange(0u32, max_position_embeddings as u32, device)
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE arange: {e}")))?
            .to_dtype(DType::F32)
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE dtype: {e}")))?
            .reshape((max_position_embeddings, 1))
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE reshape t: {e}")))?;
        let freqs = t
            .matmul(&inv_freq)
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE freqs: {e}")))?;
        let cos = freqs
            .cos()
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE cos: {e}")))?;
        let sin = freqs
            .sin()
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE sin: {e}")))?;
        Ok(Self { cos, sin })
    }

    fn apply(
        &self,
        q: &Tensor,
        k: &Tensor,
    ) -> Result<(Tensor, Tensor), AiCoreError> {
        let (_b, _num_heads, seq_len, _head_dim) = q
            .dims4()
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE q dims: {e}")))?;
        let cos = self
            .cos
            .narrow(0, 0, seq_len)
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE cos narrow: {e}")))?
            .to_dtype(q.dtype())
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE cos dtype: {e}")))?;
        let sin = self
            .sin
            .narrow(0, 0, seq_len)
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE sin narrow: {e}")))?
            .to_dtype(q.dtype())
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE sin dtype: {e}")))?;
        let q_embed = rope_precomputed(&q.contiguous()
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE q contig: {e}")))?, &cos, &sin)?;
        let k_embed = rope_precomputed(&k.contiguous()
            .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE k contig: {e}")))?, &cos, &sin)?;
        Ok((q_embed, k_embed))
    }
}

struct MlpBlock {
    gate_proj: candle_nn::Linear,
    up_proj: candle_nn::Linear,
    down_proj: candle_nn::Linear,
}

impl MlpBlock {
    fn forward(&self, x: &Tensor) -> Result<Tensor, AiCoreError> {
        let gate = self
            .gate_proj
            .forward(x)
            .map_err(|e| AiCoreError::EmbeddingError(format!("MLP gate: {e}")))?;
        let gate = ops::silu(&gate)
            .map_err(|e| AiCoreError::EmbeddingError(format!("MLP silu: {e}")))?;
        let up = self
            .up_proj
            .forward(x)
            .map_err(|e| AiCoreError::EmbeddingError(format!("MLP up: {e}")))?;
        let x = gate
            .mul(&up)
            .map_err(|e| AiCoreError::EmbeddingError(format!("MLP mul: {e}")))?;
        self.down_proj
            .forward(&x)
            .map_err(|e| AiCoreError::EmbeddingError(format!("MLP down: {e}")))
    }
}

struct AttentionBlock {
    q_proj: candle_nn::Linear,
    k_proj: candle_nn::Linear,
    v_proj: candle_nn::Linear,
    o_proj: candle_nn::Linear,
    q_norm_weight: Tensor,
    k_norm_weight: Tensor,
    q_norm_eps: f32,
    k_norm_eps: f32,
    num_heads: usize,
    num_kv_heads: usize,
    num_kv_groups: usize,
    head_dim: usize,
    hidden_size: usize,
    rotary_emb: Arc<RotaryEmbedding>,
}

impl AttentionBlock {
    fn forward(&self, x: &Tensor) -> Result<Tensor, AiCoreError> {
        let (b, l, _h) = x
            .dims3()
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn dims3: {e}")))?;

        let q = self
            .q_proj
            .forward(x)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn Q: {e}")))?
            .reshape((b, l, self.num_heads, self.head_dim))
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn Q reshape: {e}")))?
            .transpose(1, 2)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn Q transpose: {e}")))?;

        let k = self
            .k_proj
            .forward(x)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn K: {e}")))?
            .reshape((b, l, self.num_kv_heads, self.head_dim))
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn K reshape: {e}")))?
            .transpose(1, 2)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn K transpose: {e}")))?;

        let v = self
            .v_proj
            .forward(x)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn V: {e}")))?
            .reshape((b, l, self.num_kv_heads, self.head_dim))
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn V reshape: {e}")))?
            .transpose(1, 2)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn V transpose: {e}")))?;

        let q_flat = q
            .flatten(0, 2)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn Q flat: {e}")))?;
        let k_flat = k
            .flatten(0, 2)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn K flat: {e}")))?;

        let q_flat = ops::rms_norm(&q_flat, &self.q_norm_weight, self.q_norm_eps)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn Q norm: {e}")))?;
        let k_flat = ops::rms_norm(&k_flat, &self.k_norm_weight, self.k_norm_eps)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn K norm: {e}")))?;

        let q = q_flat
            .reshape((b, self.num_heads, l, self.head_dim))
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn Q unflat: {e}")))?;
        let k = k_flat
            .reshape((b, self.num_kv_heads, l, self.head_dim))
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn K unflat: {e}")))?;

        let (q, k) = self.rotary_emb.apply(&q, &k)?;

        let k = repeat_kv(&k, self.num_kv_groups)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn repeat_kv k: {e}")))?
            .contiguous()
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn k contig: {e}")))?;
        let v = repeat_kv(&v, self.num_kv_groups)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn repeat_kv v: {e}")))?
            .contiguous()
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn v contig: {e}")))?;

        let scale = 1.0 / (self.head_dim as f64).sqrt();
        let scores = q
            .matmul(&k.transpose(2, 3).map_err(|e| AiCoreError::EmbeddingError(format!("attn kT: {e}")))?)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn scores: {e}")))?
            .affine(scale, 0.0)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn scale: {e}")))?;

        let probs = ops::softmax(&scores, candle_core::D::Minus1)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn softmax: {e}")))?;
        let ctx = probs
            .matmul(&v)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn ctx: {e}")))?;

        let ctx = ctx
            .transpose(1, 2)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn transpose back: {e}")))?
            .reshape((b, l, self.hidden_size))
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn reshape back: {e}")))?;

        self.o_proj
            .forward(&ctx)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn O: {e}")))
    }
}

struct LayerBlock {
    self_attn: AttentionBlock,
    mlp: MlpBlock,
    ln1_weight: Tensor,
    ln2_weight: Tensor,
    ln_eps: f32,
}

impl LayerBlock {
    fn forward(&self, x: &Tensor) -> Result<Tensor, AiCoreError> {
        let h = ops::rms_norm(x, &self.ln1_weight, self.ln_eps)
            .map_err(|e| AiCoreError::EmbeddingError(format!("layer ln1: {e}")))?;
        let h = self
            .self_attn
            .forward(&h)
            .map_err(|e| AiCoreError::EmbeddingError(format!("layer attn: {e}")))?;
        let x = x
            .add(&h)
            .map_err(|e| AiCoreError::EmbeddingError(format!("layer resid1: {e}")))?;
        let h2 = ops::rms_norm(&x, &self.ln2_weight, self.ln_eps)
            .map_err(|e| AiCoreError::EmbeddingError(format!("layer ln2: {e}")))?;
        let h2 = self.mlp.forward(&h2)?;
        x.add(&h2)
            .map_err(|e| AiCoreError::EmbeddingError(format!("layer resid2: {e}")))
    }
}

struct EmbeddingModel {
    embed_tokens: candle_nn::Embedding,
    layers: Vec<LayerBlock>,
    norm_weight: Tensor,
    norm_eps: f32,
    hidden_size: usize,
}

impl EmbeddingModel {
    fn forward(&self, input_ids: &Tensor) -> Result<Vec<f32>, AiCoreError> {
        let (_b, _l) = input_ids
            .dims2()
            .map_err(|e| AiCoreError::EmbeddingError(format!("model dims2: {e}")))?;

        let mut h = self
            .embed_tokens
            .forward(input_ids)
            .map_err(|e| AiCoreError::EmbeddingError(format!("model embed: {e}")))?;

        for layer in &self.layers {
            h = layer.forward(&h)?;
        }

        let h = ops::rms_norm(&h, &self.norm_weight, self.norm_eps)
            .map_err(|e| AiCoreError::EmbeddingError(format!("model final norm: {e}")))?;

        let hidden = h
            .mean(1)
            .map_err(|e| AiCoreError::EmbeddingError(format!("model mean pool: {e}")))?;
        let hidden = hidden
            .squeeze(0)
            .map_err(|e| AiCoreError::EmbeddingError(format!("model squeeze: {e}")))?;
        let mut vec = hidden
            .to_vec1::<f32>()
            .map_err(|e| AiCoreError::EmbeddingError(format!("model to_vec1: {e}")))?;

        let norm_val: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_val > 0.0 {
            for x in vec.iter_mut() {
                *x /= norm_val;
            }
        }

        Ok(vec)
    }
}

fn load_tensor<R: Read + Seek>(
    ct: &gguf_file::Content,
    reader: &mut R,
    name: &str,
    device: &Device,
) -> Result<Tensor, AiCoreError> {
    let qtensor = ct
        .tensor(reader, name, device)
        .map_err(|e| AiCoreError::EmbeddingError(format!("Tensor {name} not found: {e}")))?;
    qtensor
        .dequantize(device)
        .map_err(|e| AiCoreError::EmbeddingError(format!("Failed to dequantize {name}: {e}")))
}

fn load_model(
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
        .map_err(|e| AiCoreError::EmbeddingError(format!("head_count: {e}")))? as usize;
    let num_kv_heads = md_get("qwen3.attention.head_count_kv")?
        .to_u32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("head_count_kv: {e}")))? as usize;
    let head_dim = md_get("qwen3.attention.key_length")?
        .to_u32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("key_length: {e}")))? as usize;
    let num_layers = md_get("qwen3.block_count")?
        .to_u32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("block_count: {e}")))? as usize;
    let hidden_size = md_get("qwen3.embedding_length")?
        .to_u32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("embedding_length: {e}")))? as usize;
    let max_seq_len = md_get("qwen3.context_length")?
        .to_u32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("context_length: {e}")))? as usize;
    let rms_norm_eps = md_get("qwen3.attention.layer_norm_rms_epsilon")?
        .to_f32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("rms_epsilon: {e}")))?;
    let rope_theta = md_get("qwen3.rope.freq_base")?
        .to_f32()
        .map_err(|e| AiCoreError::EmbeddingError(format!("freq_base: {e}")))? as f64;
    let num_kv_groups = num_heads / num_kv_heads;

    tracing::info!(
        "[Embedding] GGUF config: {} layers, {} heads ({} kv), {} dim, {} head_dim",
        num_layers, num_heads, num_kv_heads, hidden_size, head_dim,
    );

    let embed_weight = load_tensor(&ct, &mut file, "token_embd.weight", device)?;
    let embed_tokens = candle_nn::Embedding::new(embed_weight, hidden_size);

    let rotary_emb = Arc::new(RotaryEmbedding::new(
        head_dim, max_seq_len, rope_theta, device,
    )?);

    let mut layers = Vec::with_capacity(num_layers);
    for i in 0..num_layers {
        let prefix = format!("blk.{i}");

        let q_norm_weight = load_tensor(&ct, &mut file, &format!("{prefix}.attn_q_norm.weight"), device)?;
        let k_norm_weight = load_tensor(&ct, &mut file, &format!("{prefix}.attn_k_norm.weight"), device)?;

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
                load_tensor(&ct, &mut file, &format!("{prefix}.attn_output.weight"), device)?,
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
            hidden_size,
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
            ln1_weight: load_tensor(&ct, &mut file, &format!("{prefix}.attn_norm.weight"), device)?,
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
        num_layers, hidden_size, num_heads,
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
                .map_err(|e| AiCoreError::EmbeddingError(format!("Failed to create HF API: {e}")))?;

            let repo_id = match model_name_owned.as_str() {
                "jina-embeddings-v5-text-nano" => {
                    "jinaai/jina-embeddings-v5-text-nano-retrieval"
                }
                "jina-embeddings-v5-text-small" => {
                    "jinaai/jina-embeddings-v5-text-small-retrieval"
                }
                _ => {
                    return Err(AiCoreError::EmbeddingError(format!(
                        "Unknown model: {model_name_owned}"
                    )))
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

            let gguf_path = repo
                .get(&gguf_filename)
                .await
                .map_err(|e| AiCoreError::EmbeddingError(format!("Failed to download GGUF: {e}")))?;

            let tokenizer_path = repo
                .get("tokenizer.json")
                .await
                .map_err(|e| AiCoreError::EmbeddingError(format!("Failed to download tokenizer: {e}")))?;

            tracing::info!(
                "[Embedding] GGUF model ready: {} ({} bytes)",
                gguf_path.display(),
                std::fs::metadata(&gguf_path).map(|m| m.len()).unwrap_or(0),
            );

            Ok((gguf_path, tokenizer_path))
        })
    })
}

pub struct GgufEmbeddingProvider {
    model: Arc<Mutex<EmbeddingModel>>,
    tokenizer: Mutex<Tokenizer>,
    dims: usize,
    model_name: String,
}

impl GgufEmbeddingProvider {
    pub fn load(
        model_name: &str,
        gguf_path: &str,
        tokenizer_path: &str,
        max_length: usize,
        quantization: &str,
    ) -> Result<Self, AiCoreError> {
        let start = Instant::now();

        let device = Device::Cpu;

        let (model, _metadata) = load_model(gguf_path, &device)?;

        let mut tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| AiCoreError::EmbeddingError(format!("Failed to load tokenizer: {e}")))?;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length,
                ..Default::default()
            }))
            .map_err(|e| AiCoreError::EmbeddingError(format!("Failed to set truncation: {e}")))?;

        let dims = model.hidden_size;

        let elapsed = start.elapsed();
        tracing::info!(
            "[Embedding] GGUF({model_name}@{quantization}) loaded: {dims} dims, {max_length} max_len ({:.1}s)",
            elapsed.as_secs_f64(),
        );

        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            tokenizer: Mutex::new(tokenizer),
            dims,
            model_name: format!("{model_name}@{quantization}"),
        })
    }

    fn embed_internal(&self, text: &str, prefix: &str) -> Result<Vec<f32>, AiCoreError> {
        let prefixed = format!("{prefix}{text}");
        let tokenizer = self.tokenizer.lock().unwrap();
        let encoding = tokenizer
            .encode(prefixed.as_str(), true)
            .map_err(|e| AiCoreError::EmbeddingError(format!("Tokenization failed: {e}")))?;
        let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();

        if input_ids.is_empty() {
            return Ok(vec![0.0; self.dims]);
        }

        let input_tensor = Tensor::from_vec(
            input_ids.iter().map(|&x| x as u32).collect::<Vec<_>>(),
            (1, input_ids.len()),
            &Device::Cpu,
        )
        .map_err(|e| AiCoreError::EmbeddingError(format!("Failed to create input tensor: {e}")))?;

        let model = self.model.lock().unwrap();
        model.forward(&input_tensor)
    }
}

use async_trait::async_trait;
use super::EmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for GgufEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AiCoreError> {
        if text.trim().is_empty() {
            return Ok(vec![0.0; self.dims]);
        }
        let owned = text.to_string();
        let model = self.model_name.clone();

        tokio::task::block_in_place(|| {
            let start = Instant::now();
            let result = self.embed_internal(&owned, "Document: ");
            let elapsed = start.elapsed();
            if let Ok(_) = &result {
                tracing::debug!(
                    "[Embedding] GGUF({}) {} chars → {:.2}ms",
                    model,
                    owned.len(),
                    elapsed.as_secs_f64() * 1000.0,
                );
            }
            result
        })
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, AiCoreError> {
        if text.trim().is_empty() {
            return Ok(vec![0.0; self.dims]);
        }
        let owned = text.to_string();
        let model = self.model_name.clone();

        tokio::task::block_in_place(|| {
            let start = Instant::now();
            let result = self.embed_internal(&owned, "Query: ");
            let elapsed = start.elapsed();
            if let Ok(_) = &result {
                tracing::debug!(
                    "[Embedding] GGUF({}) query {} chars → {:.2}ms",
                    model,
                    owned.len(),
                    elapsed.as_secs_f64() * 1000.0,
                );
            }
            result
        })
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}
