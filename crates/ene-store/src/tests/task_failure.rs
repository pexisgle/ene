//! Confirmed terminal failure: the `Failed` producer's lifecycle, its stale
//! and foreign-premise refusals, the exactly-one terminal winner under races,
//! and the existing admission gates it reuses.
//!
//! The durable checks reproduce the slice G contract: `Failed` is only
//! committed by one explicit CAS over the single `task.progress` master, the
//! premise's relied revision and optional delegation are compared in the same
//! transaction, and no provider transient, `NotSent`, `Unknown`, withheld
//! result, or cancel outcome is ever phrased as failure.

use super::*;

use ene_action::{
    ActionAttemptRepository as _, ActionStartOutcome, AttemptCommitPremise, OperationKind,
    RealTargetRef,
};
use ene_task::{
    TaskAgentOutput, TaskFailureKind, TaskFailureOutcome, TaskFailurePremise, TaskProgress,
    TaskResultAcceptance, TaskResultAdoptionClaim, orchestrate_result_arrival,
};

fn failure(task: TaskRef, delegation: Option<DelegationId>) -> TaskFailurePremise {
    TaskFailurePremise {
        task,
        delegation,
        kind: TaskFailureKind::ConfirmedUnachievable,
    }
}

async fn seed_execution(store: &Store) -> (TaskRef, DelegationId, WorkspaceAssocId) {
    let workspace = task_workspace("/srv/workspace/ene", None);
    let assoc = workspace.assoc;
    let created = store
        .create_task(task_premise(Some(workspace)))
        .await
        .expect("the AU2 task must commit");
    let delegation = DelegationId::generate();
    let delegated = store
        .create_delegation(delegation_premise(
            delegation,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(Some(delegated_workspace(assoc, "/srv/workspace/ene", None))),
        ))
        .await
        .expect("the AU3 delegation must answer");
    assert!(matches!(delegated, DelegationOutcome::Delegated(_)));
    (created, delegation, assoc)
}

async fn progress(store: &Store, task: TaskId) -> TaskProgress {
    store
        .load_task(task)
        .await
        .unwrap()
        .expect("the task must load")
        .task
        .progress
}

#[tokio::test]
async fn started_task_moves_to_failed_once_and_repeats_idempotently() {
    let store = open_memory().await.unwrap();
    let created = store
        .create_task(task_premise(None))
        .await
        .expect("the AU2 task must commit");
    assert_eq!(progress(&store, created.task).await, TaskProgress::Started);

    let outcome = store
        .fail_task(failure(created, None))
        .await
        .expect("the failure commit must answer");
    assert_eq!(outcome, TaskFailureOutcome::FailedAs(created));
    assert_eq!(progress(&store, created.task).await, TaskProgress::Failed);

    // The same request again is an idempotent domain answer with zero writes.
    assert_eq!(
        store
            .fail_task(failure(created, None))
            .await
            .expect("the repeated failure must answer"),
        TaskFailureOutcome::AlreadyFailed { task: created.task }
    );
    assert_eq!(progress(&store, created.task).await, TaskProgress::Failed);
}

#[tokio::test]
async fn in_progress_task_with_its_delegation_moves_to_failed() {
    let store = open_memory().await.unwrap();
    let (created, delegation, _assoc) = seed_execution(&store).await;
    assert_eq!(
        progress(&store, created.task).await,
        TaskProgress::InProgress
    );

    assert_eq!(
        store
            .fail_task(failure(created, Some(delegation)))
            .await
            .expect("the failure commit must answer"),
        TaskFailureOutcome::FailedAs(created)
    );
    assert_eq!(progress(&store, created.task).await, TaskProgress::Failed);
}

#[tokio::test]
async fn a_failed_task_stays_failed_after_reopen() {
    let directory = tempfile::tempdir().expect("store directory");
    let path = directory.path().join("failure.db");
    let created = {
        let store = Store::open(&path).await.expect("the fresh store must open");
        let created = store
            .create_task(task_premise(None))
            .await
            .expect("the AU2 task must commit");
        assert_eq!(
            store
                .fail_task(failure(created, None))
                .await
                .expect("the failure commit must answer"),
            TaskFailureOutcome::FailedAs(created)
        );
        created
    };
    let reopened = Store::open(&path).await.expect("the reopen must succeed");
    assert_eq!(
        progress(&reopened, created.task).await,
        TaskProgress::Failed,
        "the terminal state is durable and never auto-resumed"
    );
    assert_eq!(
        reopened
            .fail_task(failure(created, None))
            .await
            .expect("the repeated failure must answer"),
        TaskFailureOutcome::AlreadyFailed { task: created.task }
    );
}

