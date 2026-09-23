//! Real listener shutdown regressions. Notifications establish ordering;
//! timeouts only bound a broken test's wait.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "fixture helpers must fail the regression rather than silently skip it"
)]

use std::future::{Future, poll_fn};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Weak};
use std::task::Poll;
use std::time::Duration;

use ene_credential::MemoryCredentialStore;
use ene_inference::fake::FakeProviderTransport;
use ene_preservation::{
    DeletionOperationPhase, DeletionPurpose, DeletionSearchMaterial, MechanicalDeletionTarget,
    PreservationRepository as _, StageTargetedDeletionRequestCommand,
    StageTargetedDeletionRequestOutcome, TargetedDeletionTarget,
};
use ene_primitive::WallClockWithTz;
use ene_store::Store;
use tokio::io::AsyncReadExt as _;
use tokio::sync::Notify;
use tokio::sync::watch;

use crate::serve::{CoreError, CredStore, HostHandle, TestGate};

const HANG_GUARD: Duration = Duration::from_secs(20);

#[derive(Default)]
pub(crate) struct ServingTest {
    pub(crate) ready: Notify,
    pub(crate) fail: Notify,
    pub(crate) shutdown_started: Notify,
    pub(crate) device_started: Notify,
    pub(crate) device_finished: Notify,
    pub(crate) park_learning: AtomicBool,
    pub(crate) learning_entered: Notify,
    pub(crate) learning_release: Notify,
}

/// Keeping the serving future on this task makes a Pending assertion a real
/// poll of cleanup, rather than an observation of another task's scheduling.
struct Serving {
    run: Pin<Box<dyn Future<Output = Result<(), CoreError>> + Send>>,
    stop: watch::Sender<bool>,
    seam: Arc<ServingTest>,
    predecessor: Weak<HostHandle>,
    dir: PathBuf,
}

impl Serving {
    async fn start(dir: &Path, handle: Arc<HostHandle>) -> Self {
        let (stop, shutdown) = watch::channel(false);
        let mut serving = Self {
            seam: Arc::clone(&handle.serving_test),
            predecessor: Arc::downgrade(&handle),
            run: Box::pin(super::run_until_shutdown(
                dir.to_path_buf(),
                handle,
                Arc::new(FakeProviderTransport::new(String::new(), None)),
                shutdown,
            )),
            stop,
            dir: dir.to_path_buf(),
        };
        let seam = Arc::clone(&serving.seam);
        serving.until(seam.ready.notified()).await;
        serving
    }

    async fn until<F: Future>(&mut self, checkpoint: F) -> F::Output {
        tokio::time::timeout(HANG_GUARD, async {
            tokio::select! {
                biased;
                outcome = &mut self.run => panic!("serving ended before checkpoint: {outcome:?}"),
                output = checkpoint => output,
            }
        })
        .await
        .expect("serving must reach the synchronized checkpoint")
    }

    async fn begin_shutdown(&mut self, fail: bool) {
        if fail {
            self.seam.fail.notify_one();
        } else {
            self.stop
                .send(true)
                .expect("serving owns the shutdown receiver");
        }
        let seam = Arc::clone(&self.seam);
        self.until(seam.shutdown_started.notified()).await;
    }

    async fn assert_pending(&mut self) {
        let outcome = poll_fn(|cx| Poll::Ready(self.run.as_mut().poll(cx))).await;
        assert!(
            outcome.is_pending(),
            "cleanup must await the parked owner: {outcome:?}"
        );
        assert!(self.predecessor.upgrade().is_some());
    }

    async fn finish(mut self, fail: bool) -> (PathBuf, Weak<HostHandle>) {
        let handle = self
            .predecessor
            .upgrade()
            .expect("serving still owns its Host");
        let outcome = tokio::time::timeout(HANG_GUARD, &mut self.run)
            .await
            .expect("serving cleanup must finish after release");
        if fail {
            assert!(
                matches!(outcome, Err(CoreError::Bind(ref message)) if message == "injected serving-loop failure"),
                "cleanup must preserve the original accept error: {outcome:?}"
            );
            assert!(
                !*self.stop.borrow(),
                "error cleanup cannot rely on external shutdown"
            );
        } else {
            outcome.expect("graceful shutdown succeeds");
        }
        drop(self.run);
        assert_eq!(handle.live_targeted_deletion_drivers_for_tests(), 0);
        assert_eq!(handle.store.live_deletion_blocking_sections_for_tests(), 0);
        drop(handle);
        assert!(
            self.predecessor.upgrade().is_none(),
            "no serving child may retain the predecessor after cleanup"
        );
        (self.dir, self.predecessor)
    }
}

