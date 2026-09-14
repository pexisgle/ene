//! Stage 4 slice F conversation-path E2E: every Task control operation is
//! started by an ordinary Owner conversation input, the Workspace comes from
//! the trusted first-party selection, and the production launcher starts the
//! existing Task Agent runner.
//!
//! The dialogue provider output carries the companion's closed-world
//! `[task-control]` directive; the production `finish_turn` interprets it, and
//! the Host composition root executes it through the existing Task owner
//! boundaries. The test never calls `propose_task` / `propose_steering` /
//! `cancel_task` / `task_report` / `run_task_agent` itself: it only scripts
//! the provider output (the model's answer), selects the trusted Workspace
//! through the first-party management inlet, and observes durable results.
//!
//! Acceptance scenario 4: 4.1/4.2 (request, association, delegation, runner),
//! 4.3 (normal chat while the Task runs), 4.4 (cancel report of what
//! completed and what is unresolved), 4.5 (completion report with changed
//! file names, save location, and remainder), and 4.7 (existing workspace
//! files survive completion and cancel).

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures and helpers live outside #[test] functions, where clippy.toml's test allowances do not apply"
)]

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ene_action::{
    ActionAttemptId, ActionAttemptRepository as _, ActionStartOutcome, AttemptCommitPremise,
    OperationKind, RealTargetRef,
};
use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::management::{
    IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome,
    RationaleOrigin, workspace_target,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{BaseViewMark, ClientIncarnationId, CommandWireId, WireMessageType};
use ene_api::v1::round::StreamClose;
use ene_companion::{CompanionRepository as _, HistoryRepository as _, HistoryRole};
use ene_inference::fake::{FakeFailure, FakeProviderTransport};
use ene_inference::{
    InferenceTechnicalError, ProviderRequest, ProviderResponse, ProviderTransport,
};
use ene_plugin_ipc::WireFrame;
use ene_primitive::{RawId, RevisionInner};
use ene_task::{DelegationId, TaskContextItem, TaskId, TaskProgress, TaskRepository as _};
use serde_json::json;
use tokio::sync::{Notify, Semaphore};

use super::task_run::ScriptedTransport;
use super::{accepted_round, current_generation, round_test_handle, submit_frame};
use crate::serve::{HostHandle, LiveInput};
use crate::task_run::BackgroundTaskAgent;
use crate::test_support::live_input;

/// Provider transport with scripted replies that can block at a chosen call,
/// so a test can hold one Task Agent execution in flight while another runs.
/// It also captures every logical input so a test can prove the adopted
/// instruction reached the replacement execution.
pub(super) struct GateTransport {
    replies: Mutex<VecDeque<String>>,
    calls: AtomicUsize,
    inputs: Mutex<Vec<String>>,
    /// 1-based call index to block at before returning its scripted reply;
    /// `0` blocks only when the script runs out.
    block_at: AtomicUsize,
    arrived: Notify,
    release: Semaphore,
}

impl GateTransport {
    pub(super) fn new(replies: Vec<String>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            calls: AtomicUsize::new(0),
            inputs: Mutex::new(Vec::new()),
            block_at: AtomicUsize::new(0),
            arrived: Notify::new(),
            release: Semaphore::new(0),
        }
    }

    /// Blocks the given 1-based call until the test releases it.
    pub(super) fn block_at(self, call: usize) -> Self {
        self.block_at.store(call, Ordering::SeqCst);
        self
    }

    /// Releases one blocked call.
    pub(super) fn release(&self) {
        self.release.add_permits(1);
    }

    pub(super) fn inputs(&self) -> Vec<String> {
        self.inputs.lock().expect("input capture lock").clone()
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Waits until at least `wanted` provider calls have started.
    pub(super) async fn wait_calls(&self, wanted: usize) {
        while self.calls() < wanted {
            self.arrived.notified().await;
        }
    }
}

impl ProviderTransport for GateTransport {
    fn complete(
        &self,
        req: ProviderRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<ProviderResponse, InferenceTechnicalError>>
                + Send
                + '_,
        >,
    > {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.inputs
            .lock()
            .expect("input capture lock")
            .push(req.input.clone());
        let reply = self.replies.lock().expect("gate script lock").pop_front();
        let block_at = self.block_at.load(Ordering::SeqCst);
        let blocked = reply.is_none() || (block_at != 0 && call == block_at);
        let arrived = &self.arrived;
        let release = &self.release;
        Box::pin(async move {
            arrived.notify_waiters();
            if blocked {
                let permit = release.acquire().await.expect("gate stays open");
                permit.forget();
            }
            let text = reply.unwrap_or_else(|| String::from("released"));
            Ok(ProviderResponse { text, usage: None })
        })
    }
}

/// One provider reply carrying exactly one companion task-control directive.
///
/// The directive must be the reply's first non-empty line with no other
/// prose, so the reply is exactly the marker line.
fn task_reply(directive: serde_json::Value) -> String {
    format!("[task-control] {directive}")
}

fn sender() -> WireSender {
    WireSender {
        device_id: None,
        incarnation_id: ClientIncarnationId {
            counter: 3,
            random: 4,
        },
        connection_id: None,
    }
}

fn request_frame(
    handle: &HostHandle,
    live: &LiveInput,
    generation: Option<u64>,
    local_id: &str,
    text: &str,
) -> WireFrame {
    submit_frame(
        handle.companion_wire(),
        generation,
        None,
        local_id,
        text,
        live.connection_id,
    )
}

/// One first-party Workspace selection over the management inlet.
fn workspace_intent_frame(live: &LiveInput, path: &str) -> WireFrame {
    let mut envelope = new_outgoing_envelope(
        ProtocolVersion::V1,
        sender(),
        WireMessageType(String::from("ManagementIntent")),
    );
    envelope.sender.connection_id = Some(live.connection_id);
    WireFrame {
        envelope,
        payload: WirePayload::ManagementIntent(ManagementIntent {
            intent_id: CommandWireId(RawId::new().as_uuid()),
            kind: ManagementIntentKind::SelectWorkspace,
            target: workspace_target(path),
            base_view: BaseViewMark(String::from("mark")),
            rationale: IntentRationaleWire {
                origin: RationaleOrigin::ManagementSurface,
                quote: None,
            },
        }),
    }
}

/// Selects the trusted Workspace and requires the Host to apply it.
async fn select_workspace(handle: &HostHandle, live: &LiveInput, path: &Path) {
    let responses = handle
        .handle_frame(
            workspace_intent_frame(live, &path.to_string_lossy()),
            live.clone(),
            &ScriptedTransport::new(Vec::new()),
        )
        .await;
    let Some(WirePayload::ManagementOutcome(ManagementOutcome::AppliedAsOneTime)) =
        responses.first().map(|frame| &frame.payload)
    else {
        panic!("the Owner workspace selection must apply, got {responses:?}");
    };
}

/// Sends one Owner message and returns the response frames, requiring the
/// turn to complete.
async fn send_owner_message(
    handle: &HostHandle,
    live: &LiveInput,
    transport: &impl ProviderTransport,
    local_id: &str,
    text: &str,
) -> Vec<WireFrame> {
    let generation = current_generation(handle).await.expect("generation");
    let responses = handle
        .handle_frame(
            request_frame(handle, live, Some(generation), local_id, text),
            live.clone(),
            transport,
        )
        .await;
    assert!(
        accepted_round(&responses).is_ok(),
        "the owner message must be accepted: {responses:?}"
    );
    assert!(
        closes_completed(&responses),
        "the owner turn must complete: {responses:?}"
    );
    responses
}

fn closes_completed(responses: &[WireFrame]) -> bool {
    responses.iter().any(|frame| {
        matches!(
            &frame.payload,
            WirePayload::TextStreamClose(close) if close.status == StreamClose::Completed
        )
    })
}

/// The user-visible provider deltas of one turn, concatenated in order.
fn streamed_text(responses: &[WireFrame]) -> String {
    let mut text = String::new();
    for frame in responses {
        if let WirePayload::TextStreamFrame(delta) = &frame.payload
            && !delta.is_final
        {
            text.push_str(&delta.delta);
        }
    }
    text
}

fn contains_directive(responses: &[WireFrame]) -> bool {
    let text = streamed_text(responses);
    text.contains("[task-control]") || (text.contains("\"kind\"") && text.contains("\"purpose\""))
}

async fn latest_companion_message(handle: &HostHandle) -> String {
    latest_message_with_role(handle, HistoryRole::Companion)
        .await
        .map(|(_, text)| text)
        .expect("the companion reply is in the timeline")
}

async fn latest_owner_message(handle: &HostHandle) -> RawId {
    latest_message_with_role(handle, HistoryRole::Owner)
        .await
        .map(|(id, _)| id)
        .expect("the owner message is in the timeline")
}

async fn latest_message_with_role(
    handle: &HostHandle,
    role: HistoryRole,
) -> Option<(RawId, String)> {
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve");
    let items = handle
        .store
        .load_recent_timeline(companion, 16)
        .await
        .expect("the timeline must load");
    items
        .iter()
        .rev()
        .find(|item| item.role == role)
        .map(|item| (item.id, item.text.clone()))
}

/// The one Task the conversation created in this fresh store.
fn only_task(dir: &Path) -> TaskId {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    let text: String = conn
        .query_row(
            "SELECT task_id FROM task ORDER BY rowid DESC LIMIT 1",
            (),
            |row| row.get(0),
        )
        .expect("the conversation must have created a task");
    TaskId::from_raw(RawId::from_uuid(
        uuid::Uuid::parse_str(&text).expect("the stored task identity parses"),
    ))
}

fn probe_count(dir: &Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), (), |row| {
        row.get(0)
    })
    .expect("the probe count must read")
}

