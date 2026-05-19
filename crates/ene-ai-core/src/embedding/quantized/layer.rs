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
