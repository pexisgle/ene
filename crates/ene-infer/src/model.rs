//! The synchronous, exclusively-owned trait local inference providers implement.

use crate::context::JobContext;
use crate::stream::ChunkSink;

/// A local inference model, owned by exactly one worker thread for its
/// entire lifetime.
///
/// `run` takes `&mut self`, which is the load-bearing part of this trait:
/// implementors cannot reach for `Arc<Mutex<_>>` to share the model across
/// threads, because there is only ever one caller (the worker loop in this
/// crate) and it never calls `run` concurrently with itself. All
/// concurrency — queuing, timeouts, cancellation, panic recovery — is
/// handled by [`crate::EngineHandle`]; implementations of this trait should
/// contain nothing but the inference logic itself.
pub trait LocalModel: Send + 'static {
    /// The request type this model consumes. Typically a small owned value
    /// (prompt, audio buffer, ...) rather than a borrowed one, since it
    /// travels across a channel to the worker thread.
    type Request: Send + 'static;

    /// The response type this model produces on success.
    type Response: Send + 'static;

    /// The error type this model produces on failure. Must not be used to
    /// signal timeout, cancellation, or caller-gone conditions — those are
    /// reported by the framework via [`crate::EngineError`] regardless of
    /// what `run` returns once [`JobContext::should_stop`] has fired.
    type Error: std::error::Error + Send + 'static;

    /// A short, stable name for this engine (e.g. `"llama-cpp"`,
    /// `"whisper"`), used in tracing spans and [`crate::EngineError::EngineDown`]
    /// messages.
    fn engine_name(&self) -> &str;

    /// Run one job to completion, synchronously, on the worker thread.
    ///
    /// Implementations **must** consult [`JobContext::should_stop`] at every
    /// natural interruption point (token boundary, chunk boundary, layer
    /// boundary, ...) and return as soon as possible once it returns
    /// `Some(_)`. The framework does not — and structurally cannot —
    /// preempt this call; cancellation, deadlines, and caller-gone
    /// detection are all cooperative. Call [`JobContext::tick`] at the same
    /// points to keep stall detection informed of progress.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` for any model-level failure. Do not attempt to
    /// encode timeout/cancellation/caller-gone as a variant of this type —
    /// return whatever is convenient (including a generic "interrupted"
    /// error) once `should_stop` fires; the framework overrides the outcome
    /// with the precise [`crate::EngineError`] reason in that case.
    fn run(&mut self, req: Self::Request, ctx: &JobContext) -> Result<Self::Response, Self::Error>;

    /// Reset scratch state (KV cache, RNN hidden state, decoder buffers, ...)
    /// so the next job starts from a clean slate.
    ///
    /// The framework calls this after **every** job that returns from `run`
    /// without panicking — success, model error, timeout, cancellation, or
    /// caller-gone all count. A job that makes `run` panic instead triggers
    /// full model reconstruction via the factory passed to
    /// [`crate::EngineHandle::spawn`], which supersedes `reset` for that
    /// exit path (the model instance is discarded, not reset).
    ///
    /// The default implementation does nothing, for models with no
    /// persistent scratch state.
    fn reset(&mut self) {}

    /// Release any resources ahead of the model being dropped (closing file
    /// handles, freeing native buffers with unusual lifetimes, ...).
    ///
    /// Called once when the worker thread is shutting down. The default
    /// implementation does nothing; `Drop` impls on the model itself remain
    /// the primary mechanism and this is only for cases that need an
    /// explicit, orderly shutdown step before the drop glue runs.
    fn shutdown(&mut self) {}
}

/// Opt-in extension of [`LocalModel`] for models that can hand back partial
/// results while a job is still executing, instead of only a single
/// completed [`LocalModel::Response`] when `run` returns.
///
/// Additive by design: implementing this trait changes nothing about an
/// existing [`LocalModel`] impl, and a model with no incremental output
/// (whisper, Kokoro, GGUF embedding, ...) simply never implements it — every
/// existing `LocalModel` keeps compiling and behaving unchanged.
/// [`crate::EngineHandle::submit_stream`] is only available for `M:
/// StreamingLocalModel`, via a second, separate `impl` block (see that
/// method's docs); [`crate::EngineHandle::submit`] keeps working for every
/// `LocalModel`, streaming-capable or not.
pub trait StreamingLocalModel: LocalModel {
    /// One incremental unit of output (a detokenized text piece, a PCM
    /// slice, ...). Typically small and owned, like [`LocalModel::Request`]
    /// — it travels across a channel to the async side.
    type Chunk: Send + 'static;

    /// Run one job to completion, synchronously, pushing zero or more
    /// [`Self::Chunk`]s through `sink` as they become available.
    ///
    /// Everything [`LocalModel::run`]'s docs say about cooperative
    /// cancellation applies here identically: check
    /// [`JobContext::should_stop`] at every natural interruption point and
    /// return as soon as possible once it fires, and call
    /// [`JobContext::tick`] at the same points. [`ChunkSink::send`] itself
    /// already consults `should_stop` (a full sink blocks the caller here
    /// until the consumer drains it, the deadline passes, or the consumer
    /// goes away — whichever comes first) and returns
    /// [`crate::StopReason`] instead of pushing forever, but that check does
    /// not substitute for this method's own `should_stop` calls at points
    /// that do not send a chunk (e.g. while preparing input before the
    /// first one).
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` for a model-level failure, exactly like
    /// [`LocalModel::run`] — including after some chunks were already sent
    /// successfully. The framework surfaces that as the final item of the
    /// stream returned by [`crate::EngineHandle::submit_stream`] (mapped to
    /// the precise [`crate::EngineError`] reason if a stop condition fired),
    /// it does not silently truncate. As with `run`, do not attempt to
    /// encode timeout/cancellation/caller-gone as a variant of this type —
    /// return whatever is convenient once `should_stop` fires or
    /// `ChunkSink::send` reports a stop.
    fn run_streaming(
        &mut self,
        req: Self::Request,
        ctx: &JobContext,
        sink: &ChunkSink<Self::Chunk, Self::Error>,
    ) -> Result<(), Self::Error>;
}
