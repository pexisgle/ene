use candle_core::{Device, Tensor};
use candle_nn::rotary_emb::rope;

use crate::error::AiCoreError;

pub fn rope_precomputed(x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<Tensor, AiCoreError> {
    rope(x, cos, sin).map_err(|e| AiCoreError::EmbeddingError(format!("RoPE failed: {e}")))
}

pub fn repeat_kv(x: &Tensor, n_rep: usize) -> Result<Tensor, AiCoreError> {
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

pub struct RotaryEmbedding {
    pub cos: Tensor,
    pub sin: Tensor,
}

impl RotaryEmbedding {
    pub fn new(
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
            .to_dtype(candle_core::DType::F32)
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

    pub fn apply(&self, q: &Tensor, k: &Tensor) -> Result<(Tensor, Tensor), AiCoreError> {
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
        let q_embed = rope_precomputed(
            &q.contiguous()
                .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE q contig: {e}")))?,
            &cos,
            &sin,
        )?;
        let k_embed = rope_precomputed(
            &k.contiguous()
                .map_err(|e| AiCoreError::EmbeddingError(format!("RoPE k contig: {e}")))?,
            &cos,
            &sin,
        )?;
        Ok((q_embed, k_embed))
    }
}
