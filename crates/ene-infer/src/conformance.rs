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
use crate::model::LocalModel;

/// Lets the conformance battery script a [`crate::LocalModel::Request`]
/// generically: how long to run, and whether to panic partway through.
pub trait ConformanceRequest: Default + Send + 'static {
    /// Builds a request that, when run, checks
    /// [`JobContext::should_stop`]/calls [`JobContext::tick`] in a loop for
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
}

#[cfg(test)]
impl ConformanceRequest for MockRequest {
    fn scripted(run_for: Duration, then_panic: bool) -> Self {
        Self {
            run_for,
            then_panic,
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
/// for `run_for`, checking [`JobContext::should_stop`] and calling
/// [`JobContext::tick`] every couple of milliseconds, optionally panicking
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
            ctx.tick();
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

#[cfg(test)]
mod self_test {
    use super::{MockModel, run_all};

    #[tokio::test]
    async fn run_all_passes_against_the_bundled_mock() {
        run_all(MockModel::new).await;
    }
}
