//! Stage 4 slice E end-to-end: the autonomous file-work task through the Host
//! composition path. Conversation / first-party-management task control is
//! slice F and is not exercised here.
//!
//! Real store, real credential/consent setup path (the production management
//! intents used by the dialogue tests), real workspace filesystem, real Task
//! Agent loop, and only the provider HTTP transport faked. Covers the
//! file/workspace half of the acceptance scenario ("read a file and create a
//! Markdown report"), the durable result and attempt correlation, restart
//! read-back without replay, the cancel / late-result contract, the
//! execution-abort accounting, the input-bound transcript trimming, the
//! concurrent-run, and the one-shot restart boundaries.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures and helpers live outside #[test] functions, where clippy.toml's test allowances do not apply"
)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use ene_action::ActionAttemptRepository as _;
use ene_companion::CompanionRepository as _;
use ene_credential::{CredentialRef, MemoryCredentialStore};
use ene_inference::{
    InferenceTechnicalError, ProviderRequest, ProviderResponse, ProviderTransport,
};
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, CancelTaskCommand, DelegatedWorkspace, DelegationCreationPremise, DelegationId,
    DelegationOutcome, DelegationScope, TaskAgentEphemeralId, TaskCancelOutcome,
    TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind, TaskCreationPremise,
    TaskProgress, TaskPurpose, TaskRef, TaskRepository as _, TaskResultAcceptance,
    WorkspaceAssocId, WorkspaceAssociationPremise, WorkspaceFolderRef, WorkspaceNeedRef,
    orchestrate_result_arrival,
};

use crate::serve::{CredStore, HostHandle};
use crate::task_run::{TaskAgentProtocolViolation, TaskAgentRunOutcome, TaskAgentRunRefusal};

use super::{live_input, round_test_handle};

/// One scripted provider response per call, in call order.
pub(super) struct ScriptedTransport {
    replies: Mutex<VecDeque<String>>,
    inputs: Mutex<Vec<String>>,
}

impl ScriptedTransport {
    pub(super) fn new(replies: Vec<String>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            inputs: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn inputs(&self) -> Vec<String> {
        self.inputs.lock().expect("input capture lock").clone()
    }
}

impl ProviderTransport for ScriptedTransport {
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
        self.inputs
            .lock()
            .expect("input capture lock")
            .push(req.input.clone());
        let text = self
            .replies
            .lock()
            .expect("script lock")
            .pop_front()
            .expect("every exercised provider call has a scripted reply");
        Box::pin(async move { Ok(ProviderResponse { text, usage: None }) })
    }
}

