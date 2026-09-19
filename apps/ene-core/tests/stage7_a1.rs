//! Stage 7 A1: `ene-client` extract, exclusive Host-local control seat,
//! serving-time approve / credential put, Client `confirmed=true` denial.
//!
//! Real listener, real [`ene_client`] / `ene-ctl` [`Client`], real Host.
//! Only the provider is fake. Empty-seat first-come occupancy is accident
//! prevention and is **not** official GUI authenticity evidence.
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
use ene_credential::{CredentialRef, MemoryCredentialStore};
use ene_ctl::client::{Client, ClientError};
use ene_ctl::cmds;
use ene_ctl::device::{StoredDevice, store_device};
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport};
use ene_local_control::{ControlOutcome, FromHost, RedactedSecret, ToHost};

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
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
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

async fn approve_and_provision(dir: &Path, approver: &HostHandle) -> Result<(), String> {
    let pendings = approver
        .pending_devices()
        .await
        .map_err(|error| format!("pendings must list: {error:?}"))?;
    let pending = pendings
        .first()
        .ok_or_else(|| String::from("a pending must list"))?;
    let approval = approver
        .approve_device(&pending.pending_id)
        .await
        .map_err(|error| format!("approve failed: {error:?}"))?;
    let Some((record, secret)) = approval else {
        return Err(String::from("approval must pair"));
    };
    store_device(
        dir,
        &StoredDevice::new(
            record
                .wire
                .parse()
                .map(ene_api::v1::refs::DeviceWireId)
                .map_err(|error| format!("opaque wire must stay UUID text: {error:?}"))?,
            secret,
        ),
    )
    .map_err(|error| format!("device file must store: {error:?}"))?;
    Ok(())
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
    let pending = Client::connect(&dir, DESCRIPTOR, "test").await;
    assert!(
        matches!(pending, Err(ClientError::ServerOutcome(_))),
        "first pairing must pend"
    );
    let approver = open_host(&dir).await;
    approve_and_provision(&dir, &approver)
        .await
        .expect("approval must pair");
    let mut client = Client::connect(&dir, DESCRIPTOR, "test")
        .await
        .expect("second connect must succeed");
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
async fn second_control_connection_is_seat_occupied() {
    let dir = tempfile::tempdir().expect("scratch");
    let (_handle, server) = serve_control_only(dir.path()).await;
    let mut first = ControlClient::connect(dir.path())
        .await
        .expect("first control dial");
    assert!(
        matches!(
            first.exchange(&ToHost::SeatHello).await,
            Ok(FromHost::SeatGranted)
        ),
        "empty-seat occupancy is accident prevention, not authenticity"
    );
    let mut second = ControlClient::connect(dir.path())
        .await
        .expect("second control dial");
    assert!(
        matches!(
            second.exchange(&ToHost::SeatHello).await,
            Ok(FromHost::SeatOccupied)
        ),
        "a second speaker must not take the seat"
    );
    drop(first);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut third = ControlClient::connect(dir.path())
        .await
        .expect("reconnect dial");
    assert!(
        matches!(
            third.exchange(&ToHost::SeatHello).await,
            Ok(FromHost::SeatGranted)
        ),
        "release must admit a later speaker"
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn session_less_confirmed_true_and_stolen_nonce_are_denied() {
    let dir = tempfile::tempdir().expect("scratch");
    let (_handle, server) = serve_control_only(dir.path()).await;
    let mut first = ControlClient::connect(dir.path()).await.expect("dial");
    assert!(matches!(
        first.exchange(&ToHost::ConfirmedTrue).await,
        Ok(FromHost::DeniedByBoundary)
    ));
    assert!(matches!(
        first.exchange(&ToHost::SeatHello).await,
        Ok(FromHost::SeatGranted)
    ));
    let challenge = first
        .exchange(&ToHost::DeviceApprove {
            pending_id: String::from("no-such-pending"),
        })
        .await
        .expect("challenge");
    let FromHost::ConfirmationChallenge {
        session_id, nonce, ..
    } = challenge
    else {
        panic!("device approve must mint a challenge, got {challenge:?}");
    };

    let mut thief = ControlClient::connect(dir.path()).await.expect("thief");
    let stolen = thief
        .exchange(&ToHost::SessionComplete { session_id, nonce })
        .await
        .expect("stolen complete");
    assert!(
        matches!(stolen, FromHost::DeniedByBoundary),
        "a second connection must not complete with a copied nonce, got {stolen:?}"
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn reconnect_invalidates_outstanding_sessions() {
    let dir = tempfile::tempdir().expect("scratch");
    let (_handle, server) = serve_control_only(dir.path()).await;
    let mut first = ControlClient::connect(dir.path()).await.expect("dial");
    first.exchange(&ToHost::SeatHello).await.expect("hello");
    let challenge = first
        .exchange(&ToHost::DeviceApprove {
            pending_id: String::from("no-such-pending"),
        })
        .await
        .expect("challenge");
    let FromHost::ConfirmationChallenge {
        session_id, nonce, ..
    } = challenge
    else {
        panic!("expected challenge, got {challenge:?}");
    };
    drop(first);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut second = ControlClient::connect(dir.path()).await.expect("reconnect");
    assert!(matches!(
        second.exchange(&ToHost::SeatHello).await,
        Ok(FromHost::SeatGranted)
    ));
    let stale = second
        .exchange(&ToHost::SessionComplete { session_id, nonce })
        .await
        .expect("stale complete");
    assert!(
        matches!(stale, FromHost::DeniedByBoundary),
        "reconnect must invalidate the old session, got {stale:?}"
    );
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn serving_time_control_approve_and_credential_put() {
    let dir = tempfile::tempdir().expect("scratch");
    let (handle, server) = serve_control_only(dir.path()).await;
    let pending = Client::connect(dir.path(), DESCRIPTOR, "test").await;
    assert!(
        matches!(pending, Err(ClientError::ServerOutcome(_))),
        "pairing must pend so approve has a target"
    );
    let pendings = handle.pending_devices().await.expect("pendings");
    let pending_id = pendings[0].pending_id.clone();
    let secret = host_control::approve_device(dir.path(), &pending_id)
        .await
        .expect("serving-time approve must speak control")
        .expect("pending must approve");
    assert!(!secret.is_empty());
    assert!(
        !format!("{secret:?}").is_empty(),
        "pairing secret is for Host-local display"
    );

    assert!(
        host_control::put_credential(dir.path(), "openai", "rotated", PUT_SECRET)
            .await
            .expect("serving-time put must speak control")
    );
    assert!(
        handle.credential_contains_for_tests("openai", "rotated"),
        "put must land in the serving store"
    );
    let stored = ControlOutcome::CredentialStored {
        provider: String::from("openai"),
        label: String::from("rotated"),
    };
    let rendered = format!("{stored:?}");
    assert!(
        !rendered.contains(PUT_SECRET),
        "control outcome Debug must not show the secret: {rendered}"
    );
    let redacted = RedactedSecret::new(PUT_SECRET);
    assert_eq!(format!("{redacted:?}"), "[redacted]");
    server.shutdown_and_join().await;
}

#[tokio::test]
async fn occupied_seat_fails_console_approve_without_client_fallback() {
    let dir = tempfile::tempdir().expect("scratch");
    let (_handle, server) = serve_control_only(dir.path()).await;
    let mut holder = ControlClient::connect(dir.path()).await.expect("holder");
    holder.exchange(&ToHost::SeatHello).await.expect("hello");
    let error = host_control::approve_device(dir.path(), "anything")
        .await
        .expect_err("occupied seat must fail");
    assert!(
        matches!(error, CoreError::SeatOccupied),
        "must not fall through to the Client channel, got {error}"
    );
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
    host_control::confirm_targeted_deletion(dir.path(), &request)
        .await
        .expect("serving-time confirm must speak control");

    let (round, stream, chat) = send_round(&mut client, "still chatting")
        .await
        .expect("chat must complete while deletion is in flight");
    assert_eq!(chat, "after-demand");
    confirm_round(&mut client, &round, stream).await;
    server.shutdown_and_join().await;
}
