//! Stage 5 slice F: Host-integration + first-party Client E2E over the real
//! Unix-socket transport (S5-01..24, OS-transport paragraph).
//!
//! Real listener socket, real `ene-ctl` [`Client`], real Host orchestration;
//! only the provider transport is fake. The fake is controllable: scripted
//! FIFO replies, a send counter, recorded inputs, and deterministic per-call
//! gates (barriers), never sleeps for ordering. Each test maps to its S5 row
//! in a comment; rows already covered at Host-handle level by slice unit
//! tests are cited, not re-driven.
//!
//! Unix-only: like `vertical_slice.rs`, these tests drive the Unix socket
//! listener. The Windows named-pipe listener shares the same handshake and
//! phase path; its transport subset runs in
//! [`stage5_windows_pipe_e2e.rs`](stage5_windows_pipe_e2e.rs) on the Windows
//! CI runner.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    reason = "integration-test helpers outside #[test] functions need the fixture allowances clippy.toml grants only to test functions"
)]

use std::collections::{BTreeSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome,
    RationaleOrigin, workspace_target,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{BaseViewMark, CommandWireId, ManagementTargetWire, RoundWireId};
use ene_api::v1::round::{PresentationStatus, RoundIntakeOutcomeWire};
use ene_api::v1::undelivered::{
    ResumeTaskOutcomeWire, TaskListPage, UndeliveredAckOutcome, UndeliveredResponse,
    UndeliveredSummary,
};
use ene_core::conn;
use ene_core::serve::{CoreError, CredStore, HostHandle};
use ene_credential::{
    CredentialRef, CredentialScrubber, MemoryCredentialStore, SecretScrubber as _,
};
use ene_ctl::client::{Client, ConnectProgress};
use ene_ctl::cmds;
use ene_inference::{ProviderRequest, ProviderResponse, ProviderTransport};
use ene_task::TaskRepository as _;

const DESCRIPTOR: &str = "stage5 e2e";
const MODEL: &str = "gpt-slice-test";

fn memory_store() -> MemoryCredentialStore {
    let store = MemoryCredentialStore::new();
    store.insert(
        CredentialRef::new("openai", "main").expect("valid test fixture"),
        "sk-test-only",
    );
    store
}

/// Controllable provider fake: scripted FIFO replies consumed in arrival
/// order, a send counter, recorded inputs, and deterministic per-call gates.
///
/// Replies pop per provider call in arrival order, so a test serializes
/// dialogue turns against agent bursts with gates: while the agent's call N
/// sits in `blocks`, the only arriving call is the dialogue's, and it pops
/// the next reply. Ordering never depends on timing.
struct GateTransport {
    replies: Mutex<VecDeque<String>>,
    inputs: Mutex<Vec<String>>,
    sends: AtomicUsize,
    blocks: Mutex<BTreeSet<usize>>,
    failures: Mutex<BTreeSet<usize>>,
}

impl GateTransport {
    fn new(replies: Vec<String>, blocks: &[usize]) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            inputs: Mutex::new(Vec::new()),
            sends: AtomicUsize::new(0),
            blocks: Mutex::new(blocks.iter().copied().collect()),
            failures: Mutex::new(BTreeSet::new()),
        }
    }

    fn sends(&self) -> usize {
        self.sends.load(Ordering::SeqCst)
    }

    fn unblock(&self, call: usize) {
        self.blocks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&call);
    }

    /// Ends a parked provider call without producing a final result or an
    /// Action directive, so graceful restart can drain the old execution.
    fn fail(&self, call: usize) {
        self.failures.lock().unwrap().insert(call);
        self.unblock(call);
    }

    fn input_texts(&self) -> Vec<String> {
        self.inputs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Observation poll for background completion (not an ordering device;
    /// ordering uses [`Self::blocks`] gates).
    async fn wait_sends(&self, wanted: usize) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while self.sends() < wanted {
            assert!(
                tokio::time::Instant::now() < deadline,
                "provider sends did not reach {wanted}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
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
        Box::pin(async move {
            let call = self.sends.fetch_add(1, Ordering::SeqCst) + 1;
            // Deterministic barrier: this call number stays here until the
            // test unblocks it. Membership only shrinks, so no wakeup races.
            loop {
                let held = self
                    .blocks
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .contains(&call);
                if !held {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            self.inputs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(req.input);
            let text = self
                .replies
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .pop_front()
                .unwrap_or_default();
            if self.failures.lock().unwrap().remove(&call) {
                return Err(
                    ene_inference::InferenceTechnicalError::ProviderTransportFailed(String::from(
                        "fixture provider disconnected before returning a result",
                    )),
                );
            }
            Ok(ProviderResponse { text, usage: None })
        })
    }
}

/// One provider reply carrying exactly one companion `[task-control]`
/// directive: the marker line must be the reply's first non-empty line.
fn task_reply(directive: serde_json::Value) -> String {
    format!("[task-control] {directive}")
}

async fn open_host(dir: &std::path::Path) -> Arc<HostHandle> {
    let opened = HostHandle::open_with_cred_store(dir, CredStore::Memory(memory_store())).await;
    assert!(opened.is_ok(), "host must open");
    Arc::new(opened.unwrap())
}

async fn wait_for_socket(dir: &std::path::Path) -> bool {
    for _ in 0..100 {
        if dir.join("ene.sock").exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// A stopped Host leaves the path behind, so this waits for an actual
/// accept, not mere path existence.
async fn wait_for_listener(dir: &std::path::Path) -> bool {
    for _ in 0..200 {
        if tokio::net::UnixStream::connect(dir.join("ene.sock"))
            .await
            .is_ok()
        {
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

/// Owns serving shutdown without retaining a strong Host reference. Joining
/// every serving child, then observing the dead Weak, precedes any successor.
struct ServingTask {
    shutdown: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<(), CoreError>>,
    host: Weak<HostHandle>,
}

impl ServingTask {
    fn start(
        dir: &std::path::Path,
        handle: Arc<HostHandle>,
        transport: Arc<GateTransport>,
    ) -> Self {
        let host = Arc::downgrade(&handle);
        let (shutdown, stopped) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(conn::run_until_shutdown(
            dir.to_path_buf(),
            handle,
            transport,
            stopped,
        ));
        Self {
            shutdown,
            task,
            host,
        }
    }

    async fn shutdown_and_join(self) {
        self.shutdown.send_replace(true);
        tokio::time::timeout(Duration::from_secs(30), self.task)
            .await
            .expect("serving shutdown must drain; release provider gates before restart")
            .expect("serving task must join")
            .expect("serving shutdown must succeed");
        assert!(
            self.host.upgrade().is_none(),
            "the predecessor Host must be gone before restart"
        );
    }

    /// Final test teardown only; a restart always uses shutdown_and_join.
    fn abort(&self) {
        self.task.abort();
    }
}

/// Serves `dir` with `transport`, pairs the first device, and completes
/// setup; returns a weak Host observer, the serving owner, and a live client.
async fn serve_and_setup(
    dir: std::path::PathBuf,
    transport: Arc<GateTransport>,
) -> (Weak<HostHandle>, ServingTask, Client) {
    let handle = open_host(&dir).await;
    let observer = Arc::downgrade(&handle);
    let server = ServingTask::start(&dir, Arc::clone(&handle), transport);
    assert!(wait_for_socket(&dir).await, "listener must bind ene.sock");
    let progress = Client::begin_connect(&dir, DESCRIPTOR, "test")
        .await
        .expect("first connection must reach pairing");
    let ConnectProgress::Pending(pending) = progress else {
        panic!("first pairing must pend");
    };
    let mut client = approve_and_complete(pending, &handle)
        .await
        .expect("approval must pair");
    let approver = open_host(&dir).await;
    setup_flow(&mut client, &approver)
        .await
        .expect("setup must complete");
    (observer, server, client)
}

/// One conversation round over the socket: submit, require acceptance, drain
/// the stream to completion. Returns the round wire id, stream id, and text.
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

async fn select_workspace(client: &mut Client, path: &std::path::Path) -> Result<(), String> {
    let mark = view_mark(client).await?;
    let answer = ask(
        client,
        WirePayload::ManagementIntent(ManagementIntent {
            intent_id: CommandWireId(uuid::Uuid::new_v4()),
            kind: ManagementIntentKind::SelectWorkspace,
            target: workspace_target(&path.to_string_lossy()),
            base_view: BaseViewMark(mark),
            rationale: IntentRationaleWire {
                origin: RationaleOrigin::ManagementSurface,
                quote: None,
            },
            confirmed: false,
        }),
        "select-workspace",
    )
    .await?;
    assert!(
        matches!(
            answer,
            WirePayload::ManagementOutcome(ManagementOutcome::AppliedAsOneTime)
        ),
        "workspace selection must apply, got {answer:?}"
    );
    Ok(())
}

/// Drains the session-deferred auto-presented summaries (pushed backlog the
/// Host sent without `reply_to`).
fn take_summaries(client: &mut Client) -> Vec<UndeliveredSummary> {
    let mut summaries = Vec::new();
    for frame in client.take_undelivered() {
        if let WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(summary)) =
            frame.payload
        {
            summaries.push(summary);
        }
    }
    summaries
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

/// One domain frame on a superseded connection answers typed
/// `StaleConnection` while the socket stays open (IPC §11.3, S5-03/04).
async fn ack_summary(
    client: &mut Client,
    summary: &UndeliveredSummary,
) -> Result<UndeliveredAckOutcome, String> {
    let acked = client
        .request_observed(
            WirePayload::UndeliveredAck(cmds::undelivered_ack(
                &summary.receipt.0,
                PresentationStatus::Presented,
            )),
            Some(summary.round.clone()),
        )
        .await
        .map_err(|error| format!("ack errored: {error:?}"))?;
    let WirePayload::UndeliveredAckOutcome(outcome) = acked else {
        return Err(format!("ack must answer an outcome, got {acked:?}"));
    };
    Ok(outcome)
}

async fn list_tasks(client: &mut Client) -> Result<TaskListPage, String> {
    let answer = ask(
        client,
        WirePayload::ListTasks(cmds::list_tasks_request(None, None)),
        "list-tasks",
    )
    .await?;
    let WirePayload::TaskListResponse(ene_api::v1::undelivered::TaskListResponse::Page(page)) =
        answer
    else {
        return Err(format!("list must answer a page, got {answer:?}"));
    };
    Ok(page)
}

async fn wait_task_progress(
    client: &mut Client,
    wanted: &str,
    tasks: usize,
) -> Result<TaskListPage, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let page = list_tasks(client).await?;
        if page.tasks.len() == tasks && page.tasks.iter().all(|item| item.progress == wanted) {
            return Ok(page);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "tasks did not reach {wanted}: {:?}",
            page.tasks
                .iter()
                .map(|item| item.progress.clone())
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_path(path: std::path::PathBuf) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "workspace write did not land: {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Gracefully joins the predecessor before opening and starting its successor.
async fn restart_host(
    dir: &std::path::Path,
    server: ServingTask,
    transport: Arc<GateTransport>,
) -> (Weak<HostHandle>, ServingTask) {
    server.shutdown_and_join().await;
    start_restarted_host(dir, transport).await
}

async fn start_restarted_host(
    dir: &std::path::Path,
    transport: Arc<GateTransport>,
) -> (Weak<HostHandle>, ServingTask) {
    let handle = open_host(dir).await;
    // Production startup mutations before the listener binds: normalization,
    // pairing cleanup, sweep, and sealed-result reconciliation.
    handle
        .run_startup_mutations()
        .await
        .expect("restart startup must complete");
    let observer = Arc::downgrade(&handle);
    let server = ServingTask::start(dir, handle, transport);
    assert!(
        wait_for_listener(dir).await,
        "listener must rebind after restart"
    );
    (observer, server)
}

/// Observation poll for the server-side disconnect fallback (not an
/// ordering device; the fallback is async after a socket close).
async fn wait_presence(dir: &std::path::Path, wanted: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if presence_row(dir).0 == wanted {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "presence did not reach {wanted}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn table_count(dir: &std::path::Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), (), |row| {
        row.get(0)
    })
    .expect("the probe count must read")
}

fn presence_row(dir: &std::path::Path) -> (String, i64) {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    conn.query_row(
        "SELECT state, generation FROM presence_attribution",
        (),
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .expect("one presence attribution must read")
}

/// S5-01 (disconnect mid-provider-wait: the Host-only Task continues, no
/// cancel/Failed, another valid round runs mid-execution) flowing into S5-02
/// (absence completion auto-displays on reconnect; send is not Presented).
#[tokio::test]
async fn s5_01_disconnect_mid_wait_then_absence_completion_presents() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    // Call order is arrival-gated: 1 propose, 2 agent-read (held), 3 chat,
    // then 4 create and 5 final after the release, and 6 summons presence
    // on the last reconnect (the submit auto-presents the backlog).
    let transport = Arc::new(GateTransport::new(
        vec![
            task_reply(
                serde_json::json!({"kind": "propose_task", "purpose": "read input.txt and write report.md"}),
            ),
            String::from("You are welcome."),
            String::from(r#"{"tool":"read","path":"input.txt"}"#),
            String::from(
                "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}",
            ),
            String::from(r#"{"final":"created report.md from input.txt"}"#),
            String::from("All done here."),
        ],
        &[2],
    ));
    let (_handle, server, mut client) = serve_and_setup(dir.clone(), Arc::clone(&transport)).await;
    select_workspace(&mut client, workspace.path())
        .await
        .expect("workspace must select");

    // Round 1 creates and delegates the Task; the production launcher starts
    // the runner, whose first provider call parks at the gate.
    let (round_wire, stream_id, text_out) =
        send_round(&mut client, "please read input.txt and write report.md")
            .await
            .expect("propose round must complete");
    assert!(text_out.contains("Task accepted"), "{text_out}");
    confirm_round(&mut client, &round_wire, stream_id).await;
    transport.wait_sends(2).await;

    // Disconnect mid-provider-wait: no cancel, no failure may follow.
    drop(client);
    // Reconnect while the execution is still parked: another valid round
    // runs to completion mid-execution (call 3 pops the chat reply while
    // call 2 stays gated, so the order is arrival-deterministic).
    let mut client = Client::connect(&dir, DESCRIPTOR, "test")
        .await
        .expect("reconnect must succeed");
    let page = list_tasks(&mut client).await.expect("list must read");
    assert_eq!(page.tasks.len(), 1, "one task must list");
    assert_eq!(
        page.tasks[0].progress, "in_progress",
        "task must run, got {:?}",
        page.tasks[0].progress
    );
    let (_round2, stream2, chat) = send_round(&mut client, "thanks!")
        .await
        .expect("a round must start mid-execution");
    assert_eq!(chat, "You are welcome.");
    confirm_round(&mut client, &_round2, stream2).await;
    // Absent again before the completion commits.
    drop(client);

    // Release the provider wait: the Host-only execution reads, writes, and
    // finalizes with no client attached.
    transport.unblock(2);
    wait_path(workspace.path().join("report.md")).await;
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("report.md")).unwrap(),
        "# Report\nnotes"
    );

    // Reconnect: the absence completion auto-displays with its file facts.
    let mut client = Client::connect(&dir, DESCRIPTOR, "test")
        .await
        .expect("second reconnect must succeed");
    let page = wait_task_progress(&mut client, "completed", 1)
        .await
        .expect("task must complete");
    assert_eq!(page.tasks[0].revision, 1, "no resume happened");
    assert_eq!(transport.sends(), 5, "no second launch, no rerun");
    // The summon submit establishes formal presence and auto-presents the
    // absence backlog without an Owner query; the pushed frames defer on
    // the session while the turn's own reply still streams normally.
    let (_round3, stream3, done_chat) = send_round(&mut client, "all done?")
        .await
        .expect("summon round must complete");
    assert_eq!(done_chat, "All done here.");
    confirm_round(&mut client, &_round3, stream3).await;
    assert_eq!(transport.sends(), 6, "only the summon turn sent");
    let pushed = take_summaries(&mut client);
    assert!(
        !pushed.is_empty(),
        "the summon must auto-present the absence backlog"
    );
    let pushed_summary = pushed
        .iter()
        .find(|candidate| !candidate.items.is_empty())
        .expect("a pushed batch must carry rows");
    // The summon published the authoritative presence fact ahead of this
    // summary, so the session already echoes the pushed generation: the
    // first ACK presents the carried batch with no probe round and no
    // second submit. A summary ahead of its fact could only be refused
    // stale, so this leg is the regression the fact ordering fixes.
    let acked = client
        .request_observed(
            WirePayload::UndeliveredAck(cmds::undelivered_ack(
                &pushed_summary.receipt.0,
                PresentationStatus::Presented,
            )),
            Some(pushed_summary.round.clone()),
        )
        .await;
    assert!(
        matches!(
            acked,
            Ok(WirePayload::UndeliveredAckOutcome(
                UndeliveredAckOutcome::Presented { .. }
            ))
        ),
        "ACK must present, got {acked:?}"
    );
    assert_eq!(
        transport.sends(),
        6,
        "the first ACK presents without an extra round"
    );
    // After the ACK the backlog drains: a fresh fetch shows no rows.
    let answer = ask(
        &mut client,
        WirePayload::UndeliveredRequest(cmds::undelivered_request(None, None, false)),
        "post-ack fetch",
    )
    .await
    .expect("post-ack fetch must answer");
    let WirePayload::UndeliveredResponse(ene_api::v1::undelivered::UndeliveredResponse::Summary(
        drained,
    )) = answer
    else {
        panic!("post-ack fetch must summarize, got {answer:?}");
    };
    assert!(
        drained.items.is_empty(),
        "acked rows must not re-display, got {:?}",
        drained.items.len()
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("report.md")).unwrap(),
        "# Report\nnotes",
        "the Host keeps the write and the result"
    );
    server.abort();
}

/// S5-05: the old close never clears the new current, in both orders: (A)
/// the old close completes before the new auth, (B) the new auth installs
/// first and the old close races in after. The same-admission comparison is
/// serve-level; these are the two orders' end states over real sockets.
#[tokio::test]
async fn s5_05_old_close_never_clears_new_current_both_orders() {
    // Order A: close, then auth.
    let temp_a = tempfile::TempDir::new().unwrap();
    let dir_a = temp_a.path().to_path_buf();
    let transport_a = Arc::new(GateTransport::new(
        vec![String::from("hi-a1"), String::from("hi-a2")],
        &[],
    ));
    let (_handle_a, server_a, mut c1) =
        serve_and_setup(dir_a.clone(), Arc::clone(&transport_a)).await;
    let (round_a, stream_a, _) = send_round(&mut c1, "hi")
        .await
        .expect("order-A first round must complete");
    confirm_round(&mut c1, &round_a, stream_a).await;
    drop(c1);
    let mut c2 = Client::connect(&dir_a, DESCRIPTOR, "test")
        .await
        .expect("order-A re-auth must succeed");
    let (_round_a2, stream_a2, chat_a) = send_round(&mut c2, "hi again")
        .await
        .expect("order-A current must serve");
    assert_eq!(chat_a, "hi-a2");
    confirm_round(&mut c2, &_round_a2, stream_a2).await;
    server_a.abort();

    // Order B: auth, then the old close.
    let temp_b = tempfile::TempDir::new().unwrap();
    let dir_b = temp_b.path().to_path_buf();
    let transport_b = Arc::new(GateTransport::new(
        vec![String::from("hi-b1"), String::from("hi-b2")],
        &[],
    ));
    let (_handle_b, server_b, mut c1) =
        serve_and_setup(dir_b.clone(), Arc::clone(&transport_b)).await;
    let (round_b, stream_b, _) = send_round(&mut c1, "hi")
        .await
        .expect("order-B first round must complete");
    confirm_round(&mut c1, &round_b, stream_b).await;
    let mut c2 = Client::connect(&dir_b, DESCRIPTOR, "test")
        .await
        .expect("order-B second auth must succeed");
    drop(c1);
    // The order is fixed above (auth before close); the end-state round
    // itself is the event gate — it retries to a deadline instead of
    // sleeping a fixed window for the server-side close to land.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (_round_b2, stream_b2, chat_b) = loop {
        match send_round(&mut c2, "hi again").await {
            Ok(done) => break done,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => panic!("order-B current must survive the old close: {error}"),
        }
    };
    assert_eq!(chat_b, "hi-b2");
    confirm_round(&mut c2, &_round_b2, stream_b2).await;
    server_b.abort();
}

fn only_task_uuid(dir: &std::path::Path) -> String {
    task_uuids(dir)
        .into_iter()
        .next()
        .expect("one task must exist")
}

/// All task UUIDs in creation order. The wire purpose ID prefixes the raw
/// task UUID (`encode_purpose`), so these correlate list rows across
/// connections, where wire refs are re-minted.
fn task_uuids(dir: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    let mut statement = conn
        .prepare("SELECT task_id FROM task ORDER BY rowid")
        .expect("tasks must list");
    statement
        .query_map((), |row| row.get(0))
        .expect("task ids must read")
        .collect::<Result<Vec<String>, _>>()
        .expect("task ids must decode")
}

/// S5-07: after a normal disconnect and reconnect, nothing old replays: the
/// pre-restart round id is stale, the old connection's receipt never
/// migrates, management stays readable, and only a fresh summon starts a
/// new round. Auth alone restores nothing (the fresh submit summons).
#[tokio::test]
async fn s5_07_stale_round_input_and_ack_never_replay() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let transport = Arc::new(GateTransport::new(
        vec![String::from("one"), String::from("two")],
        &[],
    ));
    let (_handle, server, mut c1) = serve_and_setup(dir.clone(), Arc::clone(&transport)).await;
    let (round1, _stream1, _) = send_round(&mut c1, "hello")
        .await
        .expect("first round must complete");
    // Leave the reply unconfirmed so an undelivered receipt exists.
    let receipt = fetch_summary(&mut c1, "fetch")
        .await
        .expect("fetch must answer");
    assert!(!receipt.items.is_empty(), "the reply must linger");
    drop(c1);
    // The disconnect fallback is a durable fact: wait for it before the new
    // connection so the stale-round probe cannot race it (the fallback
    // decides whether the old round can still match the current presence).
    wait_presence(&dir, "no_active").await;

    let mut c2 = Client::connect(&dir, DESCRIPTOR, "test")
        .await
        .expect("reconnect must succeed");
    // The pre-restart round never resumes.
    let companion = c2.companion_ref();
    let stale = ask(
        &mut c2,
        WirePayload::SubmitTextInput(cmds::submit_input(
            &companion,
            Some(round1.clone()),
            false,
            String::from("old round retry"),
            String::from("en"),
        )),
        "stale probe",
    )
    .await
    .expect("stale probe must answer");
    assert!(
        !matches!(
            stale,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        ),
        "pre-restart rounds must not resume, got {stale:?}"
    );
    // The old connection's receipt never migrates to the new one.
    let migrated = ack_summary(&mut c2, &receipt)
        .await
        .expect("old ACK must answer");
    assert!(
        matches!(
            migrated,
            UndeliveredAckOutcome::StaleConnection | UndeliveredAckOutcome::UnknownRef
        ),
        "old ACKs never cross connections, got {migrated:?}"
    );
    // Management stays readable without presence...
    view_mark(&mut c2).await.expect("view must read");
    let page = list_tasks(&mut c2).await.expect("list must read");
    assert!(page.tasks.is_empty());
    // ...and only the fresh summon starts a new round.
    let (_round2, stream2, chat) = send_round(&mut c2, "fresh start")
        .await
        .expect("post-reconnect round must complete");
    assert_eq!(chat, "two");
    confirm_round(&mut c2, &_round2, stream2).await;
    server.abort();
}

/// S5-09: while the progress receipt is on display, the completion commits;
/// ACKing the progress receipt presents only its batch, the completion
/// stays unpresented for the next display, and duplicate, forged-round, and
/// foreign-connection ACKs never corrupt either state.
#[tokio::test]
async fn s5_09_progress_ack_never_presents_later_completion() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    // Calls: 1 propose, 2 read, 3 create, 4 final (held), 5 post-drop chat.
    let transport = Arc::new(GateTransport::new(
        vec![
            task_reply(
                serde_json::json!({"kind": "propose_task", "purpose": "read input.txt and write report.md"}),
            ),
            String::from(r#"{"tool":"read","path":"input.txt"}"#),
            String::from(
                "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}",
            ),
            String::from(r#"{"final":"created report.md from input.txt"}"#),
            String::from("Later, then."),
        ],
        &[4],
    ));
    let (_handle, server, mut c1) = serve_and_setup(dir.clone(), Arc::clone(&transport)).await;
    select_workspace(&mut c1, workspace.path())
        .await
        .expect("workspace must select");
    let (_round1, _stream1, _) = send_round(&mut c1, "please read input.txt and write report.md")
        .await
        .expect("propose round must complete");
    // The propose submit summons and publishes the presence fact, so this
    // session already echoes the generation the receipts below are stamped
    // with: every ACK leg answers its own outcome with no probe round.
    transport.wait_sends(4).await;

    // Display the progress batch while the completion is still gated.
    let progress = fetch_summary(&mut c1, "progress fetch")
        .await
        .expect("progress fetch must answer");
    assert!(!progress.items.is_empty(), "progress must display");

    // The completion commits; ACKing the progress receipt presents only its
    // batch, never the completion.
    transport.unblock(4);
    let page = wait_task_progress(&mut c1, "completed", 1)
        .await
        .expect("task must complete");
    assert_eq!(page.tasks[0].revision, 1);
    assert_eq!(
        transport.sends(),
        4,
        "the held call resumes, nothing re-sends"
    );
    let acked = ack_summary(&mut c1, &progress)
        .await
        .expect("progress ACK must answer");
    assert!(
        matches!(acked, UndeliveredAckOutcome::Presented { .. }),
        "progress must present, got {acked:?}"
    );
    let completion = fetch_summary(&mut c1, "completion fetch")
        .await
        .expect("completion fetch must answer");
    assert!(
        !completion.items.is_empty(),
        "the completion must display next"
    );
    assert_ne!(
        completion.receipt, progress.receipt,
        "the completion rides a new receipt"
    );

    // A duplicate ACK of the consumed progress receipt changes nothing...
    let dup = ack_summary(&mut c1, &progress)
        .await
        .expect("duplicate ACK must answer");
    assert!(
        matches!(dup, UndeliveredAckOutcome::StalePresentation),
        "consumed receipts stay stale, got {dup:?}"
    );
    // ...a forged round on the live completion receipt is stale, never
    // applied...
    let forged = c1
        .request_observed(
            WirePayload::UndeliveredAck(cmds::undelivered_ack(
                &completion.receipt.0,
                PresentationStatus::Presented,
            )),
            Some(ene_api::v1::refs::RoundWireId(String::from(
                "forged-old-round",
            ))),
        )
        .await
        .expect("forged ACK must answer");
    assert!(
        matches!(
            forged,
            WirePayload::UndeliveredAckOutcome(UndeliveredAckOutcome::StalePresentation)
        ),
        "forged rounds never apply, got {forged:?}"
    );
    // ...and the completion rows stay unpresented throughout.
    let again = fetch_summary(&mut c1, "completion refetch")
        .await
        .expect("completion refetch must answer");
    assert_eq!(
        again.receipt, completion.receipt,
        "unpresented rows keep their receipt"
    );

    // The old receipt never migrates: on a new connection it answers stale
    // while the rows re-present under the new receipt, which then presents.
    drop(c1);
    let mut c2 = Client::connect(&dir, DESCRIPTOR, "test")
        .await
        .expect("reconnect must succeed");
    let (_round2, stream2, chat) = send_round(&mut c2, "later, then")
        .await
        .expect("summon round must complete");
    assert_eq!(chat, "Later, then.");
    confirm_round(&mut c2, &_round2, stream2).await;
    let foreign = ack_summary(&mut c2, &completion)
        .await
        .expect("foreign ACK must answer");
    assert!(
        matches!(foreign, UndeliveredAckOutcome::StaleConnection),
        "ACKs never cross connections, got {foreign:?}"
    );
    let represented = fetch_summary(&mut c2, "re-present fetch")
        .await
        .expect("re-present fetch must answer");
    assert!(
        !represented.items.is_empty(),
        "the completion must re-present"
    );
    let acked = ack_summary(&mut c2, &represented)
        .await
        .expect("re-presented ACK must answer");
    assert!(
        matches!(acked, UndeliveredAckOutcome::Presented { .. }),
        "re-presented rows must present, got {acked:?}"
    );
    let drained = fetch_summary(&mut c2, "final fetch")
        .await
        .expect("final fetch must answer");
    assert!(drained.items.is_empty(), "acked rows must drain");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("report.md")).unwrap(),
        "# Report\nnotes"
    );
    server.abort();
}

/// S5-17 (ordered gates; the true race is unit-level in
/// `concurrent_resume_commands_commit_at_most_once`): of two resumes at the
/// same revision exactly one commits r+1 and launches once; the loser is
/// stale, cancel stays effective on the new revision, and terminal refuses.
/// S5-18: the same-epoch retry replays the first outcome without a second
/// launch; after a crash the old command never auto-resends (stale epoch),
/// and a fresh command on the committed-away revision is stale while the
/// new revision waits unexecuted.
#[tokio::test]
async fn s5_17_18_resume_gates_and_retry_idempotency() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    // T_a calls: 1 propose, 2 read, 3 create (held: interrupted by crash).
    let transport_a = Arc::new(GateTransport::new(
        vec![
            task_reply(
                serde_json::json!({"kind": "propose_task", "purpose": "read input.txt and write report.md"}),
            ),
            String::from(r#"{"tool":"read","path":"input.txt"}"#),
        ],
        &[3],
    ));
    let (_handle, server, mut c1) = serve_and_setup(dir.clone(), Arc::clone(&transport_a)).await;
    select_workspace(&mut c1, workspace.path())
        .await
        .expect("workspace must select");
    let (round_a, stream_a, _) = send_round(&mut c1, "please read input.txt and write report.md")
        .await
        .expect("propose must complete");
    confirm_round(&mut c1, &round_a, stream_a).await;
    transport_a.wait_sends(3).await;
    drop(c1);

    // Restart with the resumed execution held before its first send, so no
    // completion can interfere with the gate assertions.
    let transport_b = Arc::new(GateTransport::new(vec![String::from("held")], &[1]));
    transport_a.fail(3);
    let (_handle, server) = restart_host(&dir, server, Arc::clone(&transport_b)).await;
    let mut c2 = Client::connect(&dir, DESCRIPTOR, "test")
        .await
        .expect("reconnect must succeed");
    let page = list_tasks(&mut c2).await.expect("list must read");
    assert_eq!(page.tasks.len(), 1);
    assert_eq!(page.tasks[0].revision, 1);
    let wire = page.tasks[0].task.0.clone();
    let purpose = page.tasks[0].purpose.clone();

    // Two resumes at the same revision: exactly one commits r+1.
    let prep1 = c2.prepare(WirePayload::ResumeTask(cmds::resume_task_request(
        &wire,
        1,
        &purpose,
        String::from("first"),
    )));
    let out1 = c2.execute(&prep1).await.expect("resume-1 must answer");
    let prep2 = c2.prepare(WirePayload::ResumeTask(cmds::resume_task_request(
        &wire,
        1,
        &purpose,
        String::from("second"),
    )));
    let out2 = c2.execute(&prep2).await.expect("resume-2 must answer");
    let WirePayload::ResumeTaskOutcome(first) = out1.clone() else {
        panic!("resume-1 must answer an outcome, got {out1:?}");
    };
    let WirePayload::ResumeTaskOutcome(second) = out2 else {
        panic!("resume-2 must answer an outcome, got {out2:?}");
    };
    assert!(
        matches!(first, ResumeTaskOutcomeWire::Resumed { revision: 2, .. }),
        "one resume must win r+1, got {first:?}"
    );
    assert!(
        matches!(
            second,
            ResumeTaskOutcomeWire::StalePremise {
                current_revision: 2
            }
        ),
        "the loser must go stale, got {second:?}"
    );
    transport_b.wait_sends(1).await;
    assert_eq!(transport_b.sends(), 1, "only the winner launches once");
    assert_eq!(table_count(&dir, "delegation"), 2, "one new delegation");

    // The same-epoch retry replays the first outcome with no second launch.
    let replay = c2.retry(&prep1).await.expect("retry must answer");
    assert_eq!(replay, out1, "retries replay instead of recommitting");
    assert_eq!(transport_b.sends(), 1, "retry launches nothing");
    assert_eq!(table_count(&dir, "delegation"), 2, "retry commits nothing");

    // End the held call without a result, then restart silent:
    // the old command never auto-resends. Over the socket the old
    // connection's task ref already resolves to nothing (refs are
    // connection-scoped, so ref resolution precedes the epoch compare that
    // answers StaleConnection at handle level); either way nothing commits
    // and nothing launches.
    drop(c2);
    let transport_c = Arc::new(GateTransport::new(Vec::new(), &[]));
    transport_b.fail(1);
    let (_handle, server) = restart_host(&dir, server, Arc::clone(&transport_c)).await;
    let mut c3 = Client::connect(&dir, DESCRIPTOR, "test")
        .await
        .expect("second reconnect must succeed");
    let stale_epoch = c3.retry(&prep1).await.expect("old retry must answer");
    assert!(
        matches!(
            stale_epoch,
            WirePayload::ResumeTaskOutcome(ResumeTaskOutcomeWire::UnknownRef)
        ),
        "old commands never auto-resend across connections, got {stale_epoch:?}"
    );
    let page = list_tasks(&mut c3).await.expect("list must read");
    let wire_c = page.tasks[0].task.0.clone();
    let purpose_c = page.tasks[0].purpose.clone();
    assert_eq!(page.tasks[0].revision, 2);
    assert!(!page.tasks[0].running, "the committed r2 waits unexecuted");
    let stale_rev = ask(
        &mut c3,
        WirePayload::ResumeTask(cmds::resume_task_request(
            &wire_c,
            1,
            &purpose_c,
            String::from("continue again"),
        )),
        "stale-revision resume",
    )
    .await
    .expect("stale resume must answer");
    assert!(
        matches!(
            stale_rev,
            WirePayload::ResumeTaskOutcome(ResumeTaskOutcomeWire::StalePremise {
                current_revision: 2
            })
        ),
        "committed-away revisions stay stale, got {stale_rev:?}"
    );
    assert_eq!(transport_c.sends(), 0, "no auto-resend, no relaunch");

    // Cancel stays effective on the new revision; the cancelled terminal
    // then refuses resume.
    let task_uuid: uuid::Uuid = only_task_uuid(&dir).parse().expect("task id parses");
    let mark = view_mark(&mut c3).await.expect("view must read");
    let cancelled = ask(
        &mut c3,
        WirePayload::ManagementIntent(ManagementIntent {
            intent_id: CommandWireId(uuid::Uuid::new_v4()),
            kind: ManagementIntentKind::CancelTask,
            target: ene_api::v1::management::task_target(task_uuid),
            base_view: BaseViewMark(mark),
            rationale: IntentRationaleWire {
                origin: RationaleOrigin::ManagementSurface,
                quote: None,
            },
            confirmed: false,
        }),
        "cancel-r2",
    )
    .await
    .expect("cancel must answer");
    assert!(
        matches!(
            cancelled,
            WirePayload::ManagementOutcome(ManagementOutcome::AppliedAsOneTime)
        ),
        "cancel must apply, got {cancelled:?}"
    );
    let page = list_tasks(&mut c3).await.expect("list must read");
    assert_eq!(page.tasks[0].progress, "cancelled");
    let wire_c2 = page.tasks[0].task.0.clone();
    let purpose_c2 = page.tasks[0].purpose.clone();
    let refused = ask(
        &mut c3,
        WirePayload::ResumeTask(cmds::resume_task_request(
            &wire_c2,
            2,
            &purpose_c2,
            String::from("once more"),
        )),
        "resume-cancelled",
    )
    .await
    .expect("must answer");
    assert!(
        matches!(
            refused,
            WirePayload::ResumeTaskOutcome(ResumeTaskOutcomeWire::TaskTerminal { .. })
        ),
        "cancelled tasks refuse resume, got {refused:?}"
    );
    assert_eq!(table_count(&dir, "delegation"), 2, "nothing else committed");
    assert_eq!(transport_c.sends(), 0, "still nothing launched");
    server.abort();
}

/// Observation poll for a durable table count (background completion, not
/// ordering; ordering uses provider gates).
fn delegation_ids(dir: &std::path::Path, task: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    let mut statement = conn
        .prepare("SELECT delegation_id FROM delegation WHERE task_id = ?1 ORDER BY rowid")
        .expect("delegations must list");
    statement
        .query_map(rusqlite::params![task], |row| row.get(0))
        .expect("delegation ids must read")
        .collect::<Result<Vec<String>, _>>()
        .expect("delegation ids must decode")
}

fn task_results(dir: &std::path::Path) -> Vec<(String, i64, String, Option<i64>)> {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    let mut statement = conn
        .prepare(
            "SELECT result_id, task_revision, delegation_id, adopted_revision FROM task_result ORDER BY rowid",
        )
        .expect("results must list");
    statement
        .query_map((), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("results must read")
        .collect::<Result<Vec<_>, _>>()
        .expect("results must decode")
}

/// S5-19: after an explicit resume, the old delegation's late result is
/// recorded once to the original execution and sealed there — adoption is
/// original-only, the resumed Task waits for (and completes with) only its
/// own delegation, and old evidence never moves into the new delegation's
/// relied set. The old provider call ends without a result before graceful
/// restart. A late-arrival fixture then enters the successor's canonical
/// arrival/adoption boundaries with the immutable original execution refs;
/// no predecessor Host or runner survives into the successor's lifetime.
/// Late failures go stale and settlements stay attempt-local at owner level
/// (`result_arrival` / `resume_orchestration`); the socket E2E pins the
/// result-routing observables.
#[tokio::test]
async fn s5_19_late_arrival_stays_with_original_execution() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    // T_a calls: 1 propose, 2 read, 3 create, 4 final-d1 (held, then failed
    // without a result so graceful shutdown can join the original runner).
    let transport_a = Arc::new(GateTransport::new(
        vec![
            task_reply(
                serde_json::json!({"kind": "propose_task", "purpose": "read input.txt and write report.md"}),
            ),
            String::from(r#"{"tool":"read","path":"input.txt"}"#),
            String::from(
                "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}",
            ),
            String::from(r#"{"final":"created report.md"}"#),
        ],
        &[4],
    ));
    let (_handle, server, mut c1) = serve_and_setup(dir.clone(), Arc::clone(&transport_a)).await;
    select_workspace(&mut c1, workspace.path())
        .await
        .expect("workspace must select");
    let (round_a, stream_a, _) = send_round(&mut c1, "please read input.txt and write report.md")
        .await
        .expect("propose must complete");
    confirm_round(&mut c1, &round_a, stream_a).await;
    transport_a.wait_sends(4).await;
    let uuid = only_task_uuid(&dir);
    let d1 = delegation_ids(&dir, &uuid)
        .into_iter()
        .next()
        .expect("d1 must exist");
    let original = {
        let predecessor = _handle.upgrade().expect("predecessor is serving");
        predecessor
            .store_for_tests()
            .load_delegation(ene_task::DelegationId::from_raw(
                ene_primitive::RawId::from_uuid(uuid::Uuid::parse_str(&d1).unwrap()),
            ))
            .await
            .expect("original delegation must load")
            .expect("original delegation must exist")
    };
    assert_eq!(original.task.revision.as_u64(), 1);
    transport_a.fail(4);
    drop(c1);

    // Join the old runner before opening the restarted Host (empty launch
    // registry: no AlreadyRunning). The new delegation's
    // own final is held so the late arrival lands while r2 still waits.
    // T_b calls: 1 create-B2, 2 final-B2 (held).
    let transport_b = Arc::new(GateTransport::new(
        vec![
            String::from(
                "{\"tool\":\"create\",\"path\":\"report2.md\",\"content\":\"# Second\\nnotes\"}",
            ),
            String::from(r#"{"final":"created report2.md"}"#),
        ],
        &[2],
    ));
    let (_handle, server) = restart_host(&dir, server, Arc::clone(&transport_b)).await;
    assert_eq!(
        table_count(&dir, "task_result"),
        0,
        "the old call ended without a result"
    );
    let mut c2 = Client::connect(&dir, DESCRIPTOR, "test")
        .await
        .expect("reconnect must succeed");
    let page = list_tasks(&mut c2).await.expect("list must read");
    assert_eq!(page.tasks[0].revision, 1);
    assert!(!page.tasks[0].running);
    let wire = page.tasks[0].task.0.clone();
    let purpose = page.tasks[0].purpose.clone();
    let resumed = ask(
        &mut c2,
        WirePayload::ResumeTask(cmds::resume_task_request(
            &wire,
            1,
            &purpose,
            String::from("finish the rest"),
        )),
        "resume",
    )
    .await
    .expect("resume must answer");
    assert!(
        matches!(
            resumed,
            WirePayload::ResumeTaskOutcome(ResumeTaskOutcomeWire::Resumed { revision: 2, .. })
        ),
        "resume must mint r+1, got {resumed:?}"
    );
    transport_b.wait_sends(2).await;
    let ids = delegation_ids(&dir, &uuid);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], d1);

    // The old final arrives late: recorded once to the original execution
    // and sealed there, never adopted as the Task's completion — r2 still
    // waits for its own delegation.
    {
        let successor = _handle.upgrade().expect("successor is serving");
        let store = successor.store_for_tests();
        let credentials = memory_store();
        let scrubbed = CredentialScrubber {
            refs: store,
            store: &credentials,
        }
        .scrub("created report.md")
        .await
        .expect("late result must be scrubbed");
        let arrival = ene_task::orchestrate_result_arrival(
            store,
            original.delegation,
            ene_task::TaskResultScrubPremise::from_scrubbed(scrubbed),
        )
        .await
        .expect("canonical late arrival must record");
        let ene_task::TaskResultArrivalOutcome::Recorded(result) = arrival else {
            panic!("late arrival must have a current credential premise");
        };
        assert_eq!(result.task, original.task);
        assert_eq!(result.delegation, original.delegation);
        let claim = store
            .load_result_adoption_claim(result.result)
            .await
            .expect("original execution's attempt set must load")
            .expect("late result must exist");
        assert_eq!(
            claim.attempt_refs.len(),
            2,
            "only the original read and create belong to d1"
        );
        assert_eq!(
            store
                .adopt_result(claim)
                .await
                .expect("late adoption must answer"),
            ene_task::TaskResultAcceptance::RecordedToOriginalOnly,
        );
    }
    let results = task_results(&dir);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].1, 1, "old result stays at its revision");
    assert_eq!(results[0].2, d1, "old result stays with d1");
    assert_eq!(
        results[0].3, None,
        "old result is sealed, not adopted as completion"
    );
    let page = list_tasks(&mut c2).await.expect("list must read");
    assert_eq!(page.tasks[0].progress, "in_progress");
    assert_eq!(page.tasks[0].revision, 2);
    assert_eq!(transport_a.sends(), 4, "the old run sent nothing more");
    assert_eq!(transport_b.sends(), 2, "the new run still waits");

    // The new delegation then completes with its own result, which alone is
    // adopted — old evidence never moves into its relied set.
    transport_b.unblock(2);
    let page = wait_task_progress(&mut c2, "completed", 1)
        .await
        .expect("resumed task must complete");
    assert_eq!(page.tasks[0].revision, 2);
    let results = task_results(&dir);
    assert_eq!(results.len(), 2, "one result per execution");
    assert_eq!(results[0].2, d1, "d1 keeps its result");
    assert_eq!(results[0].3, None, "d1 stays sealed-only");
    assert_eq!(results[1].2, ids[1], "d2 records its own result");
    assert_eq!(results[1].1, 2, "new result belongs to r2");
    assert_eq!(
        results[1].3,
        Some(2),
        "only the new result is adopted as completion"
    );
    assert_eq!(table_count(&dir, "delegation"), 2);
    assert_eq!(
        table_count(&dir, "action_attempt"),
        3,
        "read, create, create-B2: no replay, no re-attach rerun"
    );
    let d2_attempts: i64 = {
        let conn =
            rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
        conn.query_row(
            "SELECT COUNT(*) FROM action_attempt WHERE delegation_id = ?1",
            rusqlite::params![ids[1]],
            |row| row.get(0),
        )
        .expect("d2 attempts must read")
    };
    assert_eq!(
        d2_attempts, 1,
        "the new delegation relies on no old attempt as action"
    );
    let inputs = transport_b.input_texts().join("\n");
    assert!(
        inputs.contains("input.txt"),
        "the new run still sees executed facts"
    );
    assert!(workspace.path().join("report.md").exists());
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("report2.md")).unwrap(),
        "# Second\nnotes"
    );
    server.abort();
}

/// S5 subscription over the real socket: after the attach-time backlog is
/// drained, a new deliverable fact produced by the Task runner is pushed to
/// the idle Client without it sending another request.
#[tokio::test]
async fn s5_subscription_pushes_a_new_arrival_without_a_request() {
    let temp = tempfile::TempDir::new().unwrap();
    let dir = temp.path().to_path_buf();
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    // Call 1 is the propose turn; call 2 is the runner's first provider call,
    // held until the Client has gone idle.
    let transport = Arc::new(GateTransport::new(
        vec![
            task_reply(
                serde_json::json!({"kind": "propose_task", "purpose": "read input.txt and write report.md"}),
            ),
            String::from(r#"{"tool":"read","path":"input.txt"}"#),
            String::from(r#"{"final":"read input.txt"}"#),
        ],
        &[2],
    ));
    let (_handle, server, mut client) = serve_and_setup(dir.clone(), Arc::clone(&transport)).await;
    select_workspace(&mut client, workspace.path())
        .await
        .expect("workspace must select");
    let (round_wire, stream_id, text_out) = send_round(&mut client, "please read input.txt")
        .await
        .expect("the propose round must complete");
    assert!(text_out.contains("Task accepted"), "{text_out}");
    confirm_round(&mut client, &round_wire, stream_id).await;

    // The runner is parked on its first provider call, and the Client sends
    // nothing further from here on.
    transport.wait_sends(2).await;
    transport.unblock(2);

    // The runner's committed facts push without any Client request.
    let pushed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let next = client.next_frame().await.expect("a frame must read");
            if let WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(summary)) = next
                && !summary.items.is_empty()
            {
                return summary;
            }
        }
    })
    .await
    .expect("the runner's facts must push without a request");
    assert!(
        pushed
            .items
            .iter()
            .any(|item| item.source.kind != "history_message"),
        "the pushed page carries the runner's Task facts, got {:?}",
        pushed
            .items
            .iter()
            .map(|item| item.source.kind.clone())
            .collect::<Vec<_>>()
    );
    let outcome = ack_summary(&mut client, &pushed)
        .await
        .expect("the pushed receipt must ACK");
    assert!(
        matches!(outcome, UndeliveredAckOutcome::Presented { .. }),
        "got {outcome:?}"
    );
    server.abort();
}
