//! Ordinary unit tests against the trivial in-crate mock model.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::conformance::{
    ConformanceRequest, ConformanceResponse, MockError, MockModel, MockRequest,
};
use crate::{EngineConfig, EngineError, EngineHandle};

fn spawn_mock(cfg: EngineConfig) -> EngineHandle<MockModel> {
    EngineHandle::spawn(|| Ok(MockModel::new()), cfg)
}

#[test]
fn engine_config_default_is_sane() {
    let cfg = EngineConfig::default();
    assert!(cfg.queue_depth >= 1);
    assert!(cfg.job_timeout > Duration::ZERO);
    assert!(cfg.stall_timeout.is_none());
}

#[test]
fn engine_config_new_coerces_zero_queue_depth_to_one() {
    let cfg = EngineConfig::new(0, Duration::from_secs(1));
    assert_eq!(cfg.queue_depth, 1);
}

#[test]
fn is_retryable_matches_the_documented_matrix() {
    assert!(EngineError::<MockError>::Busy { queue_depth: 1 }.is_retryable());
    assert!(
        EngineError::<MockError>::Timeout {
            after: Duration::ZERO
        }
        .is_retryable()
    );
    assert!(!EngineError::<MockError>::Cancelled.is_retryable());
    assert!(
        !EngineError::<MockError>::EngineDown {
            reason: "down".to_string()
        }
        .is_retryable()
    );
}

#[tokio::test]
async fn submit_success_roundtrips_the_response() {
    let engine = spawn_mock(EngineConfig::new(4, Duration::from_secs(5)));
    let response = engine
        .submit(
            MockRequest::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await
        .expect("expected a quick job to succeed");
    assert_eq!(response.resets_seen(), 0);
}

#[tokio::test]
async fn full_queue_returns_busy_without_blocking() {
    let engine = Arc::new(spawn_mock(EngineConfig::new(1, Duration::from_secs(5))));
    let per_job = Duration::from_millis(150);

    let first = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .submit(
                    MockRequest::scripted(per_job, false),
                    CancellationToken::new(),
                )
                .await
        })
    };
    // Let the worker dequeue `first` so the single queue slot is free again.
    tokio::time::sleep(per_job / 4).await;
    let second = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move {
            engine
                .submit(
                    MockRequest::scripted(per_job, false),
                    CancellationToken::new(),
                )
                .await
        })
    };
    // `tokio::spawn` only schedules `second`'s task; it does not run it.
    // Yield a moment so it actually executes its `try_send` before we check
    // that the queue is full.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // The one queue slot is now occupied by `second`; this must fail fast.
    let third = engine
        .submit(
            MockRequest::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(third, Err(EngineError::Busy { queue_depth: 1 })),
        "expected Busy, got {third:?}"
    );

    let first = first.await.expect("spawned task panicked");
    let second = second.await.expect("spawned task panicked");
    assert!(
        first.is_ok(),
        "expected the first job to succeed, got {first:?}"
    );
    assert!(
        second.is_ok(),
        "expected the queued second job to succeed, got {second:?}"
    );
}

#[tokio::test]
async fn job_timeout_reports_timeout_not_a_hang() {
    let engine = spawn_mock(EngineConfig::new(4, Duration::from_millis(50)));
    let result = engine
        .submit(
            MockRequest::scripted(Duration::from_secs(5), false),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(result, Err(EngineError::Timeout { .. })),
        "expected Timeout, got {result:?}"
    );
}

