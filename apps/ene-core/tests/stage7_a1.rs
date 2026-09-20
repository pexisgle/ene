//! Stage 7 A1: `ene-client` extract, exclusive Host-local control seat,
//! serving-time approve / credential put, Client `confirmed=true` denial.
//!
//! Real listener, real [`ene_client`] / `ene-ctl` [`Client`], real Host.
//! Only the provider is fake. A requester can never acquire an empty seat;
//! tests drive the GUI end of the same private channel the Host creates for
//! its own spawned child.
//!
//! GUI toolkit / overlay probes are out of scope for A1 and remain 未実施.

#![cfg(any(unix, windows))]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration-test helpers outside #[test] functions need the fixture allowances clippy.toml grants only to test functions"
)]

use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ene_api::v1::deletion::DeletionStatusRequest;
use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome, RationaleOrigin,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{BaseViewMark, CommandWireId, ManagementTargetWire, RoundWireId};
use ene_api::v1::round::{PresentationStatus, RoundIntakeOutcomeWire};
use ene_api::v1::undelivered::TaskListResponse;
use ene_core::conn;
use ene_core::host_control::{self, ControlClient};
use ene_core::serve::{CoreError, CredStore, HostHandle};
use ene_credential::{
    CredentialPublicationRepository as _, CredentialRef, CredentialSetRepository as _,
    MemoryCredentialStore, MemoryVersionedStore, MutationKind, MutationOutcome, MutationPhase,
    SecretVersionId,
};
use ene_ctl::client::{Client, ConnectProgress};
use ene_ctl::cmds;
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport};
use ene_local_control::{
    ControlOp, ControlOutcome, DeletionOutcome, FromConfirmation, FromHost, RedactedSecret,
    RequestState, RequesterOutcome, ToConfirmation, ToHost,
};

const DESCRIPTOR: &str = "stage7 a1";
const MODEL: &str = "gpt-slice-test";
const PUT_SECRET: &str = "sk-stage7-a1-put-secret-4419";

fn memory_store() -> MemoryCredentialStore {
    let store = MemoryCredentialStore::new();
    store.insert(
        CredentialRef::new("openai", "main").expect("valid test fixture"),
        "sk-test-only",
    );
    store
}

struct GateTransport {
    replies: Mutex<VecDeque<String>>,
    sends: AtomicUsize,
}

impl GateTransport {
    fn with_replies(replies: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.iter().map(|text| (*text).to_string()).collect()),
            sends: AtomicUsize::new(0),
        })
    }
}

impl ProviderTransport for GateTransport {
    fn complete(
        &self,
        req: ProviderRequest,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<ProviderResponse, ene_inference::InferenceTechnicalError>>
                + Send
                + '_,
        >,
    > {
        let _ = req;
        Box::pin(async move {
            self.sends.fetch_add(1, Ordering::SeqCst);
            let reply = self
                .replies
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .unwrap_or_else(|| String::from("ok"));
            Ok(ProviderResponse {
                text: reply,
                usage: None,
            })
        })
    }
}

struct ServingTask {
    shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), CoreError>>,
}

