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
use crate::model::{LocalModel, StreamingLocalModel};
use crate::stream::{ChunkReceiver, ChunkSink, send_terminal};

/// The reply channel for one job. The worker owns this outright — no
/// sharing, no `Arc`, no reclaiming ownership back from a clone — so there
/// is no ordering dependency between anything else in the worker loop and
/// actually sending the reply.
type Reply<M> =
    oneshot::Sender<Result<<M as LocalModel>::Response, EngineError<<M as LocalModel>::Error>>>;

/// A streaming job's type-erased body, built by [`EngineHandle::submit_stream`]
/// (which knows `M::Chunk`) and invoked by [`worker_loop`] (which only knows
/// `M: LocalModel`) — see [`JobKind::Streaming`].
type StreamRunner<M> = Box<dyn FnOnce(&mut M, &JobContext) + Send>;

/// A streaming job's `EngineDown` notifier, used only if [`StreamRunner`]
/// itself panics — see [`JobKind::Streaming`].
type EngineDownNotifier = Box<dyn FnOnce(String) + Send>;

/// What a [`Job`] asks the worker to do, and how to report the outcome.
///
/// Both variants travel through the exact same bounded queue as
/// [`EngineHandle::submit`]/[`EngineHandle::submit_stream`], which is what
/// gives a streaming job and an ordinary one the same `Busy`/serialization
/// behavior — one dedicated worker thread, one queue, no separate admission
/// path for streaming work.
enum JobKind<M: LocalModel> {
    /// An [`EngineHandle::submit`] job: run to completion, reply once via
    /// the oneshot channel.
    Oneshot {
        request: M::Request,
        reply: Reply<M>,
    },
    /// An [`EngineHandle::submit_stream`] job. `run` already closes over the
    /// request, the [`ChunkSink`] and its mapped-error delivery — it is
    /// erased to `Box<dyn FnOnce(&mut M, &JobContext) + Send>` here
    /// specifically so this enum (and therefore [`Job`], [`worker_loop`])
    /// stays generic over plain `M: LocalModel`, with no `M::Chunk` bound
    /// anywhere outside [`EngineHandle::submit_stream`] itself. `notify`
    /// is a *separate* handle to the same underlying channel, used only if
    /// `run` panics (the closure `run` owns, including its own channel
    /// handle, is dropped mid-unwind along with everything else it
    /// captured — this one survives because it was never moved into `run`).
    Streaming {
        run: StreamRunner<M>,
        notify_engine_down: EngineDownNotifier,
    },
}

/// One unit of work travelling from [`EngineHandle::submit`]/
/// [`EngineHandle::submit_stream`] to the worker thread.
struct Job<M: LocalModel> {
    cancel: CancellationToken,
    /// Cancelled by a `DropGuard` living in the caller's own stack frame (or,
    /// for a stream, held inside the returned [`ChunkReceiver`]) — see
    /// [`JobContext`]'s field of the same name.
    caller_gone: CancellationToken,
    kind: JobKind<M>,
}