fn probe_workspace_rows(dir: &Path, folder: &str) -> i64 {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    conn.query_row(
        "SELECT COUNT(*) FROM workspace_assoc WHERE folder = ?1",
        rusqlite::params![folder],
        |row| row.get(0),
    )
    .expect("the workspace probe must read")
}

fn delegation_count(dir: &Path) -> i64 {
    probe_count(dir, "delegation")
}

/// The Task's newest delegation, discovered from the store for assertions
/// (the production launcher already started it).
fn latest_delegation(dir: &Path, task: TaskId) -> DelegationId {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    let text: String = conn
        .query_row(
            "SELECT delegation_id FROM delegation WHERE task_id = ?1 ORDER BY rowid DESC LIMIT 1",
            rusqlite::params![task.as_raw().as_uuid().as_hyphenated().to_string()],
            |row| row.get(0),
        )
        .expect("the conversation must have created a delegation");
    DelegationId::from_raw(RawId::from_uuid(
        uuid::Uuid::parse_str(&text).expect("the stored delegation identity parses"),
    ))
}

fn action_attempts(dir: &Path, delegation: DelegationId) -> i64 {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    conn.query_row(
        "SELECT COUNT(*) FROM action_attempt WHERE delegation_id = ?1",
        rusqlite::params![delegation.as_raw().as_uuid().as_hyphenated().to_string()],
        |row| row.get(0),
    )
    .expect("the action attempt probe must read")
}

