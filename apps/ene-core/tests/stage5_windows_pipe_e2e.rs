//! Stage 5 slice F: Host-integration + first-party Client E2E over the REAL
//! Windows named-pipe transport.
//!
//! Real pipe listener, real `ene-ctl` [`Client`], real Host orchestration;
//! only the provider transport is a scripted fake. This is the Windows
//! counterpart of `stage5_e2e.rs` (which stays `#![cfg(unix)]`): the fixture
//! code is duplicated on purpose instead of being extracted into a shared
//! module, so neither transport's harness quietly becomes the other's.
//!
//! Why the protocol behaviour must be identical on both transports: the
//! Windows arm of [`ene_core::conn::run`] creates the exclusive first server
//! instance for the data directory's pipe name, runs the OS peer token check
//! ([`conn_pipe::peer_same_user`]) before a single frame is read, and then
//! drives [`conn::serve_connection`] -- the *same* transport-generic function
//! the Unix listener calls with a `UnixStream`. Frames, transport duplicate
//! suppression, the [`ConnectionTable`] phase machine (`Accepted -> Paired ->
//! Challenged -> Authenticated -> Superseded | Closed`), the per-device
//! current slot, close admission, and the presence fallback all live behind
//! `HostHandle::handle_frame_to` in `ene-core` with no `cfg` split. The only
//! Windows-only code is pipe creation plus the peer token check
//! (`conn_pipe.rs`) and the dial in `ene-ctl`'s `client/transport.rs`; after
//! that dial the `#[cfg(any(unix, windows))]` client runs the identical
//! pairing, capability, challenge-auth, request-correlation, and deferred
//! queue logic. These tests therefore assert the typed outcomes instead of
//! poking at internals: pairing pending/approval, `AuthResult`,
//! `RejectKind::StaleConnection` on a superseded pipe connection, the
//! streaming round contract, and the presence fallback / no-auto-restore /
//! attach contract all arrive through that shared loop.
//!
//! Windows-only by construction, and executed by the existing `Check Windows`
//! CI job (`cargo test --locked --workspace` on `windows-latest`), so no
//! workflow change is needed.
//!
//! Determinism: bounded timeouts on every await, temp data directories (the
//! FNV-derived pipe name is therefore never fixed), polling only to observe
//! asynchronous state (never to order events), and each spawned server task
//! aborted before the test returns.
//!
//! Coverage map:
//!
//! - bind + client dial + pairing + challenge auth + current connection in use
//!   -> `pipe_bind_pair_authenticate_and_serve_the_current_connection` and
//!   `second_host_cannot_create_the_same_pipe`
//! - domain request/response and a real text round over the pipe
//!   -> `pipe_bind_pair_authenticate_and_serve_the_current_connection`
//! - superseded connection answers typed `StaleConnection`
//!   -> `superseded_pipe_connection_answers_typed_stale_connection`
//! - disconnect + reconnect through the connection/presence contract
//!   -> `reconnect_reauths_without_restoring_presence_and_a_fresh_summon_serves`
//! - Windows has no separate authentication/currentness implementation
//!   -> asserted behaviourally throughout (identical typed outcomes and phase
//!   semantics) and explained in the module and test comments

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
    BaseViewMark, CommandWireId, DeviceWireId, ManagementTargetWire, RoundWireId, StreamWireId,
};
use ene_api::v1::reject::RejectKind;
use ene_api::v1::round::{
    ConfirmPresentationWire, PresentationStatus, RoundIntakeOutcomeWire, StreamClose,
};
use ene_api::v1::undelivered::{UndeliveredResponse, UndeliveredSummary};
use ene_core::conn;
use ene_core::conn_pipe;
use ene_core::serve::{CoreError, CredStore, HostHandle};
use ene_credential::{CredentialRef, MemoryCredentialStore, PendingPairing};
use ene_ctl::client::Client;
use ene_ctl::cmds;
use ene_ctl::device::{StoredDevice, load_pending_id, store_device};
use ene_ctl::errors::CliError;
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport};
use rusqlite::OptionalExtension as _;
use tokio::net::windows::named_pipe::ClientOptions;

const DESCRIPTOR: &str = "stage5 pipe e2e";
const MODEL: &str = "gpt-slice-test";
/// The one scripted provider reply: every provider call answers this text, so
/// no assertion depends on provider call ordering.
const REPLY: &str = "hello back over the real named pipe";

fn memory_store() -> MemoryCredentialStore {
    let store = MemoryCredentialStore::new();
    store.insert(
        CredentialRef::new("openai", "main").expect("valid test fixture"),
        "sk-test-only",
    );
    store
}

/// Scripted provider fake: one fixed reply, a send counter, and the recorded
/// request inputs. Fixed text instead of a FIFO script keeps the rounds
/// independent of provider call ordering, so nothing here leans on timing.
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

