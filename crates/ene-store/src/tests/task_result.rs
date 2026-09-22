//! Task progress lifecycle, final result arrival/seal (AU15a), adoption
//! (AU15b), and the Task-wide completion barrier.
//!
//! The durable checks here reproduce the V-10 contract: progress is
//! orthogonal to revisions and terminal is absorbing, the result row's
//! existence is the execution seal, the authoritative Action attempt set is
//! enumerated from the delegation and compared to the claim exactly, and
//! `Completed` is only reachable through the barrier-checked CAS.

use super::*;

use ene_action::{
    ActionAttemptId, ActionAttemptRepository as _, ActionCertainty, ActionStartOutcome,
    AttemptCommitPremise, CertaintyUpdateOutcome, EffectGrounds, OperationKind, RealTargetRef,
};
use ene_task::{
    TaskAgentResultArrival, TaskProgress, TaskResultAcceptance, TaskResultAdoptionClaim,
    TaskResultId, TaskResultRecord,
};

fn target_path(name: &str) -> String {
    std::env::temp_dir()
        .join(name)
        .to_string_lossy()
        .into_owned()
}

async fn open_store() -> Store {
    open_memory().await.unwrap()
}

/// Seeds one Task with a confirmed workspace association and one delegation
/// whose copied scope relies on exactly that association.
pub(super) async fn seed_workspace_execution(
    store: &Store,
) -> (TaskRef, DelegationId, WorkspaceAssocId) {
    let workspace = task_workspace("/srv/workspace/ene", None);
    let assoc = workspace.assoc;
    let created = store
        .create_task(task_premise(Some(workspace)))
        .await
        .expect("the AU2 task must commit");
    let delegation = create_workspace_delegation(store, created, assoc).await;
    (created, delegation, assoc)
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

pub(super) fn attempt_premise(
    attempt: ActionAttemptId,
    delegation: DelegationId,
    task: TaskRef,
    assoc: WorkspaceAssocId,
    name: &str,
) -> AttemptCommitPremise {
    AttemptCommitPremise {
        attempt,
        delegation: delegation.as_raw(),
        task: task.task.as_raw(),
        task_revision: RevisionInner::from_u64(task.revision.as_u64()),
        workspace: assoc.as_raw(),
        real_target: RealTargetRef::from_canonical_path(target_path(name)),
        operation: OperationKind::Create,
        relied_evaluation: RawId::new(),
    }
}

/// Starts one Action attempt (AU5), requiring `Started`.
pub(super) async fn start_attempt(
    store: &Store,
    delegation: DelegationId,
    task: TaskRef,
    assoc: WorkspaceAssocId,
    name: &str,
) -> ActionAttemptId {
    let attempt = ActionAttemptId::generate();
    let outcome = store
        .insert_attempt_if_current(attempt_premise(attempt, delegation, task, assoc, name))
        .await
        .expect("the AU5 insert must answer");
    assert_eq!(outcome, ActionStartOutcome::Started, "start {name}");
    attempt
}

pub(super) async fn settle(
    store: &Store,
    attempt: ActionAttemptId,
    certainty: ActionCertainty,
    grounds: EffectGrounds,
) {
    let outcome = store
        .compare_and_set_certainty(attempt, ActionCertainty::Unknown, certainty, grounds)
        .await
        .expect("the certainty CAS must answer");
    assert_eq!(outcome, CertaintyUpdateOutcome::Updated);
}

/// Records one final result through the explicit finalization boundary.
pub(super) async fn finalize(
    store: &Store,
    delegation: DelegationId,
    body: &str,
) -> TaskResultRecord {
    record_result(store, delegation, body).await
}

pub(super) fn claim(result: TaskResultId, attempts: &[ActionAttemptId]) -> TaskResultAdoptionClaim {
    TaskResultAdoptionClaim {
        result,
        attempt_refs: attempts.iter().map(|attempt| attempt.as_raw()).collect(),
    }
}

pub(super) fn raw_exec(store: &Store, sql: &str) {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .execute_batch(sql)
        .expect("the raw test statement applies");
}

// --- lifecycle ---

#[tokio::test]
async fn fresh_task_starts_started_and_delegation_advances_to_in_progress() {
    let store = open_store().await;
    let created = store.create_task(task_premise(None)).await.unwrap();
    let loaded = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(loaded.task.progress, TaskProgress::Started);
    assert_eq!(loaded.task.adopted_result, None);

    let first = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            first,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(None),
        ))
        .await
        .unwrap();
    assert!(matches!(outcome, DelegationOutcome::Delegated(_)));
    let loaded = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(loaded.task.progress, TaskProgress::InProgress);
    assert_eq!(
        loaded.task.reference, created,
        "delegation creation never advances the revision"
    );

    let second = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            second,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(None),
        ))
        .await
        .unwrap();
    assert!(matches!(outcome, DelegationOutcome::Delegated(_)));
    let loaded = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(loaded.task.progress, TaskProgress::InProgress);
    assert_eq!(task_table_count(&store, "delegation"), 2);
}

