use candle_core::{Device, Tensor};
use candle_nn::rotary_emb::rope;

use crate::embedding::error::EmbeddingError;

pub fn rope_precomputed(x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<Tensor, EmbeddingError> {
    rope(x, cos, sin).map_err(super::candle_err("RoPE failed"))
}

pub fn repeat_kv(x: &Tensor, n_rep: usize) -> Result<Tensor, EmbeddingError> {
    if n_rep == 1 {
        return Ok(x.clone());
    }
    let (_b, _n_kv_heads, _seq_len, _head_dim) = x
        .dims4()
        .map_err(super::candle_err("repeat_kv dims4 failed"))?;
    let expanded = x
        .unsqueeze(2)
        .map_err(super::candle_err("repeat_kv unsqueeze failed"))?
        .expand((_b, _n_kv_heads, n_rep, _seq_len, _head_dim))
        .map_err(super::candle_err("repeat_kv expand failed"))?
        .reshape((_b, _n_kv_heads * n_rep, _seq_len, _head_dim))
        .map_err(super::candle_err("repeat_kv reshape failed"))?;
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
    ) -> Result<Self, EmbeddingError> {
        let inv_freq: Vec<f32> = (0..head_dim)
            .step_by(2)
            .map(|i| 1.0_f32 / rope_theta.powf(i as f64 / head_dim as f64) as f32)
            .collect();
        let inv_freq_len = inv_freq.len();
        let inv_freq = Tensor::from_vec(inv_freq, (1, inv_freq_len), device)
            .map_err(super::candle_err("RoPE inv_freq"))?;
        let t = Tensor::arange(0u32, max_position_embeddings as u32, device)
            .map_err(super::candle_err("RoPE arange"))?
            .to_dtype(candle_core::DType::F32)
            .map_err(super::candle_err("RoPE dtype"))?
            .reshape((max_position_embeddings, 1))
            .map_err(super::candle_err("RoPE reshape t"))?;
        let freqs = t
            .matmul(&inv_freq)
            .map_err(super::candle_err("RoPE freqs"))?;
        let cos = freqs.cos().map_err(super::candle_err("RoPE cos"))?;
        let sin = freqs.sin().map_err(super::candle_err("RoPE sin"))?;
        Ok(Self { cos, sin })
    }

    pub fn apply(&self, q: &Tensor, k: &Tensor) -> Result<(Tensor, Tensor), EmbeddingError> {
        let (_b, _num_heads, seq_len, _head_dim) =
            q.dims4().map_err(super::candle_err("RoPE q dims"))?;
        let cos = self
            .cos
            .narrow(0, 0, seq_len)
            .map_err(super::candle_err("RoPE cos narrow"))?
            .to_dtype(q.dtype())
            .map_err(super::candle_err("RoPE cos dtype"))?;
        let sin = self
            .sin
            .narrow(0, 0, seq_len)
            .map_err(super::candle_err("RoPE sin narrow"))?
            .to_dtype(q.dtype())
            .map_err(super::candle_err("RoPE sin dtype"))?;
        let q_embed = rope_precomputed(
            &q.contiguous().map_err(super::candle_err("RoPE q contig"))?,
            &cos,
            &sin,
        )?;
        let k_embed = rope_precomputed(
            &k.contiguous().map_err(super::candle_err("RoPE k contig"))?,
            &cos,
            &sin,
        )?;
        Ok((q_embed, k_embed))
    }
}