#[cfg(unix)]
async fn connect_device(dir: &Path) -> tokio::net::UnixStream {
    tokio::net::UnixStream::connect(super::socket_path(dir))
        .await
        .expect("the ready device listener accepts a connection")
}

#[cfg(windows)]
async fn connect_device(dir: &Path) -> tokio::net::windows::named_pipe::NamedPipeClient {
    tokio::net::windows::named_pipe::ClientOptions::new()
        .open(ene_plugin_ipc::pipe_name(dir))
        .expect("the ready device listener accepts a connection")
}

async fn assert_successor(dir: &Path, predecessor: &Weak<HostHandle>) {
    assert!(
        predecessor.upgrade().is_none(),
        "check before opening successor"
    );
    let handle =
        HostHandle::open_with_cred_store(dir, CredStore::Memory(MemoryCredentialStore::new()))
            .await
            .expect("successor can acquire the predecessor's data directory");
    handle.store.relax_durability_for_tests().await.unwrap();
    handle
        .run_startup_mutations()
        .await
        .expect("successor startup completes");
    let successor = Serving::start(dir, Arc::new(handle)).await;
    successor.stop.send(true).unwrap();
    successor.finish(false).await;
}

async fn stage_request(store: &Store) -> String {
    let staged = store
        .stage_targeted_deletion(StageTargetedDeletionRequestCommand::new(
            TargetedDeletionTarget {
                mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                    "shutdown-regression-target".into(),
                )),
                semantic_hints: Vec::new(),
            },
            DeletionPurpose::Privacy,
            WallClockWithTz::now(),
        ))
        .await
        .expect("fixture request stages");
    let StageTargetedDeletionRequestOutcome::Staged(request) = staged else {
        panic!("fresh request must stage: {staged:?}");
    };
    request.as_raw().as_uuid().to_string()
}

async fn control_and_device_cleanup(fail: bool) {
    let (handle, dir) = crate::test_support::memory_handle("shutdown-control")
        .await
        .expect("fixture Host opens");
    let store = handle.store.clone();
    let request = stage_request(&store).await;
    let gate = Arc::new(TestGate::default());
    *crate::lock_unpoison(&handle.host_control_confirm_gate) = Some(Arc::clone(&gate));
    let handle = Arc::new(handle);
    let mut serving = Serving::start(dir.path(), Arc::clone(&handle)).await;
    let seam = Arc::clone(&serving.seam);
    let mut device = connect_device(dir.path()).await;
    serving.until(seam.device_started.notified()).await;
    // The Owner's confirmation surface is the GUI the Host spawned; the
    // console asks, and this surface confirms on its private channel.
    let mut gui = crate::host_control::seat_test_gui_for_tests(&handle)
        .expect("the private confirmation channel must open");
    // The fixture must not retain the Host itself: `finish` asserts that no
    // serving child survives cleanup, and a local handle would be one.
    drop(handle);
    let (confirmed_tx, confirmed_rx) = tokio::sync::oneshot::channel();
    // The surface keeps its end open until the test drops it: closing the
    // channel ends the seat, and this fixture must hold it through the parked
    // confirmation.
    std::thread::spawn(move || {
        let mut reader = gui.try_clone().expect("clone");
        if let Ok(Some(ene_local_control::FromConfirmation::ConfirmationChallenge {
            session_id,
            nonce,
            ..
        })) = reader.recv()
        {
            match gui
                .send(&ene_local_control::ToConfirmation::SessionComplete { session_id, nonce })
            {
                Ok(()) | Err(_) => {}
            }
        }
        match confirmed_tx.send(()) {
            Ok(()) | Err(_) => {}
        }
        // Hold the channel open for the duration of the test.
        std::thread::sleep(Duration::from_secs(30));
    });
    let control_dir = dir.path().to_path_buf();
    let console = tokio::spawn(async move {
        crate::host_control::confirm_targeted_deletion(&control_dir, &request).await
    });
    // The serving future is driven by `until`: waiting outside it would leave
    // the accept loop unpolled and the console's request unanswered.
    serving
        .until(async move {
            tokio::time::timeout(HANG_GUARD, confirmed_rx)
                .await
                .expect("the confirmation surface must answer the challenge")
                .expect("the surface task must not drop its signal")
        })
        .await;
    serving.until(gate.wait_entered()).await;
    assert!(store.deletion_status(None, 10).await.unwrap().is_empty());

    serving.begin_shutdown(fail).await;
    serving.assert_pending().await;
    gate.release();
    let (path, predecessor) = serving.finish(fail).await;
    tokio::time::timeout(HANG_GUARD, seam.device_finished.notified())
        .await
        .expect("the active device handler was joined");
    let mut byte = [0_u8; 1];
    let read = tokio::time::timeout(HANG_GUARD, device.read(&mut byte))
        .await
        .expect("the device transport closes");
    // Windows signals named-pipe closure as BrokenPipe; Unix returns EOF.
    assert!(
        matches!(read, Ok(0)) || read.is_err(),
        "device remains open: {read:?}"
    );
    let _console_outcome = tokio::time::timeout(HANG_GUARD, console)
        .await
        .expect("control client observes closure")
        .expect("control client task does not panic");
    let records = store.deletion_status(None, 10).await.unwrap();
    assert_eq!(
        records.len(),
        1,
        "admitted confirmation must execute after release"
    );
    assert_eq!(records[0].phase, DeletionOperationPhase::Completed);
    assert_eq!(store.live_deletion_blocking_sections_for_tests(), 0);
    drop(store);
    assert_successor(&path, &predecessor).await;
}

