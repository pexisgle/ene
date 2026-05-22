use candle_core::Tensor;
use candle_nn::ops;

use crate::error::AiCoreError;

use super::attention::AttentionBlock;
use super::mlp::MlpBlock;

pub struct LayerBlock {
    pub self_attn: AttentionBlock,
    pub mlp: MlpBlock,
    pub ln1_weight: Tensor,
    pub ln2_weight: Tensor,
    pub ln_eps: f32,
}

impl LayerBlock {
    pub fn forward(&self, x: &Tensor) -> Result<Tensor, AiCoreError> {
        let h = ops::rms_norm(x, &self.ln1_weight, self.ln_eps)
            .map_err(super::candle_err("layer ln1"))?;
        let h = self
            .self_attn
            .forward(&h)
            .map_err(super::candle_err("layer attn"))?;
        let x = x
            .add(&h)
            .map_err(super::candle_err("layer resid1"))?;
        let h2 = ops::rms_norm(&x, &self.ln2_weight, self.ln_eps)
            .map_err(super::candle_err("layer ln2"))?;
        let h2 = self.mlp.forward(&h2)?;
        x.add(&h2)
            .map_err(super::candle_err("layer resid2"))
    }
}
