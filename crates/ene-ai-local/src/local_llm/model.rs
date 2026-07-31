//! Worker-owned llama.cpp chat/vision model — the [`ene_infer::LocalModel`]
//! (and [`ene_infer::StreamingLocalModel`]) `LocalLlamaCppProvider` wraps in
//! [`ene_ai::StreamingLocalLlmEngine`].

use ene_ai::error::LlmProviderError;
use ene_ai::{LlmChatRequest, LlmChatResponse, LlmResponseChunk};
use ene_infer::{ChunkSink, JobContext, LocalModel, StreamingLocalModel};
use llama_cpp_4::context::LlamaContext;
use llama_cpp_4::context::params::LlamaContextParams;

use crate::llama_cpp::{self, LlamaStreamChunk, LoadSpec, LoadedModel};

/// This crate's [`ene_infer::LocalModel::Request`] for [`LlamaChatModel`]:
/// either an ordinary chat turn (routed through
/// [`ene_ai::StreamingLocalLlmEngine`]'s blanket `LlmProvider` impl via
/// `From<LlmChatRequest>`) or a raw-RGB vision summarization, submitted
/// directly through [`ene_ai::StreamingLocalLlmEngine::handle`] — see
/// `LocalLlamaCppProvider::summarize_rgb_with_cancel` in `super`, which does
/// not fit `LlmChatRequest`'s message-based shape (it takes raw RGB8 pixels
/// plus width/height, not a base64 data-URI image part).
pub(crate) enum LlamaCppRequest {
    /// An ordinary chat completion.
    Chat(LlmChatRequest),
    /// A raw RGB8 screen-capture summarization.
    Vision {
        /// Rendered system prompt.
        system: String,
        /// Rendered user prompt.
        user: String,
        /// Image width in pixels.
        width: u32,
        /// Image height in pixels.
        height: u32,
        /// Interleaved RGB8 pixel data, `width * height * 3` bytes.
        rgb: Vec<u8>,
    },
}

impl From<LlmChatRequest> for LlamaCppRequest {
    fn from(req: LlmChatRequest) -> Self {
        Self::Chat(req)
    }
}

/// This crate's [`ene_infer::LocalModel::Response`] for [`LlamaChatModel`].
pub(crate) struct LlamaCppResponse {
    pub(crate) text: String,
    /// Tokens the prompt occupied in the context (#365).
    pub(crate) prompt_tokens: u32,
    /// Tokens sampled for the completion (#365).
    pub(crate) completion_tokens: u32,
}

impl From<LlamaCppResponse> for LlmChatResponse {
    fn from(r: LlamaCppResponse) -> Self {
        let total = r.prompt_tokens.saturating_add(r.completion_tokens);
        Self {
            text: r.text,
            usage: Some(ene_ai::TokenUsage::new(
                r.prompt_tokens,
                r.completion_tokens,
                total,
            )),
        }
    }
}

/// Worker-owned llama.cpp chat/vision model: the loaded GGUF (+ optional
/// mmproj) plus a [`LlamaContext`] cached across jobs.
///
/// # Context caching
///
/// Before this stage, `model.new_context(..)` was called once *per
/// request* (in what is now `crate::llama_cpp::generate`/`embed`). `ctx` is
/// now built lazily on the first job this model instance runs and reused
/// for every job after; [`LocalModel::reset`] below clears its KV cache
/// between jobs instead of the framework tearing down and rebuilding a
/// context every time.
///
/// # Safety invariant
///
/// `ctx`, once built, borrows `loaded.model` with an unsafely widened
/// `'static` lifetime (see [`LoadedModel::new_context_unbounded`]). That is
/// only sound as long as `loaded`'s heap address never moves while `ctx` is
/// alive, and `ctx` is always dropped before `loaded`. `loaded` is boxed so
/// its address survives this struct itself being moved (moving a `Box`
/// moves only the pointer, not the pointee), `ensure_context` never
/// replaces or moves out of `loaded`, and the explicit [`Drop`] impl below
/// clears `ctx` before the compiler-generated glue would otherwise drop
/// `loaded` — do not remove that impl even though it looks like it
/// duplicates what struct-field declaration order would already do; being
/// explicit here means the invariant does not silently break if a field is
/// ever reordered.
pub(crate) struct LlamaChatModel {
    ctx: Option<LlamaContext<'static>>,
    loaded: Box<LoadedModel>,
}

