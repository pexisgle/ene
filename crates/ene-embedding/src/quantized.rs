mod attention;
mod layer;
mod loader;
mod mlp;
mod model;
mod rotary;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use candle_core::{Device, Tensor};
use tokenizers::Tokenizer;

use crate::error::EmbeddingError;

pub use loader::resolve_gguf_paths;

use loader::load_model;
use model::EmbeddingModel;

pub(crate) fn candle_err<E: std::fmt::Display>(msg: &str) -> impl FnOnce(E) -> EmbeddingError + '_ {
    move |e| EmbeddingError::EmbeddingError(format!("{msg}: {e}"))
}

/// A local GPU-free embedding provider using GGUF-quantized models via Candle.
pub struct GgufEmbeddingProvider {
    model: Arc<Mutex<EmbeddingModel>>,
    tokenizer: Mutex<Tokenizer>,
    dims: usize,
    model_name: String,
}

impl GgufEmbeddingProvider {
    /// Loads a GGUF embedding model from disk.
    ///
    /// * `model_name` — Human-readable model identifier
    /// * `gguf_path` — Path to the GGUF weights file
    /// * `tokenizer_path` — Path to the tokenizer JSON file
    /// * `max_length` — Maximum token sequence length
    /// * `quantization` — Quantization label (e.g. `"F16"`)
    pub fn load(
        model_name: &str,
        gguf_path: &str,
        tokenizer_path: &str,
        max_length: usize,
        quantization: &str,
    ) -> Result<Self, EmbeddingError> {
        let start = Instant::now();

        let device = Device::Cpu;

        let (model, _metadata) = load_model(gguf_path, &device)?;

        let mut tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| {
            EmbeddingError::EmbeddingError(format!("Failed to load tokenizer: {e}"))
        })?;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length,
                ..Default::default()
            }))
            .map_err(|e| {
                EmbeddingError::EmbeddingError(format!("Failed to set truncation: {e}"))
            })?;

        let dims = model.hidden_size;

        let elapsed = start.elapsed();
        tracing::info!(
            "[Embedding] GGUF({model_name}@{quantization}) loaded: {dims} dims, {max_length} max_len ({:.1}s)",
            elapsed.as_secs_f64(),
        );

        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            tokenizer: Mutex::new(tokenizer),
            dims,
            model_name: format!("{model_name}@{quantization}"),
        })
    }

    fn embed_internal(
        &self,
        text: &str,
        kind: ene_provider::EmbeddingKind,
    ) -> Result<Vec<f32>, EmbeddingError> {
        let prefix = match kind {
            ene_provider::EmbeddingKind::Query | ene_provider::EmbeddingKind::Hyde => "Query: ",
            _ => "Document: ",
        };
        let prefixed = format!("{}{}", prefix, text);
        let tokenizer = self.tokenizer.lock().unwrap_or_else(|e| e.into_inner());
        let encoding = tokenizer
            .encode(prefixed.as_str(), true)
            .map_err(|e| EmbeddingError::EmbeddingError(format!("Tokenization failed: {e}")))?;
        let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();

        if input_ids.is_empty() {
            return Ok(vec![0.0; self.dims]);
        }

        let input_tensor = Tensor::from_vec(
            input_ids.iter().map(|&x| x as u32).collect::<Vec<_>>(),
            (1, input_ids.len()),
            &Device::Cpu,
        )
        .map_err(|e| {
            EmbeddingError::EmbeddingError(format!("Failed to create input tensor: {e}"))
        })?;

        let model = self.model.lock().unwrap_or_else(|e| e.into_inner());
        model.forward(&input_tensor)
    }
}

#[async_trait]
impl ene_provider::EmbeddingProvider for GgufEmbeddingProvider {
    async fn embed(
        &self,
        text: &str,
        kind: ene_provider::EmbeddingKind,
    ) -> Result<Vec<f32>, ene_provider::EmbeddingError> {
        if text.trim().is_empty() {
            return Ok(vec![0.0; self.dims]);
        }
        let owned = text.to_string();
        let model = self.model_name.clone();

        tokio::task::block_in_place(|| {
            let start = Instant::now();
            let result = self
                .embed_internal(&owned, kind)
                .map_err(ene_provider::EmbeddingError::from);
            let elapsed = start.elapsed();
            if result.is_ok() {
                tracing::debug!(
                    "[Embedding] GGUF({}) {:?} {} chars → {:.2}ms",
                    model,
                    kind,
                    owned.len(),
                    elapsed.as_secs_f64() * 1000.0,
                );
            }
            result
        })
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, ene_provider::EmbeddingError> {
        if text.trim().is_empty() {
            return Ok(vec![0.0; self.dims]);
        }
        let owned = text.to_string();
        let model = self.model_name.clone();

        tokio::task::block_in_place(|| {
            let start = Instant::now();
            let result = self
                .embed_internal(&owned, ene_provider::EmbeddingKind::Query)
                .map_err(ene_provider::EmbeddingError::from);
            let elapsed = start.elapsed();
            if result.is_ok() {
                tracing::debug!(
                    "[Embedding] GGUF({}) query {} chars → {:.2}ms",
                    model,
                    owned.len(),
                    elapsed.as_secs_f64() * 1000.0,
                );
            }
            result
        })
    }

    async fn embed_batch(
        &self,
        items: &[(&str, ene_provider::EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, ene_provider::EmbeddingError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let owned: Vec<(String, ene_provider::EmbeddingKind)> =
            items.iter().map(|(t, k)| (t.to_string(), *k)).collect();

        tokio::task::block_in_place(|| {
            let start = Instant::now();
            let mut out = Vec::with_capacity(owned.len());
            for (text, kind) in &owned {
                if text.trim().is_empty() {
                    out.push(vec![0.0; self.dims]);
                } else {
                    out.push(
                        self.embed_internal(text, *kind)
                            .map_err(ene_provider::EmbeddingError::from)?,
                    );
                }
            }
            let elapsed = start.elapsed();
            tracing::debug!(
                "[Embedding] GGUF({}) batch {} items → {:.2}ms",
                self.model_name,
                owned.len(),
                elapsed.as_secs_f64() * 1000.0,
            );
            Ok(out)
        })
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}