async fn progress(handle: &HostHandle, task: TaskId) -> TaskProgress {
    handle
        .store
        .load_task(task)
        .await
        .unwrap()
        .expect("the task must load")
        .task
        .progress
}

/// Polls the durable progress until it reaches `expected` or the timeout
/// elapses; the runner is a background task, so this observes it without
/// starting it.
async fn wait_progress(handle: &HostHandle, task: TaskId, expected: TaskProgress) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if progress(handle, task).await == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the background execution did not reach {expected:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// The Host stores the canonicalized Workspace folder (Windows canonicalize
/// yields the extended-length form), so expectations compare canonically.
fn canonical_str(path: &Path) -> String {
    std::fs::canonicalize(path)
        .expect("the fixture path canonicalizes")
        .to_string_lossy()
        .into_owned()
}

/// A platform-absolute fixture target: the Action read-back requires an
/// absolute path on every supported platform.
fn canonical_target(name: &str) -> String {
    std::env::temp_dir()
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// Installs the production launcher over the shared handle and the test's
/// provider transport. The launcher itself is production composition; the
/// conversation turn starts the runner through it.
fn install_launcher<T>(handle: &Arc<HostHandle>, transport: Arc<T>)
where
    T: ProviderTransport + Send + Sync + 'static,
{
    assert!(
        handle.install_task_launcher(Arc::new(BackgroundTaskAgent::new(
            Arc::clone(handle),
            transport,
        ))),
        "the production launcher installs once"
    );
}

#[tokio::test]
async fn owner_selection_and_conversation_control_hide_the_directive() {
    let live = live_input("conversation-control");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let dialogue = ScriptedTransport::new(vec![
        task_reply(
            json!({"kind": "propose_task", "purpose": "read input.txt and write report.md"}),
        ),
        task_reply(
            json!({"kind": "steer", "instruction": "add an executive summary", "purpose": null}),
        ),
        task_reply(json!({"kind": "report"})),
        task_reply(json!({"kind": "cancel"})),
        task_reply(json!({"kind": "report"})),
    ]);
    let (handle, dir) = round_test_handle("conversation-control", &live, &dialogue)
        .await
        .expect("the production setup path completes");
    select_workspace(&handle, &live, workspace.path()).await;

    // Owner 1: ask for the file work. The companion's turn itself creates the
    // Task, confirms the Owner-selected Workspace, and delegates.
    let responses = send_owner_message(
        &handle,
        &live,
        &dialogue,
        "ask-task",
        "please read input.txt and write report.md",
    )
    .await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("Task accepted"), "{reply}");
    assert_eq!(
        streamed_text(&responses),
        reply,
        "the presented text equals the durable companion reply"
    );

    let task = only_task(dir.path());
    let record = handle
        .store
        .load_task(task)
        .await
        .unwrap()
        .expect("the conversation-created task must load");
    assert_eq!(record.task.progress, TaskProgress::InProgress);
    let association = record
        .workspace
        .as_ref()
        .expect("the trusted Workspace is associated");
    assert_eq!(association.folder.path, canonical_str(workspace.path()));

    // Owner 2: steer through the conversation. The adopted instruction's
    // origin is the canonical Owner message record.
    let responses = send_owner_message(
        &handle,
        &live,
        &dialogue,
        "steer",
        "also add an executive summary",
    )
    .await;
    assert!(!contains_directive(&responses), "{responses:?}");
    assert_eq!(
        streamed_text(&responses),
        latest_companion_message(&handle).await
    );
    let instruction_source = latest_owner_message(&handle).await;
    let record = handle
        .store
        .load_task(task)
        .await
        .unwrap()
        .expect("the steered task must load");
    assert_eq!(
        record.task.reference.revision.as_u64(),
        2,
        "the conversation steering advanced exactly one revision"
    );
    assert!(
        record.context.iter().any(|entry| matches!(
            &entry.item,
            TaskContextItem::AdoptedInstruction
        ) && entry.origin.source == instruction_source),
        "the adopted instruction references the canonical Owner record"
    );

    // Owner 3: progress report composed from canonical Task / Action facts.
    let responses =
        send_owner_message(&handle, &live, &dialogue, "progress", "how is it going?").await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: in-progress"), "{reply}");
    assert!(reply.contains("result: none"), "{reply}");
    assert_eq!(streamed_text(&responses), reply);

    // Owner 4: cancel through the conversation.
    let responses =
        send_owner_message(&handle, &live, &dialogue, "cancel", "please cancel it").await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("Cancel accepted"), "{reply}");
    assert_eq!(progress(&handle, task).await, TaskProgress::Cancelled);

    // Owner 5: cancel report.
    let responses = send_owner_message(&handle, &live, &dialogue, "report", "what happened?").await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: cancelled"), "{reply}");

    // The directive never reaches stored History, and no runner was
    // installed, so no Action attempt exists.
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve");
    let timeline = handle
        .store
        .load_timeline(companion, None, None, 100)
        .await
        .expect("the timeline must load");
    for item in &timeline {
        assert!(
            !item.text.contains("[task-control]"),
            "the directive is never stored: {}",
            item.text
        );
    }
    assert_eq!(probe_count(dir.path(), "action_attempt"), 0);
}

