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

    /// If set, the engine monitors [`crate::JobContext::tick`] progress
    /// while a job is running, with graduated escalation:
    ///
    /// 1. **Suspected stall** — `tick` has not been called for at least
    ///    this long while a job is active. Logged as a `tracing::warn!`.
    ///    The engine keeps running; if the job completes (the worker
    ///    becomes idle), the suspicion is cleared — it was a false
    ///    positive (e.g. a legitimately long native operation that simply
    ///    does not tick for a while).
    /// 2. **Confirmed stall** — the silence persists for at least
    ///    `stall_timeout × stall_escalation_factor` (see
    ///    [`Self::stall_escalation_factor`]). Only now is the engine
    ///    marked permanently down: every *later*
    ///    [`crate::EngineHandle::submit`] call on this handle immediately
    ///    returns [`crate::EngineError::EngineDown`] instead of
    ///    [`crate::EngineError::Busy`] — a permanently wedged worker would
    ///    otherwise fill the bounded queue and report `Busy` forever,
    ///    leaving callers unable to tell "briefly saturated" apart from
    ///    "this handle will never make progress again".
    ///
    /// The currently-running job's own result is unaffected in either
    /// case: the framework has no way to preempt a synchronous `run` call
    /// that never checks back in.
    pub stall_timeout: Option<Duration>,

    /// How many multiples of [`Self::stall_timeout`] a suspected stall
    /// must persist before it is confirmed as a permanent hang and the
    /// engine is marked down. Defaults to 3: a job that stops ticking for
    /// one `stall_timeout` is logged as suspected; only if the silence
    /// reaches three `stall_timeout`s is the engine permanently disabled.
    /// Set to 1 to make the first detection immediately final. Coerced up
    /// to at least 1. Only meaningful
    /// when [`Self::stall_timeout`] is `Some`.
    pub stall_escalation_factor: u32,
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
            stall_escalation_factor: Self::DEFAULT_STALL_ESCALATION_FACTOR,
        }
    }

    /// The default [`Self::stall_escalation_factor`]: a suspected stall
    /// must persist for three multiples of [`Self::stall_timeout`] before
    /// the engine is permanently disabled.
    const DEFAULT_STALL_ESCALATION_FACTOR: u32 = 3;

    /// Sets [`Self::stall_timeout`].
    #[must_use]
    pub fn with_stall_timeout(mut self, stall_timeout: Duration) -> Self {
        self.stall_timeout = Some(stall_timeout);
        self
    }

    /// Sets [`Self::stall_escalation_factor`]. Coerced up to at least 1.
    #[must_use]
    pub fn with_stall_escalation_factor(mut self, factor: u32) -> Self {
        self.stall_escalation_factor = factor.max(1);
        self
    }
}

impl Default for EngineConfig {
    /// 8 queued jobs, a 30 second job timeout, and no stall detection.
    fn default() -> Self {
        Self::new(8, Duration::from_secs(30))
    }
}