// SAFETY: `LlamaContext` holds a raw `NonNull` pointer, so the compiler
// infers `!Send` for anything containing one — correct in general, but overly
// conservative here. `ene_infer::LocalModel: Send` exists only so a model can
// be *moved once* from wherever it is built (this crate's calling thread via
// `EngineHandle::try_spawn`, or the worker thread itself on `spawn`/rebuild)
// onto the single dedicated worker thread that then owns it exclusively for
// the rest of its life; `run`/`reset`/`shutdown` are never called from more
// than one thread, and never concurrently with that one-time move. That is
// exactly the same reasoning `llama-cpp-4` itself uses for its own
// `unsafe impl Send for LlamaModel`/`MtmdContext` (see
// `llama_cpp_4::model`/`llama_cpp_4::mtmd`) — this extends it to the
// `LlamaContext` this struct additionally caches. Not `Sync`: nothing here
// claims safe *concurrent* access from multiple threads, only a safe
// one-time transfer.
unsafe impl Send for LlamaChatModel {}

impl Drop for LlamaChatModel {
    fn drop(&mut self) {
        // See the struct's safety invariant doc comment: `ctx` must go
        // before `loaded`.
        self.ctx = None;
    }
}

impl LlamaChatModel {
    pub(crate) fn new(spec: &LoadSpec) -> Result<Self, LlmProviderError> {
        let loaded = LoadedModel::load(spec)?;
        Ok(Self {
            ctx: None,
            loaded: Box::new(loaded),
        })
    }

    /// Whether an mmproj was loaded and reports vision support. Read once,
    /// right after this model is first built, to populate the
    /// [`ene_ai::EngineDescriptor`]'s [`ene_ai::CapabilitySet`] — see
    /// `LocalLlamaCppProvider::load` in `super`.
    #[must_use]
    pub(crate) fn supports_vision(&self) -> bool {
        self.loaded.supports_vision()
    }

    /// Builds `ctx` on first use; a no-op on every call after.
    fn ensure_context(&mut self) -> Result<(), LlmProviderError> {
        if self.ctx.is_some() {
            return Ok(());
        }
        let params = LlamaContextParams::default().with_n_ctx(Some(self.loaded.context_size));
        // SAFETY: `loaded` is never replaced or moved out of by this method,
        // and `Self::drop` always clears `ctx` before `loaded` can be
        // dropped — see the struct's safety invariant doc comment.
        let built = llama_cpp::with_backend(|backend| unsafe {
            self.loaded.new_context_unbounded(backend, params)
        })?;
        self.ctx = Some(built);
        Ok(())
    }
}

impl LocalModel for LlamaChatModel {
    type Request = LlamaCppRequest;
    type Response = LlamaCppResponse;
    type Error = LlmProviderError;

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
    )]
    fn engine_name(&self) -> &str {
        "llama-cpp-local"
    }

    fn run(&mut self, req: Self::Request, job: &JobContext) -> Result<Self::Response, Self::Error> {
        // Mirrors the upfront check every `LocalModel` in this workspace
        // does: a job that went stale while queued behind another one
        // should bail out instead of paying for inference nobody wants.
        if job.should_stop().is_some() {
            return Err(LlmProviderError::LocalLlm(
                "job stopped before llama.cpp inference started".to_string(),
            ));
        }
        job.tick();
        self.ensure_context()?;

        // Split-borrow `self`'s two fields disjointly: `ensure_context`
        // above needed `&mut self` as a whole, but generation only needs
        // `&loaded` alongside `&mut ctx`.
        let Self { ctx, loaded } = self;
        let ctx = ctx.as_mut().ok_or_else(|| {
            LlmProviderError::LocalLlm("llama context missing after lazy init".to_string())
        })?;
        let loaded_ref: &LoadedModel = loaded.as_ref();

        let generated = match req {
            LlamaCppRequest::Chat(chat) => llama_cpp::generate_chat(
                loaded_ref,
                ctx,
                &chat.messages,
                chat.json_schema.as_ref(),
                job,
                None,
            )?,
            LlamaCppRequest::Vision {
                system,
                user,
                width,
                height,
                rgb,
            } => llama_cpp::generate_vision(
                loaded_ref, ctx, &system, &user, width, height, &rgb, job, None,
            )?,
        };
        Ok(LlamaCppResponse {
            text: generated.text,
            prompt_tokens: generated.prompt_tokens,
            completion_tokens: generated.completion_tokens,
        })
    }

    fn reset(&mut self) {
        if let Some(ctx) = self.ctx.as_mut() {
            ctx.clear_kv_cache();
        }
    }
}

