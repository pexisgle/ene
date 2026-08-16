use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

/// Why a running job should stop.
///
/// Returned by [`JobContext::should_stop`]. All three conditions are
/// cooperative: the framework never preempts a running
/// [`crate::LocalModel::run`] call, it only tells the implementation that it
/// should return as soon as possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// [`crate::EngineConfig::job_timeout`] has elapsed since the job began
    /// executing.
    Deadline,
    /// The [`tokio_util::sync::CancellationToken`] passed to
    /// [`crate::EngineHandle::submit`] was cancelled.
    Cancelled,
    /// The future awaiting [`crate::EngineHandle::submit`] was dropped —
    /// nothing is listening for the result anymore.
    CallerGone,
}

/// Handed to [`crate::LocalModel::run`] for the duration of one job.
///
/// `JobContext` deliberately owns everything it needs (no borrowed data, no
/// lifetime parameter) so the trait signature can take it as a plain
/// `&JobContext`. Anything it needs from the async side (whether the caller
/// is still waiting, a shared heartbeat clock for stall detection) is
/// captured by value — a `CancellationToken` or an `Arc` clone — rather
/// than borrowed.
pub struct JobContext {
    started_at: Instant,
    job_timeout: Duration,
    cancel: CancellationToken,
    /// Cancelled by a [`tokio_util::sync::DropGuard`] living in
    /// [`crate::EngineHandle::submit`]'s own stack frame: when that future
    /// is dropped (the caller went away), the guard cancels this token.
    /// Deliberately a second, independent token from `cancel` — the caller
    /// can cancel a job it's still waiting on without that meaning
    /// "gone", and going away is not the same as an explicit cancel.
    caller_gone: CancellationToken,
    /// Shared with the engine's stall watchdog thread (if
    /// [`crate::EngineConfig::stall_timeout`] is configured): nanoseconds
    /// since `epoch` as of the last [`Self::tick`] call.
    heartbeat: Option<Arc<AtomicU64>>,
    epoch: Instant,
}

impl JobContext {
    pub(crate) fn new(
        job_timeout: Duration,
        cancel: CancellationToken,
        caller_gone: CancellationToken,
        heartbeat: Option<Arc<AtomicU64>>,
        epoch: Instant,
    ) -> Self {
        let ctx = Self {
            started_at: Instant::now(),
            job_timeout,
            cancel,
            caller_gone,
            heartbeat,
            epoch,
        };
        ctx.tick();
        ctx
    }

    /// Returns the reason to stop, if any, checked in the order
    /// cancellation, deadline, caller-gone.
    ///
    /// Implementations of [`crate::LocalModel::run`] must call this at
    /// every natural interruption point and return promptly once it yields
    /// `Some(_)`.
    #[must_use]
    pub fn should_stop(&self) -> Option<StopReason> {
        if self.cancel.is_cancelled() {
            return Some(StopReason::Cancelled);
        }
        if self.started_at.elapsed() >= self.job_timeout {
            return Some(StopReason::Deadline);
        }
        if self.caller_gone.is_cancelled() {
            return Some(StopReason::CallerGone);
        }
        None
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Time remaining before [`StopReason::Deadline`] fires, saturating at
    /// zero. Useful for models that can pass a soft budget down to native
    /// code (e.g. "generate for at most N more milliseconds").
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.job_timeout.saturating_sub(self.started_at.elapsed())
    }

    /// Records a progress heartbeat, used for best-effort stall detection
    /// (see [`crate::EngineConfig::stall_timeout`]).
    ///
    /// There is no `StopReason` for a stall, and it never changes what
    /// *this* job returns — a stall cannot be preempted, only observed from
    /// the outside once it's already too late for this job. What it does
    /// affect is every *later* call to [`crate::EngineHandle::submit`] on
    /// the same handle: once the watchdog *confirms* a stall (the silence
    /// persists to `stall_timeout × stall_escalation_factor`, see
    /// [`crate::EngineConfig::stall_escalation_factor`]) it marks the whole
    /// engine down (see [`crate::EngineError::EngineDown`]) rather than
    /// letting the queue fill up and report [`crate::EngineError::Busy`]
    /// forever. A merely *suspected* stall (one silence window that later
    /// resolves because the job completes) does not disable the engine.
    /// Call `tick` at the same interruption points where
    /// [`Self::should_stop`] is checked.
    pub fn tick(&self) {
        let Some(heartbeat) = &self.heartbeat else {
            return;
        };
        let elapsed = self.epoch.elapsed().as_nanos();
        // Nanoseconds since engine start, truncated to u64: at ~584 years of
        // continuous engine uptime this wraps, which is an acceptable bound
        // for a heartbeat used only to detect multi-second stalls.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "u64 nanoseconds covers centuries of engine uptime, ample for a stall heartbeat"
        )]
        let elapsed_u64 = elapsed as u64;
        heartbeat.store(elapsed_u64, Ordering::Relaxed);
    }
}
