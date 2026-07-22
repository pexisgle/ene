//! `GgufEmbeddingProvider` backed by llama-cpp-2.

use super::EneEmbeddingError;
use crate::config::ProactiveAcceleration;
use crate::llama_cpp::{LoadSpec, LoadedModel, embed_text};
use crate::{EmbeddingKind, EmbeddingProvider};
use async_trait::async_trait;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Instant;

/// Local embedding provider using GGUF weights via llama.cpp.
pub struct GgufEmbeddingProvider {
    model: Arc<Mutex<LoadedModel>>,
    dims: usize,
    model_name: String,
}

impl GgufEmbeddingProvider {
    /// Loads a GGUF embedding model from disk (CPU by default).
    ///
    /// * `model_name` — Human-readable model identifier
    /// * `gguf_path` — Path to the GGUF weights file
    /// * `quantization` — Quantization label (e.g. `"F16"`)
    pub fn load(
        model_name: &str,
        gguf_path: &str,
        quantization: &str,
    ) -> Result<Self, EneEmbeddingError> {
        Self::load_with_acceleration(
            model_name,
            gguf_path,
            quantization,
            ProactiveAcceleration::Cpu,
        )
    }

    /// Load with an explicit acceleration preference.
    pub fn load_with_acceleration(
        model_name: &str,
        gguf_path: &str,
        quantization: &str,
        acceleration: ProactiveAcceleration,
    ) -> Result<Self, EneEmbeddingError> {
        let start = Instant::now();
        let path = LoadSpec::validate_model_path(gguf_path)?;
        let spec = LoadSpec {
            model_path: path,
            mmproj_path: None,
            acceleration,
            gpu_layers: "auto".to_string(),
            context_size: 8192,
        };
        let loaded = LoadedModel::load(&spec)?;
        let dims = usize::try_from(loaded.model.n_embd())
            .map_err(|_| EneEmbeddingError::LocalLlm("n_embd does not fit in usize".to_string()))?;
        let elapsed = start.elapsed();
        tracing::info!(
            "[Embedding] GGUF({model_name}@{quantization}) loaded via llama.cpp: {dims} dims ({:.1}s)",
            elapsed.as_secs_f64(),
        );
        Ok(Self {
            model: Arc::new(Mutex::new(loaded)),
            dims,
            model_name: format!("{model_name}@{quantization}"),
        })
    }

    fn embed_internal(
        &self,
        text: &str,
        kind: EmbeddingKind,
    ) -> Result<Vec<f32>, EneEmbeddingError> {
        let prefix = match kind {
            EmbeddingKind::Query | EmbeddingKind::Hyde => "Query: ",
            _ => "Document: ",
        };
        let prefixed = format!("{prefix}{text}");
        let guard = self.model.lock();
        let emb = embed_text(&guard, &prefixed)?;
        if emb.is_empty() {
            return Err(EneEmbeddingError::Provider(
                crate::EmbeddingError::EmptyInput,
            ));
        }
        Ok(emb)
    }
}

#[async_trait]
impl EmbeddingProvider for GgufEmbeddingProvider {
    async fn embed_batch(
        &self,
        items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, crate::EmbeddingError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let start = Instant::now();
        let result = tokio::task::block_in_place(|| {
            let mut out = Vec::with_capacity(items.len());
            for &(text, kind) in items {
                if text.trim().is_empty() {
                    return Err(crate::EmbeddingError::EmptyInput);
                }
                let emb = self
                    .embed_internal(text, kind)
                    .map_err(crate::EmbeddingError::from)?;
                if emb.len() != self.dims {
                    return Err(crate::EmbeddingError::DimensionMismatch(format!(
                        "expected {} dims, got {}",
                        self.dims,
                        emb.len()
                    )));
                }
                out.push(emb);
            }
            Ok(out)
        });
        let elapsed = start.elapsed();
        tracing::debug!(
            "[Embedding] GGUF({}) batch {} items → {:.2}ms",
            self.model_name,
            items.len(),
            elapsed.as_secs_f64() * 1000.0,
        );
        result
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}
