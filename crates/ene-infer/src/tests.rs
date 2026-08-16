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

/// Pins that the actual panic message survives into `EngineDown`'s `reason`:
/// the reply channel is not shared (see `Reply<M>`/`respond` in `handle.rs`),
/// so the worker's panic arm always delivers the real message instead of the
/// generic "worker thread is not running" fallback.
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
    // Default escalation factor (3): confirm_threshold = 30ms × 3 = 90ms.
    // The stalled job runs for 300ms without ticking, well past the
    // confirmation threshold, so the engine is permanently disabled.
    let cfg =
        EngineConfig::new(4, Duration::from_secs(5)).with_stall_timeout(Duration::from_millis(30));
    let engine = spawn_mock(cfg);

    // This job never calls `tick()`, so the watchdog should notice the
    // stall and mark the engine down while the job is still running (it
    // takes 300ms; the watchdog confirms at ~90ms, polling every ~7.5ms).
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
        "expected EngineDown once a stall was confirmed, got {after:?} (Busy would mean a wedged \
         worker fills the queue forever with no way for callers to tell it apart from transient \
         saturation)"
    );
}

/// A job that stops ticking for longer than `stall_timeout` but completes
/// before the escalation threshold (`stall_timeout × escalation_factor`)
/// is a false positive: the engine must remain fully usable afterward.
#[tokio::test]
async fn false_positive_stall_does_not_permanently_disable_engine() {
    // stall_timeout=30ms, escalation_factor=3 → confirm at 90ms.
    // The job runs for 60ms without ticking: long enough to trigger a
    // *suspected* stall (>30ms) but short enough to complete before
    // confirmation (<90ms).
    let cfg = EngineConfig::new(4, Duration::from_secs(5))
        .with_stall_timeout(Duration::from_millis(30))
        .with_stall_escalation_factor(3);
    let engine = spawn_mock(cfg);

    let result = engine
        .submit(
            MockRequest::stalled(Duration::from_millis(60)),
            CancellationToken::new(),
        )
        .await;
    assert!(
        result.is_ok(),
        "expected the false-positive stall job to still succeed, got {result:?}"
    );

    // Give the watchdog time to observe the worker going idle and clear
    // its suspicion.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let next = engine
        .submit(
            MockRequest::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await;
    assert!(
        next.is_ok(),
        "expected the engine to remain usable after a false-positive stall, got {next:?}"
    );
}

/// With `escalation_factor=1`, the first detection is immediately final: a
/// stalled job disables the engine as soon as `stall_timeout` elapses.
#[tokio::test]
async fn escalation_factor_one_confirms_immediately() {
    let cfg = EngineConfig::new(4, Duration::from_secs(5))
        .with_stall_timeout(Duration::from_millis(30))
        .with_stall_escalation_factor(1);
    let engine = spawn_mock(cfg);

    engine
        .submit(
            MockRequest::stalled(Duration::from_millis(300)),
            CancellationToken::new(),
        )
        .await
        .ok();

    tokio::time::sleep(Duration::from_millis(80)).await;

    let after = engine
        .submit(
            MockRequest::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(after, Err(EngineError::EngineDown { .. })),
        "expected EngineDown with escalation_factor=1, got {after:?}"
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
async fn try_spawn_reuses_the_eagerly_built_instance() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_in_factory = Arc::clone(&calls);
    let engine = EngineHandle::try_spawn(
        move || {
            calls_in_factory.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(MockModel::new())
        },
        EngineConfig::new(4, Duration::from_secs(5)),
    )
    .expect("factory succeeds synchronously");

    let result = engine
        .submit(
            MockRequest::scripted(Duration::ZERO, false),
            CancellationToken::new(),
        )
        .await
        .expect("expected the first job to succeed");
    assert_eq!(result.resets_seen(), 0);

    // Exactly one call: the eager, synchronous one inside `try_spawn` itself.
    // If the worker thread called `factory` again for its first job (as
    // `spawn` always does), this would be 2.
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "expected try_spawn's eagerly-built instance to be reused for the first job, not rebuilt"
    );
}

#[test]
fn try_spawn_surfaces_a_synchronous_construction_failure() {
    let result: Result<EngineHandle<MockModel>, MockError> =
        EngineHandle::try_spawn(|| Err(MockError), EngineConfig::default());
    assert!(
        matches!(result, Err(MockError)),
        "expected try_spawn to fail synchronously with the factory's own error"
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