#[tokio::test]
async fn production_launcher_runs_the_task_and_the_conversation_reports_cancel() {
    let live = live_input("conversation-cancel");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let dialogue = ScriptedTransport::new(vec![
        task_reply(
            json!({"kind": "propose_task", "purpose": "read input.txt and write report.md"}),
        ),
        task_reply(json!({"kind": "report"})),
        String::from("You are welcome."),
        task_reply(json!({"kind": "cancel"})),
        task_reply(json!({"kind": "report"})),
    ]);
    let agent = Arc::new(GateTransport::new(vec![
        String::from(r#"{"tool":"read","path":"input.txt"}"#),
        String::from(
            "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}",
        ),
    ]));
    let (handle, dir) = round_test_handle("conversation-cancel", &live, &dialogue)
        .await
        .expect("the production setup path completes");
    let handle = Arc::new(handle);
    select_workspace(&handle, &live, workspace.path()).await;
    install_launcher(&handle, Arc::clone(&agent));

    // Owner 1: the conversation creates and delegates; the production
    // launcher starts the runner in the background.
    send_owner_message(
        &handle,
        &live,
        &dialogue,
        "ask-task",
        "please read input.txt and write report.md",
    )
    .await;
    agent.wait_calls(3).await;
    assert!(
        workspace.path().join("report.md").exists(),
        "the confirmed create happened before the blocking call"
    );
    let task = only_task(dir.path());

    // A canonical Unknown effect fixture: the report must distinguish it from
    // the confirmed change instead of folding it into success.
    let record = handle
        .store
        .load_task(task)
        .await
        .unwrap()
        .expect("the task must load");
    let assoc = record
        .workspace
        .as_ref()
        .expect("the association exists")
        .assoc;
    let delegation: DelegationId = {
        let conn =
            rusqlite::Connection::open(dir.path().join("app.db")).expect("the store file opens");
        let text: String = conn
            .query_row(
                "SELECT delegation_id FROM delegation WHERE task_id = ?1 ORDER BY rowid DESC LIMIT 1",
                rusqlite::params![task.as_raw().as_uuid().as_hyphenated().to_string()],
                |row| row.get(0),
            )
            .expect("the conversation delegated");
        DelegationId::from_raw(RawId::from_uuid(
            uuid::Uuid::parse_str(&text).expect("the stored delegation identity parses"),
        ))
    };
    assert_eq!(
        handle
            .store
            .insert_attempt_if_current(AttemptCommitPremise {
                attempt: ActionAttemptId::generate(),
                delegation: delegation.as_raw(),
                task: task.as_raw(),
                task_revision: RevisionInner::from_u64(record.task.reference.revision.as_u64()),
                workspace: assoc.as_raw(),
                real_target: RealTargetRef::from_canonical_path(canonical_target("unknown.md")),
                operation: OperationKind::Create,
                relied_evaluation: RawId::new(),
            })
            .await
            .unwrap(),
        ActionStartOutcome::Started
    );

    // Owner 2: progress while the execution is in flight. Acceptance 4.3: the
    // ordinary conversation path is not blocked.
    let responses =
        send_owner_message(&handle, &live, &dialogue, "progress", "how is it going?").await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: in-progress"), "{reply}");
    assert!(reply.contains("report.md"), "{reply}");
    assert!(reply.contains("unknown.md (unknown)"), "{reply}");
    assert_eq!(streamed_text(&responses), reply);

    // Owner 3: ordinary chat still completes while the Task runs.
    let responses = send_owner_message(&handle, &live, &dialogue, "chat", "thanks!").await;
    assert_eq!(latest_companion_message(&handle).await, "You are welcome.");
    assert_eq!(streamed_text(&responses), "You are welcome.");

    // Owner 4: cancel through the conversation reaches the AU16 admission.
    let responses =
        send_owner_message(&handle, &live, &dialogue, "cancel", "cancel the task").await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("Cancel accepted"), "{reply}");
    wait_progress(&handle, task, TaskProgress::Cancelled).await;

    // Owner 5: acceptance 4.4. The report distinguishes the confirmed change
    // from the unknown effect and never claims every effect stopped.
    let responses = send_owner_message(&handle, &live, &dialogue, "report", "what happened?").await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: cancelled"), "{reply}");
    assert!(
        reply.contains("completed changes:") && reply.contains("report.md"),
        "{reply}"
    );
    assert!(reply.contains("unknown.md (unknown)"), "{reply}");
    assert!(!reply.contains("all stopped"), "{reply}");
    assert_eq!(streamed_text(&responses), reply);

    // Acceptance 4.7: cancel never deletes the workspace's files.
    assert!(workspace.path().join("input.txt").exists());
    assert!(workspace.path().join("report.md").exists());
}