impl From<LlamaStreamChunk> for LlmResponseChunk {
    /// Maps a local llama.cpp stream chunk onto the generic
    /// [`LlmResponseChunk`] (#365): a [`LlamaStreamChunk::Delta`] becomes a
    /// text delta with no usage, and the single trailing
    /// [`LlamaStreamChunk::Usage`] becomes an empty-text chunk carrying the
    /// generation's exact token counts — the "usage on the final chunk"
    /// contract remote providers satisfy with their last SSE event.
    fn from(chunk: LlamaStreamChunk) -> Self {
        match chunk {
            LlamaStreamChunk::Delta(text) => Self {
                text_delta: Some(text),
                tool_calls_delta: None,
                usage: None,
            },
            LlamaStreamChunk::Usage(usage) => Self {
                text_delta: None,
                tool_calls_delta: None,
                usage: Some(usage),
            },
        }
    }
}

impl StreamingLocalModel for LlamaChatModel {
    /// One incremental unit of output: a detokenized text piece, plus a single
    /// trailing usage chunk — see [`LlamaStreamChunk`].
    type Chunk = LlamaStreamChunk;

    fn run_streaming(
        &mut self,
        req: Self::Request,
        job: &JobContext,
        sink: &ChunkSink<Self::Chunk, Self::Error>,
    ) -> Result<(), Self::Error> {
        if job.should_stop().is_some() {
            return Err(LlmProviderError::LocalLlm(
                "job stopped before llama.cpp inference started".to_string(),
            ));
        }
        job.tick();
        self.ensure_context()?;

        let Self { ctx, loaded } = self;
        let ctx = ctx.as_mut().ok_or_else(|| {
            LlmProviderError::LocalLlm("llama context missing after lazy init".to_string())
        })?;
        let loaded_ref: &LoadedModel = loaded.as_ref();

        match req {
            LlamaCppRequest::Chat(chat) => {
                llama_cpp::generate_chat(
                    loaded_ref,
                    ctx,
                    &chat.messages,
                    chat.json_schema.as_ref(),
                    job,
                    Some(sink),
                )?;
            }
            LlamaCppRequest::Vision {
                system,
                user,
                width,
                height,
                rgb,
            } => {
                llama_cpp::generate_vision(
                    loaded_ref,
                    ctx,
                    &system,
                    &user,
                    width,
                    height,
                    &rgb,
                    job,
                    Some(sink),
                )?;
            }
        }
        Ok(())
    }
}

/// Runs `ene_infer::conformance::run_all` against a test-only stand-in for
/// [`LlamaChatModel`] (not gated behind the `local` feature: it needs
/// neither `llama-cpp-4` nor a GGUF file, only the same
/// [`ene_infer::LocalModel`] shape).
///
/// [`ConformanceRequest`]/[`ConformanceResponse`] cannot be implemented on
/// [`LlamaCppRequest`]/[`LlamaCppResponse`] directly — "run for
/// approximately this long, or panic" has no meaningful encoding as a chat
/// message list or raw RGB pixels, and this crate does not own
/// `ene_ai::LlmChatRequest`/`LlmChatResponse` to add trait impls to them
/// even if it did. [`ScriptedLlamaModel`] validates the part that *is*
/// generic — that this engine's `EngineHandle`/`EngineConfig`/factory
/// wiring gets queueing, cancellation, panic recovery, and post-cancel
/// `reset` right — independent of real llama.cpp inference, exactly like
/// `ene-voice`'s whisper/Kokoro migrations did for their own engines.
#[cfg(test)]
mod conformance_tests {
    use std::time::{Duration, Instant};

