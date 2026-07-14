use candle_core::{Module, Tensor};
use candle_nn::ops;

use crate::embedding::error::EneEmbeddingError;

use super::layer::LayerBlock;

pub struct EmbeddingModel {
    pub embed_tokens: candle_nn::Embedding,
    pub layers: Vec<LayerBlock>,
    pub norm_weight: Tensor,
    pub norm_eps: f32,
    pub hidden_size: usize,
}

impl EmbeddingModel {
    pub fn forward(&self, input_ids: &Tensor) -> Result<Vec<f32>, EneEmbeddingError> {
        let t_start = std::time::Instant::now();
        let (_b, _l) = input_ids
            .dims2()
            .map_err(super::candle_err("model dims2"))?;

        let mut h = self
            .embed_tokens
            .forward(input_ids)
            .map_err(super::candle_err("model embed"))?;
        let t_embed = t_start.elapsed();

        let t_layers_start = std::time::Instant::now();
        for layer in &self.layers {
            h = layer.forward(&h)?;
        }
        let t_layers = t_layers_start.elapsed();

        let t_norm_start = std::time::Instant::now();
        let h = ops::rms_norm(&h, &self.norm_weight, self.norm_eps)
            .map_err(super::candle_err("model final norm"))?;

        let hidden = h.mean(1).map_err(super::candle_err("model mean pool"))?;
        let hidden = hidden
            .squeeze(0)
            .map_err(super::candle_err("model squeeze"))?;
        let mut vec = hidden
            .to_vec1::<f32>()
            .map_err(super::candle_err("model to_vec1"))?;

        let norm_val: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_val > 0.0 {
            for x in &mut vec {
                *x /= norm_val;
            }
        }
        let t_norm = t_norm_start.elapsed();

        tracing::debug!(
            component = "EmbeddingModel",
            embed_ms = t_embed.as_secs_f64() * 1000.0,
            layers_ms = t_layers.as_secs_f64() * 1000.0,
            norm_pool_ms = t_norm.as_secs_f64() * 1000.0,
            "Model forward timing"
        );

        Ok(vec)
    }
}