impl ServingTask {
    fn start(dir: &Path, handle: Arc<HostHandle>, transport: Arc<GateTransport>) -> Self {
        let (shutdown, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(conn::run_until_shutdown(
            dir.to_path_buf(),
            handle,
            transport,
            rx,
        ));
        Self { shutdown, task }
    }

    async fn shutdown_and_join(self) {
        self.shutdown.send_replace(true);
        tokio::time::timeout(Duration::from_secs(30), self.task)
            .await
            .expect("serving shutdown must drain")
            .expect("serving task must join")
            .expect("serving shutdown must succeed");
    }
}

async fn open_host(dir: &Path) -> Arc<HostHandle> {
    match HostHandle::open_with_cred_store(dir, CredStore::Memory(memory_store())).await {
        Ok(handle) => Arc::new(handle),
        Err(error) => panic!("host must open: {error}"),
    }
}

async fn wait_for_control(dir: &Path) -> bool {
    for _ in 0..200 {
        #[cfg(unix)]
        if host_control::control_socket_path(dir).exists() {
            return true;
        }
        #[cfg(windows)]
        if ControlClient::connect(dir).await.is_ok() {
            // A successful open consumes the current named-pipe instance.
            // Yield so accept() can publish the next one before the test dials.
            tokio::task::yield_now().await;
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// Dials control, retrying while the listener is between pipe instances.
///
/// Windows `ClientOptions::open` is synchronous. A probe or a just-dropped
/// peer can leave no waiting server; one `.await` would then fail without
/// polling accept(). Sleeping retries both wait and let the runtime run it.
async fn dial_control(dir: &Path) -> ControlClient {
    let mut last = None;
    for _ in 0..200 {
        match ControlClient::connect(dir).await {
            Ok(client) => return client,
            Err(error) => {
                last = Some(error);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    panic!("control dial: {last:?}");
}

/// Adopts the GUI end of the private channel the Host hands to the process it
/// spawned, and waits for the challenge the request just pushed.
async fn expect_challenge(
    gui: &mut ene_local_control::GuiChannel,
    expected: ControlOp,
) -> (uuid::Uuid, String) {
    let frame = tokio::task::spawn_blocking({
        let mut channel = gui.try_clone().expect("clone");
        move || channel.recv()
    })
    .await
    .expect("join")
    .expect("read")
    .expect("a live channel carries the challenge");
    match frame {
        FromConfirmation::ConfirmationChallenge {
            session_id,
            op,
            nonce,
            ..
        } => {
            assert_eq!(
                op, expected,
                "the challenge must name the operation asked for"
            );
            (session_id, nonce)
        }
        other => panic!("expected a challenge, got {other:?}"),
    }
}

/// Polls one accepted request until the Owner's boundary settles it.
///
/// Completion is asynchronous by design: the requester observes a state, and
/// a settled state is what the test waits for rather than assuming the answer
/// arrived in the same turn.
async fn await_applied(requester: &mut ControlClient, request_id: &str) -> FromHost {
    for _ in 0..100 {
        let state = requester
            .exchange(&ToHost::RequestStatus {
                request_id: request_id.to_string(),
            })
            .await
            .expect("status");
        if let FromHost::RequestStatus {
            state: RequestState::AwaitingOwnerConfirmation,
            ..
        } = &state
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        }
        return state;
    }
    panic!("the request never settled");
}

async fn ask(client: &mut Client, payload: WirePayload, what: &str) -> Result<WirePayload, String> {
    match tokio::time::timeout(Duration::from_secs(10), client.request(payload)).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(error)) => Err(format!("{what} errored: {error:?}")),
        Err(_) => Err(format!("{what} timed out")),
    }
}

async fn ask_stream(client: &mut Client) -> Result<WirePayload, String> {
    match tokio::time::timeout(Duration::from_secs(10), client.next_frame()).await {
        Ok(Ok(payload)) => Ok(payload),
        Ok(Err(error)) => Err(format!("stream frame errored: {error:?}")),
        Err(_) => Err(String::from("stream frame timed out")),
    }
}

async fn view_mark(client: &mut Client) -> Result<String, String> {
    let answer = ask(
        client,
        WirePayload::ManagementViewRequest(cmds::setup_view_request()),
        "show",
    )
    .await?;
    let WirePayload::ManagementView(view) = answer else {
        return Err(format!("show must answer a view, got {answer:?}"));
    };
    Ok(view.mark.0.clone())
}

fn setup_complete_intent(target: &str, mark: &str) -> WirePayload {
    WirePayload::ManagementIntent(ManagementIntent {
        intent_id: CommandWireId(uuid::Uuid::new_v4()),
        kind: ManagementIntentKind::ManageRuleConsentCap,
        target: ManagementTargetWire(String::from(target)),
        base_view: BaseViewMark(String::from(mark)),
        rationale: IntentRationaleWire {
            origin: RationaleOrigin::ManagementSurface,
            quote: None,
        },
        confirmed: false,
    })
}

async fn setup_flow(client: &mut Client, approver: &HostHandle) -> Result<(), String> {
    let mark = view_mark(client).await?;
    let register = ask(
        client,
        WirePayload::ManagementIntent(cmds::credential_intent(
            CommandWireId(uuid::Uuid::new_v4()),
            &BaseViewMark(mark),
            "openai",
        )),
        "register",
    )
    .await?;
    assert!(
        matches!(
            register,
            WirePayload::ManagementOutcome(ManagementOutcome::HeldByOperation)
        ),
        "unapproved register must hold, got {register:?}"
    );
    assert!(
        matches!(
            approver.approve_credential("openai", "main").await,
            Ok(true)
        ),
        "host-local credential approval must succeed"
    );
    let mark = view_mark(client).await?;
    let assign = ask(
        client,
        WirePayload::ManagementIntent(cmds::assignment_intent(
            CommandWireId(uuid::Uuid::new_v4()),
            &BaseViewMark(mark),
            cmds::CAPABILITY_DIALOGUE,
            "openai",
            MODEL,
        )),
        "assign",
    )
    .await?;
    assert!(
        matches!(
            assign,
            WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView { .. })
        ),
        "assign must store, got {assign:?}"
    );
    let mark = view_mark(client).await?;
    let complete = ask(
        client,
        setup_complete_intent("setup:complete", &mark),
        "complete",
    )
    .await?;
    assert!(
        matches!(
            complete,
            WirePayload::ManagementOutcome(ManagementOutcome::AppliedAsOneTime)
        ),
        "complete must apply, got {complete:?}"
    );
    Ok(())
}

async fn serve_and_setup(
    dir: PathBuf,
    transport: Arc<GateTransport>,
) -> (Arc<HostHandle>, ServingTask, Client) {
    let handle = open_host(&dir).await;
    let server = ServingTask::start(&dir, Arc::clone(&handle), transport);
    assert!(wait_for_control(&dir).await, "control listener must bind");
    let progress = Client::begin_connect(&dir, DESCRIPTOR, "test")
        .await
        .expect("first connection must reach pairing");
    let ConnectProgress::Pending(pending) = progress else {
        panic!("first pairing must pend");
    };
    let approved = handle
        .approve_device(pending.pending_id())
        .await
        .expect("approval must succeed");
    assert!(approved.is_some(), "approval must pair");
    let mut client = pending
        .complete()
        .await
        .expect("provision must authenticate");
    let approver = open_host(&dir).await;
    setup_flow(&mut client, &approver)
        .await
        .expect("setup must complete");
    (handle, server, client)
}

async fn send_round(
    client: &mut Client,
    text: &str,
) -> Result<(String, Option<ene_api::v1::refs::StreamWireId>, String), String> {
    let companion = client.companion_ref();
    let send = ask(
        client,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            None,
            false,
            String::from(text),
            String::from("en"),
        )),
        "send",
    )
    .await?;
    let WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round }) = send
    else {
        return Err(format!("input must be accepted, got {send:?}"));
    };
    let round_wire = round.0.clone();
    let mut opened = false;
    let mut text_out = String::new();
    let mut previous_seq: Option<u64> = None;
    let mut stream_id = None;
    loop {
        match ask_stream(client).await? {
            WirePayload::TextStreamOpen(open) => {
                opened = true;
                stream_id = Some(open.stream);
            }
            WirePayload::TextStreamFrame(frame) => {
                assert!(
                    previous_seq.is_none_or(|previous| frame.seq == previous + 1),
                    "stream frames must order by seq"
                );
                previous_seq = Some(frame.seq);
                text_out.push_str(&frame.delta);
            }
            WirePayload::TextStreamClose(close) => {
                assert!(
                    close.status == ene_api::v1::round::StreamClose::Completed,
                    "stream must complete, got {:?}",
                    close.status
                );
                break;
            }
            other => return Err(format!("unexpected stream payload: {other:?}")),
        }
    }
    assert!(opened, "stream must open before it closes");
    Ok((round_wire, stream_id, text_out))
}

