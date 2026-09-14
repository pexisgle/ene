//! Stage 4 slice F conversation-path E2E: every Task control operation is
//! started by an ordinary Owner conversation input.
//!
//! The dialogue provider output carries the companion's closed-world
//! `[task-control]` directive; the production `finish_turn` interprets it and
//! the Host composition root executes it through the existing Task owner
//! boundaries. The test never calls `propose_task` / `propose_steering` /
//! `cancel_task` / `task_report` itself, and it never decides what the
//! companion should do: it only scripts the provider output (the model's
//! answer) and observes the durable results.
//!
//! Acceptance scenario 4: 4.3 (normal chat while the Task runs), 4.4 (cancel
//! report of what completed and what is unresolved), 4.5 (completion report
//! with changed file names, save location, and remainder), and 4.7 (existing
//! workspace files survive completion and cancel). Starting the Task Agent
//! execution from the test is the allowed runner-start exception; the
//! creation and delegation already happened through the conversation path.

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

use ene_action::{
    ActionAttemptId, ActionAttemptRepository as _, ActionStartOutcome, AttemptCommitPremise,
    OperationKind, RealTargetRef,
};
use ene_companion::{CompanionRepository as _, HistoryRepository as _, HistoryRole};
use ene_inference::fake::{FakeFailure, FakeProviderTransport};
use ene_inference::{
    InferenceTechnicalError, ProviderRequest, ProviderResponse, ProviderTransport,
};
use ene_primitive::{RawId, RevisionInner};
use ene_task::{
    DelegationId, TaskContextItem, TaskId, TaskProgress, TaskRepository as _, TaskResultAcceptance,
};
use serde_json::json;
use tokio::sync::{Notify, Semaphore};

use super::task_run::ScriptedTransport;
use super::{accepted_round, current_generation, round_test_handle, submit_frame};
use crate::serve::{HostHandle, LiveInput};
use crate::task_run::TaskAgentRunOutcome;
use crate::test_support::live_input;

/// Provider transport with scripted replies that blocks once the script runs
/// out, so a test can hold the Task Agent in flight while exercising the
/// conversation path. Released calls are never needed when the test cancels;
/// the abort reaches the blocked provider wait through the inference port.
pub(super) struct GateTransport {
    replies: Mutex<VecDeque<String>>,
    calls: AtomicUsize,
    arrived: Notify,
    release: Semaphore,
}

impl GateTransport {
    pub(super) fn new(replies: Vec<String>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            calls: AtomicUsize::new(0),
            arrived: Notify::new(),
            release: Semaphore::new(0),
        }
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
        _req: ProviderRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<ProviderResponse, InferenceTechnicalError>>
                + Send
                + '_,
        >,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let reply = self.replies.lock().expect("gate script lock").pop_front();
        let arrived = &self.arrived;
        let release = &self.release;
        Box::pin(async move {
            arrived.notify_waiters();
            let text = match reply {
                Some(text) => text,
                None => {
                    let permit = release.acquire().await.expect("gate stays open");
                    permit.forget();
                    String::from("released")
                }
            };
            Ok(ProviderResponse { text, usage: None })
        })
    }
}

/// One provider reply carrying exactly one companion task-control directive.
fn control_reply(text: &str, directive: serde_json::Value) -> String {
    format!("{text}\n[task-control] {directive}")
}

fn request_frame(
    handle: &HostHandle,
    live: &LiveInput,
    generation: Option<u64>,
    local_id: &str,
    text: &str,
) -> ene_plugin_ipc::WireFrame {
    submit_frame(
        handle.companion_wire(),
        generation,
        None,
        local_id,
        text,
        live.connection_id,
    )
}

async fn send_owner_message(
    handle: &HostHandle,
    live: &LiveInput,
    transport: &impl ProviderTransport,
    local_id: &str,
    text: &str,
) {
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
        responses.iter().any(|frame| matches!(
            &frame.payload,
            ene_api::v1::payload::WirePayload::TextStreamClose(close)
                if close.status == ene_api::v1::round::StreamClose::Completed
        )),
        "the owner turn must complete: {responses:?}"
    );
}

async fn latest_owner_message(handle: &HostHandle) -> RawId {
    latest_message_with_role(handle, HistoryRole::Owner).await
}

