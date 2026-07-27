//! Worker configuration.

use std::time::Duration;

/// Configuration for one [`crate::EngineHandle`].
///
/// There is exactly one timeout in this crate: [`Self::job_timeout`],
/// enforced cooperatively from inside the worker once a job starts
/// executing. There is deliberately no separate outer timeout wrapping
/// [`crate::EngineHandle::submit`] — see the crate-level docs for why an
/// outer/inner double timeout is the bug this crate exists to remove.
#[derive(Debug, Clone, Copy)]
pub struct EngineConfig {
    /// Maximum number of jobs allowed to wait in the queue at once. A full
    /// queue makes [`crate::EngineHandle::submit`] return
    /// [`crate::EngineError::Busy`] immediately — the queue never blocks
    /// and never grows past this bound. Coerced up to at least 1.
    pub queue_depth: usize,

    /// How long a job may run once the worker starts executing it, checked
    /// cooperatively via [`crate::JobContext::should_stop`]. Time spent
    /// waiting in the queue does not count against this budget.
    pub job_timeout: Duration,

    /// If set, the engine logs a `tracing::warn!` when
    /// [`crate::JobContext::tick`] has not been called for this long while a
    /// job is running. Purely diagnostic: it never changes what
    /// [`crate::EngineHandle::submit`] returns, it only surfaces a hung
    /// worker in logs (the framework has no way to preempt a synchronous
    /// `run` call that never checks back in).
    pub stall_timeout: Option<Duration>,
}

impl EngineConfig {
    /// Builds a config with the given queue depth and job timeout, and no
    /// stall detection.
    #[must_use]
    pub fn new(queue_depth: usize, job_timeout: Duration) -> Self {
        Self {
            queue_depth: queue_depth.max(1),
            job_timeout,
            stall_timeout: None,
        }
    }

    /// Sets [`Self::stall_timeout`].
    #[must_use]
    pub fn with_stall_timeout(mut self, stall_timeout: Duration) -> Self {
        self.stall_timeout = Some(stall_timeout);
        self
    }
}

impl Default for EngineConfig {
    /// 8 queued jobs, a 30 second job timeout, and no stall detection.
    fn default() -> Self {
        Self::new(8, Duration::from_secs(30))
    }
}