/// Dials the data directory's pipe until the listener takes the pairing
/// request, and requires the typed pending outcome: a first pairing answered
/// with anything but `PendingOwnerConfirmation` would mean the pipe carries
/// different handshake semantics than the Unix socket.
///
/// A pipe instance is listening only between accepts, and exists only once the
/// Host created it, so a failed dial is retried. The retry loop is a bounded
/// availability wait, never an ordering device.
async fn dial_until_pending(dir: &Path) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let attempt = tokio::time::timeout(
            Duration::from_secs(10),
            Client::connect(dir, DESCRIPTOR, "test"),
        )
        .await;
        match attempt {
            Ok(Err(CliError::ServerOutcome(_))) => return Ok(()),
            Ok(Err(CliError::Transport(reason))) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!("the pipe never accepted a client: {reason}"));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(Err(other)) => return Err(format!("a first pairing must pend, got {other:?}")),
            Ok(Ok(_)) => {
                return Err(String::from(
                    "a first pairing must not authenticate before Owner approval",
                ));
            }
            Err(_) => return Err(String::from("the pairing dial timed out")),
        }
    }
}

/// Connects a stored device over the pipe and requires the whole handshake to
/// succeed: capability, challenge, ownership proof, and the presence fact.
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

/// Host-local Owner approval of the pending device plus the one-time secret
/// handover into the Client's device file, exactly as the trusted surface does
/// it (the secret never travels a normal payload).
///
/// The pending the client remembers (its `client-pending.json` poll key) is
/// approved last, so the device file always ends on the identity the next
/// connect presents. A dial retried at the transport level can leave a second
/// pending row behind, so the whole listed set is approved instead of only the
/// row that happened to be listed first.
async fn approve_and_provision(dir: &Path, approver: &HostHandle) -> Result<(), String> {
    let pendings = approver
        .pending_devices()
        .await
        .map_err(|error| format!("pendings must list: {error:?}"))?;
    let remembered = load_pending_id(dir);
    let mut ordered: Vec<&PendingPairing> = pendings.iter().collect();
    ordered.sort_by_key(|pending| remembered.as_deref() == Some(pending.pending_id.as_str()));
    assert!(
        !ordered.is_empty(),
        "a pairing request must leave a pending for the Owner to approve"
    );
    for pending in ordered {
        let approval = approver
            .approve_device(&pending.pending_id)
            .await
            .map_err(|error| format!("approve failed: {error:?}"))?;
        let Some((record, secret)) = approval else {
            return Err(format!("approval of {} must pair", pending.pending_id));
        };
        store_device(
            dir,
            &StoredDevice::new(
                record
                    .wire
                    .parse()
                    .map(DeviceWireId)
                    .map_err(|error| format!("opaque wire must stay UUID text: {error:?}"))?,
                secret,
            ),
        )
        .map_err(|error| format!("device file must store: {error:?}"))?;
    }
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
    })
}

/// Registers and approves the dialogue credential, assigns the model, and
/// marks setup complete: the same management sequence the Unix harness drives,
/// here over pipe frames.
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

/// Serves `dir` over the real named pipe (the `conn::run` Windows arm), pairs
/// the first device through a real pending -> Owner-approval handshake,
/// completes setup, and returns a live current connection.
///
/// The returned client authenticated over the pipe. Because the domain ingress
/// gate admits only an authenticated-and-current connection, every later
/// domain answer in these tests is itself evidence of the authenticated current
/// slot on this transport.
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
    dial_until_pending(&dir)
        .await
        .expect("the Host must bind the data directory's pipe and pend the pairing");
    // An independent approval context sharing the file device-auth store,
    // exactly like the Host-local trusted surface.
    let approver = open_host(&dir).await;
    approve_and_provision(&dir, &approver)
        .await
        .expect("Owner approval must pair the device");
    let mut client = connect(&dir)
        .await
        .expect("the approved device must authenticate over the pipe");
    setup_flow(&mut client, &approver)
        .await
        .expect("setup must complete");
    (handle, server, client)
}

/// One conversation round over the pipe: submit, require acceptance, drain the
/// stream to completion, and return the round wire id, stream id, and reply
/// text.
///
/// A mid-stream presence fact is absorbed and skipped: `PresenceAttribution`
/// is an unsolicited push that the session loop absorbs, but
/// `Client::next_frame` hands it back, so this drain steps over it. Any other
/// unexpected payload fails the round.
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

/// The Host applies presentation confirmations silently and answers nothing.
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

/// One domain frame on a superseded pipe connection answers the typed
/// `StaleConnection` rejection while the pipe stays open (IPC 11.3). The Unix
/// socket path answers the same typed outcome because the phase snapshot and
/// the rejection both come from the shared `ConnectionTable` gate, never from
/// a transport-specific branch.
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

/// One presence attribution row (state, generation), or `None` before the Host
/// committed one. Read from the Host store because presence has no wire read
/// query; the Unix harness observes it the same way.
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

/// Observation poll for a committed presence state: the close fallback and the
/// submit attach are asynchronous, so this waits for the state instead of
/// assuming a moment. It is never used to order events.
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