async fn latest_companion_message(handle: &HostHandle) -> String {
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
        .find(|item| item.role == HistoryRole::Companion)
        .expect("the companion reply is in the timeline")
        .text
        .clone()
}

async fn latest_message_with_role(handle: &HostHandle, role: HistoryRole) -> RawId {
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
        .expect("the message is in the timeline")
        .id
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

/// The Task's newest delegation, discovered from the store so the test can
/// start the existing runner (the allowed runner-start exception).
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

fn probe_count(dir: &Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), (), |row| {
        row.get(0)
    })
    .expect("the probe count must read")
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

/// A platform-absolute fixture target: the Action read-back requires an
/// absolute path on every supported platform.
fn canonical_target(name: &str) -> String {
    std::env::temp_dir()
        .join(name)
        .to_string_lossy()
        .into_owned()
}

#[tokio::test]
async fn conversation_creation_steering_progress_and_cancel() {
    let live = live_input("conversation-control");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let dialogue = ScriptedTransport::new(vec![
        control_reply(
            "Sure, I will start on that.",
            json!({
                "kind": "propose_task",
                "purpose": "read input.txt and write report.md",
                "workspace": workspace.path().to_string_lossy(),
                "save_target": null,
            }),
        ),
        control_reply(
            "I will add it.",
            json!({
                "kind": "steer",
                "instruction": "add an executive summary",
                "purpose": null,
            }),
        ),
        control_reply("", json!({"kind": "report"})),
        control_reply("", json!({"kind": "cancel"})),
        control_reply("", json!({"kind": "report"})),
    ]);
    let (handle, dir) = round_test_handle("conversation-control", &live, &dialogue)
        .await
        .expect("the production setup path completes");

    // Owner 1: ask for the file work. The companion's turn itself creates the
    // Task, confirms the workspace association, and delegates.
    send_owner_message(
        &handle,
        &live,
        &dialogue,
        "ask-task",
        "please read input.txt and write report.md in the workspace",
    )
    .await;
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
        .expect("the conversation named the workspace");
    assert_eq!(association.folder.path, workspace.path().to_string_lossy());
    let delegation = latest_delegation(dir.path(), task);
    let correspondence = handle
        .store
        .load_delegation(delegation)
        .await
        .unwrap()
        .expect("the conversation-created delegation must load");
    assert_eq!(correspondence.task.task, task);
    assert_eq!(
        correspondence
            .scope
            .workspace
            .as_ref()
            .expect("the delegation copies the association")
            .assoc,
        association.assoc
    );
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("Task accepted"), "{reply}");

    // Owner 2: steer through the conversation. The adopted instruction's
    // origin is the canonical Owner message record.
    send_owner_message(
        &handle,
        &live,
        &dialogue,
        "steer",
        "also add an executive summary",
    )
    .await;
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
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("Instruction recorded"), "{reply}");

    // Owner 3: ask for progress. The reply is composed from canonical Task /
    // Action facts, not a scripted chat answer.
    send_owner_message(&handle, &live, &dialogue, "progress", "how is it going?").await;
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: in-progress"), "{reply}");
    assert!(reply.contains("result: none"), "{reply}");

    // Owner 4: cancel through the conversation.
    send_owner_message(&handle, &live, &dialogue, "cancel", "please cancel it").await;
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("Cancel accepted"), "{reply}");
    assert_eq!(progress(&handle, task).await, TaskProgress::Cancelled);

    // Owner 5: ask for the cancel report.
    send_owner_message(&handle, &live, &dialogue, "report", "what happened?").await;
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: cancelled"), "{reply}");

    // No control marker ever reaches stored History, and no runner was
    // started, so no Action attempt exists.
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
async fn normal_chat_continues_while_the_task_runs_and_the_conversation_reports_cancel() {
    let live = live_input("conversation-cancel");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let dialogue = ScriptedTransport::new(vec![
        control_reply(
            "Starting on the report.",
            json!({
                "kind": "propose_task",
                "purpose": "read input.txt and write report.md",
                "workspace": workspace.path().to_string_lossy(),
                "save_target": null,
            }),
        ),
        control_reply("", json!({"kind": "report"})),
        String::from("You are welcome."),
        control_reply("", json!({"kind": "cancel"})),
        control_reply("", json!({"kind": "report"})),
    ]);
    let (handle, dir) = round_test_handle("conversation-cancel", &live, &dialogue)
        .await
        .expect("the production setup path completes");
    let agent = Arc::new(GateTransport::new(vec![
        String::from(r#"{"tool":"read","path":"input.txt"}"#),
        String::from(
            "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}",
        ),
    ]));

    // Owner 1: the conversation creates and delegates the Task.
    send_owner_message(
        &handle,
        &live,
        &dialogue,
        "ask-task",
        "please read input.txt and write report.md in the workspace",
    )
    .await;
    let task = only_task(dir.path());
    let delegation = latest_delegation(dir.path(), task);

    // Start the existing runner (allowed exception); it performs the read and
    // create, then blocks on the third provider call.
    let handle = Arc::new(handle);
    let execution = {
        let handle = Arc::clone(&handle);
        let agent = Arc::clone(&agent);
        tokio::spawn(async move { handle.run_task_agent(&*agent, delegation).await })
    };
    agent.wait_calls(3).await;
    assert!(
        workspace.path().join("report.md").exists(),
        "the confirmed create happened before the blocking call"
    );

    // A canonical Unknown effect fixture: the report must distinguish it from
    // the confirmed change instead of folding it into success.
    let correspondence = handle
        .store
        .load_delegation(delegation)
        .await
        .unwrap()
        .expect("the delegation must load");
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
    assert_eq!(
        handle
            .store
            .insert_attempt_if_current(AttemptCommitPremise {
                attempt: ActionAttemptId::generate(),
                delegation: delegation.as_raw(),
                task: task.as_raw(),
                task_revision: RevisionInner::from_u64(correspondence.task.revision.as_u64()),
                workspace: assoc.as_raw(),
                real_target: RealTargetRef::from_canonical_path(canonical_target("unknown.md")),
                operation: OperationKind::Create,
                relied_evaluation: RawId::new(),
            })
            .await
            .unwrap(),
        ActionStartOutcome::Started
    );

    // Owner 2: ask for progress while the execution is in flight. Acceptance
    // 4.3: the ordinary conversation path is not blocked.
    send_owner_message(&handle, &live, &dialogue, "progress", "how is it going?").await;
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: in-progress"), "{reply}");
    assert!(reply.contains("report.md"), "{reply}");
    assert!(reply.contains("unknown.md (unknown)"), "{reply}");

    // Owner 3: an ordinary chat turn still completes while the Task runs.
    send_owner_message(&handle, &live, &dialogue, "chat", "thanks!").await;
    assert_eq!(latest_companion_message(&handle).await, "You are welcome.");

    // Owner 4: cancel through the conversation reaches the AU16 admission.
    send_owner_message(&handle, &live, &dialogue, "cancel", "cancel the task").await;
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("Cancel accepted"), "{reply}");
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), execution)
        .await
        .expect("the execution stops promptly after the admission")
        .expect("the join succeeds")
        .expect("the stop is a domain outcome");
    assert_eq!(outcome, TaskAgentRunOutcome::Cancelled);
    assert_eq!(progress(&handle, task).await, TaskProgress::Cancelled);

    // Owner 5: acceptance 4.4. The report distinguishes the confirmed change
    // from the unknown effect and never claims every effect stopped.
    send_owner_message(&handle, &live, &dialogue, "report", "what happened?").await;
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: cancelled"), "{reply}");
    assert!(
        reply.contains("completed changes:") && reply.contains("report.md"),
        "{reply}"
    );
    assert!(reply.contains("unknown.md (unknown)"), "{reply}");
    assert!(!reply.contains("all stopped"), "{reply}");

    // Acceptance 4.7: cancel never deletes the workspace's files.
    assert!(workspace.path().join("input.txt").exists());
    assert!(workspace.path().join("report.md").exists());
}

