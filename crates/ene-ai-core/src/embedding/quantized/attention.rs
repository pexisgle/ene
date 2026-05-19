use std::sync::Arc;

use candle_core::{D, Module, Tensor};
use candle_nn::ops;

use crate::error::AiCoreError;

use super::rotary::{repeat_kv, RotaryEmbedding};

pub struct AttentionBlock {
    pub q_proj: candle_nn::Linear,
    pub k_proj: candle_nn::Linear,
    pub v_proj: candle_nn::Linear,
    pub o_proj: candle_nn::Linear,
    pub q_norm_weight: Tensor,
    pub k_norm_weight: Tensor,
    pub q_norm_eps: f32,
    pub k_norm_eps: f32,
    pub num_heads: usize,
    pub num_kv_heads: usize,
    pub num_kv_groups: usize,
    pub head_dim: usize,
    pub rotary_emb: Arc<RotaryEmbedding>,
}

impl AttentionBlock {
    pub fn forward(&self, x: &Tensor) -> Result<Tensor, AiCoreError> {
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
            .matmul(
                &k.transpose(2, 3)
                    .map_err(|e| AiCoreError::EmbeddingError(format!("attn kT: {e}")))?,
            )
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn scores: {e}")))?
            .affine(scale, 0.0)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn scale: {e}")))?;

        let probs = ops::softmax(&scores, D::Minus1)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn softmax: {e}")))?;
        let ctx = probs
            .matmul(&v)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn ctx: {e}")))?;

        let ctx = ctx
            .transpose(1, 2)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn transpose back: {e}")))?
            .reshape((b, l, self.num_heads * self.head_dim))
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn reshape back: {e}")))?;

        self.o_proj
            .forward(&ctx)
            .map_err(|e| AiCoreError::EmbeddingError(format!("attn O: {e}")))
    }
}
