use candle_core::{Module, Tensor};
use candle_nn::ops;

use crate::error::AiCoreError;

pub struct MlpBlock {
    pub gate_proj: candle_nn::Linear,
    pub up_proj: candle_nn::Linear,
    pub down_proj: candle_nn::Linear,
}

impl MlpBlock {
    pub fn forward(&self, x: &Tensor) -> Result<Tensor, AiCoreError> {
        let gate = self
            .gate_proj
            .forward(x)
            .map_err(|e| AiCoreError::EmbeddingError(format!("MLP gate: {e}")))?;
        let gate =
            ops::silu(&gate).map_err(|e| AiCoreError::EmbeddingError(format!("MLP silu: {e}")))?;
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
