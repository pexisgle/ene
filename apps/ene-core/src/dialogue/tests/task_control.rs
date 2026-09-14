//! Stage 4 slice F conversation-path E2E: the Owner asks, steers, chats,
//! cancels, and reads reports through the production dialogue / Task-owner
//! boundaries, never through a direct `seed_task` + `run_task_agent` bypass.
//!
//! Acceptance scenario 4: 4.3 (normal chat while the Task runs), 4.4 (cancel
//! report of what completed and what is unresolved), 4.5 (completion report
//! with changed file names, save location, and remainder), and 4.7 (existing
//! workspace files survive completion and cancel).

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures and helpers live outside #[test] functions, where clippy.toml's test allowances do not apply"
)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ene_companion::dialogue::ProposeSteeringCommand;
use ene_companion::{CompanionRepository as _, HistoryRepository as _, HistoryRole};
use ene_inference::fake::{FakeFailure, FakeProviderTransport};
use ene_inference::{
    InferenceTechnicalError, ProviderRequest, ProviderResponse, ProviderTransport,
};
use ene_primitive::RawId;
use ene_task::{
    CancelTaskCommand, SteeringPremiseRef, TaskCancelOutcome, TaskContextOrigin,
    TaskContextOriginKind, TaskProgress, TaskProposalOutcome, TaskPurpose, TaskRef,
    TaskRepository as _, TaskResultAcceptance, WorkspaceFolderRef, WorkspaceNeedRef,
};
use tokio::sync::{Notify, Semaphore};

use super::{accepted_round, current_generation, round_test_handle, submit_frame};
use crate::serve::{HostHandle, LiveInput};
use crate::task_run::TaskAgentRunOutcome;
use crate::test_support::live_input;

use super::task_run::ScriptedTransport;

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

/// Loads the newest Owner message identity, the conversation record a Task
/// proposal or steering command references as its origin.
async fn latest_owner_message(handle: &HostHandle) -> RawId {
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve");
    let items = handle
        .store
        .load_recent_timeline(companion, 8)
        .await
        .expect("the timeline must load");
    items
        .iter()
        .rev()
        .find(|item| item.role == HistoryRole::Owner)
        .expect("the owner message is in the timeline")
        .id
}

async fn progress(handle: &HostHandle, task: ene_task::TaskId) -> TaskProgress {
    handle
        .store
        .load_task(task)
        .await
        .unwrap()
        .expect("the task must load")
        .task
        .progress
}

fn origin(source: RawId) -> TaskContextOrigin {
    TaskContextOrigin {
        kind: TaskContextOriginKind::OwnerConversation,
        source,
    }
}

fn workspace_need(path: &std::path::Path) -> WorkspaceNeedRef {
    WorkspaceNeedRef {
        folder: WorkspaceFolderRef {
            path: path.to_string_lossy().into_owned(),
        },
        save_target: None,
    }
}

#[tokio::test]
async fn conversation_proposal_steering_and_stale_premise_through_the_owner_path() {
    let live = live_input("conversation-steering");
    let chat = ScriptedTransport::new(vec![
        String::from("I will prepare the report."),
        String::from("I will add the summary."),
    ]);
    let (handle, _dir) = round_test_handle("conversation-steering", &live, &chat)
        .await
        .expect("the production setup path completes");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve");

    // The Owner asks in the conversation: the ordinary dialogue turn accepts,
    // replies, and commits the Owner record.
    let responses = handle
        .handle_frame(
            request_frame(
                &handle,
                &live,
                Some(0),
                "task-request",
                "read input.txt and write report.md",
            ),
            live.clone(),
            &chat,
        )
        .await;
    assert!(accepted_round(&responses).is_ok());
    let request = latest_owner_message(&handle).await;

    let purpose = TaskPurpose {
        text: String::from("read input.txt and write report.md"),
    };
    let outcome = handle
        .propose_task(
            companion,
            purpose.clone(),
            origin(request),
            Some(workspace_need(workspace.path())),
        )
        .await
        .expect("the proposal must answer");
    let crate::task_control::TaskProposalHostOutcome::AcceptedAsTask {
        task,
        delegation: _delegation,
    } = outcome
    else {
        panic!("the conversation proposal must create and delegate, got {outcome:?}");
    };
    assert_eq!(progress(&handle, task.task).await, TaskProgress::InProgress);

    // The Owner steers in a second conversation turn; the instruction source
    // is the canonical Owner record.
    let responses = handle
        .handle_frame(
            request_frame(
                &handle,
                &live,
                Some(current_generation(&handle).await.expect("generation")),
                "steering-request",
                "add an executive summary",
            ),
            live.clone(),
            &chat,
        )
        .await;
    assert!(accepted_round(&responses).is_ok());
    let instruction = latest_owner_message(&handle).await;

    let record = handle
        .store
        .load_task(task.task)
        .await
        .unwrap()
        .expect("the task must load");
    let outcome = handle
        .propose_steering(ProposeSteeringCommand {
            premise: SteeringPremiseRef {
                expected: record.task.reference,
                purpose: record.task.purpose,
            },
            new_purpose: None,
            instruction_source: instruction,
        })
        .await
        .expect("the steering must answer");
    let TaskProposalOutcome::AcceptedAsSteering(advanced) = outcome else {
        panic!("the conversation steering must advance, got {outcome:?}");
    };
    assert_eq!(advanced.revision.as_u64(), task.revision.as_u64() + 1);
    let record = handle.store.load_task(task.task).await.unwrap().unwrap();
    assert!(
        record
            .context
            .iter()
            .any(|entry| entry.origin == origin(instruction)),
        "the adopted instruction references the canonical Owner record"
    );

    // A stale premise returns the existing domain outcome; nothing is
    // overwritten.
    let stale = handle
        .propose_steering(ProposeSteeringCommand {
            premise: SteeringPremiseRef {
                expected: task,
                purpose: purpose_ref(task),
            },
            new_purpose: None,
            instruction_source: RawId::new(),
        })
        .await
        .expect("the stale steering must answer");
    assert_eq!(
        stale,
        TaskProposalOutcome::StalePremise { current: advanced },
        "the stale caller is sent back to the current revision"
    );
}

