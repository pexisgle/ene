use std::sync::Arc;

use candle_core::{D, Module, Tensor};
use candle_nn::ops;

use crate::error::AiCoreError;

use super::rotary::{RotaryEmbedding, repeat_kv};

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
            .map_err(super::candle_err("attn dims3"))?;

        let q = self
            .q_proj
            .forward(x)
            .map_err(super::candle_err("attn Q"))?
            .reshape((b, l, self.num_heads, self.head_dim))
            .map_err(super::candle_err("attn Q reshape"))?
            .transpose(1, 2)
            .map_err(super::candle_err("attn Q transpose"))?;

        let k = self
            .k_proj
            .forward(x)
            .map_err(super::candle_err("attn K"))?
            .reshape((b, l, self.num_kv_heads, self.head_dim))
            .map_err(super::candle_err("attn K reshape"))?
            .transpose(1, 2)
            .map_err(super::candle_err("attn K transpose"))?;

        let v = self
            .v_proj
            .forward(x)
            .map_err(super::candle_err("attn V"))?
            .reshape((b, l, self.num_kv_heads, self.head_dim))
            .map_err(super::candle_err("attn V reshape"))?
            .transpose(1, 2)
            .map_err(super::candle_err("attn V transpose"))?;

        let q_flat = q
            .flatten(0, 2)
            .map_err(super::candle_err("attn Q flat"))?;
        let k_flat = k
            .flatten(0, 2)
            .map_err(super::candle_err("attn K flat"))?;

        let q_flat = ops::rms_norm(&q_flat, &self.q_norm_weight, self.q_norm_eps)
            .map_err(super::candle_err("attn Q norm"))?;
        let k_flat = ops::rms_norm(&k_flat, &self.k_norm_weight, self.k_norm_eps)
            .map_err(super::candle_err("attn K norm"))?;

        let q = q_flat
            .reshape((b, self.num_heads, l, self.head_dim))
            .map_err(super::candle_err("attn Q unflat"))?;
        let k = k_flat
            .reshape((b, self.num_kv_heads, l, self.head_dim))
            .map_err(super::candle_err("attn K unflat"))?;

        let (q, k) = self.rotary_emb.apply(&q, &k)?;

        let k = repeat_kv(&k, self.num_kv_groups)
            .map_err(super::candle_err("attn repeat_kv k"))?
            .contiguous()
            .map_err(super::candle_err("attn k contig"))?;
        let v = repeat_kv(&v, self.num_kv_groups)
            .map_err(super::candle_err("attn repeat_kv v"))?
            .contiguous()
            .map_err(super::candle_err("attn v contig"))?;

        let scale = 1.0 / (self.head_dim as f64).sqrt();
        let scores = q
            .matmul(
                &k.transpose(2, 3)
                    .map_err(super::candle_err("attn kT"))?,
            )
            .map_err(super::candle_err("attn scores"))?
            .affine(scale, 0.0)
            .map_err(super::candle_err("attn scale"))?;

        let probs = ops::softmax(&scores, D::Minus1)
            .map_err(super::candle_err("attn softmax"))?;
        let ctx = probs
            .matmul(&v)
            .map_err(super::candle_err("attn ctx"))?;

        let ctx = ctx
            .transpose(1, 2)
            .map_err(super::candle_err("attn transpose back"))?
            .reshape((b, l, self.num_heads * self.head_dim))
            .map_err(super::candle_err("attn reshape back"))?;

        self.o_proj
            .forward(&ctx)
            .map_err(super::candle_err("attn O"))
    }
}
