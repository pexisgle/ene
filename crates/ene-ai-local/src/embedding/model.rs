//! Worker-owned llama.cpp embedding model — the [`ene_infer::LocalModel`]
//! [`super::GgufEmbeddingProvider`] wraps.

use ene_ai::EmbeddingKind;
use ene_ai::error::LlmProviderError;
use ene_infer::{JobContext, LocalModel};
use llama_cpp_4::context::LlamaContext;
use llama_cpp_4::context::params::{LlamaContextParams, LlamaPoolingType};

use crate::llama_cpp::{self, LoadSpec, LoadedModel};

/// One embedding request: text plus the prefix hint
/// ([`super::GgufEmbeddingProvider`] applies `"Query: "`/`"Document: "`
/// before submitting).
pub(crate) struct EmbedJob {
    pub(crate) text: String,
    pub(crate) kind: EmbeddingKind,
}

pub(crate) struct EmbedResult {
    pub(crate) vector: Vec<f32>,
}

/// Worker-owned llama.cpp embedding model: the loaded GGUF plus a
/// [`LlamaContext`] (configured for embeddings output, last-token pooling)
/// cached across jobs instead of rebuilt per request.
///
/// Shares `LlamaChatModel`'s (`crate::local_llm::model`) safety invariant
/// for `ctx`'s unsafely-widened `'static` lifetime: `loaded` is boxed so its
/// address survives this struct being moved, and [`Drop`] clears `ctx`
/// before the compiler-generated glue would otherwise drop `loaded`.
pub(crate) struct LlamaEmbedModel {
    ctx: Option<LlamaContext<'static>>,
    loaded: Box<LoadedModel>,
    dims: usize,
}

// SAFETY: see `crate::local_llm::model::LlamaChatModel`'s identical
// `unsafe impl Send` for the full justification — `ene_infer::LocalModel`
// only ever moves an instance once (onto its dedicated worker thread), never
// accesses it from two threads concurrently, and `llama-cpp-4` itself blesses
// the analogous `LlamaModel`/`MtmdContext` the same way.
unsafe impl Send for LlamaEmbedModel {}

impl Drop for LlamaEmbedModel {
    fn drop(&mut self) {
        self.ctx = None;
    }
}

impl LlamaEmbedModel {
    pub(crate) fn new(spec: &LoadSpec) -> Result<Self, LlmProviderError> {
        let loaded = LoadedModel::load(spec)?;
        let dims = usize::try_from(loaded.model.n_embd())
            .map_err(|_| LlmProviderError::LocalLlm("n_embd does not fit in usize".to_string()))?;
        Ok(Self {
            ctx: None,
            loaded: Box::new(loaded),
            dims,
        })
    }

    #[must_use]
    pub(crate) fn dimensions(&self) -> usize {
        self.dims
    }

    fn ensure_context(&mut self) -> Result<(), LlmProviderError> {
        if self.ctx.is_some() {
            return Ok(());
        }
        let params = LlamaContextParams::default()
            .with_n_ctx(Some(self.loaded.context_size))
            .with_embeddings(true)
            .with_pooling_type(LlamaPoolingType::Last);
        // SAFETY: see `LlamaChatModel::ensure_context` — same invariant,
        // same reasoning: `loaded` is never replaced/moved by this method,
        // and `Self::drop` always clears `ctx` first.
        let built = llama_cpp::with_backend(|backend| unsafe {
            self.loaded.new_context_unbounded(backend, params)
        })?;
        self.ctx = Some(built);
        Ok(())
    }
}

impl LocalModel for LlamaEmbedModel {
    type Request = EmbedJob;
    type Response = EmbedResult;
    type Error = LlmProviderError;

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
    )]
    fn engine_name(&self) -> &str {
        "gguf-embedding"
    }

    fn run(&mut self, req: Self::Request, job: &JobContext) -> Result<Self::Response, Self::Error> {
        if job.should_stop().is_some() {
            return Err(LlmProviderError::LocalLlm(
                "job stopped before embedding inference started".to_string(),
            ));
        }
        job.tick();
        self.ensure_context()?;

        let Self { ctx, loaded, .. } = self;
        let ctx = ctx.as_mut().ok_or_else(|| {
            LlmProviderError::LocalLlm("llama context missing after lazy init".to_string())
        })?;

        let prefix = match req.kind {
            EmbeddingKind::Query | EmbeddingKind::Hyde => "Query: ",
            EmbeddingKind::Summary
            | EmbeddingKind::Description
            | EmbeddingKind::Capability
            | EmbeddingKind::Example
            | EmbeddingKind::Negative => "Document: ",
        };
        let prefixed = format!("{prefix}{}", req.text);

        let vector = llama_cpp::embed_text(loaded.as_ref(), ctx, &prefixed)?;
        job.tick();
        Ok(EmbedResult { vector })
    }

    fn reset(&mut self) {
        if let Some(ctx) = self.ctx.as_mut() {
            ctx.clear_kv_cache();
        }
    }
}

/// Runs `ene_infer::conformance::run_all` against a test-only stand-in for
/// [`LlamaEmbedModel`] — see `crate::local_llm::model`'s equivalent module
/// doc comment for why a scripted stand-in is used instead of the real
/// request/response types.
#[cfg(test)]
mod conformance_tests {
    use std::time::{Duration, Instant};

    use ene_infer::conformance::{ConformanceRequest, ConformanceResponse};
    use ene_infer::{JobContext, LocalModel};

    #[derive(Debug, Clone, Default)]
    struct ScriptedEmbedRequest {
        run_for: Duration,
        then_panic: bool,
    }

    impl ConformanceRequest for ScriptedEmbedRequest {
        fn scripted(run_for: Duration, then_panic: bool) -> Self {
            Self {
                run_for,
                then_panic,
            }
        }
    }

    #[derive(Debug)]
    struct ScriptedEmbedResponse {
        resets_seen: usize,
    }

    impl ConformanceResponse for ScriptedEmbedResponse {
        fn resets_seen(&self) -> usize {
            self.resets_seen
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("scripted embedding stand-in stopped cooperatively")]
    struct ScriptedEmbedError;

    #[derive(Debug, Default)]
    struct ScriptedEmbedModel {
        resets_seen: usize,
    }

    impl LocalModel for ScriptedEmbedModel {
        type Request = ScriptedEmbedRequest;
        type Response = ScriptedEmbedResponse;
        type Error = ScriptedEmbedError;

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
        )]
        fn engine_name(&self) -> &str {
            "scripted-gguf-embedding"
        }

        fn run(
            &mut self,
            req: Self::Request,
            ctx: &JobContext,
        ) -> Result<Self::Response, Self::Error> {
            assert!(!req.then_panic, "scripted panic for conformance testing");
            let start = Instant::now();
            loop {
                if ctx.should_stop().is_some() {
                    return Err(ScriptedEmbedError);
                }
                ctx.tick();
                if start.elapsed() >= req.run_for {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(ScriptedEmbedResponse {
                resets_seen: self.resets_seen,
            })
        }

        fn reset(&mut self) {
            self.resets_seen += 1;
        }
    }

    #[tokio::test]
    async fn gguf_embedding_engine_wiring_passes_conformance_battery() {
        ene_infer::conformance::run_all(ScriptedEmbedModel::default).await;
    }
}