#[tokio::test]
async fn shutdown_joins_parked_confirmation_and_active_device_before_successor() {
    control_and_device_cleanup(false).await;
}

#[tokio::test]
async fn serving_error_drains_children_without_external_shutdown_and_preserves_error() {
    control_and_device_cleanup(true).await;
}

/// Release on panic too: Tokio cannot abort a parked spawn_blocking closure.
struct ReleaseDeletionPark(Store);

impl Drop for ReleaseDeletionPark {
    fn drop(&mut self) {
        self.0.release_deletion_blocking_park_for_tests();
    }
}

async fn deletion_store_and_device_cleanup(fail: bool) {
    let (handle, dir) = crate::test_support::memory_handle("shutdown-blocking")
        .await
        .expect("fixture Host opens");
    let store = handle.store.clone();
    store.arm_deletion_blocking_park_for_tests();
    let release = ReleaseDeletionPark(store.clone());
    handle.deletion_driver_wake.notify_one();
    let mut serving = Serving::start(dir.path(), Arc::new(handle)).await;
    serving
        .until(store.wait_deletion_blocking_park_for_tests())
        .await;
    assert!(store.live_deletion_blocking_sections_for_tests() > 0);
    let seam = Arc::clone(&serving.seam);
    let device = connect_device(dir.path()).await;
    serving.until(seam.device_started.notified()).await;
    serving.begin_shutdown(fail).await;
    serving.assert_pending().await;
    assert!(store.live_deletion_blocking_sections_for_tests() > 0);
    drop(release);
    let (path, predecessor) = serving.finish(fail).await;
    assert_eq!(store.live_deletion_blocking_sections_for_tests(), 0);
    drop(device);
    drop(store);
    assert_successor(&path, &predecessor).await;
}

#[tokio::test]
async fn shutdown_joins_started_deletion_store_work_and_active_device_before_successor() {
    deletion_store_and_device_cleanup(false).await;
}

#[tokio::test]
async fn serving_error_joins_started_deletion_store_work_and_active_device_before_successor() {
    deletion_store_and_device_cleanup(true).await;
}