#[tokio::test]
async fn production_launcher_completes_and_the_conversation_reports_completion() {
    let live = live_input("conversation-complete");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let dialogue = ScriptedTransport::new(vec![
        task_reply(
            json!({"kind": "propose_task", "purpose": "read input.txt and write report.md"}),
        ),
        task_reply(json!({"kind": "report"})),
    ]);
    let agent = Arc::new(ScriptedTransport::new(vec![
        String::from(r#"{"tool":"read","path":"input.txt"}"#),
        String::from(
            "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}",
        ),
        String::from(r#"{"final":"created report.md from input.txt"}"#),
    ]));
    let (handle, dir) = round_test_handle("conversation-complete", &live, &dialogue)
        .await
        .expect("the production setup path completes");
    let handle = Arc::new(handle);
    select_workspace(&handle, &live, workspace.path()).await;
    install_launcher(&handle, Arc::clone(&agent));

    // Owner 1: the conversation creates and delegates; the production
    // launcher runs the execution to completion in the background.
    send_owner_message(
        &handle,
        &live,
        &dialogue,
        "ask-task",
        "please read input.txt and write report.md",
    )
    .await;
    let task = only_task(dir.path());
    wait_progress(&handle, task, TaskProgress::Completed).await;

    // Acceptance 4.5: the conversation report names the changed file, the
    // save location, and the remainder, and carries the result body.
    let responses = send_owner_message(&handle, &live, &dialogue, "report", "how did it go?").await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: completed"), "{reply}");
    assert!(
        reply.contains("result (adopted): created report.md from input.txt"),
        "{reply}"
    );
    assert!(reply.contains("report.md"), "{reply}");
    assert!(reply.contains(&canonical_str(workspace.path())), "{reply}");
    assert!(
        reply.contains("remaining/unconfirmed effects:\n- none"),
        "{reply}"
    );
    assert_eq!(streamed_text(&responses), reply);

    // Acceptance 4.7: completion never deletes the workspace's files.
    assert!(workspace.path().join("input.txt").exists());
    assert_eq!(
        std::fs::read(workspace.path().join("report.md")).expect("report exists"),
        b"# Report\nnotes"
    );
}

#[tokio::test]
async fn production_launcher_steers_a_running_task_and_the_new_revision_executes() {
    let live = live_input("conversation-steer-run");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let dialogue = ScriptedTransport::new(vec![
        task_reply(
            json!({"kind": "propose_task", "purpose": "read input.txt and write report.md"}),
        ),
        task_reply(
            json!({"kind": "steer", "instruction": "add an executive summary", "purpose": null}),
        ),
        task_reply(json!({"kind": "report"})),
    ]);
    let agent = Arc::new(
        GateTransport::new(vec![
            String::from(r#"{"tool":"read","path":"input.txt"}"#),
            String::from(
                "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}",
            ),
            // Call 3 is held while the rev1 execution runs; after the
            // steering it must be refused as stale and never executed.
            String::from(
                "{\"tool\":\"create\",\"path\":\"stale.md\",\"content\":\"must not run\"}",
            ),
            // Calls 4..6 belong to the replacement rev2 execution.
            String::from(r#"{"tool":"read","path":"input.txt"}"#),
            String::from("{\"tool\":\"create\",\"path\":\"summary.md\",\"content\":\"# Summary\"}"),
            String::from(r#"{"final":"created report.md and summary.md"}"#),
        ])
        .block_at(3),
    );
    let (handle, dir) = round_test_handle("conversation-steer-run", &live, &dialogue)
        .await
        .expect("the production setup path completes");
    let handle = Arc::new(handle);
    select_workspace(&handle, &live, workspace.path()).await;
    install_launcher(&handle, Arc::clone(&agent));

    // Owner 1: the rev1 execution starts and blocks at its third provider
    // call.
    send_owner_message(
        &handle,
        &live,
        &dialogue,
        "ask-task",
        "please read input.txt and write report.md",
    )
    .await;
    agent.wait_calls(3).await;
    let task = only_task(dir.path());
    assert_eq!(progress(&handle, task).await, TaskProgress::InProgress);
    let old_delegation = latest_delegation(dir.path(), task);
    assert!(workspace.path().join("report.md").exists());

    // Owner 2: steer while rev1 is running. AU4 advances the revision, the
    // Host creates a fresh delegation for rev2, and the launcher starts it.
    let responses = send_owner_message(
        &handle,
        &live,
        &dialogue,
        "steer",
        "also add an executive summary",
    )
    .await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("I will continue with it"), "{reply}");

    wait_progress(&handle, task, TaskProgress::Completed).await;
    assert_eq!(
        delegation_count(dir.path()),
        2,
        "steering creates a new delegation for the new revision"
    );
    let new_delegation = latest_delegation(dir.path(), task);
    assert_ne!(
        new_delegation, old_delegation,
        "the old delegation is never reused"
    );

    // The replacement execution read the adopted instruction and did the
    // work; the old execution's post-steer create never ran.
    let inputs = agent.inputs();
    assert!(inputs.len() >= 4, "{inputs:?}");
    assert!(
        inputs[3].contains("add an executive summary"),
        "the rev2 prompt carries the adopted instruction: {}",
        inputs[3]
    );
    assert_eq!(
        std::fs::read(workspace.path().join("summary.md")).expect("summary exists"),
        b"# Summary"
    );
    assert!(!workspace.path().join("stale.md").exists());

    // The old delegation can neither act on the new revision nor adopt it.
    assert!(
        handle
            .store
            .load_delegation_result(old_delegation)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        action_attempts(dir.path(), old_delegation),
        2,
        "the old execution keeps exactly its rev1 actions"
    );
    let record = handle.store.load_task(task).await.unwrap().unwrap();
    let adopted = record.task.adopted_result.expect("rev2 completed");
    let stored = handle
        .store
        .load_task_result(adopted)
        .await
        .unwrap()
        .expect("the adopted result reads");
    assert_eq!(
        stored.delegation, new_delegation,
        "the current result belongs to the replacement execution"
    );

    // Releasing the old blocked call proves its create is refused by the
    // existing revision gate: nothing more runs and nothing is adopted.
    agent.release();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!workspace.path().join("stale.md").exists());
    assert_eq!(action_attempts(dir.path(), old_delegation), 2);

    // Owner 3: the report is composed from the completed rev2 execution.
    let responses = send_owner_message(&handle, &live, &dialogue, "report", "how did it go?").await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: completed"), "{reply}");
    assert!(reply.contains("summary.md"), "{reply}");
    assert_eq!(streamed_text(&responses), reply);
}

#[tokio::test]
async fn a_control_report_presents_only_the_scrubbed_typed_outcome() {
    let live = live_input("conversation-secret");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let dialogue = ScriptedTransport::new(vec![
        task_reply(
            json!({"kind": "propose_task", "purpose": "read input.txt and write report.md"}),
        ),
        task_reply(json!({"kind": "report"})),
    ]);
    // The registered fixture bearer is `test-bearer`; the Task Agent result
    // body echoes it, so the report rendering would expose it raw without the
    // scrub-before-presentation order.
    let agent = Arc::new(ScriptedTransport::new(vec![
        String::from(r#"{"tool":"read","path":"input.txt"}"#),
        String::from(r#"{"final":"the credential is test-bearer"}"#),
    ]));
    let (handle, dir) = round_test_handle("conversation-secret", &live, &dialogue)
        .await
        .expect("the production setup path completes");
    let handle = Arc::new(handle);
    select_workspace(&handle, &live, workspace.path()).await;
    install_launcher(&handle, Arc::clone(&agent));

    send_owner_message(&handle, &live, &dialogue, "ask-task", "write the report").await;
    let task = only_task(dir.path());
    wait_progress(&handle, task, TaskProgress::Completed).await;

    let responses = send_owner_message(&handle, &live, &dialogue, "report", "how did it go?").await;
    let presented = streamed_text(&responses);
    let reply = latest_companion_message(&handle).await;
    assert_eq!(
        presented, reply,
        "the presented text equals the durable companion reply"
    );
    for text in [&presented, &reply] {
        assert!(
            !text.contains("test-bearer"),
            "the raw credential never appears: {text}"
        );
        assert!(!text.contains("[task-control]"), "{text}");
        assert!(!text.contains("\"kind\""), "{text}");
        assert!(
            text.contains(ene_credential::REDACTED_CREDENTIAL),
            "the scrubbed text carries the redaction marker: {text}"
        );
    }
    assert!(
        reply.contains("result (adopted): the credential is [credential]"),
        "{reply}"
    );
    assert!(!contains_directive(&responses), "{responses:?}");
}

#[tokio::test]
async fn provider_workspace_injection_is_invalid_and_changes_nothing() {
    let live = live_input("conversation-inject");
    let workspace = tempfile::tempdir().expect("workspace directory");
    let outside = tempfile::tempdir().expect("outside directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let dialogue = ScriptedTransport::new(vec![
        // The adversarial provider output tries to widen the boundary. The
        // closed-world schema has no workspace field, so the proposal is
        // invalid and nothing is created; the trusted Owner selection stays
        // the only authority.
        task_reply(json!({
            "kind": "propose_task",
            "purpose": "read input.txt and write report.md",
            "workspace": outside.path().to_string_lossy(),
            "save_target": outside.path().to_string_lossy(),
        })),
    ]);
    let (handle, dir) = round_test_handle("conversation-inject", &live, &dialogue)
        .await
        .expect("the production setup path completes");
    select_workspace(&handle, &live, workspace.path()).await;

    let responses = send_owner_message(
        &handle,
        &live,
        &dialogue,
        "ask-task",
        "please read input.txt and write report.md",
    )
    .await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("could not interpret"), "{reply}");
    assert!(!reply.contains("[task-control]"), "{reply}");
    assert_eq!(probe_count(dir.path(), "task"), 0);
    assert_eq!(probe_count(dir.path(), "delegation"), 0);
    assert_eq!(
        probe_workspace_rows(dir.path(), &canonical_str(outside.path())),
        0,
        "the injected path never becomes an association"
    );
    assert!(!outside.path().join("report.md").exists());
}

#[tokio::test]
async fn a_transient_provider_failure_never_becomes_a_task_failure() {
    let live = live_input("conversation-transient");
    let workspace = tempfile::tempdir().expect("workspace directory");
    let dialogue = ScriptedTransport::new(vec![
        task_reply(json!({"kind": "propose_task", "purpose": "write the report"})),
        task_reply(json!({"kind": "report"})),
    ]);
    let (handle, dir) = round_test_handle("conversation-transient", &live, &dialogue)
        .await
        .expect("the production setup path completes");
    let handle = Arc::new(handle);
    select_workspace(&handle, &live, workspace.path()).await;
    install_launcher(
        &handle,
        Arc::new(FakeProviderTransport::failing(FakeFailure::ResponseLost)),
    );

    send_owner_message(&handle, &live, &dialogue, "ask-task", "write the report").await;
    let task = only_task(dir.path());
    // The runner claimed its attempt before the provider call fails; the
    // failure stays technical and the Task is never marked failed.
    wait_for_inference_attempt(dir.path()).await;
    assert_eq!(
        progress(&handle, task).await,
        TaskProgress::InProgress,
        "a transient provider failure is never a confirmed Task failure"
    );

    let responses = send_owner_message(&handle, &live, &dialogue, "report", "status?").await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: in-progress"), "{reply}");
    assert!(reply.contains("result: none"), "{reply}");
    assert_eq!(streamed_text(&responses), reply);
}

/// Waits until the background execution has claimed at least one inference
/// attempt, proving the production launcher ran the runner.
async fn wait_for_inference_attempt(dir: &Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if inference_attempts(dir) > 0 {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the background execution did not claim an attempt"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn inference_attempts(dir: &Path) -> i64 {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    conn.query_row("SELECT COUNT(*) FROM inference_attempt", (), |row| {
        row.get(0)
    })
    .expect("the inference attempt probe reads")
}

#[tokio::test]
async fn superseded_propose_directive_creates_no_task() {
    let live = live_input("race-propose");
    let workspace = tempfile::tempdir().expect("workspace directory");
    let dialogue = Arc::new(ScriptedTransport::new(vec![
        task_reply(json!({"kind": "propose_task", "purpose": "write the report"})),
        String::from("Never mind, hold on."),
    ]));
    let (handle, dir) = round_test_handle("race-propose", &live, &*dialogue)
        .await
        .expect("the production setup path completes");
    let handle = Arc::new(handle);
    select_workspace(&handle, &live, workspace.path()).await;
    let gate = handle.arm_task_control_gate();

    let old_live = live.clone();
    let old = {
        let handle = Arc::clone(&handle);
        let dialogue = Arc::clone(&dialogue);
        tokio::spawn(async move {
            let live = old_live;
            let generation = current_generation(&handle).await.expect("generation");
            handle
                .handle_frame(
                    request_frame(&handle, &live, Some(generation), "old", "do the thing"),
                    live.clone(),
                    &*dialogue,
                )
                .await
        })
    };
    gate.wait_entered().await;
    // A newer Owner input commits while the old directive is paused.
    send_owner_message(&handle, &live, &*dialogue, "newer", "hold on").await;
    gate.release();

    let responses = old.await.expect("the old turn joins");
    assert!(
        !closes_completed(&responses),
        "the superseded turn closes interrupted: {responses:?}"
    );
    let displayed = streamed_text(&responses);
    for success in ["Task accepted", "Instruction recorded", "Cancel accepted"] {
        assert!(
            !displayed.contains(success),
            "no uncommitted success claim is presented: {displayed}"
        );
    }
    assert_eq!(
        probe_count(dir.path(), "task"),
        0,
        "a superseded propose directive creates no Task"
    );
    assert_eq!(probe_count(dir.path(), "delegation"), 0);
}

#[tokio::test]
async fn superseded_steer_directive_keeps_the_revision() {
    let live = live_input("race-steer");
    let workspace = tempfile::tempdir().expect("workspace directory");
    let dialogue = Arc::new(ScriptedTransport::new(vec![
        task_reply(json!({"kind": "propose_task", "purpose": "write the report"})),
        task_reply(json!({"kind": "steer", "instruction": "add a summary", "purpose": null})),
        String::from("Hold on."),
    ]));
    let (handle, dir) = round_test_handle("race-steer", &live, &*dialogue)
        .await
        .expect("the production setup path completes");
    let handle = Arc::new(handle);
    select_workspace(&handle, &live, workspace.path()).await;
    send_owner_message(&handle, &live, &*dialogue, "ask-task", "do the thing").await;
    let task = only_task(dir.path());
    assert_eq!(progress(&handle, task).await, TaskProgress::InProgress);
    let gate = handle.arm_task_control_gate();

    let old_live = live.clone();
    let old = {
        let handle = Arc::clone(&handle);
        let dialogue = Arc::clone(&dialogue);
        tokio::spawn(async move {
            let live = old_live;
            let generation = current_generation(&handle).await.expect("generation");
            handle
                .handle_frame(
                    request_frame(&handle, &live, Some(generation), "old", "add a summary"),
                    live.clone(),
                    &*dialogue,
                )
                .await
        })
    };
    gate.wait_entered().await;
    send_owner_message(&handle, &live, &*dialogue, "newer", "hold on").await;
    gate.release();

    let responses = old.await.expect("the old turn joins");
    assert!(!closes_completed(&responses), "{responses:?}");
    let displayed = streamed_text(&responses);
    for success in ["Task accepted", "Instruction recorded", "Cancel accepted"] {
        assert!(
            !displayed.contains(success),
            "no uncommitted success claim is presented: {displayed}"
        );
    }
    let record = handle.store.load_task(task).await.unwrap().unwrap();
    assert_eq!(
        record.task.reference.revision.as_u64(),
        1,
        "a superseded steer directive leaves the revision unchanged"
    );
}

#[tokio::test]
async fn superseded_cancel_directive_does_not_cancel() {
    let live = live_input("race-cancel");
    let workspace = tempfile::tempdir().expect("workspace directory");
    let dialogue = Arc::new(ScriptedTransport::new(vec![
        task_reply(json!({"kind": "propose_task", "purpose": "write the report"})),
        task_reply(json!({"kind": "cancel"})),
        String::from("Hold on."),
    ]));
    let (handle, dir) = round_test_handle("race-cancel", &live, &*dialogue)
        .await
        .expect("the production setup path completes");
    let handle = Arc::new(handle);
    select_workspace(&handle, &live, workspace.path()).await;
    send_owner_message(&handle, &live, &*dialogue, "ask-task", "do the thing").await;
    let task = only_task(dir.path());
    let gate = handle.arm_task_control_gate();

    let old_live = live.clone();
    let old = {
        let handle = Arc::clone(&handle);
        let dialogue = Arc::clone(&dialogue);
        tokio::spawn(async move {
            let live = old_live;
            let generation = current_generation(&handle).await.expect("generation");
            handle
                .handle_frame(
                    request_frame(&handle, &live, Some(generation), "old", "cancel it"),
                    live.clone(),
                    &*dialogue,
                )
                .await
        })
    };
    gate.wait_entered().await;
    send_owner_message(&handle, &live, &*dialogue, "newer", "hold on").await;
    gate.release();

    let responses = old.await.expect("the old turn joins");
    assert!(!closes_completed(&responses), "{responses:?}");
    let displayed = streamed_text(&responses);
    for success in ["Task accepted", "Instruction recorded", "Cancel accepted"] {
        assert!(
            !displayed.contains(success),
            "no uncommitted success claim is presented: {displayed}"
        );
    }
    assert_eq!(
        progress(&handle, task).await,
        TaskProgress::InProgress,
        "a superseded cancel directive does not cancel the Task"
    );
}

#[tokio::test]
async fn conversation_invalid_directive_clarifies_without_changing_anything() {
    let live = live_input("conversation-invalid");
    let dialogue = ScriptedTransport::new(vec![String::from("[task-control] {this is not json}")]);
    let (handle, dir) = round_test_handle("conversation-invalid", &live, &dialogue)
        .await
        .expect("the production setup path completes");

    let responses = send_owner_message(&handle, &live, &dialogue, "invalid", "do the thing").await;
    assert!(!contains_directive(&responses), "{responses:?}");
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("could not interpret"), "{reply}");
    assert!(!reply.contains("[task-control]"), "{reply}");
    assert_eq!(streamed_text(&responses), reply);
    assert_eq!(probe_count(dir.path(), "task"), 0);
    assert_eq!(probe_count(dir.path(), "delegation"), 0);
}