#[tokio::test]
async fn completion_cannot_overwrite_failed_and_failed_cannot_overwrite_completion() {
    let store = open_memory().await.unwrap();

    // Failed first: a later completion attempt records to the original only.
    let (failed_task, failed_delegation, _) = seed_execution(&store).await;
    assert_eq!(
        store
            .fail_task(failure(failed_task, Some(failed_delegation)))
            .await
            .unwrap(),
        TaskFailureOutcome::FailedAs(failed_task)
    );
    let late = orchestrate_result_arrival(
        &store,
        failed_delegation,
        TaskAgentOutput::new(String::from("late final body")),
    )
    .await
    .expect("the arrival record survives failure");
    assert_eq!(
        store
            .adopt_result(TaskResultAdoptionClaim {
                result: late.result,
                attempt_refs: Vec::new(),
            })
            .await
            .unwrap(),
        TaskResultAcceptance::RecordedToOriginalOnly
    );
    assert_eq!(
        progress(&store, failed_task.task).await,
        TaskProgress::Failed,
        "a delayed result never reverses failure"
    );

    // Completed first: the failure producer answers the terminal state.
    let (completed_task, completed_delegation, _) = seed_execution(&store).await;
    let result = orchestrate_result_arrival(
        &store,
        completed_delegation,
        TaskAgentOutput::new(String::from("done")),
    )
    .await
    .expect("the result must record");
    assert_eq!(
        store
            .adopt_result(TaskResultAdoptionClaim {
                result: result.result,
                attempt_refs: Vec::new(),
            })
            .await
            .unwrap(),
        TaskResultAcceptance::AdoptedAsCompletion(completed_task)
    );
    assert_eq!(
        store
            .fail_task(failure(completed_task, Some(completed_delegation)))
            .await
            .unwrap(),
        TaskFailureOutcome::TaskTerminal {
            task: completed_task.task,
            progress: TaskProgress::Completed,
        }
    );
    assert_eq!(
        progress(&store, completed_task.task).await,
        TaskProgress::Completed
    );
}

#[tokio::test]
async fn cancel_cannot_overwrite_failed_and_failed_cannot_overwrite_cancel() {
    let store = open_memory().await.unwrap();

    // Failed first: cancel answers the terminal state.
    let (failed_task, _, _) = seed_execution(&store).await;
    assert_eq!(
        store.fail_task(failure(failed_task, None)).await.unwrap(),
        TaskFailureOutcome::FailedAs(failed_task)
    );
    assert_eq!(
        store.cancel_task(failed_task.task).await.unwrap(),
        ene_task::TaskCancelOutcome::TaskTerminal {
            task: failed_task.task,
            progress: TaskProgress::Failed,
        }
    );
    assert_eq!(
        progress(&store, failed_task.task).await,
        TaskProgress::Failed
    );

    // Cancelled first: the failure producer cannot rewrite the terminal value.
    let (cancelled_task, cancelled_delegation, _) = seed_execution(&store).await;
    assert_eq!(
        store.cancel_task(cancelled_task.task).await.unwrap(),
        ene_task::TaskCancelOutcome::CancelAccepted
    );
    assert_eq!(
        store
            .fail_task(failure(cancelled_task, Some(cancelled_delegation)))
            .await
            .unwrap(),
        TaskFailureOutcome::TaskTerminal {
            task: cancelled_task.task,
            progress: TaskProgress::Cancelled,
        }
    );
    assert_eq!(
        progress(&store, cancelled_task.task).await,
        TaskProgress::Cancelled
    );
}

#[tokio::test]
async fn a_failure_completion_race_has_exactly_one_terminal_winner() {
    let store = open_memory().await.unwrap();
    let (task, delegation, _) = seed_execution(&store).await;

    let failure = store.fail_task(failure(task, Some(delegation)));
    let completion = async {
        let result = orchestrate_result_arrival(
            &store,
            delegation,
            TaskAgentOutput::new(String::from("done")),
        )
        .await
        .expect("the result must record");
        store
            .adopt_result(TaskResultAdoptionClaim {
                result: result.result,
                attempt_refs: Vec::new(),
            })
            .await
            .expect("the adoption must answer")
    };
    let (failed, adopted) = tokio::join!(failure, completion);
    let final_progress = progress(&store, task.task).await;
    match failed.expect("the failure must answer") {
        TaskFailureOutcome::FailedAs(_) => {
            assert_eq!(final_progress, TaskProgress::Failed);
            assert_eq!(adopted, TaskResultAcceptance::RecordedToOriginalOnly);
        }
        TaskFailureOutcome::TaskTerminal {
            progress: TaskProgress::Completed,
            ..
        } => {
            assert_eq!(final_progress, TaskProgress::Completed);
            assert_eq!(adopted, TaskResultAcceptance::AdoptedAsCompletion(task));
        }
        other => panic!("unexpected race loser: {other:?}"),
    }
}

