use candle_core::{Module, Tensor};
use candle_nn::ops;

use crate::error::AiCoreError;

use super::layer::LayerBlock;

pub struct EmbeddingModel {
    pub embed_tokens: candle_nn::Embedding,
    pub layers: Vec<LayerBlock>,
    pub norm_weight: Tensor,
    pub norm_eps: f32,
    pub hidden_size: usize,
}

impl EmbeddingModel {
    pub fn forward(&self, input_ids: &Tensor) -> Result<Vec<f32>, AiCoreError> {
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