async fn confirm_round(
    client: &mut Client,
    round_wire: &str,
    stream_id: Option<ene_api::v1::refs::StreamWireId>,
) {
    let notified = tokio::time::timeout(
        Duration::from_secs(10),
        client.notify(WirePayload::ConfirmPresentation(
            ene_api::v1::round::ConfirmPresentationWire {
                round: RoundWireId(String::from(round_wire)),
                stream: stream_id,
                status: PresentationStatus::Presented,
                detail: None,
            },
        )),
    )
    .await;
    assert!(
        matches!(notified, Ok(Ok(()))),
        "confirm must send, got {notified:?}"
    );
}

#[tokio::test]
async fn client_regression_pairing_chat_deletion_status_and_tasks() {
    let dir = tempfile::tempdir().expect("scratch");
    let transport = GateTransport::with_replies(&["hello from host"]);
    let (_handle, server, mut client) = serve_and_setup(dir.path().to_path_buf(), transport).await;

    let (round, stream, chat) = send_round(&mut client, "hi")
        .await
        .expect("chat round must complete");
    assert_eq!(chat, "hello from host");
    confirm_round(&mut client, &round, stream).await;

    let deletion = ask(
        &mut client,
        WirePayload::DeletionStatusRequest(DeletionStatusRequest {
            cursor: None,
            limit: Some(10),
        }),
        "deletion-status",
    )
    .await
    .expect("deletion status must answer");
    assert!(
        matches!(deletion, WirePayload::DeletionStatusResponse(_)),
        "deletion status stays on the Client channel, got {deletion:?}"
    );

    let tasks = ask(
        &mut client,
        WirePayload::ListTasks(cmds::list_tasks_request(None, Some(10))),
        "list-tasks",
    )
    .await
    .expect("list tasks must answer");
    assert!(
        matches!(
            tasks,
            WirePayload::TaskListResponse(TaskListResponse::Page(_))
        ),
        "task list must be a page, got {tasks:?}"
    );

    server.shutdown_and_join().await;
}