/// The premise purpose identity of a freshly created Task.
fn purpose_ref(task: TaskRef) -> ene_task::TaskPurposeRef {
    ene_task::TaskPurposeRef {
        task: task.task,
        adopted_revision: task.revision,
    }
}

#[tokio::test]
async fn normal_chat_continues_while_the_task_runs_and_cancel_reports_completed_changes() {
    let live = live_input("conversation-cancel");
    let agent = Arc::new(GateTransport::new(vec![
        String::from(r#"{"tool":"read","path":"input.txt"}"#),
        String::from(
            "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}",
        ),
    ]));
    let (handle, _dir) = round_test_handle("conversation-cancel", &live, &*agent)
        .await
        .expect("the production setup path completes");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve");

    let chat = ScriptedTransport::new(vec![String::from("Starting on the report.")]);
    let responses = handle
        .handle_frame(
            request_frame(
                &handle,
                &live,
                Some(0),
                "cancel-request",
                "read input.txt and write report.md",
            ),
            live.clone(),
            &chat,
        )
        .await;
    assert!(accepted_round(&responses).is_ok());
    let request = latest_owner_message(&handle).await;
    let outcome = handle
        .propose_task(
            companion,
            TaskPurpose {
                text: String::from("read input.txt and write report.md"),
            },
            origin(request),
            Some(workspace_need(workspace.path())),
        )
        .await
        .unwrap();
    let crate::task_control::TaskProposalHostOutcome::AcceptedAsTask { task, delegation } = outcome
    else {
        panic!("the proposal must create and delegate, got {outcome:?}");
    };

    // Run the Task Agent in the background. It performs the read and create,
    // then blocks on the third provider call.
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

    // Acceptance 4.3: the ordinary conversation still completes while the
    // Task Agent is in flight.
    let chat_while_running =
        ScriptedTransport::new(vec![String::from("The report is nearly ready.")]);
    let responses = handle
        .handle_frame(
            request_frame(
                &handle,
                &live,
                Some(current_generation(&handle).await.expect("generation")),
                "chat-while-running",
                "how is it going?",
            ),
            live.clone(),
            &chat_while_running,
        )
        .await;
    assert!(
        accepted_round(&responses).is_ok(),
        "the ordinary chat is accepted while the Task runs"
    );
    assert!(
        responses.iter().any(|frame| matches!(
            &frame.payload,
            ene_api::v1::payload::WirePayload::TextStreamClose(close)
                if close.status == ene_api::v1::round::StreamClose::Completed
        )),
        "the ordinary chat completes while the Task runs"
    );

    // Acceptance 4.4: cancel from the conversation reaches the durable
    // admission and stops the loop best-effort.
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
    assert_eq!(progress(&handle, task.task).await, TaskProgress::Cancelled);

    let report = handle
        .task_report(task.task, Some(delegation))
        .await
        .expect("the report must answer")
        .expect("the task exists");
    assert_eq!(report.progress, TaskProgress::Cancelled);
    assert!(!report.result_adopted);
    let rendered = report.render();
    assert!(
        rendered.contains("create") && rendered.contains("report.md"),
        "the report lists the completed change: {rendered}"
    );
    assert!(
        rendered.contains(&workspace.path().to_string_lossy().to_string()),
        "the report names the save location: {rendered}"
    );
    assert!(
        !rendered.contains("all stopped"),
        "the report never claims every effect stopped"
    );

    // Acceptance 4.7: cancel never deletes the workspace's files.
    assert!(workspace.path().join("input.txt").exists());
    assert!(workspace.path().join("report.md").exists());
}

#[tokio::test]
async fn completion_report_through_the_conversation_path_names_files_and_keeps_the_workspace() {
    let live = live_input("conversation-complete");
    let agent = ScriptedTransport::new(vec![
        String::from(r#"{"tool":"read","path":"input.txt"}"#),
        String::from(
            "{\"tool\":\"create\",\"path\":\"report.md\",\"content\":\"# Report\\nnotes\"}",
        ),
        String::from(r#"{"final":"created report.md from input.txt"}"#),
    ]);
    let (handle, _dir) = round_test_handle("conversation-complete", &live, &agent)
        .await
        .expect("the production setup path completes");
    let workspace = tempfile::tempdir().expect("workspace directory");
    std::fs::write(workspace.path().join("input.txt"), b"notes").expect("input fixture");
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve");

    let chat = ScriptedTransport::new(vec![String::from("I will write the report.")]);
    let responses = handle
        .handle_frame(
            request_frame(
                &handle,
                &live,
                Some(0),
                "complete-request",
                "read input.txt and write report.md",
            ),
            live.clone(),
            &chat,
        )
        .await;
    assert!(accepted_round(&responses).is_ok());
    let request = latest_owner_message(&handle).await;
    let outcome = handle
        .propose_task(
            companion,
            TaskPurpose {
                text: String::from("read input.txt and write report.md"),
            },
            origin(request),
            Some(workspace_need(workspace.path())),
        )
        .await
        .unwrap();
    let crate::task_control::TaskProposalHostOutcome::AcceptedAsTask { task, delegation } = outcome
    else {
        panic!("the proposal must create and delegate, got {outcome:?}");
    };

    let run = handle.run_task_agent(&agent, delegation).await.unwrap();
    let TaskAgentRunOutcome::Finalized { acceptance, .. } = run else {
        panic!("the conversation-created task must complete, got {run:?}");
    };
    assert_eq!(acceptance, TaskResultAcceptance::AdoptedAsCompletion(task));
    assert_eq!(progress(&handle, task.task).await, TaskProgress::Completed);

    // Acceptance 4.5: the report names the changed file, the save location,
    // and the remainder, and carries the final result body.
    let report = handle
        .task_report(task.task, Some(delegation))
        .await
        .unwrap()
        .expect("the task exists");
    assert!(report.result_adopted);
    assert_eq!(
        report.result_body.as_deref(),
        Some("created report.md from input.txt")
    );
    let rendered = report.render();
    assert!(rendered.contains("task status: completed"), "{rendered}");
    assert!(rendered.contains("report.md"), "{rendered}");
    assert!(
        rendered.contains(&workspace.path().to_string_lossy().to_string()),
        "{rendered}"
    );
    assert!(
        rendered.contains("remaining/unconfirmed effects:\n- none"),
        "{rendered}"
    );

    // Acceptance 4.7: completion never deletes the workspace's files.
    assert!(workspace.path().join("input.txt").exists());
    assert_eq!(
        std::fs::read(workspace.path().join("report.md")).expect("report exists"),
        b"# Report\nnotes"
    );
}

#[tokio::test]
async fn a_transient_provider_failure_never_becomes_a_task_failure() {
    let live = live_input("conversation-transient");
    let chat = ScriptedTransport::new(vec![String::from("I will start.")]);
    let (handle, _dir) = round_test_handle("conversation-transient", &live, &chat)
        .await
        .expect("the production setup path completes");
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve");
    let responses = handle
        .handle_frame(
            request_frame(
                &handle,
                &live,
                Some(0),
                "transient-request",
                "write the report",
            ),
            live.clone(),
            &chat,
        )
        .await;
    assert!(accepted_round(&responses).is_ok());
    let request = latest_owner_message(&handle).await;
    let outcome = handle
        .propose_task(
            companion,
            TaskPurpose {
                text: String::from("write the report"),
            },
            origin(request),
            None,
        )
        .await
        .unwrap();
    let crate::task_control::TaskProposalHostOutcome::AcceptedAsTask { task, delegation } = outcome
    else {
        panic!("the proposal must create and delegate, got {outcome:?}");
    };

    let failing = FakeProviderTransport::failing(FakeFailure::ResponseLost);
    let run = handle.run_task_agent(&failing, delegation).await;
    assert!(
        run.is_err(),
        "a provider transport failure stays a technical class, got {run:?}"
    );
    assert_eq!(
        progress(&handle, task.task).await,
        TaskProgress::InProgress,
        "a transient provider failure is never a confirmed Task failure"
    );
    let report = handle
        .task_report(task.task, Some(delegation))
        .await
        .unwrap()
        .expect("the task exists");
    assert_eq!(report.progress, TaskProgress::InProgress);
    assert!(report.result_body.is_none());
}
