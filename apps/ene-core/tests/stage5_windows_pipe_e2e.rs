#![cfg(windows)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration-test helpers outside #[test] functions need the fixture allowances clippy.toml grants only to test functions"
)]

use std::future::Future;
use std::os::windows::io::AsRawHandle as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome, RationaleOrigin,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{
    BaseViewMark, CommandWireId, ManagementTargetWire, RoundWireId, StreamWireId,
};
use ene_api::v1::reject::RejectKind;
use ene_api::v1::round::{
    ConfirmPresentationWire, PresentationStatus, RoundIntakeOutcomeWire, StreamClose,
};
use ene_api::v1::undelivered::{UndeliveredResponse, UndeliveredSummary};
use ene_core::conn;
use ene_core::conn_pipe;
use ene_core::serve::{CoreError, CredStore, HostHandle};
use ene_credential::{CredentialRef, MemoryCredentialStore};
use ene_ctl::client::{Client, ClientError, ConnectProgress, PendingPairingClient};
use ene_ctl::cmds;
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport};
use rusqlite::OptionalExtension as _;
use tokio::net::windows::named_pipe::ClientOptions;

const DESCRIPTOR: &str = "stage5 pipe e2e";
const MODEL: &str = "gpt-slice-test";
const REPLY: &str = "hello back over the real named pipe";

fn memory_store() -> MemoryCredentialStore {
    let store = MemoryCredentialStore::new();
    store.insert(
        CredentialRef::new("openai", "main").expect("valid test fixture"),
        "sk-test-only",
    );
    store
}

struct ScriptedTransport {
    reply: String,
    sends: AtomicUsize,
    inputs: Mutex<Vec<String>>,
}

impl ScriptedTransport {
    fn new(reply: &str) -> Self {
        Self {
            reply: String::from(reply),
            sends: AtomicUsize::new(0),
            inputs: Mutex::new(Vec::new()),
        }
    }

    fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }

    fn input_texts(&self) -> Vec<String> {
        self.inputs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ProviderTransport for ScriptedTransport {
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
        Box::pin(async move {
            self.sends.fetch_add(1, Ordering::SeqCst);
            self.inputs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(req.input);
            Ok(ProviderResponse {
                text: self.reply.clone(),
                usage: None,
            })
        })
    }
}

async fn open_host(dir: &Path) -> Arc<HostHandle> {
    let opened = HostHandle::open_with_cred_store(dir, CredStore::Memory(memory_store())).await;
    assert!(opened.is_ok(), "host must open");
    Arc::new(opened.unwrap())
}

async fn dial_until_pending(dir: &Path) -> Result<PendingPairingClient, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let attempt = tokio::time::timeout(
            Duration::from_secs(10),
            Client::begin_connect(dir, DESCRIPTOR, "test"),
        )
        .await;
        match attempt {
            Ok(Ok(ConnectProgress::Pending(pending))) => return Ok(pending),
            Ok(Err(ClientError::Transport(reason))) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!("the pipe never accepted a client: {reason}"));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(Err(other)) => return Err(format!("a first pairing must pend, got {other:?}")),
            Ok(Ok(ConnectProgress::Connected(_))) => {
                return Err(String::from(
                    "a first pairing must not authenticate before Owner approval",
                ));
            }
            Err(_) => return Err(String::from("the pairing dial timed out")),
        }
    }
}

async fn connect(dir: &Path) -> Result<Client, String> {
    match tokio::time::timeout(
        Duration::from_secs(15),
        Client::connect(dir, DESCRIPTOR, "test"),
    )
    .await
    {
        Ok(Ok(client)) => Ok(client),
        Ok(Err(error)) => Err(format!("connect failed: {error:?}")),
        Err(_) => Err(String::from("connect timed out")),
    }
}

