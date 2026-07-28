//! A generic regression battery for [`crate::LocalModel`] implementations.
//!
//! # Why `ConformanceRequest` / `ConformanceResponse` exist
//!
//! [`crate::LocalModel::Request`] and [`crate::LocalModel::Response`] are
//! opaque to this crate (`Send + 'static` and nothing else) — that is the
//! whole point of keeping this crate free of domain knowledge. But a
//! battery that runs the *same* checks against *any* engine needs some way
//! to script "run for about this long", "panic partway through", and to
//! read back "how many times was `reset` called on this model instance" —
//! none of which are expressible through an opaque request/response pair.
//!
//! These two small traits close that gap. They are test-only scaffolding:
//! implement them on a lightweight request/response type built specifically
//! for testing your [`crate::LocalModel`], not on your production request
//! type.
#![expect(
    clippy::expect_used,
    reason = "conformance is a test-only battery (feature = \"test-util\"); it reports failures via expect()/assert! by design"
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::config::EngineConfig;
#[cfg(test)]
use crate::context::JobContext;
use crate::error::EngineError;
use crate::handle::EngineHandle;
use crate::model::{LocalModel, StreamingLocalModel};
#[cfg(test)]
use crate::stream::ChunkSink;

/// Lets the conformance battery script a [`crate::LocalModel::Request`]
/// generically: how long to run, and whether to panic partway through.
pub trait ConformanceRequest: Default + Send + 'static {
    /// Builds a request that, when run, checks
    /// [`crate::JobContext::should_stop`]/calls [`crate::JobContext::tick`] in a loop for
    /// approximately `run_for` before completing, or panics almost
    /// immediately if `then_panic` is `true`.
    fn scripted(run_for: Duration, then_panic: bool) -> Self;
}

/// Lets the conformance battery read back observability signals generically
/// from a [`crate::LocalModel::Response`].
///
/// Requires `Debug` so the battery can include the response in assertion
/// failure messages.
pub trait ConformanceResponse: std::fmt::Debug + Send + 'static {
    /// How many times [`crate::LocalModel::reset`] had run on this model
    /// instance before the job that produced this response started. Used to
    /// confirm `reset` actually ran after a cancelled/timed-out job, not
    /// just after successful ones.
    fn resets_seen(&self) -> usize;
}

/// Lets the streaming conformance battery
/// ([`run_all_streaming`]) script a [`crate::StreamingLocalModel`]-producing
/// [`crate::LocalModel::Request`] generically, the streaming counterpart of
/// [`ConformanceRequest`].
pub trait ConformanceStreamingRequest: Default + Send + 'static {
    /// Builds a request that, when run via
    /// [`crate::StreamingLocalModel::run_streaming`], pushes `chunk_count`
    /// chunks through the sink one at a time (sleeping `pause_before_each`
    /// before every send — pass [`Duration::ZERO`] for "as fast as the sink
    /// allows"). If `fail_after` is `Some(k)`, returns a model error
    /// immediately after the `k`-th chunk is sent instead of completing
    /// normally; `k` must be `<= chunk_count`.
    fn scripted_stream(
        chunk_count: usize,
        pause_before_each: Duration,
        fail_after: Option<usize>,
    ) -> Self;
}