    use ene_infer::conformance::{
        ConformanceRequest, ConformanceResponse, ConformanceStreamingRequest,
    };
    use ene_infer::{ChunkSink, JobContext, LocalModel, StreamingLocalModel};

    #[derive(Debug, Clone, Default)]
    struct ScriptedLlamaRequest {
        run_for: Duration,
        then_panic: bool,
        /// `run_streaming`-only scripting (see [`ConformanceStreamingRequest`]),
        /// inert for every [`ConformanceRequest::scripted`] request — those
        /// only ever run via [`LocalModel::run`], mirroring the equivalent
        /// fields on `ene_infer::conformance`'s own `MockRequest`.
        stream_chunks: usize,
        stream_pause: Duration,
        stream_fail_after: Option<usize>,
    }

    impl ConformanceRequest for ScriptedLlamaRequest {
        fn scripted(run_for: Duration, then_panic: bool) -> Self {
            Self {
                run_for,
                then_panic,
                stream_chunks: 0,
                stream_pause: Duration::ZERO,
                stream_fail_after: None,
            }
        }
    }

    impl ConformanceStreamingRequest for ScriptedLlamaRequest {
        fn scripted_stream(
            chunk_count: usize,
            pause_before_each: Duration,
            fail_after: Option<usize>,
        ) -> Self {
            Self {
                run_for: Duration::ZERO,
                then_panic: false,
                stream_chunks: chunk_count,
                stream_pause: pause_before_each,
                stream_fail_after: fail_after,
            }
        }
    }

    #[derive(Debug)]
    struct ScriptedLlamaResponse {
        resets_seen: usize,
    }

    impl ConformanceResponse for ScriptedLlamaResponse {
        fn resets_seen(&self) -> usize {
            self.resets_seen
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("scripted llama.cpp stand-in stopped cooperatively")]
    struct ScriptedLlamaError;

    #[derive(Debug, Default)]
    struct ScriptedLlamaModel {
        resets_seen: usize,
    }

    impl LocalModel for ScriptedLlamaModel {
        type Request = ScriptedLlamaRequest;
        type Response = ScriptedLlamaResponse;
        type Error = ScriptedLlamaError;

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
        )]
        fn engine_name(&self) -> &str {
            "scripted-llama-cpp"
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
                    return Err(ScriptedLlamaError);
                }
                ctx.tick();
                if start.elapsed() >= req.run_for {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(ScriptedLlamaResponse {
                resets_seen: self.resets_seen,
            })
        }

        fn reset(&mut self) {
            self.resets_seen += 1;
        }
    }

    impl StreamingLocalModel for ScriptedLlamaModel {
        /// A stand-in for the detokenized `String` pieces the real
        /// [`super::LlamaChatModel`] streams — the chunk's actual type
        /// doesn't matter for validating queueing/backpressure/cancellation,
        /// only that it travels through the sink in order.
        type Chunk = usize;

        fn run_streaming(
            &mut self,
            req: Self::Request,
            ctx: &JobContext,
            sink: &ChunkSink<Self::Chunk, Self::Error>,
        ) -> Result<(), Self::Error> {
            for i in 0..req.stream_chunks {
                if ctx.should_stop().is_some() {
                    return Err(ScriptedLlamaError);
                }
                ctx.tick();
                if !req.stream_pause.is_zero() {
                    std::thread::sleep(req.stream_pause);
                }
                if sink.send(i, ctx).is_err() {
                    return Err(ScriptedLlamaError);
                }
                if req.stream_fail_after == Some(i + 1) {
                    return Err(ScriptedLlamaError);
                }
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn llama_cpp_engine_wiring_passes_conformance_battery() {
        ene_infer::conformance::run_all(ScriptedLlamaModel::default).await;
    }

    #[tokio::test]
    async fn llama_cpp_engine_streaming_wiring_passes_conformance_battery() {
        ene_infer::conformance::run_all_streaming(ScriptedLlamaModel::default).await;
    }
}
