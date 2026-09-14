//! Host composition checks for conversation / first-party Task control:
//! proposal → creation → delegation, late certainty settlement →
//! re-evaluation, bounded recovery reconciliation, canonical-fact reports,
//! and the Failed admission gates.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures and helpers live outside #[test] functions, where clippy.toml's test allowances do not apply"
)]

use std::sync::atomic::{AtomicUsize, Ordering};

use ene_action::{
    ActionAttemptId, ActionAttemptRepository as _, ActionCertainty, ActionStartOutcome,
    AttemptCommitPremise, CertaintyUpdateOutcome, EffectGrounds, OperationKind, RealTargetRef,
};
use ene_companion::CompanionRepository as _;
use ene_companion::dialogue::TaskReportCertainty;
use ene_inference::{
    InferenceTechnicalError, ProviderRequest, ProviderResponse, ProviderTransport,
};
use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
use ene_task::{
    AssigneeRef, CancelTaskCommand, DelegatedWorkspace, DelegationCreationPremise, DelegationId,
    DelegationOutcome, DelegationScope, TaskAgentEphemeralId, TaskAgentOutput, TaskCancelOutcome,
    TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind, TaskCreationPremise,
    TaskFailureKind, TaskFailureOutcome, TaskFailurePremise, TaskId, TaskProgress, TaskPurpose,
    TaskRef, TaskRepository as _, TaskResultAcceptance, TaskResultAdoptionClaim, WorkspaceAssocId,
    WorkspaceAssociationPremise, WorkspaceFolderRef, WorkspaceNeedRef, orchestrate_result_arrival,
};

use super::{RECONCILIATION_PAGE_SIZE, TaskProposalHostOutcome};
use crate::serve::HostHandle;
use crate::test_support::memory_handle_with;

/// Provider transport that counts calls and never answers usefully. A call
/// means an execution reached provider I/O when the test pinned zero.
#[derive(Default)]
struct CountingTransport {
    calls: AtomicUsize,
}

impl CountingTransport {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ProviderTransport for CountingTransport {
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
        Box::pin(async move { Err(InferenceTechnicalError::ResponseLost) })
    }
}

async fn open_handle(tag: &str) -> (HostHandle, tempfile::TempDir) {
    memory_handle_with(tag, |_| {})
        .await
        .expect("the handle must open")
}

/// Seeds one Task with a confirmed workspace association and one delegation,
/// mirroring the proposal path without exercising it.
async fn seed_execution(
    handle: &HostHandle,
    folder: &str,
) -> (TaskRef, DelegationId, WorkspaceAssocId) {
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve");
    let assoc = WorkspaceAssocId::generate();
    let task = handle
        .store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("read the input and write the report"),
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
                    folder: WorkspaceFolderRef {
                        path: folder.to_owned(),
                    },
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
                    folder: WorkspaceFolderRef {
                        path: folder.to_owned(),
                    },
                    save_target: None,
                }),
            },
        })
        .await
        .expect("delegation creation commits");
    assert!(matches!(delegated, DelegationOutcome::Delegated(_)));
    (task, delegation, assoc)
}