#[tokio::test]
async fn conversation_completion_report_names_files_and_keeps_the_workspace() {
    let live = live_input("conversation-complete");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let dialogue = ScriptedTransport::new(vec![
        control_reply(
            "I will write the report.",
            json!({
                "kind": "propose_task",
                "purpose": "read input.txt and write report.md",
                "workspace": workspace.path().to_string_lossy(),
                "save_target": null,
            }),
        ),
        control_reply("", json!({"kind": "report"})),
    ]);
    let (handle, dir) = round_test_handle("conversation-complete", &live, &dialogue)
        .await
        .expect("the production setup path completes");
    let agent = ScriptedTransport::new(vec![
        String::from(r#"{"tool":"read","path":"input.txt"}"#),
        String::from(
            "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}",
        ),
        String::from(r#"{"final":"created report.md from input.txt"}"#),
    ]);

    send_owner_message(
        &handle,
        &live,
        &dialogue,
        "ask-task",
        "please read input.txt and write report.md in the workspace",
    )
    .await;
    let task = only_task(dir.path());
    let delegation = latest_delegation(dir.path(), task);
    let run = handle.run_task_agent(&agent, delegation).await.unwrap();
    let TaskAgentRunOutcome::Finalized { acceptance, .. } = run else {
        panic!("the conversation-created task must complete, got {run:?}");
    };
    assert!(matches!(
        acceptance,
        TaskResultAcceptance::AdoptedAsCompletion(_)
    ));
    assert_eq!(progress(&handle, task).await, TaskProgress::Completed);

    // Acceptance 4.5: the conversation report names the changed file, the
    // save location, and the remainder, and carries the result body.
    send_owner_message(&handle, &live, &dialogue, "report", "how did it go?").await;
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: completed"), "{reply}");
    assert!(
        reply.contains("result (adopted): created report.md from input.txt"),
        "{reply}"
    );
    assert!(reply.contains("report.md"), "{reply}");
    assert!(
        reply.contains(&workspace.path().to_string_lossy().to_string()),
        "{reply}"
    );
    assert!(
        reply.contains("remaining/unconfirmed effects:\n- none"),
        "{reply}"
    );

    // Acceptance 4.7: completion never deletes the workspace's files.
    assert!(workspace.path().join("input.txt").exists());
    assert_eq!(
        std::fs::read(workspace.path().join("report.md")).expect("report exists"),
        b"# Report\nnotes"
    );
}

#[tokio::test]
async fn conversation_invalid_directive_clarifies_without_changing_anything() {
    let live = live_input("conversation-invalid");
    let dialogue = ScriptedTransport::new(vec![String::from(
        "I am not sure.\n[task-control] {this is not json}",
    )]);
    let (handle, dir) = round_test_handle("conversation-invalid", &live, &dialogue)
        .await
        .expect("the production setup path completes");

    send_owner_message(&handle, &live, &dialogue, "invalid", "do the thing").await;
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("could not interpret"), "{reply}");
    assert!(!reply.contains("[task-control]"), "{reply}");
    assert_eq!(probe_count(dir.path(), "task"), 0);
    assert_eq!(probe_count(dir.path(), "delegation"), 0);
}

#[tokio::test]
async fn a_transient_provider_failure_never_becomes_a_task_failure() {
    let live = live_input("conversation-transient");
    let dialogue = ScriptedTransport::new(vec![
        control_reply(
            "I will start.",
            json!({
                "kind": "propose_task",
                "purpose": "write the report",
                "workspace": null,
                "save_target": null,
            }),
        ),
        control_reply("", json!({"kind": "report"})),
    ]);
    let (handle, dir) = round_test_handle("conversation-transient", &live, &dialogue)
        .await
        .expect("the production setup path completes");

    send_owner_message(&handle, &live, &dialogue, "ask-task", "write the report").await;
    let task = only_task(dir.path());
    let delegation = latest_delegation(dir.path(), task);

    let failing = FakeProviderTransport::failing(FakeFailure::ResponseLost);
    let run = handle.run_task_agent(&failing, delegation).await;
    assert!(
        run.is_err(),
        "a provider transport failure stays a technical class, got {run:?}"
    );
    assert_eq!(
        progress(&handle, task).await,
        TaskProgress::InProgress,
        "a transient provider failure is never a confirmed Task failure"
    );

    // The conversation progress report still reads the durable state.
    send_owner_message(&handle, &live, &dialogue, "report", "status?").await;
    let reply = latest_companion_message(&handle).await;
    assert!(reply.contains("task status: in-progress"), "{reply}");
    assert!(reply.contains("result: none"), "{reply}");
}