/// Runs the streaming battery against engines built from `factory` — the
/// streaming counterpart of [`run_all`], covering
/// [`crate::EngineHandle::submit_stream`] specifically: incremental
/// delivery, a dropped consumer, a mid-stream model error, and backpressure.
/// Run this *in addition to*, not instead of, [`run_all`] — a
/// [`crate::StreamingLocalModel`] is still a [`crate::LocalModel`] and
/// [`crate::EngineHandle::submit`] must keep working on it exactly as it
/// does on any other.
///
/// `factory` must build a *fresh* model each time it is called, same as
/// [`run_all`].
///
/// # Panics
///
/// Panics (via `assert!`) with a descriptive message on the first check that
/// fails. This is a test utility, not a library function with recoverable
/// errors — call it from your own `#[tokio::test]`.
pub async fn run_all_streaming<M>(factory: impl Fn() -> M + Clone + Send + 'static)
where
    M: StreamingLocalModel,
    M::Request: ConformanceRequest + ConformanceStreamingRequest,
    M::Response: ConformanceResponse,
    M::Chunk: std::fmt::Debug,
{
    chunks_arrive_incrementally(factory.clone()).await;
    dropped_stream_consumer_stops_promptly_and_engine_stays_available(factory.clone()).await;
    mid_stream_model_error_surfaces(factory.clone()).await;
    backpressure_blocks_a_fast_producer_against_a_slow_consumer(factory).await;
}

/// Runs the full battery against engines built from `factory`.
///
/// `factory` must build a *fresh* model each time it is called (the battery
/// spawns several independent [`EngineHandle`]s, and also relies on it being
/// called again after the panic-recovery case rebuilds the model).
///
/// # Panics
///
/// Panics (via `assert!`) with a descriptive message on the first check that
/// fails. This is a test utility, not a library function with recoverable
/// errors — call it from your own `#[tokio::test]`.
pub async fn run_all<M>(factory: impl Fn() -> M + Clone + Send + 'static)
where
    M: LocalModel,
    M::Request: ConformanceRequest,
    M::Response: ConformanceResponse,
{
    concurrency_is_serialized_and_busy_past_capacity(factory.clone()).await;
    cancel_mid_job_returns_promptly_and_engine_stays_available(factory.clone()).await;
    dropped_caller_does_not_wedge_the_engine(factory.clone()).await;
    panicking_run_yields_engine_down_then_recovers(factory.clone()).await;
    reset_runs_after_a_cancelled_job(factory).await;
}

fn spawn_engine<M>(factory: impl Fn() -> M + Send + 'static, cfg: EngineConfig) -> EngineHandle<M>
where
    M: LocalModel,
{
    EngineHandle::spawn(move || Ok(factory()), cfg)
}

/// N concurrent submissions execute one at a time, and once the bounded
/// queue is full, further submissions get `Busy` immediately.
async fn concurrency_is_serialized_and_busy_past_capacity<M>(
    factory: impl Fn() -> M + Send + 'static,
) where
    M: LocalModel,
    M::Request: ConformanceRequest,
    M::Response: ConformanceResponse,
{
    let per_job = Duration::from_millis(150);
    // A single-slot queue makes "the queue is full" unambiguous: after the
    // first job is dequeued (freeing the slot) and a second is enqueued
    // (filling it again), a third submission must be rejected.
    let engine = Arc::new(spawn_engine(
        factory,
        EngineConfig::new(1, Duration::from_secs(10)),
    ));

    let started = Instant::now();
    let first = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .submit(
                    M::Request::scripted(per_job, false),
                    CancellationToken::new(),
                )
                .await
        })
    };
    // Give the worker a chance to dequeue the first job so the one queue
    // slot is free again before we occupy it with the second.
    tokio::time::sleep(per_job / 4).await;
    let second = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .submit(
                    M::Request::scripted(per_job, false),
                    CancellationToken::new(),
                )
                .await
        })
    };
    // `tokio::spawn` only schedules `second`'s task; it does not run it.
    // Yield the queue-filling job a moment to actually execute its
    // `try_send` before we check that the queue is full.
    tokio::time::sleep(Duration::from_millis(20)).await;
    // The queue slot is occupied by `second`; this one must be rejected
    // outright rather than wait.
    let third = engine
        .submit(
            M::Request::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(third, Err(EngineError::Busy { queue_depth: 1 })),
        "expected Busy{{ queue_depth: 1 }} once the queue is at capacity, got {third:?}"
    );

    let first = first.await.expect("spawned submit task panicked");
    let second = second.await.expect("spawned submit task panicked");
    assert!(
        first.is_ok(),
        "expected the first long job to succeed, got {first:?}"
    );
    assert!(
        second.is_ok(),
        "expected the queued second job to succeed, got {second:?}"
    );

    // Serialization proxy: if both jobs truly ran one at a time, total wall
    // time is close to 2x per_job; if they overlapped, it would be close to
    // a single per_job.
    let elapsed = started.elapsed();
    let serialized_floor = per_job * 3 / 2;
    assert!(
        elapsed >= serialized_floor,
        "jobs appear to have run concurrently instead of one at a time: elapsed {elapsed:?} < expected floor {serialized_floor:?}"
    );
}

