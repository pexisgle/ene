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

/// The reply channel for one job. The worker owns this outright — no
/// sharing, no `Arc`, no reclaiming ownership back from a clone — so there
/// is no ordering dependency between anything else in the worker loop and
/// actually sending the reply.
type Reply<M> =
    oneshot::Sender<Result<<M as LocalModel>::Response, EngineError<<M as LocalModel>::Error>>>;

/// One unit of work travelling from [`EngineHandle::submit`] to the worker
/// thread.
struct Job<M: LocalModel> {
    request: M::Request,
    cancel: CancellationToken,
    /// Cancelled by a `DropGuard` living in [`EngineHandle::submit`]'s own
    /// stack frame — see [`JobContext`]'s field of the same name.
    caller_gone: CancellationToken,
    reply: Reply<M>,
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
    /// `factory` builds the model. It is called once at startup — on the
    /// worker thread, not synchronously here, see [`Self::try_spawn`] if
    /// that matters for your model — and again any time `run` (or `reset`)
    /// panics, to rebuild a fresh model. See the crate-level docs for why
    /// this requires `Fn`, not `FnOnce`.
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
        Self::spawn_with_initial(factory, cfg, None)
    }

    /// As [`Self::spawn`], but builds the first model synchronously on the
    /// calling thread instead of deferring that first `factory()` call to
    /// the worker thread.
    ///
    /// `spawn` cannot report a construction failure to its caller: it always
    /// calls `factory` on the background worker thread, so a model whose
    /// constructor is meant to fail fast (missing file, bad config, ...)
    /// would otherwise have that failure silently deferred to the first
    /// [`Self::submit`] call, surfacing as an opaque
    /// [`EngineError::EngineDown`] instead of the model's own `M::Error` at
    /// the call site where a caller can act on it. `try_spawn` fixes that:
    /// it calls `factory` once, right here, and — only if that succeeds —
    /// hands the already-built instance to the worker thread for its first
    /// job instead of calling `factory` again. `factory` is still retained
    /// for every later rebuild after a panic, exactly as with `spawn`.
    ///
    /// # Errors
    ///
    /// Returns whatever `factory()` returns if its first, synchronous call
    /// fails. Unlike `spawn`, there is no handle to hand back in that case.
    /// A panic from that first `factory()` call is not caught here (this
    /// crate's `catch_unwind` recovery only wraps calls made from inside the
    /// worker thread) and propagates as an ordinary Rust panic to the
    /// caller — consistent with any other constructor that can panic, and
    /// arguably more useful here than silently swallowing a construction-time
    /// bug the way a deferred `EngineDown` would.
    pub fn try_spawn(
        factory: impl Fn() -> Result<M, M::Error> + Send + 'static,
        cfg: EngineConfig,
    ) -> Result<Self, M::Error> {
        let first = factory()?;
        Ok(Self::spawn_with_initial(factory, cfg, Some(first)))
    }

    /// Shared body of [`Self::spawn`]/[`Self::try_spawn`]. `initial`, when
    /// `Some`, is handed to the worker loop as the already-built first model
    /// instance so it does not call `factory` again for that first job.
    fn spawn_with_initial(
        factory: impl Fn() -> Result<M, M::Error> + Send + 'static,
        cfg: EngineConfig,
        initial: Option<M>,
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
                Arc::clone(&down_reason),
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
                    heartbeat.as_ref(),
                    &worker_job_active,
                    epoch,
                    &worker_engine_name,
                    initial,
                );
            });

        if let Err(err) = spawned {
            tracing::error!(error = %err, "ene-infer: failed to spawn worker thread");
            // `rx` was dropped along with the closure that failed to become
            // a thread, so the channel is already closed; every `submit`
            // will observe `TrySendError::Closed` and consult `down_reason`.
            // `set` can only fail if it raced another writer, which cannot
            // happen here (this runs at most once); either way there is
            // nothing more useful to do than drop the `Result`.
            drop(down_reason.set(format!("failed to spawn worker thread: {err}")));
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
    /// If [`EngineConfig::stall_timeout`] previously caught a wedged
    /// worker, this returns [`EngineError::EngineDown`] immediately rather
    /// than [`EngineError::Busy`] — a permanently stuck worker fills the
    /// queue forever, and a caller needs to be able to tell "briefly
    /// saturated, retry shortly" apart from "this handle will never make
    /// progress again".
    ///
    /// # Errors
    ///
    /// See [`EngineError`].
    pub async fn submit(
        &self,
        req: M::Request,
        cancel: CancellationToken,
    ) -> Result<M::Response, EngineError<M::Error>> {
        if let Some(reason) = self.down_reason.get() {
            return Err(EngineError::EngineDown {
                reason: reason.clone(),
            });
        }

        // Cancelled by `_drop_guard`'s `Drop` impl the moment this future
        // (or its caller's future, transitively) is dropped — including a
        // dropped `tokio::time::timeout`/`select!` race — which is exactly
        // the "caller went away" condition `StopReason::CallerGone` exists
        // to detect. `caller_gone` and `_drop_guard` share the same
        // underlying cancellation flag (one is a clone of the other), so
        // cancelling one is observed by the other.
        let caller_gone = CancellationToken::new();
        let _drop_guard = caller_gone.clone().drop_guard();

        let (reply_tx, reply_rx) = oneshot::channel();
        let job = Job {
            request: req,
            cancel,
            caller_gone,
            reply: reply_tx,
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
fn respond<T: Send>(reply: oneshot::Sender<T>, value: T) {
    drop(reply.send(value));
}

/// Extracts a human-readable message from a `catch_unwind` payload.
///
/// Takes `&Box<dyn Any + Send>` rather than the seemingly-equivalent
/// `&(dyn Any + Send)`: the latter's default object-lifetime elision binds
/// the trait object to the *reference's* lifetime rather than `'static`,
/// which silently breaks `downcast_ref` (it always returns `None`, since
/// the payload really is `Any + 'static` and the two no longer type-match
/// the way they appear to). Confirmed with a standalone repro before fixing
/// this — see the regression test in `src/tests.rs` that pins the actual
/// message surviving into `EngineError::EngineDown`.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
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
            drop(engine_name.set(model.engine_name().to_string()));
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
///
/// `initial`, when `Some` (see [`EngineHandle::try_spawn`]), is used as the
/// first model instance instead of calling `factory` again — the caller
/// already built and validated it synchronously before this thread started.
fn worker_loop<M: LocalModel>(
    factory: &(impl Fn() -> Result<M, M::Error> + Send + 'static),
    mut rx: mpsc::Receiver<Job<M>>,
    job_timeout: Duration,
    heartbeat: Option<&Arc<AtomicU64>>,
    job_active: &AtomicBool,
    epoch: Instant,
    engine_name: &OnceLock<String>,
    initial: Option<M>,
) {
    let mut model: Option<M> = match initial {
        Some(model) => {
            // Mirrors `build_model`'s bookkeeping for the success path: only
            // ever set once in practice, but tolerate a later call losing
            // the race.
            drop(engine_name.set(model.engine_name().to_string()));
            Some(model)
        }
        None => match build_model(factory, engine_name) {
            Ok(model) => Some(model),
            Err(reason) => {
                tracing::error!(reason = %reason, "ene-infer: initial model construction failed, will retry on first job");
                None
            }
        },
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
            caller_gone,
            reply,
        } = job;
        job_active.store(true, Ordering::Relaxed);

        let ctx = JobContext::new(job_timeout, cancel, caller_gone, heartbeat.cloned(), epoch);

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

                // A genuine `Ok` from `run` always wins, even if a stop
                // condition also fired: a job that legitimately finished a
                // moment before its deadline (or around the same time as a
                // cancellation) should not have a good result thrown away
                // in favor of `Timeout`/`Cancelled`. The stop reason only
                // matters to explain an `Err` — a model that noticed
                // `should_stop()` and gave up returns *some* `Err`, and we
                // report the framework's precise reason for it rather than
                // the model's own (likely generic) "interrupted" error.
                let outcome = match model_result {
                    Ok(response) => Ok(response),
                    Err(model_err) => match stop {
                        Some(StopReason::Deadline) => Err(EngineError::Timeout { after: elapsed }),
                        // `CallerGone` is folded into `Cancelled` here:
                        // nobody is listening for the result in that case,
                        // so the exact variant `respond` is about to
                        // discard doesn't matter.
                        Some(StopReason::Cancelled | StopReason::CallerGone) => {
                            Err(EngineError::Cancelled)
                        }
                        None => Err(EngineError::Model(model_err)),
                    },
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
    if let Some(mut model) = model
        && panic::catch_unwind(AssertUnwindSafe(|| model.shutdown())).is_err()
    {
        tracing::error!("ene-infer: shutdown() panicked while stopping worker thread");
    }
}

/// A long-lived thread (one per [`EngineHandle`], not one per job) that
/// watches for stalled jobs.
///
/// A genuine stall means the worker thread is stuck inside a synchronous
/// `run` call that is never going to check `should_stop`/call `tick` again
/// — there is no way to preempt that safely, so this does not try. Instead,
/// once a stall is confirmed it records the reason in `down_reason` (the
/// same cell [`EngineHandle::submit`] already consults) and stops watching:
/// from that point on `submit` reports [`EngineError::EngineDown`] instead
/// of letting the bounded queue fill up and return [`EngineError::Busy`]
/// forever, which would leave callers unable to tell "briefly saturated"
/// apart from "this handle will never make progress again".
fn spawn_stall_watchdog(
    job_active: Arc<AtomicBool>,
    heartbeat: Arc<AtomicU64>,
    epoch: Instant,
    stall_timeout: Duration,
    engine_name: Arc<OnceLock<String>>,
    down_reason: Arc<OnceLock<String>>,
) {
    let poll_interval = stall_timeout
        .checked_div(4)
        .filter(|d| !d.is_zero())
        .unwrap_or(stall_timeout);
    let build = std::thread::Builder::new()
        .name("ene-infer-stall-watch".to_string())
        .spawn(move || {
            loop {
                std::thread::sleep(poll_interval);
                if Arc::strong_count(&job_active) == 1 {
                    // Every other clone (the worker's) is gone: the engine
                    // itself has shut down, nothing left to watch.
                    return;
                }
                if !job_active.load(Ordering::Relaxed) {
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
                if stalled_for >= stall_timeout {
                    tracing::warn!(
                        engine = engine_name.get().map_or("unknown", String::as_str),
                        stalled_for = ?stalled_for,
                        "ene-infer: job appears stalled; marking engine down (a stuck worker \
                         thread cannot be preempted, so this handle will refuse further jobs \
                         from now on)"
                    );
                    drop(down_reason.set(format!(
                        "engine job stalled for at least {stalled_for:?} without JobContext::tick \
                         progress (stall_timeout={stall_timeout:?}); the worker thread is presumed \
                         permanently stuck"
                    )));
                    return;
                }
            }
        });
    if let Err(err) = build {
        tracing::error!(error = %err, "ene-infer: failed to spawn stall watchdog thread; stall detection disabled for this engine");
    }
}
