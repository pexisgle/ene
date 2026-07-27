//! The async handle and its dedicated worker thread.

use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::config::EngineConfig;
use crate::context::{JobContext, StopReason};
use crate::error::EngineError;
use crate::model::LocalModel;

/// One unit of work travelling from [`EngineHandle::submit`] to the worker
/// thread.
struct Job<M: LocalModel> {
    request: M::Request,
    cancel: CancellationToken,
    /// Wrapped in `Arc` so [`JobContext`] can hold a clone to poll
    /// [`oneshot::Sender::is_closed`] (the caller-gone check) while the
    /// worker retains its own clone to eventually reclaim ownership via
    /// [`Arc::try_unwrap`] and actually send the result. `JobContext` has no
    /// lifetime parameter, so it cannot merely borrow this sender.
    reply: Arc<oneshot::Sender<Result<M::Response, EngineError<M::Error>>>>,
}

/// A handle to one model running on its own dedicated OS thread.
///
/// Cloning is intentionally not supported: the queue is a single bounded
/// `mpsc` channel and a `Clone` impl would only ever be a thin wrapper
/// around cloning the sender, which callers can already get by wrapping the
/// handle in an `Arc` themselves if they need to share it.
pub struct EngineHandle<M: LocalModel> {
    tx: mpsc::Sender<Job<M>>,
    queue_depth: usize,
    down_reason: Arc<OnceLock<String>>,
}

impl<M: LocalModel> EngineHandle<M> {
    /// Spawns a dedicated worker thread that owns the model for its entire
    /// lifetime, and returns a handle for submitting jobs to it.
    ///
    /// `factory` builds the model. It is called once at startup and again
    /// any time `run` (or `reset`) panics, to rebuild a fresh model —
    /// see the crate-level docs for why this requires `Fn`, not `FnOnce`.
    ///
    /// # Errors
    ///
    /// This function itself cannot fail (matching the trait it wraps): if
    /// the initial `factory()` call fails or panics, or if the OS refuses
    /// to create the thread, the returned handle is still valid but every
    /// [`Self::submit`] call will fail with [`EngineError::EngineDown`]
    /// until the condition clears (the worker retries `factory` on the next
    /// job after a failed build).
    pub fn spawn(
        factory: impl Fn() -> Result<M, M::Error> + Send + 'static,
        cfg: EngineConfig,
    ) -> Self {
        let queue_depth = cfg.queue_depth.max(1);
        let (tx, rx) = mpsc::channel(queue_depth);
        let down_reason: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
        let engine_name: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
        let epoch = Instant::now();
        let job_active = Arc::new(AtomicBool::new(false));
        let heartbeat = cfg.stall_timeout.map(|_| Arc::new(AtomicU64::new(0)));

        if let (Some(stall_timeout), Some(heartbeat)) = (cfg.stall_timeout, heartbeat.clone()) {
            spawn_stall_watchdog(
                Arc::clone(&job_active),
                heartbeat,
                epoch,
                stall_timeout,
                Arc::clone(&engine_name),
            );
        }

        let job_timeout = cfg.job_timeout;
        let worker_job_active = Arc::clone(&job_active);
        let worker_engine_name = Arc::clone(&engine_name);
        let spawned = std::thread::Builder::new()
            .name("ene-infer-worker".to_string())
            .spawn(move || {
                worker_loop(
                    &factory,
                    rx,
                    job_timeout,
                    heartbeat,
                    &worker_job_active,
                    epoch,
                    &worker_engine_name,
                );
            });

        if let Err(err) = spawned {
            tracing::error!(error = %err, "ene-infer: failed to spawn worker thread");
            // `rx` was dropped along with the closure that failed to become
            // a thread, so the channel is already closed; every `submit`
            // will observe `TrySendError::Closed` and consult `down_reason`.
            let _ = down_reason.set(format!("failed to spawn worker thread: {err}"));
        }

        Self {
            tx,
            queue_depth,
            down_reason,
        }
    }