#[tokio::test(flavor = "current_thread")]
async fn learning_failure_waits_for_started_sibling_store_work() {
    let (handle, _dir) = crate::test_support::memory_handle("learning-failure-drain")
        .await
        .expect("fixture Host opens");
    let store = handle.store.clone();

    for failure_already_observed in [false, true] {
        store.arm_deletion_blocking_park_for_tests();
        let release = ReleaseDeletionPark(store.clone());
        let mut tasks = tokio::task::JoinSet::new();
        let (panicking, entered) = tokio::sync::oneshot::channel();
        let failed_id = tasks
            .spawn(async move {
                panicking
                    .send(())
                    .expect("the test waits for the failing worker");
                // No await after the signal: this current-thread runtime
                // finishes the panic before the test can receive it.
                panic!("injected Learning worker failure");
            })
            .id();
        tokio::time::timeout(HANG_GUARD, entered)
            .await
            .expect("the failing worker starts")
            .expect("the failing worker signals before panicking");
        let sibling_store = store.clone();
        tasks.spawn(async move {
            sibling_store
                .unfinished_deletions(None, 1)
                .await
                .expect("the sibling completes its real Store read");
        });
        tokio::time::timeout(HANG_GUARD, store.wait_deletion_blocking_park_for_tests())
            .await
            .expect("the sibling enters the Store blocking section");
        assert_eq!(store.live_deletion_blocking_sections_for_tests(), 1);

        let initial_failure = if failure_already_observed {
            Some(
                tokio::time::timeout(HANG_GUARD, tasks.join_next())
                    .await
                    .expect("the failed worker is ready")
                    .expect("the failed worker remains in the set")
                    .expect_err("the first worker panicked"),
            )
        } else {
            None
        };
        let failure = {
            let mut draining = std::pin::pin!(super::drain_learning(&mut tasks, initial_failure));
            let polled = poll_fn(|cx| Poll::Ready(draining.as_mut().poll(cx))).await;
            assert!(
                polled.is_pending(),
                "a worker failure must not abandon its parked sibling: {polled:?}"
            );
            assert_eq!(store.live_deletion_blocking_sections_for_tests(), 1);
            drop(release);
            tokio::time::timeout(HANG_GUARD, draining)
                .await
                .expect("drain completes after the sibling's Store work is released")
                .expect("drain preserves the worker failure")
        };
        assert!(failure.is_panic());
        assert_eq!(failure.id(), failed_id, "the original failure is preserved");
        assert!(
            tasks.is_empty(),
            "all siblings must be joined before returning"
        );
        assert_eq!(store.live_deletion_blocking_sections_for_tests(), 0);
    }
}

#[tokio::test]
async fn shutdown_joins_connection_learning_worker_before_successor() {
    use std::sync::atomic::Ordering;

    use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
    use ene_api::v1::handshake::PairingRequest;
    use ene_api::v1::payload::WirePayload;
    use ene_api::v1::refs::{ClientIncarnationId, WireMessageType};
    use ene_companion::CompanionRepository as _;
    use ene_learning::{ExperienceCandidate, ExperienceSourceKind, SourceRangeRef};
    use ene_primitive::RawId;
    use tokio::io::AsyncWriteExt as _;

    let (handle, dir) = crate::test_support::memory_handle("shutdown-learning")
        .await
        .expect("fixture Host opens");
    let companion = handle.store.ensure_running_companion().await.unwrap();
    let source = RawId::new();
    // The real queue supplies the trigger. A successful provider result is
    // irrelevant to whether the connection owns and joins its worker.
    handle
        .queue_learning_formation(ExperienceCandidate {
            companion: companion.as_raw(),
            source: SourceRangeRef {
                kind: ExperienceSourceKind::Dialogue,
                start: source,
                end: source,
            },
            sources: Vec::new(),
            transcript: Vec::new(),
            at: WallClockWithTz::now(),
        })
        .await;
    let queue = Arc::clone(&handle.learning_queue);
    handle
        .serving_test
        .park_learning
        .store(true, Ordering::SeqCst);
    let mut serving = Serving::start(dir.path(), Arc::new(handle)).await;
    let seam = Arc::clone(&serving.seam);
    let mut device = connect_device(dir.path()).await;
    serving.until(seam.device_started.notified()).await;
    let payload = WirePayload::PairingRequest(PairingRequest {
        device_descriptor: "shutdown-learning-device".into(),
    });
    let frame = ene_plugin_ipc::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            WireSender {
                device_id: None,
                incarnation_id: ClientIncarnationId {
                    counter: 1,
                    random: 7,
                },
                connection_id: None,
            },
            WireMessageType(payload.message_type().into()),
        ),
        payload,
    };
    device
        .write_all(&ene_plugin_ipc::encode_frame(&frame).unwrap())
        .await
        .expect("the real device request triggers queued learning");
    serving.until(seam.learning_entered.notified()).await;
    serving.begin_shutdown(false).await;
    serving.assert_pending().await;
    assert!(!crate::lock_unpoison(&queue).is_empty());
    seam.learning_release.notify_one();
    let (path, predecessor) = serving.finish(false).await;
    assert!(
        crate::lock_unpoison(&queue).is_empty(),
        "the joined worker drained its queue"
    );
    drop(device);
    drop(queue);
    assert_successor(&path, &predecessor).await;
}