async fn ask(client: &mut Client, payload: WirePayload, what: &str) -> Result<WirePayload, String> {
    match tokio::time::timeout(Duration::from_secs(15), client.request(payload)).await {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(error)) => Err(format!("{what} errored: {error:?}")),
        Err(_) => Err(format!("{what} timed out")),
    }
}

async fn ask_stream(client: &mut Client) -> Result<WirePayload, String> {
    match tokio::time::timeout(Duration::from_secs(15), client.next_frame()).await {
        Ok(Ok(payload)) => Ok(payload),
        Ok(Err(error)) => Err(format!("stream frame errored: {error:?}")),
        Err(_) => Err(String::from("stream frame timed out")),
    }
}

async fn approve_and_complete(
    pending: ene_ctl::client::PendingPairingClient,
    approver: &HostHandle,
) -> Result<Client, String> {
    let approved = approver
        .approve_device(pending.pending_id())
        .await
        .map_err(|error| format!("approve failed: {error:?}"))?;
    if approved.is_none() {
        return Err(String::from("approval must pair"));
    }
    pending
        .complete()
        .await
        .map_err(|error| format!("provision completion failed: {error:?}"))
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
    transport: Arc<ScriptedTransport>,
) -> (
    Arc<HostHandle>,
    tokio::task::JoinHandle<Result<(), CoreError>>,
    Client,
) {
    let handle = open_host(&dir).await;
    let server = tokio::spawn(conn::run(dir.clone(), Arc::clone(&handle), transport));
    let pending = dial_until_pending(&dir)
        .await
        .expect("the Host must bind the data directory's pipe and pend the pairing");
    let mut client = approve_and_complete(pending, &handle)
        .await
        .expect("Owner approval must pair the device");
    let approver = open_host(&dir).await;
    setup_flow(&mut client, &approver)
        .await
        .expect("setup must complete");
    (handle, server, client)
}

async fn send_round(
    client: &mut Client,
    text: &str,
) -> Result<(String, Option<StreamWireId>, String), String> {
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
                    close.status == StreamClose::Completed,
                    "stream must complete, got {:?}",
                    close.status
                );
                break;
            }
            WirePayload::PresenceAttribution(_) => {}
            other => return Err(format!("unexpected stream payload: {other:?}")),
        }
    }
    assert!(opened, "stream must open before it closes");
    Ok((round_wire, stream_id, text_out))
}

async fn confirm_round(client: &mut Client, round_wire: &str, stream_id: Option<StreamWireId>) {
    let notified = tokio::time::timeout(
        Duration::from_secs(15),
        client.notify(WirePayload::ConfirmPresentation(ConfirmPresentationWire {
            round: RoundWireId(String::from(round_wire)),
            stream: stream_id,
            status: PresentationStatus::Presented,
            detail: None,
        })),
    )
    .await;
    assert!(
        matches!(notified, Ok(Ok(()))),
        "confirm must send, got {notified:?}"
    );
}

async fn fetch_summary(client: &mut Client, what: &str) -> Result<UndeliveredSummary, String> {
    let answer = ask(
        client,
        WirePayload::UndeliveredRequest(cmds::undelivered_request(None, None, false)),
        what,
    )
    .await?;
    let WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(summary)) = answer else {
        return Err(format!("fetch must summarize, got {answer:?}"));
    };
    Ok(summary)
}

async fn expect_stale_connection(client: &mut Client, payload: WirePayload, what: &str) {
    let answer = ask(client, payload, what)
        .await
        .expect("a superseded connection still answers");
    let WirePayload::Reject(reject) = answer else {
        panic!("{what} on a superseded connection must reject, got {answer:?}");
    };
    assert_eq!(
        reject.kind,
        RejectKind::StaleConnection,
        "{what} must name the stale connection"
    );
}