    /// Submits one job and awaits its result.
    ///
    /// Never blocks on a full queue: a full queue fails fast with
    /// [`EngineError::Busy`]. There is no timeout wrapping this call —
    /// [`EngineConfig::job_timeout`] is enforced only inside the worker,
    /// starting when the job begins executing.
    ///
    /// # Errors
    ///
    /// See [`EngineError`].
    pub async fn submit(
        &self,
        req: M::Request,
        cancel: CancellationToken,
    ) -> Result<M::Response, EngineError<M::Error>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let job = Job {
            request: req,
            cancel,
            reply: Arc::new(reply_tx),
        };

        match self.tx.try_send(job) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                return Err(EngineError::Busy {
                    queue_depth: self.queue_depth,
                });
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                return Err(EngineError::EngineDown {
                    reason: self.down_reason(),
                });
            }
        }

        match reply_rx.await {
            Ok(outcome) => outcome,
            Err(_recv_error) => Err(EngineError::EngineDown {
                reason: self.down_reason(),
            }),
        }
    }

    fn down_reason(&self) -> String {
        self.down_reason
            .get()
            .cloned()
            .unwrap_or_else(|| "worker thread is not running".to_string())
    }
}

/// Sends `value` back to the caller, tolerating the caller having already
/// gone away (the receiver was dropped — nothing is listening, `send`
/// returning `Err` is expected and not an error worth logging).
fn respond<T: Send>(reply: Arc<oneshot::Sender<T>>, value: T) {
    match Arc::try_unwrap(reply) {
        Ok(sender) => {
            let _ = sender.send(value);
        }
        Err(_still_shared) => {
            // Should not happen: the `JobContext` holding the other clone is
            // dropped before this is called. Fail safe rather than leak.
            tracing::error!(
                "ene-infer: reply sender still shared after job completion, dropping result"
            );
        }
    }
}

