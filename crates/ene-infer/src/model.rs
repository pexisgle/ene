//! The synchronous, exclusively-owned trait local inference providers implement.

use crate::context::JobContext;

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
