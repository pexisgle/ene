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

use ene_task::{
    TaskFailureKind, TaskFailureOutcome, TaskFailurePremise, TaskProgress, TaskResultAcceptance,
    TaskResultAdoptionClaim,
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
async fn a_failure_completion_race_has_exactly_one_terminal_winner() {
    let store = open_memory().await.unwrap();
    let (task, delegation, _) = seed_execution(&store).await;

    let failure = store.fail_task(failure(task, Some(delegation)));
    let completion = async {
        let result = record_result(&store, delegation, "done").await;
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