/// Cancelling mid-job returns `Cancelled` promptly, and the engine accepts
/// the next job immediately afterward — the regression check for "timeout
/// doesn't actually stop the worker".
async fn cancel_mid_job_returns_promptly_and_engine_stays_available<M>(
    factory: impl Fn() -> M + Send + 'static,
) where
    M: LocalModel,
    M::Request: ConformanceRequest,
    M::Response: ConformanceResponse,
{
    let engine = Arc::new(spawn_engine(
        factory,
        EngineConfig::new(4, Duration::from_secs(10)),
    ));
    let cancel = CancellationToken::new();

    let long_job = {
        let engine = Arc::clone(&engine);
        let cancel = cancel.clone();
        tokio::spawn(async move {
            engine
                .submit(
                    M::Request::scripted(Duration::from_millis(500), false),
                    cancel,
                )
                .await
        })
    };

    tokio::time::sleep(Duration::from_millis(50)).await;
    let cancelled_at = Instant::now();
    cancel.cancel();
    let result = long_job.await.expect("spawned submit task panicked");
    let cancel_latency = cancelled_at.elapsed();

    assert!(
        matches!(result, Err(EngineError::Cancelled)),
        "expected Cancelled, got {result:?}"
    );
    assert!(
        cancel_latency < Duration::from_millis(300),
        "cancellation took too long to take effect ({cancel_latency:?}); the worker likely kept running past cancel()"
    );

    let accept_started = Instant::now();
    let next = engine
        .submit(
            M::Request::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await;
    let accept_latency = accept_started.elapsed();
    assert!(
        next.is_ok(),
        "expected the engine to accept the next job immediately, got {next:?}"
    );
    assert!(
        accept_latency < Duration::from_millis(200),
        "engine did not accept the next job promptly after a cancellation ({accept_latency:?})"
    );
}

/// Dropping the future awaiting `submit` (the caller going away) must not
/// wedge the engine: the next submission still succeeds promptly.
async fn dropped_caller_does_not_wedge_the_engine<M>(factory: impl Fn() -> M + Send + 'static)
where
    M: LocalModel,
    M::Request: ConformanceRequest,
    M::Response: ConformanceResponse,
{
    let engine = Arc::new(spawn_engine(
        factory,
        EngineConfig::new(4, Duration::from_secs(10)),
    ));

    {
        let engine = Arc::clone(&engine);
        let fut = engine.submit(
            M::Request::scripted(Duration::from_millis(300), false),
            CancellationToken::new(),
        );
        // Losing the race drops `fut` — and with it the reply receiver —
        // which is exactly the "caller gone" condition. Whichever way the
        // race goes, we don't care about the outcome here.
        drop(tokio::time::timeout(Duration::from_millis(30), fut).await);
    }

    let accept_started = Instant::now();
    let next = engine
        .submit(
            M::Request::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await;
    let accept_latency = accept_started.elapsed();
    assert!(
        next.is_ok(),
        "expected the engine to remain responsive after a dropped caller, got {next:?}"
    );
    assert!(
        accept_latency < Duration::from_millis(200),
        "engine did not accept the next job promptly after a dropped caller ({accept_latency:?})"
    );
}

/// A panicking `run` yields `EngineDown` for that job, without poisoning the
/// engine: the following submission succeeds once the model is rebuilt.
async fn panicking_run_yields_engine_down_then_recovers<M>(factory: impl Fn() -> M + Send + 'static)
where
    M: LocalModel,
    M::Request: ConformanceRequest,
    M::Response: ConformanceResponse,
{
    let engine = spawn_engine(factory, EngineConfig::new(4, Duration::from_secs(10)));

    let panicked = engine
        .submit(
            M::Request::scripted(Duration::ZERO, true),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(panicked, Err(EngineError::EngineDown { .. })),
        "expected EngineDown after a panic, got {panicked:?}"
    );

    let recovered = engine
        .submit(
            M::Request::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await;
    assert!(
        recovered.is_ok(),
        "expected the engine to recover after rebuilding the model, got {recovered:?}"
    );
}

/// `reset` runs after a cancelled job, not just after successful ones.
async fn reset_runs_after_a_cancelled_job<M>(factory: impl Fn() -> M + Send + 'static)
where
    M: LocalModel,
    M::Request: ConformanceRequest,
    M::Response: ConformanceResponse,
{
    let engine = Arc::new(spawn_engine(
        factory,
        EngineConfig::new(4, Duration::from_secs(10)),
    ));
    let cancel = CancellationToken::new();

    let long_job = {
        let engine = Arc::clone(&engine);
        let cancel = cancel.clone();
        tokio::spawn(async move {
            engine
                .submit(
                    M::Request::scripted(Duration::from_millis(500), false),
                    cancel,
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel.cancel();
    let cancelled = long_job.await.expect("spawned submit task panicked");
    assert!(
        matches!(cancelled, Err(EngineError::Cancelled)),
        "expected Cancelled, got {cancelled:?}"
    );

    let after = engine
        .submit(
            M::Request::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await
        .expect("expected the follow-up job to succeed");
    assert!(
        after.resets_seen() >= 1,
        "expected reset() to have run at least once after the cancelled job, saw {}",
        after.resets_seen()
    );
}

/// Chunks are delivered as they are produced, not buffered until the job
/// completes: the first chunk arrives after roughly one `pause`, and the
/// full drain takes roughly `chunk_count - 1` further pauses — a model that
/// (incorrectly) accumulated everything internally and only sent it all at
/// the very end would make the first chunk arrive just as late as the last.
async fn chunks_arrive_incrementally<M>(factory: impl Fn() -> M + Send + 'static)
where
    M: StreamingLocalModel,
    M::Request: ConformanceStreamingRequest,
    M::Chunk: std::fmt::Debug,
{
    let engine = spawn_engine(factory, EngineConfig::new(4, Duration::from_secs(10)));
    let pause = Duration::from_millis(40);
    let chunk_count = 4_usize;
    let mut stream = engine.submit_stream(
        M::Request::scripted_stream(chunk_count, pause, None),
        CancellationToken::new(),
        8,
    );

    let started = Instant::now();
    let first = stream
        .recv()
        .await
        .expect("stream ended before its first chunk");
    let first_latency = started.elapsed();
    assert!(first.is_ok(), "expected a chunk first, got {first:?}");
    assert!(
        first_latency < pause * 3,
        "first chunk took {first_latency:?} to arrive; expected close to one pause ({pause:?}), \
         not something proportional to all {chunk_count} chunks"
    );

    let mut received = 1_usize;
    while let Some(item) = stream.recv().await {
        assert!(item.is_ok(), "expected only chunks, got {item:?}");
        received += 1;
    }
    assert_eq!(
        received, chunk_count,
        "expected exactly {chunk_count} chunks"
    );

    let total = started.elapsed();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "chunk_count is a small test-scripted constant, never near u32::MAX"
    )]
    let remaining_pauses = (chunk_count - 1) as u32;
    let floor = pause * remaining_pauses;
    assert!(
        total >= floor,
        "draining all {chunk_count} chunks took only {total:?}, expected at least {floor:?} if \
         the model genuinely paced them one at a time instead of producing them all upfront"
    );
}

/// Dropping the stream (the consumer going away) stops the worker promptly
/// via the same `JobContext::should_stop` → `StopReason::CallerGone` path
/// `EngineHandle::submit`'s dropped-caller case uses, and the engine accepts
/// its next job immediately afterward instead of staying wedged on a
/// streaming job nobody is listening to anymore.
async fn dropped_stream_consumer_stops_promptly_and_engine_stays_available<M>(
    factory: impl Fn() -> M + Send + 'static,
) where
    M: StreamingLocalModel,
    M::Request: ConformanceRequest + ConformanceStreamingRequest,
    M::Response: ConformanceResponse,
    M::Chunk: std::fmt::Debug,
{
    let engine = spawn_engine(factory, EngineConfig::new(4, Duration::from_secs(10)));
    let pause = Duration::from_millis(50);

    {
        // Many more chunks than we intend to read: if dropping the stream
        // did not stop the worker, it would keep producing (and blocking on
        // a full, now-abandoned sink) for a long time after this scope ends.
        let mut stream = engine.submit_stream(
            M::Request::scripted_stream(200, pause, None),
            CancellationToken::new(),
            1,
        );
        let first = stream.recv().await;
        assert!(
            first.is_some_and(|item| item.is_ok()),
            "expected at least one chunk before dropping the stream"
        );
    } // `stream` dropped here: its `DropGuard` cancels `caller_gone`.

    let accept_started = Instant::now();
    let next = engine
        .submit(
            M::Request::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await;
    let accept_latency = accept_started.elapsed();
    assert!(
        next.is_ok(),
        "expected the engine to accept the next job after a dropped stream consumer, got {next:?}"
    );
    assert!(
        accept_latency < pause * 3,
        "engine did not accept the next job promptly after a dropped stream consumer \
         ({accept_latency:?}); the streaming job likely kept running past the drop"
    );
}

/// A model error partway through a stream surfaces as a terminal `Err` after
/// the chunks that were already sent, rather than the stream silently
/// truncating with no explanation.
async fn mid_stream_model_error_surfaces<M>(factory: impl Fn() -> M + Send + 'static)
where
    M: StreamingLocalModel,
    M::Request: ConformanceStreamingRequest,
    M::Chunk: std::fmt::Debug,
{
    let engine = spawn_engine(factory, EngineConfig::new(4, Duration::from_secs(10)));
    let mut stream = engine.submit_stream(
        M::Request::scripted_stream(5, Duration::ZERO, Some(3)),
        CancellationToken::new(),
        8,
    );

    let mut ok_count = 0_usize;
    let mut saw_terminal_error = false;
    while let Some(item) = stream.recv().await {
        match item {
            Ok(_chunk) => {
                assert!(
                    !saw_terminal_error,
                    "received a chunk after the stream's terminal error"
                );
                ok_count += 1;
            }
            Err(err) => {
                assert!(
                    matches!(err, EngineError::Model(_)),
                    "expected a mapped model error after the scripted mid-stream failure, got {err:?}"
                );
                saw_terminal_error = true;
            }
        }
    }
    assert_eq!(
        ok_count, 3,
        "expected exactly 3 chunks before the scripted failure, got {ok_count}"
    );
    assert!(
        saw_terminal_error,
        "expected a terminal model error after 3 chunks, the stream ended without one"
    );
}

/// A full sink genuinely blocks the worker thread rather than buffering
/// unboundedly: with a `chunk_buffer` of 1 and a producer scripted to send
/// as fast as the sink allows, a job that is never drained cannot finish —
/// it blocks inside its second `send` call until
/// [`crate::EngineConfig::job_timeout`] fires — which occupies the engine's
/// one worker thread for roughly that whole deadline, provably delaying a
/// job queued behind it.
async fn backpressure_blocks_a_fast_producer_against_a_slow_consumer<M>(
    factory: impl Fn() -> M + Send + 'static,
) where
    M: StreamingLocalModel,
    M::Request: ConformanceRequest + ConformanceStreamingRequest,
    M::Response: ConformanceResponse,
    M::Chunk: std::fmt::Debug,
{
    let job_timeout = Duration::from_millis(200);
    let engine = spawn_engine(factory, EngineConfig::new(4, job_timeout));

    // Never drained: the assertions below run before this is read from or
    // dropped, so the only way the streaming job can end on its own is its
    // own deadline — which it can only reach if `send` actually blocks.
    let stream = engine.submit_stream(
        M::Request::scripted_stream(20, Duration::ZERO, None),
        CancellationToken::new(),
        1,
    );

    let started = Instant::now();
    let queued = engine
        .submit(
            M::Request::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await;
    let elapsed = started.elapsed();

    assert!(
        queued.is_ok(),
        "expected the job queued behind the streaming one to eventually succeed, got {queued:?}"
    );
    let floor = job_timeout * 3 / 4;
    assert!(
        elapsed >= floor,
        "the queued follow-up job ran after only {elapsed:?}; expected it to wait out roughly the \
         streaming job's {job_timeout:?} deadline, which only happens if the sink genuinely \
         blocked the producer instead of buffering every chunk unboundedly"
    );

    drop(stream);
}

// ---------------------------------------------------------------------
// A trivial in-crate mock model, used both to self-test `run_all` above
// and by this crate's own unit tests (see `src/tests.rs`). `#[cfg(test)]`
// because nothing outside test builds ever constructs one — downstream
// crates bring their own mock for their own migrated engine.
// ---------------------------------------------------------------------

/// A scripted request for [`MockModel`].
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub(crate) struct MockRequest {
    run_for: Duration,
    then_panic: bool,
    /// If set, `run` never calls [`crate::JobContext::tick`] — a stand-in
    /// for a model that stops honoring the `LocalModel` contract, used to
    /// exercise stall detection. Not part of [`ConformanceRequest`]: real
    /// engines are expected to always tick, so this is a `src/tests.rs`-only
    /// escape hatch, not something the generic battery scripts.
    no_tick: bool,
    /// `run_streaming`-only scripting (see [`ConformanceStreamingRequest`]),
    /// inert (`stream_chunks: 0`) for every [`ConformanceRequest::scripted`]
    /// request — those are only ever run via [`LocalModel::run`], which
    /// never reads these fields.
    stream_chunks: usize,
    stream_pause: Duration,
    stream_fail_after: Option<usize>,
}

#[cfg(test)]
impl MockRequest {
    /// Builds a request that runs for `run_for` without ever calling
    /// [`crate::JobContext::tick`], simulating a stalled model.
    pub(crate) fn stalled(run_for: Duration) -> Self {
        Self {
            run_for,
            then_panic: false,
            no_tick: true,
            stream_chunks: 0,
            stream_pause: Duration::ZERO,
            stream_fail_after: None,
        }
    }
}

#[cfg(test)]
impl ConformanceRequest for MockRequest {
    fn scripted(run_for: Duration, then_panic: bool) -> Self {
        Self {
            run_for,
            then_panic,
            no_tick: false,
            stream_chunks: 0,
            stream_pause: Duration::ZERO,
            stream_fail_after: None,
        }
    }
}

#[cfg(test)]
impl ConformanceStreamingRequest for MockRequest {
    fn scripted_stream(
        chunk_count: usize,
        pause_before_each: Duration,
        fail_after: Option<usize>,
    ) -> Self {
        Self {
            run_for: Duration::ZERO,
            then_panic: false,
            no_tick: false,
            stream_chunks: chunk_count,
            stream_pause: pause_before_each,
            stream_fail_after: fail_after,
        }
    }
}

/// [`MockModel`]'s response, reporting the model's own reset counter.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct MockResponse {
    pub(crate) resets_seen: usize,
}

#[cfg(test)]
impl ConformanceResponse for MockResponse {
    fn resets_seen(&self) -> usize {
        self.resets_seen
    }
}

/// [`MockModel`]'s error, returned when a job stops cooperatively.
#[cfg(test)]
#[derive(Debug, thiserror::Error)]
#[error("mock model stopped cooperatively")]
pub(crate) struct MockError;

/// A trivial [`LocalModel`] used for this crate's own tests: it busy-waits
/// for `run_for`, checking [`crate::JobContext::should_stop`] and calling
/// [`crate::JobContext::tick`] every couple of milliseconds, optionally panicking
/// instead, and counts how many times [`LocalModel::reset`] has run.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct MockModel {
    resets_seen: usize,
}

#[cfg(test)]
impl MockModel {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
impl LocalModel for MockModel {
    type Request = MockRequest;
    type Response = MockResponse;
    type Error = MockError;

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
    )]
    fn engine_name(&self) -> &str {
        "conformance-mock"
    }

    fn run(&mut self, req: Self::Request, ctx: &JobContext) -> Result<Self::Response, Self::Error> {
        assert!(
            !req.then_panic,
            "scripted mock panic for conformance testing"
        );
        let start = Instant::now();
        loop {
            if ctx.should_stop().is_some() {
                return Err(MockError);
            }
            if !req.no_tick {
                ctx.tick();
            }
            if start.elapsed() >= req.run_for {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(MockResponse {
            resets_seen: self.resets_seen,
        })
    }

    fn reset(&mut self) {
        self.resets_seen += 1;
    }
}

/// [`MockModel`]'s streaming counterpart, scripted by
/// [`MockRequest`]'s `stream_chunks`/`stream_pause`/`stream_fail_after`
/// fields (see [`ConformanceStreamingRequest::scripted_stream`]). Chunks are
/// just the 0-based index of each one, sent in order.
#[cfg(test)]
impl StreamingLocalModel for MockModel {
    type Chunk = usize;

    fn run_streaming(
        &mut self,
        req: Self::Request,
        ctx: &JobContext,
        sink: &ChunkSink<Self::Chunk, Self::Error>,
    ) -> Result<(), Self::Error> {
        for i in 0..req.stream_chunks {
            if ctx.should_stop().is_some() {
                return Err(MockError);
            }
            ctx.tick();
            if !req.stream_pause.is_zero() {
                std::thread::sleep(req.stream_pause);
            }
            if sink.send(i, ctx).is_err() {
                // `should_stop` fired (or the channel closed) while trying
                // to deliver this chunk — stop exactly like a real model
                // would on any other cooperative stop signal.
                return Err(MockError);
            }
            if req.stream_fail_after == Some(i + 1) {
                return Err(MockError);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod self_test {
    use super::{MockModel, run_all, run_all_streaming};

    #[tokio::test]
    async fn run_all_passes_against_the_bundled_mock() {
        run_all(MockModel::new).await;
    }

    #[tokio::test]
    async fn run_all_streaming_passes_against_the_bundled_mock() {
        run_all_streaming(MockModel::new).await;
    }
}
