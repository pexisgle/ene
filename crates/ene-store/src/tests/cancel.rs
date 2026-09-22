//! Cancel admission (AU16): the progress CAS, its terminal idempotence, the
//! existing admission gates it reuses, and the late-result contract.
//!
//! The durable checks here reproduce the PR #1545 contract: the cancel commit
//! is the only admission fact, it never touches already-started activity, a
//! cancelled Task is absorbing (no in-place resume), and a delayed final result
//! is still recorded and sealed but never adopted into the cancelled Task.

use super::*;

use ene_action::{
    ActionAttemptId, ActionAttemptRepository as _, ActionCertainty, ActionStartOutcome,
    AttemptCommitPremise, CertaintyUpdateOutcome, EffectGrounds, OperationKind, RealTargetRef,
};
use ene_task::{
    TaskAgentResultArrival, TaskCancelOutcome, TaskProgress, TaskResultAcceptance,
    TaskResultAdoptionClaim, TaskResultArrivalOutcome, TaskResultId,
};

async fn open_store() -> Store {
    open_memory().await.unwrap()
}

fn workspace_task_premise() -> (TaskCreationPremise, WorkspaceAssocId) {
    let assoc = WorkspaceAssocId::generate();
    (
        task_premise(Some(WorkspaceAssociationPremise {
            assoc,
            need: WorkspaceNeedRef {
                folder: WorkspaceFolderRef {
                    path: String::from("/srv/workspace/ene"),
                },
                save_target: None,
            },
        })),
        assoc,
    )
}

async fn create_workspace_delegation(
    store: &Store,
    task: TaskRef,
    assoc: WorkspaceAssocId,
) -> DelegationId {
    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            delegation,
            task,
            TaskAgentEphemeralId::generate(),
            delegation_scope(Some(delegated_workspace(assoc, "/srv/workspace/ene", None))),
        ))
        .await
        .expect("the AU3 delegation must answer");
    assert!(
        matches!(outcome, DelegationOutcome::Delegated(_)),
        "the delegation must commit, got {outcome:?}"
    );
    delegation
}

async fn seed_workspace_execution(store: &Store) -> (TaskRef, DelegationId, WorkspaceAssocId) {
    let (premise, assoc) = workspace_task_premise();
    let created = store
        .create_task(premise)
        .await
        .expect("the AU2 task must commit");
    let delegation = create_workspace_delegation(store, created, assoc).await;
    (created, delegation, assoc)
}

async fn start_attempt(
    store: &Store,
    delegation: DelegationId,
    task: TaskRef,
    assoc: WorkspaceAssocId,
    name: &str,
) -> ActionAttemptId {
    let attempt = ActionAttemptId::generate();
    let outcome = store
        .insert_attempt_if_current(AttemptCommitPremise {
            attempt,
            delegation: delegation.as_raw(),
            task: task.task.as_raw(),
            task_revision: RevisionInner::from_u64(task.revision.as_u64()),
            workspace: assoc.as_raw(),
            real_target: RealTargetRef::from_canonical_path(
                std::env::temp_dir()
                    .join(name)
                    .to_string_lossy()
                    .into_owned(),
            ),
            operation: OperationKind::Create,
            relied_evaluation: RawId::new(),
        })
        .await
        .expect("the AU5 insert must answer");
    assert_eq!(outcome, ActionStartOutcome::Started, "start {name}");
    attempt
}

async fn settle(
    store: &Store,
    attempt: ActionAttemptId,
    certainty: ActionCertainty,
    grounds: EffectGrounds,
) {
    let outcome = store
        .compare_and_set_certainty(attempt, ActionCertainty::Unknown, certainty, grounds)
        .await
        .expect("the certainty CAS must answer");
    assert_eq!(
        outcome,
        CertaintyUpdateOutcome::Updated,
        "settle {attempt:?}"
    );
}