/// (1)(2)(3)(5): the Host binds the data directory's pipe, the first-party
/// client dials it, pairing plus challenge authentication complete, and the
/// current connection serves domain frames: a management view, the
/// undelivered read, and a real text round whose reply streams back from the
/// scripted provider.
#[tokio::test]
async fn pipe_bind_pair_authenticate_and_serve_the_current_connection() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(ScriptedTransport::new(REPLY));
    let (_handle, server, mut client) = serve_and_setup(dir.clone(), Arc::clone(&transport)).await;

    // A live management view is answered. The domain ingress gate answers it
    // only for an authenticated-and-current connection; a stale or
    // unauthenticated one gets a typed rejection instead, so this answer is
    // the behavioural evidence that the pipe connection became current.
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

    // A second normal domain request/response over the pipe, and the presence
    // gate over it: this connection is authenticated and current, but no
    // summon attached presence yet, so the client-dependent read answers the
    // typed `NoCurrentPresence` outcome instead of a summary.
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

    // A real text round: presence attach plus round intake plus the streamed
    // reply, all carried by the shared loop over pipe frames.
    let (round, stream, text) = send_round(&mut client, "hello over the pipe")
        .await
        .expect("a round must complete over the pipe");
    assert_eq!(text, REPLY, "the scripted provider reply must stream back");
    assert!(stream.is_some(), "a round opens a stream");
    confirm_round(&mut client, &round, stream).await;

    // With presence attached the same read answers a summary; the round's own
    // answer was presented in band, so the backlog is empty.
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

/// (1): binding is single-instance on Windows too. The exclusive first pipe
/// instance for the data directory's name makes a second Host fail to create
/// it with the typed `CoreError::Bind`, the same outcome the Unix listener
/// produces from its `AddrInUse` probe, instead of letting two Hosts serve
/// one data directory.
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

    // The pipe name is data-directory scoped (FNV-1a over the directory's
    // string form), never a fixed name every Host would share.
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

/// (4): a second authentication for the same device supersedes the first pipe
/// connection. The superseded connection answers the typed
/// `RejectKind::StaleConnection` while its pipe stays open, the newer current
/// keeps serving, and closing the superseded pipe never clears the newer
/// current (close admission compares connection identity before clearing the
/// slot).
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

    // A second connection for the same device: the accept loop mints a fresh
    // connection id and its challenge authentication installs it as current.
    let mut c2 = connect(&dir)
        .await
        .expect("c2 must authenticate over the pipe");
    let (round2, stream2, text2) = send_round(&mut c2, "second over the pipe")
        .await
        .expect("c2 must serve");
    assert_eq!(text2, REPLY);
    confirm_round(&mut c2, &round2, stream2).await;

    // c1 is superseded: its domain frames answer the typed rejection while its
    // pipe stays open, the same observable contract the Unix socket carries.
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

    // The current connection is untouched by the replays and keeps serving.
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

    // Closing the superseded pipe must never clear the newer current.
    drop(c1);
    let (round4, stream4, text4) = send_round(&mut c2, "after the old pipe closed")
        .await
        .expect("c2 must keep serving after the superseded pipe closes");
    assert_eq!(text4, REPLY);
    confirm_round(&mut c2, &round4, stream4).await;
    server.abort();
}

/// (6): disconnect and reconnect through the same connection/presence
/// contract. Reconnecting re-runs the whole challenge handshake on a new pipe
/// connection; authentication alone never restores attribution, and only a
/// fresh summon attaches presence again and serves.
#[tokio::test]
async fn reconnect_reauths_without_restoring_presence_and_a_fresh_summon_serves() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(ScriptedTransport::new(REPLY));
    let (_handle, server, mut c1) = serve_and_setup(dir.clone(), Arc::clone(&transport)).await;

    // The first submit attaches presence (NoActive -> Present) and serves.
    let (round1, stream1, text1) = send_round(&mut c1, "summon over the pipe")
        .await
        .expect("the summon round must complete");
    assert_eq!(text1, REPLY);
    confirm_round(&mut c1, &round1, stream1).await;
    wait_presence(&dir, "present").await;

    // The pipe closes: close admission runs the normal-disconnect fallback. No
    // other current-authenticated candidate exists, so the commit is NoActive
    // with a fresh generation.
    drop(c1);
    let (_, fallen_back) = wait_presence(&dir, "no_active").await;

    // The reconnect is a new pipe connection that re-runs capability plus
    // challenge authentication; the shared handshake pushes the current
    // presence fact and attaches nothing.
    let mut c2 = connect(&dir)
        .await
        .expect("the reconnect must authenticate");
    assert_eq!(
        presence_row(&dir),
        Some((String::from("no_active"), fallen_back)),
        "authentication alone must not restore attribution"
    );

    // A fresh summon round works and attaches presence again.
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

/// The OS peer token check is the Windows analogue of the Unix uid match: the
/// listener drops an unprovable peer before a single frame is read. This drives
/// the same predicate the listener runs, against a real same-user pipe client,
/// so the E2E above is not passing merely because the check is unreachable.
#[tokio::test]
async fn os_peer_token_check_admits_a_same_user_pipe_client() {
    let temp = tempfile::TempDir::new().unwrap();
    // A name only this test derives: the data-directory hash of a path this
    // test owns, so no Host and no other test shares the instance.
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