fn presence_row(dir: &Path) -> Option<(String, i64)> {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    conn.query_row(
        "SELECT state, generation FROM presence_attribution",
        (),
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .expect("the presence attribution must read")
}

async fn wait_presence(dir: &Path, wanted: &str) -> (String, i64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(row) = presence_row(dir)
            && row.0 == wanted
        {
            return row;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "presence never reached {wanted}: {:?}",
            presence_row(dir)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn pipe_bind_pair_authenticate_and_serve_the_current_connection() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(ScriptedTransport::new(REPLY));
    let (_handle, server, mut client) = serve_and_setup(dir.clone(), Arc::clone(&transport)).await;

    let view = ask(
        &mut client,
        WirePayload::ManagementViewRequest(cmds::setup_view_request()),
        "show",
    )
    .await
    .expect("a current connection must answer a management view");
    let WirePayload::ManagementView(view) = view else {
        panic!("show must answer a view, got {view:?}");
    };
    assert!(!view.mark.0.is_empty(), "a view carries its mark");
    assert!(
        !view.sections.is_empty(),
        "the requested setup sections must render"
    );

    let before = ask(
        &mut client,
        WirePayload::UndeliveredRequest(cmds::undelivered_request(None, None, false)),
        "undelivered before summon",
    )
    .await
    .expect("the undelivered read must answer");
    assert!(
        matches!(
            before,
            WirePayload::UndeliveredResponse(UndeliveredResponse::NoCurrentPresence)
        ),
        "no presence backs a summary before the first summon, got {before:?}"
    );

    let (round, stream, text) = send_round(&mut client, "hello over the pipe")
        .await
        .expect("a round must complete over the pipe");
    assert_eq!(text, REPLY, "the scripted provider reply must stream back");
    assert!(stream.is_some(), "a round opens a stream");
    confirm_round(&mut client, &round, stream).await;

    let summary = fetch_summary(&mut client, "undelivered after summon")
        .await
        .expect("the post-summon undelivered read must answer");
    assert!(
        summary.items.is_empty(),
        "nothing is undelivered after the presented round, got {} item(s)",
        summary.items.len()
    );
    assert_eq!(transport.sends(), 1, "one provider call served the round");
    let inputs = transport.input_texts();
    assert_eq!(inputs.len(), 1, "one recorded provider input");
    assert!(
        inputs[0].contains("hello over the pipe"),
        "the provider sees the submitted text: {}",
        inputs[0]
    );

    server.abort();
}

#[tokio::test]
async fn second_host_cannot_create_the_same_pipe() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(ScriptedTransport::new(REPLY));
    let handle = open_host(&dir).await;
    let server = tokio::spawn(conn::run(
        dir.clone(),
        Arc::clone(&handle),
        Arc::clone(&transport),
    ));
    dial_until_pending(&dir)
        .await
        .expect("the first Host must bind and serve the pipe");

    let name = conn_pipe::pipe_name(&dir);
    assert!(name.contains("pipe"), "the name is a pipe name: {name}");
    assert_ne!(
        name,
        conn_pipe::pipe_name(Path::new("another-data-directory")),
        "distinct data directories use distinct pipes"
    );

    let second = tokio::time::timeout(
        Duration::from_secs(10),
        conn::run(dir.clone(), open_host(&dir).await, Arc::clone(&transport)),
    )
    .await;
    assert!(
        matches!(second, Ok(Err(CoreError::Bind(_)))),
        "a second Host must fail to create the pipe, got {second:?}"
    );
    server.abort();
}

#[tokio::test]
async fn superseded_pipe_connection_answers_typed_stale_connection() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(ScriptedTransport::new(REPLY));
    let (_handle, server, mut c1) = serve_and_setup(dir.clone(), Arc::clone(&transport)).await;
    let (round1, stream1, text1) = send_round(&mut c1, "first over the pipe")
        .await
        .expect("c1 must serve");
    assert_eq!(text1, REPLY);
    confirm_round(&mut c1, &round1, stream1).await;

    let mut c2 = connect(&dir)
        .await
        .expect("c2 must authenticate over the pipe");
    let (round2, stream2, text2) = send_round(&mut c2, "second over the pipe")
        .await
        .expect("c2 must serve");
    assert_eq!(text2, REPLY);
    confirm_round(&mut c2, &round2, stream2).await;

    let companion = c2.companion_ref();
    let stale_round = ask(
        &mut c2,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            Some(round1.clone()),
            false,
            String::from("join c1 round over the pipe"),
            String::from("en"),
        )),
        "old round join",
    )
    .await
    .expect("the old-round submit must answer typed");
    assert!(
        matches!(
            stale_round,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound { .. })
        ),
        "C2 must not inherit C1's open round, got {stale_round:?}"
    );

    let companion = c1.companion_ref();
    expect_stale_connection(
        &mut c1,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            None,
            false,
            String::from("replay over the pipe"),
            String::from("en"),
        )),
        "superseded input",
    )
    .await;
    expect_stale_connection(
        &mut c1,
        WirePayload::ManagementViewRequest(cmds::setup_view_request()),
        "superseded view",
    )
    .await;

    let (round3, stream3, text3) = send_round(&mut c2, "still current")
        .await
        .expect("c2 must stay current");
    assert_eq!(text3, REPLY);
    confirm_round(&mut c2, &round3, stream3).await;
    assert_eq!(
        transport.sends(),
        3,
        "one provider call per accepted round: the stale replays sent nothing"
    );

    drop(c1);
    let (round4, stream4, text4) = send_round(&mut c2, "after the old pipe closed")
        .await
        .expect("c2 must keep serving after the superseded pipe closes");
    assert_eq!(text4, REPLY);
    confirm_round(&mut c2, &round4, stream4).await;
    server.abort();
}

