//! `GgufEmbeddingProvider` backed by llama-cpp-4, via `ene_infer::EngineHandle`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ene_ai::config::ProactiveAcceleration;
use ene_ai::{EmbeddingError, EmbeddingKind, EmbeddingProvider, ResourceClass, ResourceRegistry};
use ene_infer::{EngineConfig, EngineError, EngineHandle};
use tokio_util::sync::CancellationToken;

use super::EneEmbeddingError;
use super::model::{EmbedJob, LlamaEmbedModel};
use crate::llama_cpp::{LoadSpec, resource_class_for};

/// Number of embedding jobs allowed to queue behind the one currently
/// executing. `ene-ai`'s `ConcurrencyHint::default` sizing (1 in flight, 2
/// queued) is the conservative default for a local model whose author
/// hasn't tuned concurrency; embedding batches (Tool RAG indexing, memory
/// search) can submit many items back to back, so a slightly deeper queue
/// avoids `Busy` under ordinary sequential use without changing the
/// underlying single-worker serialization.
const EMBED_QUEUE_DEPTH: usize = 4;

/// Generous upper bound on a single embedding forward pass — comfortably
/// longer than any realistic short-text embedding on CPU or GPU.
const EMBED_JOB_TIMEOUT: Duration = Duration::from_secs(30);

/// No stall timeout: `ctx.decode()` is a single opaque FFI call with no
/// natural interruption point (same reasoning as the chat/vision engine in
/// `crate::local_llm::model` and `ene-voice`'s whisper/Kokoro engines) — any
/// threshold shorter than the slowest realistic decode would misidentify a
/// merely slow (but healthy) embedding as a wedged worker.
const EMBED_STALL_TIMEOUT: Option<Duration> = None;

/// Local embedding provider using GGUF weights via llama.cpp.
pub struct GgufEmbeddingProvider {
    handle: EngineHandle<LlamaEmbedModel>,
    resource: ResourceClass,
    dims: usize,
    model_name: String,
}

fn map_engine_error(err: EngineError<ene_ai::error::LlmProviderError>) -> EneEmbeddingError {
    match err {
        EngineError::Busy { queue_depth } => EneEmbeddingError::Provider(EmbeddingError::Provider(
            format!("engine busy: queue depth {queue_depth} exceeded"),
        )),
        EngineError::Timeout { .. } => EneEmbeddingError::Provider(EmbeddingError::Provider(
            "local engine job timed out".to_string(),
        )),
        EngineError::Cancelled => EneEmbeddingError::Provider(EmbeddingError::Provider(
            "local engine job cancelled".to_string(),
        )),
        EngineError::EngineDown { reason } => {
            EneEmbeddingError::LocalLlm(format!("engine down: {reason}"))
        }
        EngineError::Model(model_err) => EneEmbeddingError::from(model_err),
    }
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
    ///
    /// # Errors
    ///
    /// Returns [`EneEmbeddingError`] when the GGUF path is invalid or the
    /// first model build fails ([`EngineHandle::try_spawn`] builds it
    /// synchronously here, so this failure is not deferred to the first
    /// [`EmbeddingProvider::embed_batch`] call).
    pub fn load_with_acceleration(
        model_name: &str,
        gguf_path: &str,
        quantization: &str,
        acceleration: ProactiveAcceleration,
    ) -> Result<Self, EneEmbeddingError> {
        let path = LoadSpec::validate_model_path(gguf_path)?;
        let spec = LoadSpec {
            model_path: path,
            mmproj_path: None,
            acceleration,
            gpu_layers: "auto".to_string(),
            context_size: 8192,
        };
        let resource = resource_class_for(&spec)?;

        // `try_spawn` builds the first `LlamaEmbedModel` synchronously, on
        // this thread, before returning — but it does not hand that
        // instance back to us, only the `EngineHandle`. `dims_detected` is a
        // side channel the factory populates as it builds that first model,
        // read right after `try_spawn` returns; this avoids loading the
        // GGUF a second time just to ask it for `n_embd()` (the same
        // technique `LocalLlamaCppProvider::load` uses for its
        // `vision_detected` cell).
        let dims_detected = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let factory = {
            let spec = spec.clone();
            let dims_detected = Arc::clone(&dims_detected);
            move || {
                let model = LlamaEmbedModel::new(&spec)?;
                dims_detected.store(model.dimensions(), std::sync::atomic::Ordering::Relaxed);
                Ok(model)
            }
        };
        let mut cfg = EngineConfig::new(EMBED_QUEUE_DEPTH, EMBED_JOB_TIMEOUT);
        if let Some(stall) = EMBED_STALL_TIMEOUT {
            cfg = cfg.with_stall_timeout(stall);
        }
        let handle = EngineHandle::try_spawn(factory, cfg)?;
        let dims = dims_detected.load(std::sync::atomic::Ordering::Relaxed);
        if dims == 0 {
            return Err(EneEmbeddingError::LocalLlm(
                "embedding dimensions unknown after model build".to_string(),
            ));
        }

        tracing::info!(
            "[Embedding] GGUF({model_name}@{quantization}) loaded via llama.cpp: {dims} dims",
        );

        Ok(Self {
            handle,
            resource,
            dims,
            model_name: format!("{model_name}@{quantization}"),
        })
    }
}

#[async_trait]
impl EmbeddingProvider for GgufEmbeddingProvider {
    async fn embed_batch(
        &self,
        items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let mut out = Vec::with_capacity(items.len());
        for &(text, kind) in items {
            if text.trim().is_empty() {
                return Err(EmbeddingError::EmptyInput);
            }

            let permit = ResourceRegistry::acquire(self.resource)
                .await
                .map_err(|e| {
                    EmbeddingError::Provider(format!("resource admission semaphore closed: {e}"))
                })?;
            let request = EmbedJob {
                text: text.to_string(),
                kind,
            };
            let outcome = self.handle.submit(request, CancellationToken::new()).await;
            drop(permit);

            let response = outcome
                .map_err(map_engine_error)
                .map_err(EmbeddingError::from)?;
            if response.vector.len() != self.dims {
                return Err(EmbeddingError::DimensionMismatch(format!(
                    "expected {} dims, got {}",
                    self.dims,
                    response.vector.len()
                )));
            }
            out.push(response.vector);
        }
        Ok(out)
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}
