use candle_core::{Module, Tensor};
use candle_nn::ops;

use crate::embedding::error::EneEmbeddingError;

#[expect(
    clippy::struct_field_names,
    reason = "quantized MLP fields mirror weight tensor names"
)]
pub struct MlpBlock {
    pub gate_proj: candle_nn::Linear,
    pub up_proj: candle_nn::Linear,
    pub down_proj: candle_nn::Linear,
}

impl MlpBlock {
    pub fn forward(&self, x: &Tensor) -> Result<Tensor, EneEmbeddingError> {
        let gate = self
            .gate_proj
            .forward(x)
            .map_err(super::candle_err("MLP gate"))?;
        let gate = ops::silu(&gate).map_err(super::candle_err("MLP silu"))?;
        let up = self
            .up_proj
            .forward(x)
            .map_err(super::candle_err("MLP up"))?;
        let x = gate.mul(&up).map_err(super::candle_err("MLP mul"))?;
        self.down_proj
            .forward(&x)
            .map_err(super::candle_err("MLP down"))
    }
}
