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
    TaskAgentOutput, TaskAgentResultArrival, TaskProgress, TaskResultAcceptance,
    TaskResultAdoptionClaim, TaskResultId, TaskResultRecord, orchestrate_result_arrival,
};

/// One Task Agent attempt premise for the sealed/terminal claim checks.
fn task_agent_claim_for(delegation: DelegationId, task: TaskRef) -> InferenceAttempt {
    InferenceAttempt {
        ticket: InferenceTicketId(RawId::new()),
        consumer: ConsumerKind::TaskAgent,
        capability: CapabilityKind::Dialogue,
        purpose: PurposeKind::TaskAgentTurn,
        expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
        expected_credential_set: CredentialSetRevision::initial(),
        provider: String::from("openai"),
        model: String::from("dialogue-1"),
        task_agent: Some(TaskAgentAttemptPremise {
            delegation: delegation.as_raw(),
            task: task.task.as_raw(),
            task_revision: RevisionInner::from_u64(task.revision.as_u64()),
        }),
    }
}

async fn seed_dialogue_consent(store: &Store) {
    let saved = save_consent(
        store,
        None,
        ConsentRecord {
            capability: CapabilityKind::Dialogue,
            id: String::from("consent-1"),
            rev: ConsentRevision::from_u64(1),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            credential_id: String::from("openai:main"),
        },
    )
    .await;
    assert!(matches!(saved, ConsentCommitOutcome::Committed { .. }));
}

fn target_path(name: &str) -> String {
    std::env::temp_dir()
        .join(name)
        .to_string_lossy()
        .into_owned()
}

async fn open_store() -> Store {
    open_memory().await.unwrap()
}

/// One Task creation premise with a fresh confirmed workspace association,
/// returned so a delegation and Action start can rely on exactly that
/// boundary. Each call mints a new association identity: one assoc belongs to
/// one Task.
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

