use std::time::Duration;

/// Everything [`crate::EngineHandle::submit`] can fail with.
///
/// Kept as a distinct enum (never collapsed to `String`) specifically so
/// callers can tell "retry is pointless" apart from "this was transient" —
/// see [`Self::is_retryable`]. `E` is the model's own
/// [`crate::LocalModel::Error`].
#[derive(Debug, thiserror::Error)]
pub enum EngineError<E: std::error::Error> {
    /// The bounded queue was full; the job was never accepted. Enqueue
    /// contention only — the engine itself is fine.
    #[error("engine busy: queue depth {queue_depth} exceeded")]
    Busy { queue_depth: usize },

    /// [`crate::EngineConfig::job_timeout`] elapsed while the job was
    /// executing and the model honored [`crate::JobContext::should_stop`].
    #[error("job timed out after {after:?}")]
    Timeout { after: Duration },

    /// The [`tokio_util::sync::CancellationToken`] passed to `submit` was
    /// cancelled and the model honored it.
    #[error("job cancelled")]
    Cancelled,

    /// The engine's worker thread is not available: it failed to start,
    /// exited, or the model could not be rebuilt after a panic.
    #[error("engine down: {reason}")]
    EngineDown {
        /// Human-readable diagnostic, not intended to be pattern-matched.
        reason: String,
    },

    /// The model itself reported a failure. Not overridden by the
    /// framework — this variant only appears when `run` returned `Err` and
    /// no stop condition had fired for that job.
    #[error("model error: {0}")]
    Model(#[source] E),
}

impl<E: std::error::Error> EngineError<E> {
    /// Whether resubmitting the same request is likely to help.
    ///
    /// [`Self::Busy`] and [`Self::Timeout`] are transient — a full queue
    /// drains, and a slow-but-alive engine may succeed given a fresh
    /// deadline — so both are retryable. [`Self::Cancelled`] reflects
    /// caller intent, not engine health, so retrying is the caller's call,
    /// not something this method encourages. [`Self::EngineDown`] and
    /// [`Self::Model`] are conservatively reported as not retryable: engine
    /// death may or may not have self-healed by the time of a retry, and a
    /// model error's retryability is domain-specific and not something this
    /// crate can judge — callers with more context should inspect the
    /// wrapped error themselves.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Busy { .. } | Self::Timeout { .. })
    }
}
