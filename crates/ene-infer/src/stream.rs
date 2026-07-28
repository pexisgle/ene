//! The producer/consumer halves of a streaming job:
//! [`ChunkSink`] (worker-thread side, used from inside
//! [`crate::StreamingLocalModel::run_streaming`]) and [`ChunkReceiver`]
//! (async-side handle returned by [`crate::EngineHandle::submit_stream`]).
//!
//! Neither type implements `futures_core::Stream`/`tokio_stream::Stream`:
//! this crate stays a leaf (`tokio`, `tokio-util`, `thiserror`, `tracing`
//! only, see the crate docs), and pulling in a stream-combinator crate just
//! to expose one is not worth giving up that property. [`ChunkReceiver`]
//! exposes both an `async fn recv` and a bare `poll_recv` (the same shape
//! [`tokio::sync::mpsc::Receiver`] itself offers) so a downstream crate that
//! already depends on `tokio-stream` can implement `Stream` for a thin local
//! newtype around it in one line — see `ene-ai`'s streaming LLM adapter.

use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::DropGuard;

use crate::context::{JobContext, StopReason};
use crate::error::EngineError;

/// How long [`ChunkSink::send`] sleeps between polling attempts while the
/// bounded channel is full. Short enough that cancellation/deadline/consumer
/// drain latency stays imperceptible, long enough not to busy-spin a whole
/// CPU core while genuinely blocked on a slow consumer.
const SEND_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Upper bound on attempts [`send_terminal`] makes to deliver the one
/// terminal message (the mapped [`EngineError`] explaining why the stream
/// ends without having pushed every chunk, or `EngineDown` after a panic) to
/// a full channel. Bounded so a consumer that stopped draining without
/// dropping its receiver cannot wedge the worker thread indefinitely on this
/// best-effort final delivery — the run itself already finished by this
/// point, there is no deadline left to respect, only a bound on how long the
/// *next* job may be kept waiting behind this one.
const TERMINAL_SEND_ATTEMPTS: u32 = 50;

/// Best-effort delivery of one final `Result` item (a terminal error, or —
/// via [`ChunkSink::send`]'s own retry loop — an ordinary chunk) into a
/// bounded channel, tolerating both a full buffer (bounded retries) and a
/// dropped receiver (gives up silently: nothing is listening for a stream
/// that has already ended from the consumer's side).
pub(crate) fn send_terminal<C, E>(tx: &mpsc::Sender<Result<C, EngineError<E>>>, err: EngineError<E>)
where
    C: Send + 'static,
    E: std::error::Error + Send + 'static,
{
    let mut item = Err(err);
    for _ in 0..TERMINAL_SEND_ATTEMPTS {
        match tx.try_send(item) {
            Err(mpsc::error::TrySendError::Full(returned)) => {
                item = returned;
                std::thread::sleep(SEND_POLL_INTERVAL);
            }
            // Delivered, or nobody is listening anymore — either way there
            // is nothing left to retry.
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => return,
        }
    }
    tracing::warn!(
        "ene-infer: dropped a streaming job's terminal error — the consumer's buffer stayed \
         full for {TERMINAL_SEND_ATTEMPTS} polling attempts without draining or disconnecting"
    );
}

/// The worker-thread side of one streaming job's channel, handed to
/// [`crate::StreamingLocalModel::run_streaming`] as `sink`.
///
/// Generic directly over the chunk (`C`) and model-error (`E`) types rather
/// than over the whole `M: StreamingLocalModel`, so code that only produces
/// chunks (e.g. a model's own token-sampling loop) does not need to name the
/// model type itself.
pub struct ChunkSink<C: Send + 'static, E: std::error::Error + Send + 'static> {
    tx: mpsc::Sender<Result<C, EngineError<E>>>,
}

impl<C: Send + 'static, E: std::error::Error + Send + 'static> ChunkSink<C, E> {
    pub(crate) fn new(tx: mpsc::Sender<Result<C, EngineError<E>>>) -> Self {
        Self { tx }
    }

    /// Pushes one chunk, blocking the calling (worker) thread while the
    /// bounded channel is full — this is the backpressure mechanism: a slow
    /// consumer stalls the producer, the buffer never grows past its
    /// configured bound.
    ///
    /// Polls [`JobContext::should_stop`] between attempts, so a full sink
    /// cannot outlive [`crate::EngineConfig::job_timeout`] (`should_stop`
    /// reports [`StopReason::Deadline`] once it elapses) and stops promptly
    /// once the consumer goes away (the channel closing and `should_stop`
    /// reporting [`StopReason::CallerGone`] happen together, both driven by
    /// the same drop of the [`crate::ChunkReceiver`] returned to that
    /// consumer) — the same stop conditions and the same method
    /// [`crate::LocalModel::run`] implementors already check, not a second,
    /// parallel cancellation mechanism.
    ///
    /// # Errors
    ///
    /// Returns the [`StopReason`] that fired before the chunk could be
    /// delivered. Implementors should treat this exactly like an ordinary
    /// `should_stop` hit: stop working and return promptly.
    pub fn send(&self, chunk: C, ctx: &JobContext) -> Result<(), StopReason> {
        let mut item = Ok(chunk);
        loop {
            if let Some(stop) = ctx.should_stop() {
                return Err(stop);
            }
            match self.tx.try_send(item) {
                Ok(()) => return Ok(()),
                Err(mpsc::error::TrySendError::Full(returned)) => {
                    item = returned;
                    std::thread::sleep(SEND_POLL_INTERVAL);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return Err(StopReason::CallerGone),
            }
        }
    }

    /// Delivers the one terminal [`EngineError`] that explains why
    /// `run_streaming` ended without (necessarily) having pushed every
    /// chunk. Used by [`crate::EngineHandle::submit_stream`]'s own
    /// bookkeeping after `run_streaming` returns `Err`; not part of the
    /// contract a model implementor drives directly.
    pub(crate) fn notify_terminal(&self, err: EngineError<E>) {
        send_terminal(&self.tx, err);
    }
}

/// The async-side handle for one streaming job, returned by
/// [`crate::EngineHandle::submit_stream`].
///
/// Dropping this — including a dropped `tokio::time::timeout`/`select!`
/// race, exactly like [`crate::EngineHandle::submit`]'s own reply future —
/// cancels the job's `caller_gone` token via the held [`DropGuard`], which
/// [`JobContext::should_stop`] observes as [`StopReason::CallerGone`] the
/// next time the worker checks it (including inside [`ChunkSink::send`]).
pub struct ChunkReceiver<C: Send + 'static, E: std::error::Error + Send + 'static> {
    rx: mpsc::Receiver<Result<C, EngineError<E>>>,
    _drop_guard: DropGuard,
}

impl<C: Send + 'static, E: std::error::Error + Send + 'static> ChunkReceiver<C, E> {
    pub(crate) fn new(
        rx: mpsc::Receiver<Result<C, EngineError<E>>>,
        drop_guard: DropGuard,
    ) -> Self {
        Self {
            rx,
            _drop_guard: drop_guard,
        }
    }

    /// Awaits the next chunk, the terminal error (if any), or `None` once
    /// the stream has ended.
    pub async fn recv(&mut self) -> Option<Result<C, EngineError<E>>> {
        self.rx.recv().await
    }

    /// Poll-based receive, the same shape
    /// [`tokio::sync::mpsc::Receiver::poll_recv`] itself offers — lets a
    /// downstream crate implement `futures_core::Stream`/`tokio_stream::Stream`
    /// for a thin local wrapper without this crate depending on either (see
    /// this module's docs).
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<C, EngineError<E>>>> {
        self.rx.poll_recv(cx)
    }
}