#[tokio::test]
async fn client_confirmed_true_is_denied_by_boundary() {
    let dir = tempfile::tempdir().expect("scratch");
    let transport = GateTransport::with_replies(&[]);
    let (_handle, server, mut client) = serve_and_setup(dir.path().to_path_buf(), transport).await;
    let mark = view_mark(&mut client).await.expect("view");
    let mut intent = cmds::credential_intent(
        CommandWireId(uuid::Uuid::new_v4()),
        &BaseViewMark(mark),
        "openai",
    );
    intent.confirmed = true;
    let answer = ask(
        &mut client,
        WirePayload::ManagementIntent(intent),
        "confirmed-true",
    )
    .await
    .expect("intent must answer");
    assert!(
        matches!(
            answer,
            WirePayload::ManagementOutcome(ManagementOutcome::DeniedByBoundary)
        ),
        "Client confirmed=true must not complete a session, got {answer:?}"
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn connection_replacement_does_not_reuse_the_old_session() {
    let dir = tempfile::tempdir().expect("scratch");
    let transport = GateTransport::with_replies(&["first", "second"]);
    let (_handle, server, mut first) = serve_and_setup(dir.path().to_path_buf(), transport).await;
    let mut second = Client::connect(dir.path(), DESCRIPTOR, "test")
        .await
        .expect("replacement connect must succeed");
    let replaced = ask(
        &mut first,
        WirePayload::ManagementViewRequest(cmds::setup_view_request()),
        "stale-view",
    )
    .await;
    assert!(
        replaced.is_err()
            || matches!(
                replaced,
                Ok(WirePayload::Reject(_)) | Ok(WirePayload::DisconnectNotice(_))
            ),
        "the superseded connection must not keep currentness, got {replaced:?}"
    );
    let mark = view_mark(&mut second).await.expect("new connection view");
    assert!(!mark.is_empty());
    server.shutdown_and_join().await;
}

async fn serve_control_only(dir: &Path) -> (Arc<HostHandle>, ServingTask) {
    let handle = open_host(dir).await;
    let server = ServingTask::start(dir, Arc::clone(&handle), GateTransport::with_replies(&[]));
    assert!(wait_for_control(dir).await, "control listener must bind");
    (handle, server)
}

#[tokio::test]
async fn a_general_connection_cannot_take_a_seat_or_complete_a_session() {
    let dir = tempfile::tempdir().expect("scratch");
    let (handle, server) = serve_control_only(dir.path()).await;

    // No Host-spawned GUI exists yet, so the listener has no seat to hand out
    // and every request it accepts says so.
    let mut requester = dial_control(dir.path()).await;
    assert!(matches!(
        requester.exchange(&ToHost::ConfirmedTrue).await,
        Ok(FromHost::DeniedByBoundary)
    ));
    let accepted = requester
        .exchange(&ToHost::RequestDeviceApprove {
            pending_id: String::from("no-such-pending"),
        })
        .await
        .expect("a request is accepted even with no surface");
    let FromHost::RequestAccepted { request_id } = accepted else {
        panic!("expected RequestAccepted, got {accepted:?}");
    };
    let state = requester
        .exchange(&ToHost::RequestStatus { request_id })
        .await
        .expect("status");
    assert!(
        matches!(
            state,
            FromHost::RequestStatus {
                state: RequestState::ConfirmationUnavailable,
                ..
            }
        ),
        "a request with no confirmation surface must report exactly that, got {state:?}"
    );

    // The Host spawns its GUI: now a seat exists, and only that child's
    // private channel can answer the challenge.
    let mut gui = host_control::seat_test_gui_for_tests(&handle).expect("private channel");
    let accepted = requester
        .exchange(&ToHost::RequestDeviceApprove {
            pending_id: String::from("no-such-pending"),
        })
        .await
        .expect("request");
    let FromHost::RequestAccepted { request_id } = accepted else {
        panic!("expected RequestAccepted, got {accepted:?}");
    };
    let (session_id, nonce) = expect_challenge(&mut gui, ControlOp::DeviceApprove).await;

    // A second requester connection knows the session id and cannot use it:
    // completion frames do not exist on this listener at all.
    let mut thief = dial_control(dir.path()).await;
    let raw = format!(r#"{{"SessionComplete":{{"session_id":"{session_id}","nonce":"{nonce}"}}}}"#);
    let refused = thief.exchange_raw(&raw).await;
    assert!(
        refused.is_err(),
        "a requester listener must not even decode a completion frame"
    );
    // The Owner's surface completes it, and the requester observes the
    // non-secret state under the id it was given.
    gui.send(&ToConfirmation::SessionComplete { session_id, nonce })
        .expect("send");
    let state = await_applied(&mut requester, &request_id).await;
    assert!(
        matches!(
            state,
            FromHost::RequestStatus {
                state: RequestState::Applied {
                    outcome: RequesterOutcome::DeviceUnknown { .. }
                },
                ..
            }
        ),
        "the Owner's decision must reach the requester as its own state, got {state:?}"
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn a_new_spawned_gui_invalidates_the_previous_seats_sessions() {
    let dir = tempfile::tempdir().expect("scratch");
    let (handle, server) = serve_control_only(dir.path()).await;
    let mut first = host_control::seat_test_gui_for_tests(&handle).expect("private channel");
    let mut requester = dial_control(dir.path()).await;
    let accepted = requester
        .exchange(&ToHost::RequestDeviceApprove {
            pending_id: String::from("no-such-pending"),
        })
        .await
        .expect("request");
    let FromHost::RequestAccepted { request_id } = accepted else {
        panic!("expected RequestAccepted, got {accepted:?}");
    };
    let (session_id, nonce) = expect_challenge(&mut first, ControlOp::DeviceApprove).await;

    // The Host spawns a new GUI: the old child's session dies with its seat.
    let second = host_control::seat_test_gui_for_tests(&handle).expect("private channel");
    let _ = second;
    let stale = tokio::task::spawn_blocking({
        let mut channel = first.try_clone().expect("clone");
        move || channel.send(&ToConfirmation::SessionComplete { session_id, nonce })
    })
    .await
    .expect("join");
    assert!(stale.is_ok(), "the old channel is still writable");
    let reply = tokio::task::spawn_blocking(move || first.recv())
        .await
        .expect("join")
        .expect("read")
        .expect("answer");
    assert!(
        matches!(reply, FromConfirmation::DeniedByBoundary),
        "a session from the previous seat generation must not complete, got {reply:?}"
    );
    let state = requester
        .exchange(&ToHost::RequestStatus { request_id })
        .await
        .expect("invalidated request status");
    assert!(
        matches!(
            state,
            FromHost::RequestStatus {
                state: RequestState::ConfirmationUnavailable,
                ..
            }
        ),
        "a replaced seat must not leave its requester waiting forever: {state:?}"
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn a_recovered_prepared_credential_write_is_inspected_not_repeated() {
    let dir = tempfile::tempdir().expect("scratch");
    let handle = HostHandle::open_with_cred_store(
        dir.path(),
        CredStore::MemoryVersioned(MemoryVersionedStore::new()),
    )
    .await
    .expect("versioned host");
    let revision = handle
        .store_for_tests()
        .current_set_revision()
        .await
        .expect("revision");
    handle
        .store_for_tests()
        .begin_credential_mutation(
            String::from("interrupted-put"),
            MutationKind::Register,
            String::from("openai"),
            String::from("main"),
            Some(revision.as_u64()),
            Some(SecretVersionId::from_u64(77)),
        )
        .await
        .expect("Prepared is durable before the external write");

    let outcome = handle
        .publish_credential("openai", "main", "interrupted-put", PUT_SECRET)
        .await
        .expect("the unknown write is a domain outcome");
    assert_eq!(
        outcome,
        MutationOutcome::Unknown,
        "a recovered Prepared mutation must not repeat an external put"
    );
    assert!(
        !handle.credential_contains_for_tests("openai", "main"),
        "an unknown write outcome must not publish a usable snapshot"
    );
    let stored = handle
        .store_for_tests()
        .credential_mutation("interrupted-put")
        .await
        .expect("journal read")
        .expect("journal row");
    assert_eq!(stored.phase, MutationPhase::Prepared);
    assert_eq!(stored.outcome, None);
}

#[tokio::test]
async fn concurrent_credential_publications_leave_the_latest_committed_snapshot_active() {
    let dir = tempfile::tempdir().expect("scratch");
    let handle = Arc::new(
        HostHandle::open_with_cred_store(
            dir.path(),
            CredStore::MemoryVersioned(MemoryVersionedStore::new()),
        )
        .await
        .expect("versioned host"),
    );
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..8_u8 {
        let handle = Arc::clone(&handle);
        tasks.spawn(async move {
            let mutation_id = format!("concurrent-{index}");
            let secret = format!("sk-concurrent-{index}");
            let outcome = handle
                .publish_credential("openai", "main", &mutation_id, &secret)
                .await
                .expect("publication must return a domain outcome");
            (mutation_id, secret, outcome)
        });
    }
    let mut latest: Option<(u64, String, String)> = None;
    while let Some(joined) = tasks.join_next().await {
        let (mutation_id, secret, outcome) = joined.expect("publication task");
        if let MutationOutcome::Activated { revision } = outcome
            && latest
                .as_ref()
                .is_none_or(|(current, _, _)| revision > *current)
        {
            latest = Some((revision, mutation_id, secret));
        }
    }
    let (revision, mutation_id, secret) = latest.expect("one publication must activate");
    let mutation = handle
        .store_for_tests()
        .credential_mutation(&mutation_id)
        .await
        .expect("journal read")
        .expect("winning mutation");
    let active = handle
        .store_for_tests()
        .active_credential_version("openai", "main")
        .await
        .expect("active version");
    assert_eq!(
        mutation.outcome,
        Some(MutationOutcome::Activated { revision })
    );
    assert_eq!(active.active, mutation.candidate_version);
    assert!(
        handle.credential_matches_for_tests("openai", "main", &secret),
        "the immutable snapshot must match the latest activation commit"
    );
}

#[tokio::test]
async fn serving_time_control_approve_and_credential_put() {
    let dir = tempfile::tempdir().expect("scratch");
    let (handle, server) = serve_control_only(dir.path()).await;
    let progress = Client::begin_connect(dir.path(), DESCRIPTOR, "test")
        .await
        .expect("pairing must start");
    let ConnectProgress::Pending(pending) = progress else {
        panic!("pairing must pend so approve has a target");
    };
    let pending_id = pending.pending_id().to_owned();

    // The Host's own GUI holds the seat; the requester asks, and the Owner's
    // surface confirms. The requester never receives the pairing secret: it
    // learns the non-secret device identity only.
    let mut gui = host_control::seat_test_gui_for_tests(&handle).expect("private channel");
    let mut requester = dial_control(dir.path()).await;
    let accepted = requester
        .exchange(&ToHost::RequestDeviceApprove {
            pending_id: pending_id.clone(),
        })
        .await
        .expect("serving-time approve must speak the requester listener");
    let FromHost::RequestAccepted { request_id } = accepted else {
        panic!("expected RequestAccepted, got {accepted:?}");
    };
    let (session_id, nonce) = expect_challenge(&mut gui, ControlOp::DeviceApprove).await;
    gui.send(&ToConfirmation::SessionComplete { session_id, nonce })
        .expect("send");
    let answer = tokio::task::spawn_blocking({
        let mut channel = gui.try_clone().expect("clone");
        move || channel.recv()
    })
    .await
    .expect("join")
    .expect("read")
    .expect("answer");
    let FromConfirmation::Outcome(ControlOutcome::DeviceApproved { .. }) = &answer else {
        panic!("expected DeviceApproved, got {answer:?}");
    };
    let rendered = format!("{answer:?}");
    assert!(
        !rendered.contains("pairing_secret"),
        "control outcome must not contain a provision field: {rendered}"
    );
    let _client = pending.complete().await.expect("origin receives provision");
    let state = await_applied(&mut requester, &request_id).await;
    match state {
        FromHost::RequestStatus { state, .. } => match state {
            RequestState::Applied {
                outcome: RequesterOutcome::DeviceApproved { device_id, .. },
            } => assert!(!device_id.is_empty()),
            other => panic!("expected the approved device identity, got {other:?}"),
        },
        other => panic!("expected RequestStatus, got {other:?}"),
    }

    // A credential value never travels the requester listener: asking for a
    // registration names the pair only, and the value arrives on the private
    // channel. A put with no staged value is refused rather than stored.
    let accepted = requester
        .exchange(&ToHost::RequestCredentialPut {
            provider: String::from("openai"),
            label: String::from("rotated"),
        })
        .await
        .expect("credential request");
    let FromHost::RequestAccepted { request_id } = accepted else {
        panic!("expected RequestAccepted, got {accepted:?}");
    };
    let (session_id, nonce) = expect_challenge(&mut gui, ControlOp::CredentialPut).await;
    gui.send(&ToConfirmation::SessionComplete { session_id, nonce })
        .expect("send");
    let answer = tokio::task::spawn_blocking({
        let mut channel = gui.try_clone().expect("clone");
        move || channel.recv()
    })
    .await
    .expect("join")
    .expect("read")
    .expect("answer");
    assert!(
        matches!(
            answer,
            FromConfirmation::Outcome(ControlOutcome::CredentialRefused { .. })
        ),
        "completing without a staged value must refuse, got {answer:?}"
    );
    assert!(
        !handle.credential_contains_for_tests("openai", "rotated"),
        "a refused registration must not leave a usable credential"
    );
    let _ = request_id;

    let redacted = RedactedSecret::new(PUT_SECRET);
    assert_eq!(format!("{redacted:?}"), "[redacted]");
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn response_correlation_and_slow_consumer_keep_chat_intact() {
    let dir = tempfile::tempdir().expect("scratch");
    let transport = GateTransport::with_replies(&["first-turn", "second-turn"]);
    let (_handle, server, mut client) = serve_and_setup(dir.path().to_path_buf(), transport).await;

    let first = client.prepare(WirePayload::ManagementViewRequest(
        cmds::setup_view_request(),
    ));
    let second = client.prepare(WirePayload::ManagementViewRequest(
        cmds::setup_view_request(),
    ));
    let first_answer = tokio::time::timeout(Duration::from_secs(10), client.execute(&first))
        .await
        .expect("first view must not time out")
        .expect("first view must answer");
    let second_answer = tokio::time::timeout(Duration::from_secs(10), client.execute(&second))
        .await
        .expect("second view must not time out")
        .expect("second view must answer");
    assert!(
        matches!(first_answer, WirePayload::ManagementView(_)),
        "first correlated answer must be the view, got {first_answer:?}"
    );
    assert!(
        matches!(second_answer, WirePayload::ManagementView(_)),
        "second correlated answer must be the view, got {second_answer:?}"
    );

    let companion = client.companion_ref();
    let send = ask(
        &mut client,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            None,
            false,
            String::from("slow"),
            String::from("en"),
        )),
        "slow-send",
    )
    .await
    .expect("slow send must accept");
    let WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. }) = send
    else {
        panic!("slow send must be accepted, got {send:?}");
    };
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    let mut opened = false;
    let mut text_out = String::new();
    loop {
        match ask_stream(&mut client).await.expect("slow stream") {
            WirePayload::TextStreamOpen(_) => opened = true,
            WirePayload::TextStreamFrame(frame) => text_out.push_str(&frame.delta),
            WirePayload::TextStreamClose(close) => {
                assert_eq!(
                    close.status,
                    ene_api::v1::round::StreamClose::Completed,
                    "slow consumer must still see a completed stream"
                );
                break;
            }
            other => panic!("unexpected slow stream payload: {other:?}"),
        }
    }
    assert!(opened);
    assert_eq!(text_out, "first-turn");
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn deletion_demand_while_waiting_does_not_steal_the_answer() {
    let dir = tempfile::tempdir().expect("scratch");
    let transport = GateTransport::with_replies(&["after-demand"]);
    let (handle, server, mut client) = serve_and_setup(dir.path().to_path_buf(), transport).await;
    let page = ask(
        &mut client,
        WirePayload::DeletionStatusRequest(DeletionStatusRequest {
            cursor: None,
            limit: Some(10),
        }),
        "deletion-status",
    )
    .await
    .expect("status");
    let WirePayload::DeletionStatusResponse(ene_api::v1::deletion::DeletionStatusResponse::Page(
        surface,
    )) = page
    else {
        panic!("deletion status must be a page, got {page:?}");
    };
    let staged = ask(
        &mut client,
        WirePayload::ManagementIntent(cmds::deletion_intent(
            CommandWireId(uuid::Uuid::new_v4()),
            &surface.mark.0,
            ene_api::v1::deletion::DeletionPurposeWire::Privacy,
            "forget the test phrase",
        )),
        "deletion-intent",
    )
    .await
    .expect("deletion intent");
    assert!(
        matches!(
            staged,
            WirePayload::ManagementOutcome(ManagementOutcome::NeedsClarification)
        ),
        "the Client intent only stages, got {staged:?}"
    );
    let pending = handle
        .pending_targeted_deletions(None, 10)
        .await
        .expect("pending");
    assert_eq!(pending.len(), 1);
    let request = pending[0]
        .request()
        .as_raw()
        .as_uuid()
        .as_hyphenated()
        .to_string();
    // The Owner's confirmation surface is the Host-spawned GUI. The console
    // asks; the GUI confirms on its private channel.
    let mut gui = host_control::seat_test_gui_for_tests(&handle).expect("private channel");
    let mut requester = dial_control(dir.path()).await;
    requester
        .exchange(&ToHost::RequestDeletionConfirm {
            request_id: request,
        })
        .await
        .expect("serving-time confirm must speak the requester listener");
    let (session_id, nonce) = expect_challenge(&mut gui, ControlOp::DeletionConfirm).await;
    gui.send(&ToConfirmation::SessionComplete { session_id, nonce })
        .expect("send");
    let answer = tokio::task::spawn_blocking({
        let mut channel = gui.try_clone().expect("clone");
        move || channel.recv()
    })
    .await
    .expect("join")
    .expect("read")
    .expect("answer");
    assert!(
        matches!(
            answer,
            FromConfirmation::Outcome(ControlOutcome::Deletion(DeletionOutcome::Started { .. }))
        ),
        "the Owner's confirmation must start the deletion, got {answer:?}"
    );

    let (round, stream, chat) = send_round(&mut client, "still chatting")
        .await
        .expect("chat must complete while deletion is in flight");
    assert_eq!(chat, "after-demand");
    confirm_round(&mut client, &round, stream).await;
    server.shutdown_and_join().await;
}