async fn start_attempt(
    handle: &HostHandle,
    delegation: DelegationId,
    task: TaskRef,
    assoc: WorkspaceAssocId,
    path: &str,
    operation: OperationKind,
) -> ActionAttemptId {
    let attempt = ActionAttemptId::generate();
    let outcome = handle
        .store
        .insert_attempt_if_current(AttemptCommitPremise {
            attempt,
            delegation: delegation.as_raw(),
            task: task.task.as_raw(),
            task_revision: RevisionInner::from_u64(task.revision.as_u64()),
            workspace: assoc.as_raw(),
            real_target: RealTargetRef::from_canonical_path(path.to_owned()),
            operation,
            relied_evaluation: RawId::new(),
        })
        .await
        .expect("the AU5 insert must answer");
    assert_eq!(outcome, ActionStartOutcome::Started);
    attempt
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

/// A platform-absolute attempt target: the Action read-back requires an
/// absolute path on every supported platform, so tests must not hardcode a
/// Unix-shaped one.
fn canonical_target(name: &str) -> String {
    std::env::temp_dir()
        .join(name)
        .to_string_lossy()
        .into_owned()
}

#[tokio::test]
async fn a_conversation_proposal_creates_the_task_association_and_delegation() {
    let (handle, _dir) = open_handle("proposal").await;
    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve");
    let workspace = tempfile::tempdir().expect("workspace directory");
    let outcome = handle
        .propose_task(
            companion,
            TaskPurpose {
                text: String::from("read input.txt and write report.md"),
            },
            TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            Some(WorkspaceNeedRef {
                folder: WorkspaceFolderRef {
                    path: workspace.path().to_string_lossy().into_owned(),
                },
                save_target: None,
            }),
        )
        .await
        .expect("the proposal must answer");
    let TaskProposalHostOutcome::AcceptedAsTask { task, delegation } = outcome else {
        panic!("the proposal must be accepted and delegated, got {outcome:?}");
    };

    let record = handle
        .store
        .load_task(task.task)
        .await
        .unwrap()
        .expect("the created task must load");
    assert_eq!(record.task.reference, task);
    assert_eq!(record.task.assignee.companion, companion.as_raw());
    assert_eq!(record.task.progress, TaskProgress::InProgress);
    let association = record.workspace.expect("the association must commit");
    assert_eq!(association.folder.path, workspace.path().to_string_lossy());
    let correspondence = handle
        .store
        .load_delegation(delegation)
        .await
        .unwrap()
        .expect("the delegation must commit");
    assert_eq!(correspondence.task, task);
    assert_eq!(
        correspondence
            .scope
            .workspace
            .expect("the scope copies the association")
            .assoc,
        association.assoc
    );
}

#[tokio::test]
async fn late_certainty_settlement_re_evaluates_the_sealed_result() {
    let (handle, _dir) = open_handle("settlement").await;
    let (task, delegation, assoc) = seed_execution(&handle, "/srv/workspace/ene").await;
    let attempt = start_attempt(
        &handle,
        delegation,
        task,
        assoc,
        &canonical_target("report.md"),
        OperationKind::Create,
    )
    .await;
    let result = orchestrate_result_arrival(
        &handle.store,
        delegation,
        TaskAgentOutput::new(String::from("report.md")),
    )
    .await
    .expect("the arrival records");
    let withheld = handle
        .store
        .adopt_result(TaskResultAdoptionClaim {
            result: result.result,
            attempt_refs: vec![attempt.as_raw()],
        })
        .await
        .unwrap();
    assert!(matches!(
        withheld,
        TaskResultAcceptance::WithheldByEffectFacts { .. }
    ));
    assert_eq!(progress(&handle, task.task).await, TaskProgress::InProgress);

    // The settlement commits the Action owner's certainty and then re-runs the
    // Task owner's adoption gate.
    let settled = handle
        .settle_action_certainty(
            attempt,
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget,
        )
        .await
        .expect("the settlement must answer");
    assert_eq!(settled.certainty, CertaintyUpdateOutcome::Updated);
    assert_eq!(
        settled.adoption,
        Some(TaskResultAcceptance::AdoptedAsCompletion(task))
    );
    assert_eq!(progress(&handle, task.task).await, TaskProgress::Completed);

    // A duplicate settlement is stale and triggers no second evaluation.
    let repeated = handle
        .settle_action_certainty(
            attempt,
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget,
        )
        .await
        .expect("the duplicate settlement must answer");
    assert_eq!(
        repeated.certainty,
        CertaintyUpdateOutcome::StaleCurrent {
            current: ActionCertainty::ConfirmedSuccess
        }
    );
    assert_eq!(repeated.adoption, None);
}

#[tokio::test]
async fn recovery_reconciliation_is_idempotent_and_visits_every_candidate() {
    let (handle, _dir) = open_handle("reconcile").await;
    let (first_task, first_delegation, first_assoc) =
        seed_execution(&handle, "/srv/workspace/ene").await;
    let first_attempt = start_attempt(
        &handle,
        first_delegation,
        first_task,
        first_assoc,
        &canonical_target("first.md"),
        OperationKind::Create,
    )
    .await;
    handle
        .settle_action_certainty(
            first_attempt,
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget,
        )
        .await
        .unwrap();
    let _first = orchestrate_result_arrival(
        &handle.store,
        first_delegation,
        TaskAgentOutput::new(String::from("first")),
    )
    .await
    .unwrap();

    let (second_task, second_delegation, _) = seed_execution(&handle, "/srv/workspace/ene").await;
    let _second = orchestrate_result_arrival(
        &handle.store,
        second_delegation,
        TaskAgentOutput::new(String::from("second")),
    )
    .await
    .unwrap();

    let adopted = handle.reconcile_sealed_results().await.unwrap();
    assert_eq!(adopted.len(), 2);
    assert_eq!(
        progress(&handle, first_task.task).await,
        TaskProgress::Completed
    );
    assert_eq!(
        progress(&handle, second_task.task).await,
        TaskProgress::Completed
    );
    assert!(matches!(
        adopted[0].adoption,
        Ok(TaskResultAcceptance::AdoptedAsCompletion(_))
    ));

    // All candidates are adopted, so a further pass is empty and writes
    // nothing.
    assert!(handle.reconcile_sealed_results().await.unwrap().is_empty());
}

#[tokio::test]
async fn reconciliation_reaches_recoverable_results_behind_permanently_unadopted_ones() {
    let (handle, dir) = open_handle("reconcile-starvation").await;
    let mut permanently_unadopted = Vec::new();
    let mut result_order = Vec::new();
    for index in 0..RECONCILIATION_PAGE_SIZE + 1 {
        let (task, delegation, _) = seed_execution(&handle, "/srv/workspace/ene").await;
        assert_eq!(
            handle
                .cancel_task(CancelTaskCommand { task: task.task })
                .await
                .unwrap(),
            TaskCancelOutcome::CancelAccepted
        );
        let result = orchestrate_result_arrival(
            &handle.store,
            delegation,
            TaskAgentOutput::new(format!("late body {index}")),
        )
        .await
        .unwrap();
        permanently_unadopted.push((task, result.result));
        result_order.push(result.result);
    }

    // One recoverable result sorts strictly after the permanently-unadopted
    // front.
    let (recoverable_task, recoverable_delegation, recoverable_assoc) =
        seed_execution(&handle, "/srv/workspace/ene").await;
    let attempt = start_attempt(
        &handle,
        recoverable_delegation,
        recoverable_task,
        recoverable_assoc,
        &canonical_target("recoverable.md"),
        OperationKind::Create,
    )
    .await;
    handle
        .settle_action_certainty(
            attempt,
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget,
        )
        .await
        .unwrap();
    let recoverable = orchestrate_result_arrival(
        &handle.store,
        recoverable_delegation,
        TaskAgentOutput::new(String::from("recoverable")),
    )
    .await
    .unwrap();
    result_order.push(recoverable.result);
    rewrite_result_times(dir.path(), &result_order);

    // Each storage read is bounded by the page size, and the recoverable
    // candidate is not in the first page but is reached by the next one.
    let first_page = handle
        .store
        .list_unadopted_results_after(None, RECONCILIATION_PAGE_SIZE)
        .await
        .unwrap();
    assert_eq!(first_page.len() as u64, RECONCILIATION_PAGE_SIZE);
    assert!(
        first_page
            .iter()
            .all(|page| page.result != recoverable.result)
    );
    let second_page = handle
        .store
        .list_unadopted_results_after(first_page.last().copied(), RECONCILIATION_PAGE_SIZE)
        .await
        .unwrap();
    assert_eq!(second_page.len(), 2);

    // One pass visits every candidate: the older permanently-unadopted
    // results keep their semantics, and the later recoverable result
    // completes.
    let outcomes = handle.reconcile_sealed_results().await.unwrap();
    assert_eq!(
        outcomes.len() as u64,
        RECONCILIATION_PAGE_SIZE + 2,
        "one pass reaches the result behind the permanently-unadopted front"
    );
    assert_eq!(
        progress(&handle, recoverable_task.task).await,
        TaskProgress::Completed
    );
    for (task, result) in &permanently_unadopted {
        assert_eq!(progress(&handle, task.task).await, TaskProgress::Cancelled);
        let outcome = outcomes
            .iter()
            .find(|outcome| outcome.result == *result)
            .expect("every permanently-unadopted candidate is evaluated");
        assert_eq!(
            outcome.adoption,
            Ok(TaskResultAcceptance::RecordedToOriginalOnly)
        );
    }
    assert_eq!(
        attempt_rows(dir.path()),
        1,
        "reconciliation never replays an Action"
    );
}

/// Rewrites the reconciliation keys of the given results to a deterministic
/// increasing order, so the starvation regression does not depend on clock
/// resolution.
///
/// The nanoseconds are non-zero and not a multiple of 1000, exactly the
/// representation `WallClockWithTz::to_rfc3339` produces for such values, so
/// the stored text parses and re-renders byte-identically and the keyset
/// cursor comparison is stable.
fn rewrite_result_times(dir: &std::path::Path, results: &[ene_task::TaskResultId]) {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    for (index, result) in results.iter().enumerate() {
        let recorded_at = format!(
            "2026-09-08T12:{:02}:{:02}.{:09}+09:00",
            index / 60,
            index % 60,
            7 * index + 1
        );
        conn.execute(
            "UPDATE task_result SET recorded_at = ?1 WHERE result_id = ?2",
            rusqlite::params![
                recorded_at,
                result.as_raw().as_uuid().as_hyphenated().to_string()
            ],
        )
        .expect("the test keyset rewrite must apply");
    }
}

fn attempt_rows(dir: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the store file must open");
    conn.query_row("SELECT COUNT(*) FROM action_attempt", (), |row| row.get(0))
        .expect("the attempt count must read")
}

#[tokio::test]
async fn the_task_report_composes_canonical_task_and_action_facts() {
    let (handle, _dir) = open_handle("report").await;
    let (task, delegation, assoc) = seed_execution(&handle, "/srv/workspace/ene").await;
    let attempt = start_attempt(
        &handle,
        delegation,
        task,
        assoc,
        &canonical_target("report.md"),
        OperationKind::Create,
    )
    .await;
    handle
        .settle_action_certainty(
            attempt,
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget,
        )
        .await
        .unwrap();
    let result = orchestrate_result_arrival(
        &handle.store,
        delegation,
        TaskAgentOutput::new(String::from("report.md was created from input.txt")),
    )
    .await
    .unwrap();
    assert_eq!(
        handle
            .store
            .adopt_result(TaskResultAdoptionClaim {
                result: result.result,
                attempt_refs: vec![attempt.as_raw()],
            })
            .await
            .unwrap(),
        TaskResultAcceptance::AdoptedAsCompletion(task)
    );

    let report = handle
        .task_report(task.task, Some(delegation))
        .await
        .expect("the report read must answer")
        .expect("the task exists");
    assert_eq!(report.progress, TaskProgress::Completed);
    assert_eq!(
        report.workspace_folder.as_deref(),
        Some("/srv/workspace/ene")
    );
    assert!(report.result_adopted);
    assert_eq!(
        report.result_body.as_deref(),
        Some("report.md was created from input.txt")
    );
    assert_eq!(report.correlated_attempts.len(), 1);
    let rendered = report.render();
    assert!(rendered.contains("report.md"), "{rendered}");
    assert!(rendered.contains("/srv/workspace/ene"), "{rendered}");
    assert!(
        rendered.contains("remaining/unconfirmed effects:\n- none"),
        "{rendered}"
    );

    assert!(
        handle
            .task_report(TaskId::generate(), None)
            .await
            .expect("the missing-task read must answer")
            .is_none()
    );
}

#[tokio::test]
async fn a_failed_task_refuses_every_new_work_start_with_zero_provider_calls() {
    let (handle, _dir) = open_handle("failed-gates").await;
    let (task, delegation, _assoc) = seed_execution(&handle, "/srv/workspace/ene").await;
    assert_eq!(
        handle
            .store
            .fail_task(TaskFailurePremise {
                task,
                delegation: Some(delegation),
                kind: TaskFailureKind::ConfirmedUnachievable,
            })
            .await
            .unwrap(),
        TaskFailureOutcome::FailedAs(task)
    );

    let transport = CountingTransport::default();
    let run = handle
        .run_task_agent(&transport, delegation)
        .await
        .expect("a terminal task answers a domain outcome");
    assert_eq!(
        run,
        crate::task_run::TaskAgentRunOutcome::Refused(
            crate::task_run::TaskAgentRunRefusal::TaskTerminal {
                task: task.task,
                progress: TaskProgress::Failed,
            }
        )
    );
    assert_eq!(
        transport.calls(),
        0,
        "a failed Task never reaches provider I/O"
    );

    let report = handle
        .task_report(task.task, Some(delegation))
        .await
        .unwrap()
        .expect("the task exists");
    assert_eq!(report.progress, TaskProgress::Failed);
    assert!(report.result_body.is_none());
}

#[tokio::test]
async fn a_cancel_report_distinguishes_confirmed_changes_from_unknown_effects() {
    let (handle, _dir) = open_handle("cancel-report").await;
    let (task, delegation, assoc) = seed_execution(&handle, "/srv/workspace/ene").await;
    start_attempt(
        &handle,
        delegation,
        task,
        assoc,
        &canonical_target("half.md"),
        OperationKind::Create,
    )
    .await;
    // The attempt started but no evidence settled it: it stays Unknown.
    assert_eq!(
        handle
            .cancel_task(CancelTaskCommand { task: task.task })
            .await
            .unwrap(),
        TaskCancelOutcome::CancelAccepted
    );

    let report = handle
        .task_report(task.task, Some(delegation))
        .await
        .unwrap()
        .expect("the task exists");
    assert_eq!(report.progress, TaskProgress::Cancelled);
    assert!(report.result_body.is_none());
    assert_eq!(report.other_attempts.len(), 1);
    assert_eq!(
        report.other_attempts[0].certainty,
        TaskReportCertainty::Unknown,
        "the unknown effect stays unknown and is never rounded"
    );
    let rendered = report.render();
    assert!(rendered.contains("task status: cancelled"), "{rendered}");
    assert!(rendered.contains("(unknown)"), "{rendered}");
    assert!(
        rendered.contains("completed changes:\n- none"),
        "an unknown effect is not a completed change: {rendered}"
    );
    assert!(
        !rendered.contains("all stopped"),
        "cancel acceptance never claims every effect stopped: {rendered}"
    );
}

#[tokio::test]
async fn an_admission_not_sent_never_becomes_a_task_failure() {
    // No consent or credential is provisioned, so the Task Agent admission is
    // declined before any provider I/O.
    let (handle, _dir) = open_handle("not-sent").await;
    let (task, delegation, _assoc) = seed_execution(&handle, "/srv/workspace/ene").await;
    let transport = CountingTransport::default();
    let run = handle
        .run_task_agent(&transport, delegation)
        .await
        .expect("a declined admission is a domain outcome");
    assert!(
        matches!(run, crate::task_run::TaskAgentRunOutcome::NotSent(_)),
        "the admission decline stays NotSent, got {run:?}"
    );
    assert_eq!(transport.calls(), 0);
    assert_eq!(
        progress(&handle, task.task).await,
        TaskProgress::InProgress,
        "NotSent is never a confirmed Task failure"
    );
}