#[tokio::test]
async fn reconnect_reauths_without_restoring_presence_and_a_fresh_summon_serves() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(ScriptedTransport::new(REPLY));
    let (_handle, server, mut c1) = serve_and_setup(dir.clone(), Arc::clone(&transport)).await;

    let (round1, stream1, text1) = send_round(&mut c1, "summon over the pipe")
        .await
        .expect("the summon round must complete");
    assert_eq!(text1, REPLY);
    confirm_round(&mut c1, &round1, stream1).await;
    wait_presence(&dir, "present").await;

    drop(c1);
    let (_, fallen_back) = wait_presence(&dir, "no_active").await;

    let mut c2 = connect(&dir)
        .await
        .expect("the reconnect must authenticate");
    assert_eq!(
        presence_row(&dir),
        Some((String::from("no_active"), fallen_back)),
        "authentication alone must not restore attribution"
    );

    let (round2, stream2, text2) = send_round(&mut c2, "summon again over the pipe")
        .await
        .expect("the post-reconnect summon must complete");
    assert_eq!(text2, REPLY);
    confirm_round(&mut c2, &round2, stream2).await;
    let (state, attached) = presence_row(&dir).expect("the attach must commit a row");
    assert_eq!(state, "present", "the fresh summon attaches presence");
    assert!(
        attached > fallen_back,
        "attaching advances the presence generation: {fallen_back} -> {attached}"
    );
    assert_eq!(
        transport.sends(),
        2,
        "one provider call per round: the reconnect and the attach sent nothing"
    );
    server.abort();
}

#[tokio::test]
async fn os_peer_token_check_admits_a_same_user_pipe_client() {
    let temp = tempfile::TempDir::new().unwrap();
    let pipe = conn_pipe::pipe_name(&temp.path().join("peer-check"));
    let server = conn_pipe::create_first_server(&pipe).expect("the pipe must create");
    let client = ClientOptions::new()
        .open(&pipe)
        .expect("a same-user client must open the pipe");
    tokio::time::timeout(Duration::from_secs(10), server.connect())
        .await
        .expect("the instance must accept the connection")
        .expect("the pipe must connect");
    assert!(
        conn_pipe::peer_same_user(server.as_raw_handle()),
        "the OS peer token check must admit this same-user client"
    );
    drop(client);
}