#[tokio::test]
async fn completed_task_refuses_delegation_and_steering_without_writes() {
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "done.txt").await;
    settle(
        &store,
        attempt,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let result = finalize(&store, delegation, "final body").await;
    let adopted = store
        .adopt_result(claim(result.result, &[attempt]))
        .await
        .unwrap();
    assert!(matches!(
        adopted,
        TaskResultAcceptance::AdoptedAsCompletion(_)
    ));

    let completed = store.load_task(task.task).await.unwrap().unwrap();
    assert_eq!(completed.task.progress, TaskProgress::Completed);

    // A terminal Task refuses a new delegation and never advances or writes.
    let refused = store
        .create_delegation(delegation_premise(
            DelegationId::generate(),
            task,
            TaskAgentEphemeralId::generate(),
            delegation_scope(Some(delegated_workspace(assoc, "/srv/workspace/ene", None))),
        ))
        .await
        .unwrap();
    assert_eq!(
        refused,
        DelegationOutcome::TaskTerminal {
            task: task.task,
            progress: TaskProgress::Completed,
        }
    );
    assert_eq!(task_table_count(&store, "delegation"), 1);

    // A terminal Task refuses steering: no revision, no context entry.
    let revisions_before = task_table_count(&store, "task_revision");
    let entries_before = task_table_count(&store, "task_context_entry");
    let refused = store
        .forward_steering(TaskCommitPremise {
            expected: task,
            new_purpose: None,
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .unwrap();
    assert_eq!(
        refused,
        TaskCommitOutcome::TaskTerminal {
            task: task.task,
            progress: TaskProgress::Completed,
        }
    );
    assert_eq!(task_table_count(&store, "task_revision"), revisions_before);
    assert_eq!(
        task_table_count(&store, "task_context_entry"),
        entries_before
    );
    assert_eq!(
        store
            .load_task(task.task)
            .await
            .unwrap()
            .unwrap()
            .task
            .reference,
        task
    );
}

// --- AU15a arrival / seal ---

#[tokio::test]
async fn arrival_is_durable_before_adoption_and_reopen_preserves_the_body() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("arrival.db");
    let (task, delegation, assoc, result) = {
        let store = Store::open(&path).await.unwrap();
        let (task, delegation, assoc) = seed_workspace_execution(&store).await;
        let result = finalize(&store, delegation, "unadopted body").await;
        assert_eq!(result.attempt_refs, Vec::new());
        assert_eq!(result.adopted_revision, None);
        assert_eq!(
            store
                .load_task(task.task)
                .await
                .unwrap()
                .unwrap()
                .task
                .progress,
            TaskProgress::InProgress,
            "arrival never completes the Task"
        );
        assert_eq!(task_table_count(&store, "task_result"), 1);
        assert_eq!(task_table_count(&store, "task_result_attempt"), 0);
        (task, delegation, assoc, result)
    };

    let reopened = Store::open(&path).await.unwrap();
    let sealed = reopened
        .load_delegation_result(delegation)
        .await
        .unwrap()
        .expect("the committed result row is the seal");
    assert_eq!(sealed.body.text(), "unadopted body");
    assert_eq!(sealed.result, result.result);
    assert_eq!(sealed.task, task);
    assert_eq!(sealed.adopted_revision, None);
    assert_eq!(
        reopened
            .load_task_result(result.result)
            .await
            .unwrap()
            .unwrap(),
        sealed
    );
    assert_eq!(
        reopened
            .load_task(task.task)
            .await
            .unwrap()
            .unwrap()
            .task
            .progress,
        TaskProgress::InProgress,
        "reopen does not auto-adopt or auto-complete"
    );
    // The seal survives restart: a new Action under the execution is refused.
    let refused = reopened
        .insert_attempt_if_current(attempt_premise(
            ActionAttemptId::generate(),
            delegation,
            task,
            assoc,
            "after-restart.txt",
        ))
        .await
        .unwrap();
    assert_eq!(refused, ActionStartOutcome::ExecutionSealed);
}

#[tokio::test]
async fn result_retry_is_idempotent_and_identity_reuse_fails_closed() {
    let store = open_store().await;
    let (task, delegation, _assoc) = seed_workspace_execution(&store).await;
    let result = finalize(&store, delegation, "body one").await;

    // Exact retry: no second row, same durable identity.
    let retried = store
        .record_task_result_arrival(TaskAgentResultArrival {
            delegation,
            result: result.result,
            body: scrubbed_result(&store, "body one").await,
        })
        .await
        .unwrap();
    assert_eq!(retried, TaskResultArrivalOutcome::Recorded(result.clone()));
    assert_eq!(task_table_count(&store, "task_result"), 1);

    // Same identity with a different body is a technical error, never an
    // overwrite.
    let error = store
        .record_task_result_arrival(TaskAgentResultArrival {
            delegation,
            result: result.result,
            body: scrubbed_result(&store, "probe secret body").await,
        })
        .await
        .expect_err("reusing the identity with another body must fail closed");
    assert!(
        !error.to_string().contains("probe secret body"),
        "the technical error must not carry the body"
    );

    // A different final result for the same delegation is the second final
    // result that the durable invariant refuses.
    let second = store
        .record_task_result_arrival(TaskAgentResultArrival {
            delegation,
            result: TaskResultId::generate(),
            body: scrubbed_result(&store, "body two").await,
        })
        .await;
    assert!(
        matches!(second, Err(TaskTechnicalError::StorageUnavailable { .. })),
        "one delegation has at most one final result"
    );
    assert_eq!(task_table_count(&store, "task_result"), 1);

    // An arrival whose delegation has no correspondence fails closed.
    let orphan = store
        .record_task_result_arrival(TaskAgentResultArrival {
            delegation: DelegationId::generate(),
            result: TaskResultId::generate(),
            body: scrubbed_result(&store, "orphan").await,
        })
        .await;
    assert!(matches!(
        orphan,
        Err(TaskTechnicalError::StorageUnavailable { .. })
    ));
    assert_eq!(
        store
            .load_task(task.task)
            .await
            .unwrap()
            .unwrap()
            .task
            .progress,
        TaskProgress::InProgress
    );
}

// --- authoritative set and adoption outcomes ---

#[tokio::test]
async fn confirmed_success_attempts_adopt_as_completion() {
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let a1 = start_attempt(&store, delegation, task, assoc, "a1.txt").await;
    let a2 = start_attempt(&store, delegation, task, assoc, "a2.txt").await;
    for attempt in [a1, a2] {
        settle(
            &store,
            attempt,
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget,
        )
        .await;
    }
    let result = finalize(&store, delegation, "final body").await;
    let adopted = store
        .adopt_result(claim(result.result, &[a1, a2]))
        .await
        .unwrap();
    assert_eq!(adopted, TaskResultAcceptance::AdoptedAsCompletion(task));
    let loaded = store.load_task(task.task).await.unwrap().unwrap();
    assert_eq!(loaded.task.progress, TaskProgress::Completed);
    assert_eq!(loaded.task.adopted_result, Some(result.result));
    let stored = store
        .load_task_result(result.result)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.adopted_revision, Some(task.revision));
    let mut refs = stored.attempt_refs.clone();
    refs.sort_by_key(|attempt| attempt.as_uuid());
    let mut expected = vec![a1.as_raw(), a2.as_raw()];
    expected.sort_by_key(|attempt| attempt.as_uuid());
    assert_eq!(refs, expected);
    assert_eq!(task_table_count(&store, "task_result_attempt"), 2);
}