fn claim(result: TaskResultId, attempts: &[ActionAttemptId]) -> TaskResultAdoptionClaim {
    TaskResultAdoptionClaim {
        result,
        attempt_refs: attempts.iter().map(|attempt| attempt.as_raw()).collect(),
    }
}

async fn progress_of(store: &Store, task: TaskRef) -> TaskProgress {
    store
        .load_task(task.task)
        .await
        .unwrap()
        .unwrap()
        .task
        .progress
}

async fn table_count(store: &Store, table: &str) -> i64 {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), (), |row| {
            row.get(0)
        })
        .expect("the row count must read")
}

// --- admission ---

#[tokio::test]
async fn cancel_accepts_started_and_in_progress_and_is_durable_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cancel-admission.db");
    let (started, in_progress) = {
        let store = Store::open(&path).await.unwrap();

        let started = store.create_task(task_premise(None)).await.unwrap();
        assert_eq!(
            store.cancel_task(started.task).await.unwrap(),
            TaskCancelOutcome::CancelAccepted
        );
        assert_eq!(progress_of(&store, started).await, TaskProgress::Cancelled);

        let (premise, assoc) = workspace_task_premise();
        let in_progress = store.create_task(premise).await.unwrap();
        create_workspace_delegation(&store, in_progress, assoc).await;
        assert_eq!(
            store.cancel_task(in_progress.task).await.unwrap(),
            TaskCancelOutcome::CancelAccepted
        );
        assert_eq!(
            progress_of(&store, in_progress).await,
            TaskProgress::Cancelled,
            "the AU3 advance to in_progress is not an obstacle to cancel"
        );
        (started, in_progress)
    };

    let reopened = Store::open(&path).await.unwrap();
    for task in [started, in_progress] {
        assert_eq!(
            progress_of(&reopened, task).await,
            TaskProgress::Cancelled,
            "the admission is durable across restart"
        );
        assert_eq!(
            reopened.cancel_task(task.task).await.unwrap(),
            TaskCancelOutcome::AlreadyCancelled,
            "admission happens exactly once"
        );
        assert_eq!(progress_of(&reopened, task).await, TaskProgress::Cancelled);
    }
}

// --- gates and races ---

#[tokio::test]
async fn cancelled_task_keeps_late_result_and_never_adopts_it() {
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "partial.txt").await;
    settle(
        &store,
        attempt,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    assert_eq!(
        store.cancel_task(task.task).await.unwrap(),
        TaskCancelOutcome::CancelAccepted
    );

    // AU15a still records and seals the delayed final result: cancel does not
    // swallow the body or the arrival identity.
    let arrival = TaskAgentResultArrival {
        delegation,
        result: TaskResultId::generate(),
        body: scrubbed_result(&store, "late final body").await,
    };
    let recorded = match store
        .record_task_result_arrival(arrival.clone())
        .await
        .expect("cancel does not block the arrival record")
    {
        TaskResultArrivalOutcome::Recorded(recorded) => recorded,
        TaskResultArrivalOutcome::StaleCredentialSet { .. } => {
            panic!("the fixture scrubbed at the current revision")
        }
    };
    assert_eq!(recorded.delegation, delegation);
    assert!(recorded.adopted_revision.is_none());

    // AU15b verifies the authoritative set and stamps the correlation, but the
    // terminal progress keeps the result against its original execution.
    let acceptance = store
        .adopt_result(claim(recorded.result, &[attempt]))
        .await
        .unwrap();
    assert_eq!(
        acceptance,
        TaskResultAcceptance::RecordedToOriginalOnly,
        "a late result never completes or revives the cancelled Task"
    );
    assert_eq!(progress_of(&store, task).await, TaskProgress::Cancelled);
    let loaded = store
        .load_task_result(recorded.result)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.attempt_refs, vec![attempt.as_raw()]);
    assert!(
        loaded.adopted_revision.is_none(),
        "no adopted revision is stamped into a cancelled Task"
    );
    assert_eq!(loaded.body.text(), "late final body");
    assert_eq!(table_count(&store, "task_result_attempt").await, 1);
}