/// Transport whose call reports that it started and then waits forever, so
/// the test can cancel while the provider call is in flight.
#[derive(Default, Clone)]
struct BlockingTransport {
    started: Arc<tokio::sync::Notify>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl BlockingTransport {
    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl ProviderTransport for BlockingTransport {
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
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let started = Arc::clone(&self.started);
        Box::pin(async move {
            started.notify_one();
            std::future::pending::<()>().await;
            Ok(ProviderResponse {
                text: String::new(),
                usage: None,
            })
        })
    }
}

/// One durable usage fact read straight from the Host database.
#[derive(Debug, PartialEq, Eq)]
struct UsageRow {
    source: String,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
}

fn usage_rows(data_dir: &std::path::Path) -> Vec<UsageRow> {
    let conn = rusqlite::Connection::open(data_dir.join("app.db"))
        .expect("the store file must open for the probe");
    let mut statement = conn
        .prepare("SELECT source, input_tokens, output_tokens FROM usage_fact ORDER BY ticket")
        .expect("the usage probe statement must prepare");
    let rows = statement
        .query_map([], |row| {
            Ok(UsageRow {
                source: row.get(0)?,
                input_tokens: row.get(1)?,
                output_tokens: row.get(2)?,
            })
        })
        .expect("the usage probe must run");
    rows.collect::<Result<Vec<_>, _>>()
        .expect("the usage probe rows must decode")
}

fn inference_attempt_count(data_dir: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(data_dir.join("app.db"))
        .expect("the store file must open for the probe");
    conn.query_row("SELECT COUNT(*) FROM inference_attempt", (), |row| {
        row.get(0)
    })
    .expect("the attempt count must read")
}

fn memory_store() -> MemoryCredentialStore {
    let store = MemoryCredentialStore::new();
    store.insert(
        CredentialRef::new("openai", "main").expect("valid test fixture"),
        "sk-stage4-test",
    );
    store
}

/// Seeds one Task with a confirmed workspace association and one delegation,
/// mirroring the Host wiring the conversation path will perform.
async fn seed_task(
    handle: &HostHandle,
    workspace: &std::path::Path,
) -> (TaskRef, DelegationId, WorkspaceAssocId) {
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the running companion resolves");
    let assoc = WorkspaceAssocId::generate();
    let folder = WorkspaceFolderRef {
        path: workspace.to_string_lossy().into_owned(),
    };
    let task = handle
        .store
        .create_task(TaskCreationPremise {
            task: ene_task::TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("read the notes and write the report"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            acquired_at: WallClockWithTz::now(),
            assignee: AssigneeRef {
                companion: companion.as_raw(),
            },
            workspace: Some(WorkspaceAssociationPremise {
                assoc,
                need: WorkspaceNeedRef {
                    folder: folder.clone(),
                    save_target: None,
                },
            }),
        })
        .await
        .expect("task creation commits");
    let delegation = DelegationId::generate();
    let delegated = handle
        .store
        .create_delegation(DelegationCreationPremise {
            delegation,
            task,
            agent: TaskAgentEphemeralId::generate(),
            scope_copy: DelegationScope {
                workspace: Some(DelegatedWorkspace {
                    assoc,
                    folder,
                    save_target: None,
                }),
            },
        })
        .await
        .expect("delegation creation commits");
    assert!(matches!(delegated, DelegationOutcome::Delegated(_)));
    (task, delegation, assoc)
}

#[tokio::test]
async fn stage4_reads_the_workspace_writes_the_report_and_survives_restart() {
    let live = live_input("stage4-e2e");
    // The escape attempt points at a real file outside the workspace carrying
    // a sentinel that must never reach the provider. A sibling scratch
    // directory under the system temp dir keeps the path relative and cleans
    // itself up.
    let outside = tempfile::tempdir_in(std::env::temp_dir()).expect("outside directory");
    let outside_sibling = outside
        .path()
        .file_name()
        .expect("the scratch directory has a name")
        .to_string_lossy()
        .into_owned();
    std::fs::write(outside.path().join("sentinel.txt"), b"SENTINEL-OUTSIDE")
        .expect("outside fixture");
    let transport = ScriptedTransport::new(vec![
        format!("{{\"tool\":\"read\",\"path\":\"../{outside_sibling}/sentinel.txt\"}}"),
        String::from(r#"{"tool":"read","path":"input.txt"}"#),
        String::from(
            "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}",
        ),
        String::from(r#"{"final":"report.md was created from input.txt"}"#),
    ]);
    let (handle, data_dir) = round_test_handle("stage4-e2e", &live, &transport)
        .await
        .expect("the production setup path completes");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let (task, delegation, _assoc) = seed_task(&handle, workspace.path()).await;

    let outcome = handle
        .run_task_agent(&transport, delegation)
        .await
        .expect("the execution answers a domain outcome");
    let TaskAgentRunOutcome::Finalized { result, acceptance } = outcome else {
        panic!("expected Finalized, got {outcome:?}");
    };
    assert_eq!(acceptance, TaskResultAcceptance::AdoptedAsCompletion(task));
    assert_eq!(result.body.text(), "report.md was created from input.txt");
    assert_eq!(
        std::fs::read(workspace.path().join("report.md")).expect("the report exists"),
        b"# Report\nnotes"
    );
    assert!(
        workspace.path().join("input.txt").exists(),
        "a completed task never deletes the workspace's existing files (acceptance 4.7)"
    );
    // The logical input replayed the observed file content, never the outside
    // sentinel and never a file list the model did not ask for.
    let inputs = transport.inputs();
    assert_eq!(inputs.len(), 4, "one provider call per turn");
    assert!(
        inputs[0].contains("[RESPONSE FORMAT]") && !inputs[0].contains("input.txt"),
        "the first turn sends no file list"
    );
    assert!(
        inputs[1].contains("refused:"),
        "the traversal refusal is replayed as an observation, got {}",
        inputs[1]
    );
    assert!(
        inputs[2].contains("read ok:\nnotes") && inputs[2].contains("[TOOL CALL]"),
        "the third turn replays the read observation"
    );
    for input in &inputs {
        assert!(
            !input.contains("SENTINEL-OUTSIDE"),
            "the outside file content never reaches the provider"
        );
    }

    // Durable correlation: exactly the two executed actions, both confirmed;
    // the refused traversal never claimed an attempt.
    let loaded = handle.store.load_task(task.task).await.unwrap().unwrap();
    assert_eq!(loaded.task.progress, TaskProgress::Completed);
    assert_eq!(loaded.task.adopted_result, Some(result.result));
    let stamped = handle
        .store
        .load_task_result(result.result)
        .await
        .unwrap()
        .expect("the adopted result is readable");
    assert!(stamped.adopted_revision.is_some());
    assert_eq!(stamped.attempt_refs.len(), 2);
    for attempt in &stamped.attempt_refs {
        let record = handle
            .store
            .load_attempt(ene_action::ActionAttemptId::from_raw(*attempt))
            .await
            .unwrap()
            .expect("the attempt is durable");
        assert_eq!(record.delegation, delegation.as_raw());
    }
    drop(handle);

    // Restart: the outcome is durable, and a new run never replays work.
    let reopened =
        HostHandle::open_with_cred_store(data_dir.path(), CredStore::Memory(memory_store()))
            .await
            .expect("the host reopens");
    let reloaded = reopened.store.load_task(task.task).await.unwrap().unwrap();
    assert_eq!(reloaded.task.progress, TaskProgress::Completed);
    assert_eq!(reloaded.task.adopted_result, Some(result.result));
    let replayed = reopened
        .run_task_agent(&transport, delegation)
        .await
        .expect("a terminal task answers a domain outcome");
    assert_eq!(
        replayed,
        TaskAgentRunOutcome::Refused(TaskAgentRunRefusal::TaskTerminal {
            task: task.task,
            progress: TaskProgress::Completed,
        }),
        "a completed execution is never resumed or replayed"
    );
    assert_eq!(
        transport.inputs().len(),
        4,
        "the restart attempt sends no provider call"
    );
    assert_eq!(
        reopened
            .cancel_task(CancelTaskCommand { task: task.task })
            .await
            .unwrap(),
        TaskCancelOutcome::TaskTerminal {
            task: task.task,
            progress: TaskProgress::Completed,
        },
        "completion is never retracted by a late cancel"
    );
}

#[tokio::test]
async fn stage4_cancel_stops_the_loop_and_a_late_result_stays_original_only() {
    let live = live_input("stage4-cancel");
    let transport = BlockingTransport::default();
    let (handle, data_dir) = round_test_handle("stage4-cancel", &live, &transport)
        .await
        .expect("the production setup path completes");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let (task, delegation, _assoc) = seed_task(&handle, workspace.path()).await;
    let handle = Arc::new(handle);
    let observed = transport.clone();
    let started = Arc::clone(&transport.started);

    let execution = {
        let handle = Arc::clone(&handle);
        tokio::spawn(async move { handle.run_task_agent(&transport, delegation).await })
    };
    // Wait until the provider call is actually in flight, then cancel.
    tokio::time::timeout(std::time::Duration::from_secs(10), started.notified())
        .await
        .expect("the provider call starts");
    assert_eq!(
        handle
            .cancel_task(CancelTaskCommand { task: task.task })
            .await
            .unwrap(),
        TaskCancelOutcome::CancelAccepted
    );
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), execution)
        .await
        .expect("the execution stops promptly after the admission")
        .expect("the join succeeds")
        .expect("the stop is a domain outcome");
    assert_eq!(outcome, TaskAgentRunOutcome::Cancelled);
    assert!(
        handle
            .store
            .load_delegation_result(delegation)
            .await
            .unwrap()
            .is_none(),
        "a stopped execution is not sealed"
    );
    // The abort dropped the in-flight provider future only after the claimed
    // attempt's accounting completed: the attempt and its unknown-usage fact
    // are durable before Cancelled is observed, and no second call started.
    assert_eq!(observed.calls(), 1, "exactly one provider call was started");
    assert_eq!(
        inference_attempt_count(data_dir.path()),
        1,
        "the aborted provider wait kept its claimed attempt"
    );
    assert_eq!(
        usage_rows(data_dir.path()),
        vec![UsageRow {
            source: String::from("unknown"),
            input_tokens: None,
            output_tokens: None,
        }],
        "the claimed attempt records Unknown usage before the cancel returns"
    );

    // A delayed final result from the same execution is still recorded and
    // sealed, but never adopted into the cancelled Task.
    let recorded = orchestrate_result_arrival(
        &handle.store,
        delegation,
        ene_task::TaskAgentOutput::new(String::from("late final body")),
    )
    .await
    .expect("the arrival record survives cancel");
    let acceptance = handle
        .store
        .adopt_result(ene_task::TaskResultAdoptionClaim {
            result: recorded.result,
            attempt_refs: Vec::new(),
        })
        .await
        .expect("the adoption answers");
    assert_eq!(acceptance, TaskResultAcceptance::RecordedToOriginalOnly);
    let loaded = handle
        .store
        .load_task_result(recorded.result)
        .await
        .unwrap()
        .expect("the late result is readable");
    assert_eq!(loaded.body.text(), "late final body");
    assert!(loaded.adopted_revision.is_none());
    assert_eq!(
        handle
            .store
            .load_task(task.task)
            .await
            .unwrap()
            .unwrap()
            .task
            .progress,
        TaskProgress::Cancelled
    );
    drop(handle);

    // Restart keeps the cancelled marker, never resumes the Task, and a new
    // execution attempt is refused before any provider I/O.
    let reopened =
        HostHandle::open_with_cred_store(data_dir.path(), CredStore::Memory(memory_store()))
            .await
            .expect("the host reopens");
    let reloaded = reopened.store.load_task(task.task).await.unwrap().unwrap();
    assert_eq!(reloaded.task.progress, TaskProgress::Cancelled);
    assert_eq!(
        reopened
            .cancel_task(CancelTaskCommand { task: task.task })
            .await
            .unwrap(),
        TaskCancelOutcome::AlreadyCancelled,
        "the admission happened exactly once"
    );
    assert!(
        reopened
            .store
            .load_delegation_result(delegation)
            .await
            .unwrap()
            .is_some(),
        "the late result stays durable against its original execution"
    );
    let refusing_transport = ScriptedTransport::new(Vec::new());
    let refused = reopened
        .run_task_agent(&refusing_transport, delegation)
        .await
        .expect("a cancelled task answers a domain outcome");
    assert_eq!(
        refused,
        TaskAgentRunOutcome::Refused(TaskAgentRunRefusal::TaskTerminal {
            task: task.task,
            progress: TaskProgress::Cancelled,
        }),
        "a restart never resumes or re-executes a cancelled Task"
    );
    assert!(
        refusing_transport.inputs().is_empty(),
        "the cancelled Task sends no provider call"
    );
}

#[tokio::test]
async fn stage4_a_concurrent_execution_of_one_delegation_is_refused() {
    let live = live_input("stage4-concurrent");
    let transport = BlockingTransport::default();
    let (handle, _data_dir) = round_test_handle("stage4-concurrent", &live, &transport)
        .await
        .expect("the production setup path completes");
    let workspace = tempfile::tempdir().expect("workspace directory");
    let (task, delegation, _assoc) = seed_task(&handle, workspace.path()).await;
    let handle = Arc::new(handle);
    let started = Arc::clone(&transport.started);

    let execution = {
        let handle = Arc::clone(&handle);
        let first = transport.clone();
        tokio::spawn(async move { handle.run_task_agent(&first, delegation).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), started.notified())
        .await
        .expect("the first provider call starts");

    // The in-memory registration is atomic per delegation: the second loop is
    // refused before it can start any provider call or Action.
    let refused = handle
        .run_task_agent(&transport, delegation)
        .await
        .expect("the concurrent refusal is a domain outcome");
    assert_eq!(
        refused,
        TaskAgentRunOutcome::Refused(TaskAgentRunRefusal::ExecutionAlreadyRunning { delegation })
    );
    assert_eq!(
        transport.calls(),
        1,
        "the refused concurrent run starts no provider call"
    );

    // Stop the first execution so the test reaps it deterministically.
    assert_eq!(
        handle
            .cancel_task(CancelTaskCommand { task: task.task })
            .await
            .unwrap(),
        TaskCancelOutcome::CancelAccepted
    );
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), execution)
        .await
        .expect("the first execution stops after the admission")
        .expect("the join succeeds")
        .expect("the stop is a domain outcome");
    assert_eq!(outcome, TaskAgentRunOutcome::Cancelled);
    assert_eq!(transport.calls(), 1, "still exactly one provider call");
}

#[tokio::test]
async fn stage4_a_stopped_unsealed_execution_is_not_restarted_after_reopen() {
    let live = live_input("stage4-one-shot");
    let transport = ScriptedTransport::new(vec![String::from("I could answer, but I will not.")]);
    let (handle, data_dir) = round_test_handle("stage4-one-shot", &live, &transport)
        .await
        .expect("the production setup path completes");
    let workspace = tempfile::tempdir().expect("workspace directory");
    let (task, delegation, _assoc) = seed_task(&handle, workspace.path()).await;

    let outcome = handle
        .run_task_agent(&transport, delegation)
        .await
        .expect("a malformed answer is a domain outcome");
    assert_eq!(
        outcome,
        TaskAgentRunOutcome::ProtocolViolation {
            turn: 1,
            reason: TaskAgentProtocolViolation::NotAJsonObject,
        }
    );
    // The execution started (one claimed attempt), stopped unsealed, and the
    // Task is still non-terminal, so only the durable start marker can
    // prevent a second run.
    assert_eq!(inference_attempt_count(data_dir.path()), 1);
    let loaded = handle.store.load_task(task.task).await.unwrap().unwrap();
    assert_eq!(loaded.task.progress, TaskProgress::InProgress);
    assert!(
        handle
            .store
            .load_delegation_result(delegation)
            .await
            .unwrap()
            .is_none()
    );
    drop(handle);

    // A restart drops the in-memory registration, but the durable attempt
    // facts refuse a fresh run under the same delegation.
    let reopened =
        HostHandle::open_with_cred_store(data_dir.path(), CredStore::Memory(memory_store()))
            .await
            .expect("the host reopens");
    let refusing_transport = ScriptedTransport::new(Vec::new());
    let refused = reopened
        .run_task_agent(&refusing_transport, delegation)
        .await
        .expect("the one-shot refusal is a domain outcome");
    assert_eq!(
        refused,
        TaskAgentRunOutcome::Refused(TaskAgentRunRefusal::ExecutionAlreadyStarted { delegation }),
        "a stopped unsealed execution is never restarted under the same identity"
    );
    assert!(
        refusing_transport.inputs().is_empty(),
        "the refused restart sends no provider call"
    );
    assert_eq!(
        inference_attempt_count(data_dir.path()),
        1,
        "the one-shot refusal claims nothing new"
    );
}

#[tokio::test]
async fn stage4_a_multi_read_transcript_stays_within_the_input_bound() {
    let live = live_input("stage4-input-bound");
    let transport = ScriptedTransport::new(vec![
        String::from(r#"{"tool":"read","path":"first.txt"}"#),
        String::from(r#"{"tool":"read","path":"second.txt"}"#),
        String::from(r#"{"final":"read both files"}"#),
    ]);
    let (handle, _data_dir) = round_test_handle("stage4-input-bound", &live, &transport)
        .await
        .expect("the production setup path completes");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("first.txt"), "a".repeat(4_000)).expect("first fixture");
    std::fs::write(workspace.path().join("second.txt"), "b".repeat(4_000)).expect("second fixture");
    let (_task, delegation, _assoc) = seed_task(&handle, workspace.path()).await;

    let outcome = handle
        .run_task_agent(&transport, delegation)
        .await
        .expect("the execution answers a domain outcome");
    assert!(
        matches!(outcome, TaskAgentRunOutcome::Finalized { .. }),
        "a transcript that outgrows the bound is trimmed, not wedged, got {outcome:?}"
    );
    let inputs = transport.inputs();
    assert_eq!(inputs.len(), 3, "one provider call per turn");
    for (index, input) in inputs.iter().enumerate() {
        assert!(
            input.chars().count() <= ene_inference::MAX_INPUT_CHARS,
            "turn {} stays within the inference input cap, got {} chars",
            index + 1,
            input.chars().count()
        );
    }
    assert!(
        inputs[2].contains("[NOTE] earlier tool exchanges were omitted to fit the input bound"),
        "the model is told the older exchange was omitted"
    );
    assert_eq!(
        inputs[2].matches("[TOOL CALL]").count(),
        1,
        "only the newest exchange is replayed once the transcript outgrows the bound"
    );
}

#[tokio::test]
async fn stage4_an_oversized_single_read_stops_without_sealing_or_truncating() {
    let live = live_input("stage4-oversized-read");
    let transport = ScriptedTransport::new(vec![
        String::from(r#"{"tool":"read","path":"huge.txt"}"#),
        String::from(r#"{"final":"never reached"}"#),
    ]);
    let (handle, _data_dir) = round_test_handle("stage4-oversized-read", &live, &transport)
        .await
        .expect("the production setup path completes");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("huge.txt"), "x".repeat(9_000)).expect("huge fixture");
    let (task, delegation, _assoc) = seed_task(&handle, workspace.path()).await;

    let outcome = handle
        .run_task_agent(&transport, delegation)
        .await
        .expect("the over-limit stop is a domain outcome");
    assert_eq!(
        outcome,
        TaskAgentRunOutcome::NotSent(ene_task::TaskAgentNotSent::OverLimit),
        "an observation that cannot fit alone is refused, never silently truncated"
    );
    assert_eq!(
        transport.inputs().len(),
        1,
        "the over-limit turn is refused before any provider call"
    );
    assert!(
        handle
            .store
            .load_delegation_result(delegation)
            .await
            .unwrap()
            .is_none(),
        "the over-limit stop never seals the execution"
    );
    assert_eq!(
        handle
            .store
            .load_task(task.task)
            .await
            .unwrap()
            .unwrap()
            .task
            .progress,
        TaskProgress::InProgress,
        "the Task stays non-terminal for the conversation path (slice F) to report and continue"
    );
}