/// Maps a model-level failure to the precise [`EngineError`] reason, giving
/// priority to whatever stop condition fired: a model that returns its own
/// (likely generic) "interrupted" error after noticing `should_stop` still
/// gets reported as the framework's precise [`EngineError::Timeout`] /
/// [`EngineError::Cancelled`], not [`EngineError::Model`]. Shared by the
/// oneshot outcome mapping in [`worker_loop`] and by
/// [`EngineHandle::submit_stream`]'s streaming-job closure.
fn map_stop_to_error<E: std::error::Error>(
    model_err: E,
    stop: Option<StopReason>,
    elapsed: Duration,
) -> EngineError<E> {
    match stop {
        Some(StopReason::Deadline) => EngineError::Timeout { after: elapsed },
        // `CallerGone` is folded into `Cancelled` here: nobody is listening
        // for the exact variant in either case (the oneshot reply receiver
        // was dropped, or the stream's `ChunkReceiver` was dropped).
        Some(StopReason::Cancelled | StopReason::CallerGone) => EngineError::Cancelled,
        None => EngineError::Model(model_err),
    }
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
                cfg.stall_escalation_factor,
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
    /// If [`EngineConfig::stall_timeout`] previously confirmed a wedged
    /// worker (a suspected stall that persisted to the escalation
    /// threshold), this returns [`EngineError::EngineDown`] immediately
    /// rather than [`EngineError::Busy`] — a permanently stuck worker
    /// fills the queue forever, and a caller needs to be able to tell
    /// "briefly saturated, retry shortly" apart from "this handle will
    /// never make progress again". A merely *suspected* stall (one silence
    /// window that later resolved) does not affect this call.
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
            cancel,
            caller_gone,
            kind: JobKind::Oneshot {
                request: req,
                reply: reply_tx,
            },
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

impl<M: StreamingLocalModel> EngineHandle<M> {
    /// Submits one job that delivers its output incrementally, returning
    /// immediately with a [`ChunkReceiver`] rather than awaiting completion.
    ///
    /// This exists as a second `impl` block, gated on `M:
    /// StreamingLocalModel`, specifically so [`Self::submit`] keeps working
    /// unconditionally for every `M: LocalModel` — a model that never
    /// implements [`StreamingLocalModel`] simply never has this method in
    /// its `EngineHandle`'s API, with no change to how it compiles or runs
    /// today.
    ///
    /// The returned [`ChunkReceiver`] yields `Ok(chunk)` for each chunk
    /// [`StreamingLocalModel::run_streaming`] pushes, in order, followed —
    /// only if the job did not simply run to completion — by one final
    /// `Err(_)` explaining why (mapped exactly like [`Self::submit`]'s own
    /// outcome: [`EngineError::Timeout`], [`EngineError::Cancelled`], or
    /// [`EngineError::Model`]), then ends. `chunk_buffer` bounds the
    /// channel between the worker and this handle's caller — coerced up to
    /// at least 1 — and is the backpressure knob: once it fills,
    /// [`ChunkSink::send`] blocks the worker thread until the consumer
    /// drains it, [`EngineConfig::job_timeout`] elapses, or the consumer
    /// drops this [`ChunkReceiver`], never growing past that bound.
    ///
    /// Dropping the returned [`ChunkReceiver`] — the streaming equivalent of
    /// dropping [`Self::submit`]'s reply future — is observed by the worker
    /// as [`crate::StopReason::CallerGone`], the same single cancellation
    /// path [`Self::submit`] uses, not a second mechanism.
    ///
    /// Like [`Self::submit`], this call itself never blocks: a full queue or
    /// a down engine is reported as the first (and only) item the returned
    /// [`ChunkReceiver`] yields, rather than a synchronous error — a
    /// consumer only ever has to deal with one shape ("read chunks from the
    /// receiver until it ends"), regardless of whether the job ever actually
    /// started.
    pub fn submit_stream(
        &self,
        req: M::Request,
        cancel: CancellationToken,
        chunk_buffer: usize,
    ) -> ChunkReceiver<M::Chunk, M::Error> {
        let (tx, rx) =
            mpsc::channel::<Result<M::Chunk, EngineError<M::Error>>>(chunk_buffer.max(1));

        let caller_gone = CancellationToken::new();
        let drop_guard = caller_gone.clone().drop_guard();

        if let Some(reason) = self.down_reason.get() {
            drop(tx.try_send(Err(EngineError::EngineDown {
                reason: reason.clone(),
            })));
            return ChunkReceiver::new(rx, drop_guard);
        }

        // Three independent handles to the same channel, cloned up front
        // rather than reached for later, because `sink` is about to be
        // moved into `run` below:
        // - `sink` (wraps one clone of `tx`): used by `run_streaming` itself
        //   via `ChunkSink::send`, and to report a mapped mid-stream error.
        // - `notify_tx`: survives a panic *inside* `run` (which drops `sink`
        //   along with everything else `run` captured) so `worker_loop`'s
        //   panic arm can still report `EngineDown` instead of the stream
        //   silently ending.
        // - `enqueue_result_tx`: used right here, only if the job never
        //   makes it onto the worker's queue at all (`Busy`/`EngineDown`),
        //   since in that case neither `run` nor `notify_engine_down` is
        //   ever invoked.
        let notify_tx = tx.clone();
        let enqueue_result_tx = tx.clone();
        let sink = ChunkSink::new(tx);

        let run: StreamRunner<M> = Box::new(move |model, ctx| {
            if let Err(model_err) = model.run_streaming(req, ctx, &sink) {
                let stop = ctx.should_stop();
                let elapsed = ctx.elapsed();
                sink.notify_terminal(map_stop_to_error(model_err, stop, elapsed));
            }
        });
        let notify_engine_down: EngineDownNotifier = Box::new(move |reason| {
            send_terminal(&notify_tx, EngineError::EngineDown { reason });
        });

        let job = Job {
            cancel,
            caller_gone,
            kind: JobKind::Streaming {
                run,
                notify_engine_down,
            },
        };

        match self.tx.try_send(job) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                drop(enqueue_result_tx.try_send(Err(EngineError::Busy {
                    queue_depth: self.queue_depth,
                })));
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                drop(enqueue_result_tx.try_send(Err(EngineError::EngineDown {
                    reason: self.down_reason(),
                })));
            }
        }

        ChunkReceiver::new(rx, drop_guard)
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

/// Runs `model`'s [`LocalModel::reset`] after a job that returned from
/// `run`/`run_streaming` without panicking — shared by [`worker_loop`]'s
/// oneshot and streaming arms, since this bookkeeping does not depend on
/// which kind of job just finished. Discards the model instead (triggering a
/// full rebuild via `factory` on the next job) if `reset` itself panics.
fn reset_or_rebuild<M: LocalModel>(model: &mut Option<M>, engine_name: &OnceLock<String>) {
    let Some(model_mut) = model.as_mut() else {
        return;
    };
    let reset_panicked = panic::catch_unwind(AssertUnwindSafe(|| model_mut.reset())).is_err();
    if reset_panicked {
        tracing::error!(
            engine = engine_name.get().map_or("unknown", String::as_str),
            "ene-infer: reset() panicked, rebuilding model"
        );
        *model = None;
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
                    match job.kind {
                        JobKind::Oneshot { reply, .. } => {
                            respond(reply, Err(EngineError::EngineDown { reason }));
                        }
                        JobKind::Streaming {
                            notify_engine_down, ..
                        } => notify_engine_down(reason),
                    }
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
            cancel,
            caller_gone,
            kind,
        } = job;
        job_active.store(true, Ordering::Relaxed);

        let ctx = JobContext::new(job_timeout, cancel, caller_gone, heartbeat.cloned(), epoch);

        match kind {
            JobKind::Oneshot { request, reply } => {
                let run_result =
                    panic::catch_unwind(AssertUnwindSafe(|| model_mut.run(request, &ctx)));
                job_active.store(false, Ordering::Relaxed);

                match run_result {
                    Ok(model_result) => {
                        let stop = ctx.should_stop();
                        let elapsed = ctx.elapsed();
                        drop(ctx);

                        reset_or_rebuild(&mut model, engine_name);

                        // A genuine `Ok` from `run` always wins, even if a
                        // stop condition also fired: a job that legitimately
                        // finished a moment before its deadline (or around
                        // the same time as a cancellation) should not have
                        // a good result thrown away in favor of
                        // `Timeout`/`Cancelled`. The stop reason only
                        // matters to explain an `Err` — a model that
                        // noticed `should_stop()` and gave up returns
                        // *some* `Err`, and we report the framework's
                        // precise reason for it rather than the model's own
                        // (likely generic) "interrupted" error.
                        let outcome = model_result
                            .map_err(|model_err| map_stop_to_error(model_err, stop, elapsed));
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
            JobKind::Streaming {
                run,
                notify_engine_down,
            } => {
                let run_result = panic::catch_unwind(AssertUnwindSafe(|| run(model_mut, &ctx)));
                job_active.store(false, Ordering::Relaxed);
                drop(ctx);

                match run_result {
                    // `run` (built in `EngineHandle::submit_stream`) already
                    // pushed every chunk and, on a model-level failure,
                    // already mapped and delivered the terminal
                    // `EngineError` itself — there is nothing left to
                    // `respond` to here, only the same post-job
                    // reset-or-rebuild bookkeeping the oneshot path does.
                    Ok(()) => reset_or_rebuild(&mut model, engine_name),
                    Err(panic_payload) => {
                        let msg = panic_message(&panic_payload);
                        tracing::error!(
                            engine = engine_name.get().map_or("unknown", String::as_str),
                            error = %msg,
                            "ene-infer: run_streaming() panicked, rebuilding model"
                        );
                        model = None;
                        notify_engine_down(format!("run_streaming() panicked: {msg}"));
                    }
                }
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
/// watches for stalled jobs, with graduated escalation.
///
/// A genuine stall means the worker thread is stuck inside a synchronous
/// `run` call that is never going to check `should_stop`/call `tick` again
/// — there is no way to preempt that safely, so this does not try. But a
/// single silence window is not proof of a permanent hang: a legitimately
/// long native operation (e.g. `whisper_rs`'s full-segment decode) may
/// simply not tick for a while and then finish normally.
///
/// Graduated escalation distinguishes the two:
///
/// 1. **Suspected stall** — `tick` has not been called for at least
///    `stall_timeout` while a job is active. Logged as a `tracing::warn!`,
///    but the engine keeps running. If the worker becomes idle (the job
///    finished), the suspicion is cleared — it was a false positive.
/// 2. **Confirmed stall** — the silence persists for at least
///    `stall_timeout × escalation_factor`. Only now is the reason recorded
///    in `down_reason` (the same cell [`EngineHandle::submit`] already
///    consults) and the watchdog stops: from that point on `submit` reports
///    [`EngineError::EngineDown`] instead of letting the bounded queue fill
///    up and return [`EngineError::Busy`] forever, which would leave
///    callers unable to tell "briefly saturated" apart from "this handle
///    will never make progress again".
fn spawn_stall_watchdog(
    job_active: Arc<AtomicBool>,
    heartbeat: Arc<AtomicU64>,
    epoch: Instant,
    stall_timeout: Duration,
    escalation_factor: u32,
    engine_name: Arc<OnceLock<String>>,
    down_reason: Arc<OnceLock<String>>,
) {
    let poll_interval = stall_timeout
        .checked_div(4)
        .filter(|d| !d.is_zero())
        .unwrap_or(stall_timeout);
    let confirm_threshold = stall_timeout.saturating_mul(escalation_factor);
    let build = std::thread::Builder::new()
        .name("ene-infer-stall-watch".to_string())
        .spawn(move || {
            // Whether we have already logged the suspected-stall warning
            // for the current job. Reset to `false` whenever the worker
            // becomes idle (the job finished), so the next job gets its
            // own warning if it too goes silent.
            let mut suspected = false;
            loop {
                std::thread::sleep(poll_interval);
                if Arc::strong_count(&job_active) == 1 {
                    // Every other clone (the worker's) is gone: the engine
                    // itself has shut down, nothing left to watch.
                    return;
                }
                if !job_active.load(Ordering::Relaxed) {
                    // The worker is idle: any prior suspicion was a false
                    // positive (the job finished despite the silence).
                    suspected = false;
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
                if stalled_for >= confirm_threshold {
                    tracing::error!(
                        engine = engine_name.get().map_or("unknown", String::as_str),
                        stalled_for = ?stalled_for,
                        confirm_threshold = ?confirm_threshold,
                        "ene-infer: suspected stall confirmed — no JobContext::tick progress \
                         for {stalled_for:?} (confirm_threshold={confirm_threshold:?}); marking \
                         engine down (a stuck worker thread cannot be preempted, so this \
                         handle will refuse further jobs from now on)"
                    );
                    drop(down_reason.set(format!(
                        "engine job stalled for at least {stalled_for:?} without JobContext::tick \
                         progress (stall_timeout={stall_timeout:?}, \
                         escalation_factor={escalation_factor}); the worker thread is presumed \
                         permanently stuck"
                    )));
                    return;
                }
                if stalled_for >= stall_timeout && !suspected {
                    suspected = true;
                    tracing::warn!(
                        engine = engine_name.get().map_or("unknown", String::as_str),
                        stalled_for = ?stalled_for,
                        stall_timeout = ?stall_timeout,
                        confirm_threshold = ?confirm_threshold,
                        "ene-infer: job appears stalled (no JobContext::tick for {stalled_for:?}); \
                         monitoring — the engine will be marked down if the silence persists \
                         to {confirm_threshold:?}, or the suspicion will be cleared if the job \
                         completes"
                    );
                }
            }
        });
    if let Err(err) = build {
        tracing::error!(error = %err, "ene-infer: failed to spawn stall watchdog thread; stall detection disabled for this engine");
    }
}