/// Seeds one Task with a confirmed workspace association and one delegation
/// whose copied scope relies on exactly that association.
async fn seed_workspace_execution(store: &Store) -> (TaskRef, DelegationId, WorkspaceAssocId) {
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

fn attempt_premise(
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
async fn start_attempt(
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
    assert_eq!(outcome, CertaintyUpdateOutcome::Updated);
}

/// Records one final result through the explicit finalization boundary.
async fn finalize(store: &Store, delegation: DelegationId, body: &str) -> TaskResultRecord {
    orchestrate_result_arrival(store, delegation, TaskAgentOutput::new(body.to_owned()))
        .await
        .expect("finalization records the result before any adoption")
}

fn claim(result: TaskResultId, attempts: &[ActionAttemptId]) -> TaskResultAdoptionClaim {
    TaskResultAdoptionClaim {
        result,
        attempt_refs: attempts.iter().map(|attempt| attempt.as_raw()).collect(),
    }
}

fn raw_exec(store: &Store, sql: &str) {
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

#[tokio::test]
async fn inference_claim_refuses_seal_and_terminal_before_any_attempt_row() {
    let store = open_store().await;
    seed_dialogue_consent(&store).await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "one.txt").await;
    settle(
        &store,
        attempt,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    // The seal exists before adoption: a new claim under the sealed execution
    // is refused even while the Task stays `InProgress`.
    let result = finalize(&store, delegation, "final body").await;
    let sealed_claim = store
        .begin_inference_attempt(task_agent_claim_for(delegation, task))
        .await
        .unwrap();
    assert_eq!(sealed_claim, AttemptBeginOutcome::TaskPremiseStale);
    assert_eq!(task_table_count(&store, "inference_attempt"), 0);

    // A second delegation is created while the Task is still non-terminal;
    // after the completion CAS, its claim is refused by the terminal gate.
    let late = create_workspace_delegation(&store, task, assoc).await;
    let adopted = store
        .adopt_result(claim(result.result, &[attempt]))
        .await
        .unwrap();
    assert!(matches!(
        adopted,
        TaskResultAcceptance::AdoptedAsCompletion(_)
    ));
    let terminal_claim = store
        .begin_inference_attempt(task_agent_claim_for(late, task))
        .await
        .unwrap();
    assert_eq!(terminal_claim, AttemptBeginOutcome::TaskPremiseStale);
    assert_eq!(task_table_count(&store, "inference_attempt"), 0);
}

#[tokio::test]
async fn action_start_refuses_seal_and_terminal_without_an_attempt() {
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "first.txt").await;
    settle(
        &store,
        attempt,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let result = finalize(&store, delegation, "final body").await;
    // Seal before adoption: the sealed execution admits no new start.
    let sealed = store
        .insert_attempt_if_current(attempt_premise(
            ActionAttemptId::generate(),
            delegation,
            task,
            assoc,
            "sealed.txt",
        ))
        .await
        .unwrap();
    assert_eq!(sealed, ActionStartOutcome::ExecutionSealed);
    assert_eq!(task_table_count(&store, "action_attempt"), 1);

    let late = create_workspace_delegation(&store, task, assoc).await;
    let adopted = store
        .adopt_result(claim(result.result, &[attempt]))
        .await
        .unwrap();
    assert!(matches!(
        adopted,
        TaskResultAcceptance::AdoptedAsCompletion(_)
    ));
    let terminal = store
        .insert_attempt_if_current(attempt_premise(
            ActionAttemptId::generate(),
            late,
            task,
            assoc,
            "terminal.txt",
        ))
        .await
        .unwrap();
    assert_eq!(terminal, ActionStartOutcome::TaskTerminal);
    assert_eq!(task_table_count(&store, "action_attempt"), 1);
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
            body: TaskAgentOutput::new(String::from("body one")),
        })
        .await
        .unwrap();
    assert_eq!(retried, result);
    assert_eq!(task_table_count(&store, "task_result"), 1);

    // Same identity with a different body is a technical error, never an
    // overwrite.
    let error = store
        .record_task_result_arrival(TaskAgentResultArrival {
            delegation,
            result: result.result,
            body: TaskAgentOutput::new(String::from("probe secret body")),
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
            body: TaskAgentOutput::new(String::from("body two")),
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
            body: TaskAgentOutput::new(String::from("orphan")),
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

#[tokio::test]
async fn claim_must_match_the_authoritative_set_exactly() {
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let a1 = start_attempt(&store, delegation, task, assoc, "a1.txt").await;
    settle(
        &store,
        a1,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let result = finalize(&store, delegation, "final body").await;

    for (label, refs) in [
        ("missing", Vec::new()),
        ("extra", vec![a1.as_raw(), RawId::new()]),
        ("duplicate", vec![a1.as_raw(), a1.as_raw()]),
    ] {
        let adoption = store
            .adopt_result(TaskResultAdoptionClaim {
                result: result.result,
                attempt_refs: refs,
            })
            .await;
        assert!(
            matches!(adoption, Err(TaskTechnicalError::StorageUnavailable { .. })),
            "a {label} claim is an inconsistent unit, got {adoption:?}"
        );
    }
    // The failed claims wrote nothing.
    assert_eq!(task_table_count(&store, "task_result_attempt"), 0);
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
    // The exact claim still works.
    let adopted = store
        .adopt_result(claim(result.result, &[a1]))
        .await
        .unwrap();
    assert!(matches!(
        adopted,
        TaskResultAcceptance::AdoptedAsCompletion(_)
    ));
}

#[tokio::test]
async fn omitting_an_unknown_attempt_is_a_technical_error_not_withheld() {
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let a1 = start_attempt(&store, delegation, task, assoc, "a1.txt").await;
    let result = finalize(&store, delegation, "final body").await;

    let omission = store.adopt_result(claim(result.result, &[])).await;
    assert!(
        matches!(omission, Err(TaskTechnicalError::StorageUnavailable { .. })),
        "a claim that drops a durable Unknown is inconsistent, not withheld: {omission:?}"
    );
    assert_eq!(task_table_count(&store, "task_result_attempt"), 0);

    let honest = store
        .adopt_result(claim(result.result, &[a1]))
        .await
        .unwrap();
    assert_eq!(
        honest,
        TaskResultAcceptance::WithheldByEffectFacts {
            attempts: vec![a1.as_raw()],
        }
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
}

#[tokio::test]
async fn a_claim_cannot_borrow_another_delegations_attempt() {
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let a1 = start_attempt(&store, delegation, task, assoc, "a1.txt").await;
    settle(
        &store,
        a1,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let other = create_workspace_delegation(&store, task, assoc).await;
    let foreign = start_attempt(&store, other, task, assoc, "foreign.txt").await;
    settle(
        &store,
        foreign,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let result = finalize(&store, delegation, "final body").await;
    let adoption = store
        .adopt_result(claim(result.result, &[a1, foreign]))
        .await;
    assert!(
        matches!(adoption, Err(TaskTechnicalError::StorageUnavailable { .. })),
        "the claim is pinned to the sealed execution lifetime"
    );
    let adopted = store
        .adopt_result(claim(result.result, &[a1]))
        .await
        .unwrap();
    assert!(matches!(
        adopted,
        TaskResultAcceptance::AdoptedAsCompletion(_)
    ));
}

#[tokio::test]
async fn a_durable_no_action_execution_adopts_with_an_empty_claim() {
    let store = open_store().await;
    let created = store.create_task(task_premise(None)).await.unwrap();
    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            delegation,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(None),
        ))
        .await
        .unwrap();
    assert!(matches!(outcome, DelegationOutcome::Delegated(_)));
    let result = finalize(&store, delegation, "no action body").await;
    let adopted = store.adopt_result(claim(result.result, &[])).await.unwrap();
    assert_eq!(adopted, TaskResultAcceptance::AdoptedAsCompletion(created));
    assert_eq!(
        store
            .load_task(created.task)
            .await
            .unwrap()
            .unwrap()
            .task
            .progress,
        TaskProgress::Completed
    );
    assert_eq!(task_table_count(&store, "task_result_attempt"), 0);
}

// --- stale and idempotency ---

#[tokio::test]
async fn a_result_after_steering_is_recorded_to_the_original_revision_only() {
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "a1.txt").await;
    settle(
        &store,
        attempt,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let advanced = store
        .forward_steering(TaskCommitPremise {
            expected: task,
            new_purpose: None,
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .unwrap();
    let TaskCommitOutcome::CommittedAs(second) = advanced else {
        panic!("the steering must commit, got {advanced:?}");
    };
    let result = finalize(&store, delegation, "stale body").await;
    let adoption = store
        .adopt_result(claim(result.result, &[attempt]))
        .await
        .unwrap();
    assert_eq!(adoption, TaskResultAcceptance::RecordedToOriginalOnly);
    let loaded = store.load_task(task.task).await.unwrap().unwrap();
    assert_eq!(loaded.task.reference, second);
    assert_eq!(loaded.task.progress, TaskProgress::InProgress);
    assert_eq!(loaded.task.adopted_result, None);
    assert_eq!(
        store
            .load_task_result(result.result)
            .await
            .unwrap()
            .unwrap()
            .adopted_revision,
        None
    );
    assert_eq!(
        store
            .load_task_result(result.result)
            .await
            .unwrap()
            .unwrap()
            .attempt_refs,
        vec![attempt.as_raw()],
        "the result-local correlation is still recorded against the original revision"
    );
}

#[tokio::test]
async fn same_result_retry_after_completion_does_not_transition_twice() {
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "a1.txt").await;
    settle(
        &store,
        attempt,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let result = finalize(&store, delegation, "final body").await;
    let first = store
        .adopt_result(claim(result.result, &[attempt]))
        .await
        .unwrap();
    assert_eq!(first, TaskResultAcceptance::AdoptedAsCompletion(task));
    let completed_at = store
        .load_task(task.task)
        .await
        .unwrap()
        .unwrap()
        .task
        .progress;

    let retry = store
        .adopt_result(claim(result.result, &[attempt]))
        .await
        .unwrap();
    assert_eq!(retry, TaskResultAcceptance::AdoptedAsCompletion(task));
    assert_eq!(task_table_count(&store, "task_result_attempt"), 1);
    assert_eq!(task_table_count(&store, "task_revision"), 1);
    assert_eq!(
        store
            .load_task(task.task)
            .await
            .unwrap()
            .unwrap()
            .task
            .progress,
        completed_at
    );
}

#[tokio::test]
async fn another_result_after_terminal_is_recorded_to_the_original_only() {
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let a1 = start_attempt(&store, delegation, task, assoc, "a1.txt").await;
    settle(
        &store,
        a1,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let winner = finalize(&store, delegation, "winner body").await;
    // The second execution is created before the completion CAS.
    let late = create_workspace_delegation(&store, task, assoc).await;
    let a2 = start_attempt(&store, late, task, assoc, "a2.txt").await;
    settle(
        &store,
        a2,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let adopted = store
        .adopt_result(claim(winner.result, &[a1]))
        .await
        .unwrap();
    assert!(matches!(
        adopted,
        TaskResultAcceptance::AdoptedAsCompletion(_)
    ));

    let loser = finalize(&store, late, "loser body").await;
    let recorded = store
        .adopt_result(claim(loser.result, &[a2]))
        .await
        .unwrap();
    assert_eq!(recorded, TaskResultAcceptance::RecordedToOriginalOnly);
    let loaded = store.load_task(task.task).await.unwrap().unwrap();
    assert_eq!(loaded.task.progress, TaskProgress::Completed);
    assert_eq!(
        loaded.task.adopted_result,
        Some(winner.result),
        "the first completion stays the adopted result"
    );
    assert_eq!(
        store
            .load_task_result(loser.result)
            .await
            .unwrap()
            .unwrap()
            .attempt_refs,
        vec![a2.as_raw()],
        "the losing result still records its own verified correlation"
    );
}

// --- Task-wide completion barrier ---

#[tokio::test]
async fn task_wide_barrier_sees_another_delegations_unknown_and_clears_on_settlement() {
    let store = open_store().await;
    let (task, d1, assoc) = seed_workspace_execution(&store).await;
    let a1 = start_attempt(&store, d1, task, assoc, "a1.txt").await;
    let x = finalize(&store, d1, "x body").await;
    let withheld = store.adopt_result(claim(x.result, &[a1])).await.unwrap();
    assert_eq!(
        withheld,
        TaskResultAcceptance::WithheldByEffectFacts {
            attempts: vec![a1.as_raw()],
        }
    );

    let d2 = create_workspace_delegation(&store, task, assoc).await;
    let a2 = start_attempt(&store, d2, task, assoc, "a2.txt").await;
    settle(
        &store,
        a1,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let withheld = store.adopt_result(claim(x.result, &[a1])).await.unwrap();
    assert_eq!(
        withheld,
        TaskResultAcceptance::WithheldByEffectFacts {
            attempts: vec![a2.as_raw()],
        },
        "the other execution's unresolved Unknown blocks completion"
    );
    // Barrier attempts are never stamped as result-local dependencies.
    assert_eq!(
        store
            .load_task_result(x.result)
            .await
            .unwrap()
            .unwrap()
            .attempt_refs,
        vec![a1.as_raw()]
    );

    settle(
        &store,
        a2,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let adopted = store.adopt_result(claim(x.result, &[a1])).await.unwrap();
    assert_eq!(adopted, TaskResultAcceptance::AdoptedAsCompletion(task));
}

#[tokio::test]
async fn cross_delegation_settled_facts_do_not_block_by_themselves() {
    let store = open_store().await;
    let (task, d1, assoc) = seed_workspace_execution(&store).await;
    let a1 = start_attempt(&store, d1, task, assoc, "a1.txt").await;
    settle(
        &store,
        a1,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let d2 = create_workspace_delegation(&store, task, assoc).await;
    let a2 = start_attempt(&store, d2, task, assoc, "a2.txt").await;
    settle(
        &store,
        a2,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let x = finalize(&store, d1, "x body").await;
    let adopted = store.adopt_result(claim(x.result, &[a1])).await.unwrap();
    assert!(
        matches!(adopted, TaskResultAcceptance::AdoptedAsCompletion(_)),
        "a settled cross-delegation success is not a blocker"
    );

    // A cross-delegation ConfirmedFailure is likewise not a blocker.
    let store = open_store().await;
    let (task, d1, assoc) = seed_workspace_execution(&store).await;
    let a1 = start_attempt(&store, d1, task, assoc, "a1.txt").await;
    settle(
        &store,
        a1,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let d2 = create_workspace_delegation(&store, task, assoc).await;
    let a2 = start_attempt(&store, d2, task, assoc, "a2.txt").await;
    settle(
        &store,
        a2,
        ActionCertainty::ConfirmedFailure,
        EffectGrounds::RefusedBeforeEffect,
    )
    .await;
    let x = finalize(&store, d1, "x body").await;
    let adopted = store.adopt_result(claim(x.result, &[a1])).await.unwrap();
    assert!(
        matches!(adopted, TaskResultAcceptance::AdoptedAsCompletion(_)),
        "a cross-delegation ConfirmedFailure is not a blocker"
    );

    // A result-local ConfirmedFailure still blocks.
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let local = start_attempt(&store, delegation, task, assoc, "local.txt").await;
    settle(
        &store,
        local,
        ActionCertainty::ConfirmedFailure,
        EffectGrounds::RefusedBeforeEffect,
    )
    .await;
    let x = finalize(&store, delegation, "x body").await;
    let withheld = store.adopt_result(claim(x.result, &[local])).await.unwrap();
    assert_eq!(
        withheld,
        TaskResultAcceptance::WithheldByEffectFacts {
            attempts: vec![local.as_raw()],
        }
    );
}

#[tokio::test]
async fn an_unsealed_delegation_without_started_actions_is_not_a_barrier() {
    let store = open_store().await;
    let (task, d1, assoc) = seed_workspace_execution(&store).await;
    let a1 = start_attempt(&store, d1, task, assoc, "a1.txt").await;
    settle(
        &store,
        a1,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    // D2 has no final result and no started Action: neither is a barrier.
    let _d2 = create_workspace_delegation(&store, task, assoc).await;
    let x = finalize(&store, d1, "x body").await;
    let adopted = store.adopt_result(claim(x.result, &[a1])).await.unwrap();
    assert!(matches!(
        adopted,
        TaskResultAcceptance::AdoptedAsCompletion(_)
    ));
}

#[tokio::test]
async fn old_revision_unknown_blocks_until_settled() {
    let store = open_store().await;
    let (task, d1, assoc) = seed_workspace_execution(&store).await;
    let old = start_attempt(&store, d1, task, assoc, "old.txt").await;
    let advanced = store
        .forward_steering(TaskCommitPremise {
            expected: task,
            new_purpose: Some(ene_task::TaskPurposeAdoptionPremise {
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
    let TaskCommitOutcome::CommittedAs(second) = advanced else {
        panic!("steering must commit, got {advanced:?}");
    };
    let d2 = create_workspace_delegation(&store, second, assoc).await;
    let fresh = start_attempt(&store, d2, second, assoc, "fresh.txt").await;
    settle(
        &store,
        fresh,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let x = finalize(&store, d2, "x body").await;
    let withheld = store.adopt_result(claim(x.result, &[fresh])).await.unwrap();
    assert_eq!(
        withheld,
        TaskResultAcceptance::WithheldByEffectFacts {
            attempts: vec![old.as_raw()],
        },
        "a pre-steering Unknown still blocks the current revision's completion"
    );
    settle(
        &store,
        old,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let adopted = store.adopt_result(claim(x.result, &[fresh])).await.unwrap();
    assert_eq!(adopted, TaskResultAcceptance::AdoptedAsCompletion(second));
}

#[tokio::test]
async fn au5_and_adoption_serialize_into_exactly_two_orderings() {
    for _ in 0..10 {
        let store = open_store().await;
        let (task, d1, assoc) = seed_workspace_execution(&store).await;
        let a1 = start_attempt(&store, d1, task, assoc, "a1.txt").await;
        settle(
            &store,
            a1,
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget,
        )
        .await;
        let x = finalize(&store, d1, "x body").await;
        let d2 = create_workspace_delegation(&store, task, assoc).await;
        let a2 = ActionAttemptId::generate();
        let premise = attempt_premise(a2, d2, task, assoc, "a2.txt");

        let (adoption, start) = tokio::join!(
            store.adopt_result(claim(x.result, &[a1])),
            store.insert_attempt_if_current(premise),
        );
        let adoption = adoption.expect("adoption answers a domain outcome");
        let start = start.expect("the start answers a domain outcome");
        match (start, adoption) {
            (
                ActionStartOutcome::Started,
                TaskResultAcceptance::WithheldByEffectFacts { attempts },
            ) => assert_eq!(
                attempts,
                vec![a2.as_raw()],
                "AU5 first: the barrier sees the new Unknown"
            ),
            (ActionStartOutcome::TaskTerminal, TaskResultAcceptance::AdoptedAsCompletion(_)) => {}
            other => {
                panic!("no third ordering may leave a start invisible to the barrier: {other:?}")
            }
        }
        let started = matches!(start, ActionStartOutcome::Started);
        assert_eq!(
            task_table_count(&store, "action_attempt"),
            if started { 2 } else { 1 },
            "a terminal refusal writes no attempt row"
        );
        assert_eq!(
            store
                .load_task(task.task)
                .await
                .unwrap()
                .unwrap()
                .task
                .progress
                == TaskProgress::Completed,
            !started
        );
    }
}

#[tokio::test]
async fn certainty_settlement_before_or_after_adoption_both_reach_completion() {
    // Settlement first: the barrier is already clear.
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "a1.txt").await;
    settle(
        &store,
        attempt,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let x = finalize(&store, delegation, "x body").await;
    let adopted = store
        .adopt_result(claim(x.result, &[attempt]))
        .await
        .unwrap();
    assert_eq!(adopted, TaskResultAcceptance::AdoptedAsCompletion(task));

    // Withheld first: the same result is re-evaluated after settlement.
    let store = open_store().await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "a1.txt").await;
    let x = finalize(&store, delegation, "x body").await;
    let withheld = store
        .adopt_result(claim(x.result, &[attempt]))
        .await
        .unwrap();
    assert!(matches!(
        withheld,
        TaskResultAcceptance::WithheldByEffectFacts { .. }
    ));
    settle(
        &store,
        attempt,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let adopted = store
        .adopt_result(claim(x.result, &[attempt]))
        .await
        .unwrap();
    assert_eq!(adopted, TaskResultAcceptance::AdoptedAsCompletion(task));
}

// --- load / restart invariants ---

#[tokio::test]
async fn completed_task_reopens_with_its_adopted_result_and_no_unknown_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("completed.db");
    let (task, result) = {
        let store = Store::open(&path).await.unwrap();
        let (task, delegation, assoc) = seed_workspace_execution(&store).await;
        let attempt = start_attempt(&store, delegation, task, assoc, "a1.txt").await;
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
        (task, result)
    };
    let reopened = Store::open(&path).await.unwrap();
    let loaded = reopened.load_task(task.task).await.unwrap().unwrap();
    assert_eq!(loaded.task.progress, TaskProgress::Completed);
    assert_eq!(loaded.task.adopted_result, Some(result.result));
    // The completion invariant is recoverable from durable facts alone.
    let guard = match reopened.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let unknown: i64 = guard
        .query_row(
            "SELECT COUNT(*) FROM action_attempt WHERE task_id = ?1 AND certainty = 'unknown'",
            params![crate::codec::encode_id(task.task.as_raw())],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unknown, 0);
}

#[tokio::test]
async fn load_fails_closed_on_corrupt_progress_or_adoption_state() {
    // An unknown stored progress name is unreadable.
    let store = open_store().await;
    let created = store.create_task(task_premise(None)).await.unwrap();
    raw_exec(&store, "UPDATE task SET progress = 'paused'");
    let loaded = store.load_task(created.task).await;
    assert!(matches!(
        loaded,
        Err(TaskTechnicalError::StorageUnavailable { .. })
    ));

    // Completed without an adopted result contradicts the producer.
    let store = open_store().await;
    let created = store.create_task(task_premise(None)).await.unwrap();
    raw_exec(&store, "UPDATE task SET progress = 'completed'");
    let loaded = store.load_task(created.task).await;
    assert!(matches!(
        loaded,
        Err(TaskTechnicalError::StorageUnavailable { .. })
    ));
}

// --- V20 migration ---

/// Rewinds a V20 database to the V19 shape: the added column, the two result
/// tables, and the query-support indexes are removed, while every pre-existing
/// Task / delegation / attempt row stays.
fn rewind_to_v19(path: &std::path::Path) {
    let conn = rusqlite::Connection::open(path).expect("the rewind opens");
    conn.execute_batch(
        "DROP TABLE task_result_attempt;
         DROP TABLE task_result;
         DROP INDEX IF EXISTS idx_action_attempt_task;
         DROP INDEX IF EXISTS idx_action_attempt_delegation;
         ALTER TABLE task DROP COLUMN progress;
         PRAGMA user_version = 19;",
    )
    .expect("the V19 rewind applies");
}

#[tokio::test]
async fn v20_backfill_maps_delegation_existence_and_never_fabricates_terminals() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v20-backfill.db");
    let seed = {
        let store = Store::open(&path).await.unwrap();
        assert_eq!(read_schema_version(&path), Some(20));

        // (A) Task only: fresh creation starts `started`.
        let task_only = store.create_task(task_premise(None)).await.unwrap();
        assert_eq!(progress_of(&store, task_only).await, TaskProgress::Started);

        // (B) One delegation moves `started -> in_progress` in the AU3 commit.
        let (one_premise, one_assoc) = workspace_task_premise();
        let one = store.create_task(one_premise).await.unwrap();
        let d1 = create_workspace_delegation(&store, one, one_assoc).await;
        let one_delegation = store.load_delegation(d1).await.unwrap().unwrap();
        assert_eq!(progress_of(&store, one).await, TaskProgress::InProgress);

        // (C) Multiple delegations of the same revision are retained.
        let (two_premise, two_assoc) = workspace_task_premise();
        let two = store.create_task(two_premise).await.unwrap();
        let d2a = create_workspace_delegation(&store, two, two_assoc).await;
        let d2b = create_workspace_delegation(&store, two, two_assoc).await;
        let two_delegations = vec![
            store.load_delegation(d2a).await.unwrap().unwrap(),
            store.load_delegation(d2b).await.unwrap().unwrap(),
        ];
        assert_eq!(task_table_count(&store, "delegation"), 3);

        // (D) A ConfirmedSuccess Action is not Task completion.
        let (success_premise, success_assoc) = workspace_task_premise();
        let success = store.create_task(success_premise).await.unwrap();
        let d3 = create_workspace_delegation(&store, success, success_assoc).await;
        let attempt_success =
            start_attempt(&store, d3, success, success_assoc, "success.txt").await;
        settle(
            &store,
            attempt_success,
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget,
        )
        .await;

        // (E) A ConfirmedFailure Action is not Task failure.
        let (failure_premise, failure_assoc) = workspace_task_premise();
        let failure = store.create_task(failure_premise).await.unwrap();
        let d4 = create_workspace_delegation(&store, failure, failure_assoc).await;
        let attempt_failure =
            start_attempt(&store, d4, failure, failure_assoc, "failure.txt").await;
        settle(
            &store,
            attempt_failure,
            ActionCertainty::ConfirmedFailure,
            EffectGrounds::RefusedBeforeEffect,
        )
        .await;

        BackfillSeed {
            task_only,
            one,
            one_delegation,
            two,
            two_delegations,
            success,
            failure,
            attempt_success,
            attempt_failure,
        }
    };

    rewind_to_v19(&path);
    assert_eq!(read_schema_version(&path), Some(19));
    assert!(
        !table_columns(&path, "task").contains(&String::from("progress")),
        "the rewind removes the V20 column"
    );

    let reopened = Store::open(&path).await.expect("the V20 migration applies");
    assert_eq!(read_schema_version(&path), Some(20));
    assert_eq!(
        table_columns(&path, "task_result"),
        vec![
            "result_id",
            "task_id",
            "task_revision",
            "delegation_id",
            "body",
            "adopted_revision",
            "recorded_at",
        ]
    );
    assert_eq!(
        table_columns(&path, "task_result_attempt"),
        vec!["result_id", "attempt_id"]
    );

    // (A) Task only -> started.
    assert_eq!(
        progress_of(&reopened, seed.task_only).await,
        TaskProgress::Started
    );
    // (B) one delegation -> in_progress.
    assert_eq!(
        progress_of(&reopened, seed.one).await,
        TaskProgress::InProgress
    );
    // (C) multiple delegations -> in_progress, both retained.
    assert_eq!(
        progress_of(&reopened, seed.two).await,
        TaskProgress::InProgress
    );
    // (D) a ConfirmedSuccess Action is not Task completion.
    assert_eq!(
        progress_of(&reopened, seed.success).await,
        TaskProgress::InProgress
    );
    // (E) failure facts are not Task failure.
    assert_eq!(
        progress_of(&reopened, seed.failure).await,
        TaskProgress::InProgress
    );

    // Existing correspondence is preserved, never reissued or shrunk.
    assert_eq!(
        reopened
            .load_delegation(seed.one_delegation.delegation)
            .await
            .unwrap(),
        Some(seed.one_delegation.clone())
    );
    for delegation in &seed.two_delegations {
        assert_eq!(
            reopened
                .load_delegation(delegation.delegation)
                .await
                .unwrap(),
            Some(delegation.clone())
        );
    }
    assert_eq!(
        reopened
            .load_task(seed.one.task)
            .await
            .unwrap()
            .unwrap()
            .task
            .reference,
        seed.one
    );
    assert_eq!(
        reopened
            .load_task(seed.two.task)
            .await
            .unwrap()
            .unwrap()
            .task
            .reference,
        seed.two
    );
    assert_eq!(
        reopened
            .load_attempt(seed.attempt_success)
            .await
            .unwrap()
            .unwrap()
            .certainty,
        ActionCertainty::ConfirmedSuccess
    );
    assert_eq!(
        reopened
            .load_attempt(seed.attempt_failure)
            .await
            .unwrap()
            .unwrap()
            .certainty,
        ActionCertainty::ConfirmedFailure
    );
    assert_eq!(task_table_count(&reopened, "task"), 5);
    assert_eq!(task_table_count(&reopened, "delegation"), 5);
    assert_eq!(task_table_count(&reopened, "task_result"), 0);
    assert_eq!(task_table_count(&reopened, "task_result_attempt"), 0);
}

struct BackfillSeed {
    task_only: TaskRef,
    one: TaskRef,
    one_delegation: DelegationRef,
    two: TaskRef,
    two_delegations: Vec<DelegationRef>,
    success: TaskRef,
    failure: TaskRef,
    attempt_success: ActionAttemptId,
    attempt_failure: ActionAttemptId,
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

#[tokio::test]
async fn v20_version_rewind_does_not_overwrite_existing_progress() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v20-rewind.db");
    let task = {
        let store = Store::open(&path).await.unwrap();
        store.create_task(task_premise(None)).await.unwrap()
    };
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("UPDATE task SET progress = 'failed'; PRAGMA user_version = 19;")
            .unwrap();
    }
    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(read_schema_version(&path), Some(20));
    assert_eq!(
        reopened
            .load_task(task.task)
            .await
            .unwrap()
            .unwrap()
            .task
            .progress,
        TaskProgress::Failed,
        "a re-run backfills only NULL rows"
    );
}

// --- debug / leakage ---

#[test]
fn result_debug_renderings_redact_the_body() {
    let probe = "probe result body text";
    let arrival = TaskAgentResultArrival {
        delegation: DelegationId::generate(),
        result: TaskResultId::generate(),
        body: TaskAgentOutput::new(probe.to_owned()),
    };
    assert!(
        !format!("{arrival:?}").contains(probe),
        "the arrival Debug must redact the body"
    );
    let record = TaskResultRecord {
        result: TaskResultId::generate(),
        task: TaskRef {
            task: TaskId::generate(),
            revision: TaskRevision::initial(),
        },
        delegation: DelegationId::generate(),
        body: TaskAgentOutput::new(probe.to_owned()),
        attempt_refs: Vec::new(),
        adopted_revision: None,
        recorded_at: fixture_clock(),
    };
    assert!(
        !format!("{record:?}").contains(probe),
        "the result record Debug must redact the body"
    );
}