#[tokio::test]
async fn a_failure_cancel_race_has_exactly_one_terminal_winner() {
    let store = open_memory().await.unwrap();
    let (task, _, _) = seed_execution(&store).await;

    let failure = store.fail_task(failure(task, None));
    let cancel = store.cancel_task(task.task);
    let (failed, cancelled) = tokio::join!(failure, cancel);
    let final_progress = progress(&store, task.task).await;
    match failed.expect("the failure must answer") {
        TaskFailureOutcome::FailedAs(_) => {
            assert_eq!(final_progress, TaskProgress::Failed);
            assert_eq!(
                cancelled.unwrap(),
                ene_task::TaskCancelOutcome::TaskTerminal {
                    task: task.task,
                    progress: TaskProgress::Failed,
                }
            );
        }
        TaskFailureOutcome::TaskTerminal {
            progress: TaskProgress::Cancelled,
            ..
        } => {
            assert_eq!(final_progress, TaskProgress::Cancelled);
            assert_eq!(
                cancelled.unwrap(),
                ene_task::TaskCancelOutcome::CancelAccepted
            );
        }
        other => panic!("unexpected race loser: {other:?}"),
    }
}

#[tokio::test]
async fn a_stale_premise_cannot_fail_a_steered_task() {
    let store = open_memory().await.unwrap();
    let (created, delegation, _) = seed_execution(&store).await;

    // Steering advances the revision; the old observation must not terminate
    // the current Task.
    let advanced = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(TaskPurposeAdoptionPremise {
                purpose: TaskPurpose {
                    text: String::from("new direction"),
                },
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source: RawId::new(),
                },
                acquired_at: fixture_clock(),
            }),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .unwrap();
    let TaskCommitOutcome::CommittedAs(current) = advanced else {
        panic!("the steering must commit, got {advanced:?}");
    };

    assert_eq!(
        store
            .fail_task(failure(created, Some(delegation)))
            .await
            .expect("the stale failure must answer"),
        TaskFailureOutcome::StalePremise { current }
    );
    assert_eq!(
        progress(&store, created.task).await,
        TaskProgress::InProgress
    );

    // The same stale delegation premise alone is also refused.
    assert_eq!(
        store
            .fail_task(failure(current, Some(delegation)))
            .await
            .expect("the stale delegation must answer"),
        TaskFailureOutcome::StalePremise { current }
    );
}

#[tokio::test]
async fn a_foreign_delegation_is_a_fail_closed_technical_error() {
    let store = open_memory().await.unwrap();
    let (first, _, _) = seed_execution(&store).await;
    let (_, foreign_delegation, _) = seed_execution(&store).await;
    assert!(matches!(
        store
            .fail_task(failure(first, Some(foreign_delegation)))
            .await,
        Err(TaskTechnicalError::StorageUnavailable { .. })
    ));
    assert_eq!(progress(&store, first.task).await, TaskProgress::InProgress);
}

#[tokio::test]
async fn missing_identities_answer_their_own_variants() {
    let store = open_memory().await.unwrap();
    let missing = TaskRef {
        task: TaskId::generate(),
        revision: TaskRevision::initial(),
    };
    assert_eq!(
        store.fail_task(failure(missing, None)).await.unwrap(),
        TaskFailureOutcome::MissingTask { task: missing.task }
    );

    let (created, _, _) = seed_execution(&store).await;
    let absent_delegation = DelegationId::generate();
    assert_eq!(
        store
            .fail_task(failure(created, Some(absent_delegation)))
            .await
            .unwrap(),
        TaskFailureOutcome::MissingDelegation {
            delegation: absent_delegation
        }
    );
}

#[tokio::test]
async fn existing_admission_gates_refuse_a_failed_task() {
    let store = open_memory().await.unwrap();
    let (created, delegation, assoc) = seed_execution(&store).await;
    assert_eq!(
        store
            .fail_task(failure(created, Some(delegation)))
            .await
            .unwrap(),
        TaskFailureOutcome::FailedAs(created)
    );

    // AU3 delegation.
    let refused = store
        .create_delegation(delegation_premise(
            DelegationId::generate(),
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(Some(delegated_workspace(assoc, "/srv/workspace/ene", None))),
        ))
        .await
        .unwrap();
    assert_eq!(
        refused,
        DelegationOutcome::TaskTerminal {
            task: created.task,
            progress: TaskProgress::Failed,
        }
    );

    // AU4 steering.
    let refused = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: None,
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .unwrap();
    assert_eq!(
        refused,
        TaskCommitOutcome::TaskTerminal {
            task: created.task,
            progress: TaskProgress::Failed,
        }
    );

    // AU5 Action start.
    let refused = store
        .insert_attempt_if_current(AttemptCommitPremise {
            attempt: ene_action::ActionAttemptId::generate(),
            delegation: delegation.as_raw(),
            task: created.task.as_raw(),
            task_revision: RevisionInner::from_u64(created.revision.as_u64()),
            workspace: assoc.as_raw(),
            real_target: RealTargetRef::from_canonical_path(String::from("/srv/workspace/ene/x")),
            operation: OperationKind::Create,
            relied_evaluation: RawId::new(),
        })
        .await
        .unwrap();
    assert_eq!(refused, ActionStartOutcome::TaskTerminal);

    // Exactly one delegation and zero new attempts were written.
    assert_eq!(task_table_count(&store, "delegation"), 1);
    assert_eq!(task_table_count(&store, "action_attempt"), 0);
}