/// Extracts a human-readable message from a `catch_unwind` payload.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Builds a fresh model via `factory`, tolerating both an `Err` return and a
/// panic from inside `factory` itself.
fn build_model<M: LocalModel>(
    factory: &(impl Fn() -> Result<M, M::Error> + Send + 'static),
    engine_name: &OnceLock<String>,
) -> Result<M, String> {
    match panic::catch_unwind(AssertUnwindSafe(factory)) {
        Ok(Ok(model)) => {
            // Only ever set once in practice (the name is stable for a
            // given `M`), but tolerate a later call losing the race.
            let _ = engine_name.set(model.engine_name().to_string());
            Ok(model)
        }
        Ok(Err(err)) => Err(format!("model factory returned an error: {err}")),
        Err(panic_payload) => Err(format!(
            "model factory panicked: {}",
            panic_message(&panic_payload)
        )),
    }
}

/// The body of the dedicated worker thread: owns the model, pulls jobs off
/// the bounded channel one at a time, and never lets more than one `run`
/// call execute at once (there is only ever one worker thread per handle).
fn worker_loop<M: LocalModel>(
    factory: &(impl Fn() -> Result<M, M::Error> + Send + 'static),
    mut rx: mpsc::Receiver<Job<M>>,
    job_timeout: Duration,
    heartbeat: Option<Arc<AtomicU64>>,
    job_active: &AtomicBool,
    epoch: Instant,
    engine_name: &OnceLock<String>,
) {
    let mut model: Option<M> = match build_model(factory, engine_name) {
        Ok(model) => Some(model),
        Err(reason) => {
            tracing::error!(reason = %reason, "ene-infer: initial model construction failed, will retry on first job");
            None
        }
    };

    while let Some(job) = rx.blocking_recv() {
        if model.is_none() {
            match build_model(factory, engine_name) {
                Ok(rebuilt) => model = Some(rebuilt),
                Err(reason) => {
                    respond(job.reply, Err(EngineError::EngineDown { reason }));
                    continue;
                }
            }
        }

        let Some(model_mut) = model.as_mut() else {
            // Unreachable: the branch above either populates `model` or
            // `continue`s past this point. No panic macro needed either
            // way.
            continue;
        };

        let Job {
            request,
            cancel,
            reply,
        } = job;
        job_active.store(true, Ordering::Relaxed);

        let reply_for_ctx = Arc::clone(&reply);
        let caller_gone: Box<dyn Fn() -> bool + Send + Sync> =
            Box::new(move || reply_for_ctx.is_closed());
        let ctx = JobContext::new(job_timeout, cancel, caller_gone, heartbeat.clone(), epoch);

        let run_result = panic::catch_unwind(AssertUnwindSafe(|| model_mut.run(request, &ctx)));

        job_active.store(false, Ordering::Relaxed);

        match run_result {
            Ok(model_result) => {
                let stop = ctx.should_stop();
                let elapsed = ctx.elapsed();
                drop(ctx);

                let reset_panicked =
                    panic::catch_unwind(AssertUnwindSafe(|| model_mut.reset())).is_err();
                if reset_panicked {
                    tracing::error!(
                        engine = engine_name.get().map_or("unknown", String::as_str),
                        "ene-infer: reset() panicked, rebuilding model"
                    );
                    model = None;
                }

                let outcome = match stop {
                    Some(StopReason::Cancelled) => Err(EngineError::Cancelled),
                    Some(StopReason::Deadline) => Err(EngineError::Timeout { after: elapsed }),
                    // Nobody is listening; the value is discarded by
                    // `respond` regardless of what we put here.
                    Some(StopReason::CallerGone) => Err(EngineError::Cancelled),
                    None => model_result.map_err(EngineError::Model),
                };
                respond(reply, outcome);
            }
            Err(panic_payload) => {
                let msg = panic_message(&panic_payload);
                tracing::error!(
                    engine = engine_name.get().map_or("unknown", String::as_str),
                    error = %msg,
                    "ene-infer: run() panicked, rebuilding model"
                );
                model = None;
                respond(
                    reply,
                    Err(EngineError::EngineDown {
                        reason: format!("run() panicked: {msg}"),
                    }),
                );
            }
        }
    }

    job_active.store(false, Ordering::Relaxed);
    if let Some(mut model) = model {
        if panic::catch_unwind(AssertUnwindSafe(|| model.shutdown())).is_err() {
            tracing::error!("ene-infer: shutdown() panicked while stopping worker thread");
        }
    }
}

/// A long-lived thread (one per [`EngineHandle`], not one per job) that
/// watches for stalled jobs and logs a warning. Never affects what
/// [`EngineHandle::submit`] returns — see [`EngineConfig::stall_timeout`].
fn spawn_stall_watchdog(
    job_active: Arc<AtomicBool>,
    heartbeat: Arc<AtomicU64>,
    epoch: Instant,
    stall_timeout: Duration,
    engine_name: Arc<OnceLock<String>>,
) {
    let poll_interval = stall_timeout
        .checked_div(4)
        .filter(|d| !d.is_zero())
        .unwrap_or(stall_timeout);
    let build = std::thread::Builder::new()
        .name("ene-infer-stall-watch".to_string())
        .spawn(move || {
            let mut already_warned = false;
            loop {
                std::thread::sleep(poll_interval);
                if Arc::strong_count(&job_active) == 1 {
                    // Every other clone (the worker's) is gone: the engine
                    // itself has shut down, nothing left to watch.
                    return;
                }
                if !job_active.load(Ordering::Relaxed) {
                    already_warned = false;
                    continue;
                }
                let now_nanos = epoch.elapsed().as_nanos();
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "see JobContext::tick for the same bound"
                )]
                let now_nanos_u64 = now_nanos as u64;
                let last_tick = heartbeat.load(Ordering::Relaxed);
                let stalled_for = Duration::from_nanos(now_nanos_u64.saturating_sub(last_tick));
                if stalled_for >= stall_timeout && !already_warned {
                    tracing::warn!(
                        engine = engine_name.get().map_or("unknown", String::as_str),
                        stalled_for = ?stalled_for,
                        "ene-infer: job appears stalled (no JobContext::tick progress)"
                    );
                    already_warned = true;
                }
            }
        });
    if let Err(err) = build {
        tracing::error!(error = %err, "ene-infer: failed to spawn stall watchdog thread; stall detection disabled for this engine");
    }
}