#[tokio::test]
async fn cancellation_returns_cancelled() {
    let engine = Arc::new(spawn_mock(EngineConfig::new(4, Duration::from_secs(5))));
    let cancel = CancellationToken::new();
    let job = {
        let engine = Arc::clone(&engine);
        let cancel = cancel.clone();
        tokio::spawn(async move {
            engine
                .submit(
                    MockRequest::scripted(Duration::from_millis(500), false),
                    cancel,
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    cancel.cancel();
    let result = job.await.expect("spawned task panicked");
    assert!(
        matches!(result, Err(EngineError::Cancelled)),
        "expected Cancelled, got {result:?}"
    );

    // The engine must stay usable after a cancellation on the same handle.
    let next = engine
        .submit(
            MockRequest::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await;
    assert!(
        next.is_ok(),
        "expected the engine to accept another job after a cancellation, got {next:?}"
    );
}

#[tokio::test]
async fn panic_in_run_yields_engine_down_and_engine_recovers() {
    let engine = spawn_mock(EngineConfig::new(4, Duration::from_secs(5)));
    let panicked = engine
        .submit(
            MockRequest::scripted(Duration::ZERO, true),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(panicked, Err(EngineError::EngineDown { .. })),
        "expected EngineDown, got {panicked:?}"
    );

    let recovered = engine
        .submit(
            MockRequest::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await;
    assert!(
        recovered.is_ok(),
        "expected the engine to recover, got {recovered:?}"
    );
}

/// Regression test: earlier, `respond` reclaimed the reply sender via
/// `Arc::try_unwrap`, which only succeeded if `JobContext` (holding a second
/// `Arc` clone) had already been dropped. The panic arm of `worker_loop`
/// didn't drop it first, so `try_unwrap` failed, the real panic message was
/// silently discarded, and callers saw the generic
/// "worker thread is not running" fallback instead. The reply channel is no
/// longer shared (see `Reply<M>`/`respond` in `handle.rs`), which makes this
/// unrepresentable — this test pins the panic message actually surviving
/// into `EngineDown`.
#[tokio::test]
async fn panic_reason_survives_into_engine_down() {
    let engine = spawn_mock(EngineConfig::new(4, Duration::from_secs(5)));
    let panicked = engine
        .submit(
            MockRequest::scripted(Duration::ZERO, true),
            CancellationToken::new(),
        )
        .await;
    let Err(EngineError::EngineDown { reason }) = panicked else {
        panic!("expected EngineDown, got {panicked:?}");
    };
    assert!(
        reason.contains("scripted mock panic for conformance testing"),
        "expected the actual panic message to survive into EngineDown's reason, got: {reason}"
    );
    assert!(
        !reason.contains("not running"),
        "expected the real panic reason, not the generic worker-down fallback, got: {reason}"
    );
}

#[tokio::test]
async fn stalled_worker_reports_engine_down_not_busy_forever() {
    let cfg =
        EngineConfig::new(4, Duration::from_secs(5)).with_stall_timeout(Duration::from_millis(30));
    let engine = spawn_mock(cfg);

    // This job never calls `tick()`, so the watchdog should notice the
    // stall and mark the engine down while the job is still running (it
    // takes 300ms; the watchdog polls roughly every 7.5ms).
    engine
        .submit(
            MockRequest::stalled(Duration::from_millis(300)),
            CancellationToken::new(),
        )
        .await
        .ok();

    // Give the watchdog a little extra margin past its poll interval.
    tokio::time::sleep(Duration::from_millis(80)).await;

    let after = engine
        .submit(
            MockRequest::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(after, Err(EngineError::EngineDown { .. })),
        "expected EngineDown once a stall was detected, got {after:?} (Busy would mean a wedged \
         worker fills the queue forever with no way for callers to tell it apart from transient \
         saturation)"
    );
}

#[tokio::test]
async fn reset_runs_between_jobs() {
    let engine = spawn_mock(EngineConfig::new(4, Duration::from_secs(5)));
    let first = engine
        .submit(
            MockRequest::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await
        .expect("expected the first job to succeed");
    assert_eq!(first.resets_seen(), 0);

    let second = engine
        .submit(
            MockRequest::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await
        .expect("expected the second job to succeed");
    assert_eq!(
        second.resets_seen(),
        1,
        "expected reset() to have run exactly once between the two jobs"
    );
}

#[tokio::test]
async fn stall_timeout_is_purely_diagnostic_and_does_not_break_jobs() {
    // The stall watchdog only logs; it must never affect what a job
    // returns, even when the watchdog's own poll window elapses mid-job.
    let cfg =
        EngineConfig::new(4, Duration::from_secs(5)).with_stall_timeout(Duration::from_millis(20));
    let engine = spawn_mock(cfg);
    let result = engine
        .submit(
            MockRequest::scripted(Duration::from_millis(120), false),
            CancellationToken::new(),
        )
        .await;
    assert!(
        result.is_ok(),
        "expected a stall-monitored job to still succeed, got {result:?}"
    );
}