#[tokio::test]
async fn unknown_and_failure_attempts_withhold_completion() {
    for (certainty, grounds) in [
        (ActionCertainty::Unknown, EffectGrounds::OutcomeUnverified),
        (
            ActionCertainty::ConfirmedFailure,
            EffectGrounds::RefusedBeforeEffect,
        ),
    ] {
        let store = open_store().await;
        let (task, delegation, assoc) = seed_workspace_execution(&store).await;
        let attempt = start_attempt(&store, delegation, task, assoc, "blocked.txt").await;
        if certainty != ActionCertainty::Unknown {
            settle(&store, attempt, certainty, grounds).await;
        }
        let result = finalize(&store, delegation, "final body").await;
        let adoption = store
            .adopt_result(claim(result.result, &[attempt]))
            .await
            .unwrap();
        assert_eq!(
            adoption,
            TaskResultAcceptance::WithheldByEffectFacts {
                attempts: vec![attempt.as_raw()],
            },
            "a non-success relied attempt withholds completion ({certainty:?})"
        );
        assert_eq!(
            store
                .load_task(task.task)
                .await
                .unwrap()
                .unwrap()
                .task
                .progress,
            TaskProgress::InProgress
        );
        assert_eq!(
            store
                .load_task_result(result.result)
                .await
                .unwrap()
                .unwrap()
                .adopted_revision,
            None
        );
        // The result-local verified correlation is stamped even when withheld.
        let stamped = store
            .load_task_result(result.result)
            .await
            .unwrap()
            .unwrap()
            .attempt_refs;
        assert_eq!(stamped, vec![attempt.as_raw()]);
    }
}

// --- stale and idempotency ---

// --- Task-wide completion barrier ---

// --- load / restart invariants ---

// --- corruption fail-closed (review #5190132687) ---

// --- adopted current-unit bounded-read corruption (review #5190282818) ---

// --- adopted current-unit snapshot / purpose identity corruption (review #5190349125) ---

// --- debug / leakage ---
