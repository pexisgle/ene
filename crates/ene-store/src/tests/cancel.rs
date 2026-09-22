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
    AttemptCommitPremise, EffectGrounds, OperationKind, RealTargetRef,
};
use ene_inference::AttemptBeginOutcome;
use ene_task::{
    TaskAgentResultArrival, TaskCancelOutcome, TaskProgress, TaskResultAcceptance,
    TaskResultArrivalOutcome, TaskResultId,
};

use super::task_result::{
    claim, create_workspace_delegation, finalize, progress, raw_exec, seed_dialogue_consent,
    seed_workspace_execution, settle, start_attempt, task_agent_claim_for, workspace_task_premise,
};

async fn open_store() -> Store {
    open_memory().await.unwrap()
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

/// Reads one attempt's ordered `data_use` correlation, rendered as stored.
fn data_use_of(store: &Store, ticket: ene_inference::InferenceTicketId) -> Vec<String> {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut statement = guard
        .prepare("SELECT source FROM inference_attempt_data_use WHERE ticket = ?1 ORDER BY ordinal")
        .expect("the probe statement must prepare");
    let rows = statement
        .query_map([crate::codec::encode_id(ticket.0)], |row| {
            row.get::<_, String>(0)
        })
        .expect("the probe must run");
    rows.collect::<Result<Vec<_>, _>>()
        .expect("the probe rows must decode")
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
        assert_eq!(
            progress(&store, started.task).await,
            TaskProgress::Cancelled
        );

        let (premise, assoc) = workspace_task_premise();
        let in_progress = store.create_task(premise).await.unwrap();
        create_workspace_delegation(&store, in_progress, assoc, None).await;
        assert_eq!(
            store.cancel_task(in_progress.task).await.unwrap(),
            TaskCancelOutcome::CancelAccepted
        );
        assert_eq!(
            progress(&store, in_progress.task).await,
            TaskProgress::Cancelled,
            "the AU3 advance to in_progress is not an obstacle to cancel"
        );
        (started, in_progress)
    };

    let reopened = Store::open(&path).await.unwrap();
    for task in [started, in_progress] {
        assert_eq!(
            progress(&reopened, task.task).await,
            TaskProgress::Cancelled,
            "the admission is durable across restart"
        );
        assert_eq!(
            reopened.cancel_task(task.task).await.unwrap(),
            TaskCancelOutcome::AlreadyCancelled,
            "admission happens exactly once"
        );
        assert_eq!(
            progress(&reopened, task.task).await,
            TaskProgress::Cancelled
        );
    }
}

#[tokio::test]
async fn cancel_refuses_terminal_tasks_and_missing_identities_without_writes() {
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

    let revisions_before = table_count(&store, "task_revision").await;
    assert_eq!(
        store.cancel_task(task.task).await.unwrap(),
        TaskCancelOutcome::TaskTerminal {
            task: task.task,
            progress: TaskProgress::Completed,
        },
        "completion wins and is never retracted by a later cancel"
    );
    assert_eq!(progress(&store, task.task).await, TaskProgress::Completed);
    assert_eq!(table_count(&store, "task_revision").await, revisions_before);

    // The terminal value is written directly so the check is independent of
    // the failure producer (`Store::fail_task`); cancel must still refuse it.
    let failed = store.create_task(task_premise(None)).await.unwrap();
    raw_exec(
        &store,
        &format!(
            "UPDATE task SET progress = 'failed' WHERE task_id = '{}';",
            task_text(failed.task)
        ),
    );
    assert_eq!(
        store.cancel_task(failed.task).await.unwrap(),
        TaskCancelOutcome::TaskTerminal {
            task: failed.task,
            progress: TaskProgress::Failed,
        }
    );
    assert_eq!(progress(&store, failed.task).await, TaskProgress::Failed);

    let missing = TaskId::generate();
    assert_eq!(
        store.cancel_task(missing).await.unwrap(),
        TaskCancelOutcome::MissingTask { task: missing }
    );
}

/// Renders one Task identity into the raw text the fixture SQL compares.
fn task_text(task: TaskId) -> String {
    crate::codec::encode_id(task.as_raw())
}

// --- gates and races ---

#[tokio::test]
async fn cancel_refuses_new_delegation_steering_inference_and_action() {
    let store = open_store().await;
    seed_dialogue_consent(&store).await;
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "before.txt").await;
    assert_eq!(
        store.cancel_task(task.task).await.unwrap(),
        TaskCancelOutcome::CancelAccepted
    );

    // AU3: a new delegation is refused by the existing non-terminal gate.
    let later = DelegationId::generate();
    let refused = store
        .create_delegation(delegation_premise(
            later,
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
            progress: TaskProgress::Cancelled,
        }
    );

    // AU4: steering is refused and the revision does not move.
    let refused = store
        .forward_steering(TaskCommitPremise {
            expected: task,
            new_purpose: Some(TaskPurposeAdoptionPremise {
                purpose: TaskPurpose {
                    text: String::from("late direction"),
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
    assert_eq!(
        refused,
        TaskCommitOutcome::TaskTerminal {
            task: task.task,
            progress: TaskProgress::Cancelled,
        }
    );

    // AU14: the inference claim is refused with no attempt row.
    let claim_count_before = table_count(&store, "inference_attempt").await;
    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim_for(delegation, task))
            .await
            .unwrap(),
        AttemptBeginOutcome::TaskPremiseStale
    );
    assert_eq!(
        table_count(&store, "inference_attempt").await,
        claim_count_before
    );

    // AU5: the Action start is refused with no new attempt row.
    let attempts_before = table_count(&store, "action_attempt").await;
    let refused = store
        .insert_attempt_if_current(AttemptCommitPremise {
            attempt: ActionAttemptId::generate(),
            delegation: delegation.as_raw(),
            task: task.task.as_raw(),
            task_revision: RevisionInner::from_u64(task.revision.as_u64()),
            workspace: assoc.as_raw(),
            real_target: RealTargetRef::from_canonical_path(String::from(
                "/srv/workspace/ene/late.txt",
            )),
            operation: OperationKind::Create,
            relied_evaluation: RawId::new(),
        })
        .await
        .unwrap();
    assert_eq!(refused, ActionStartOutcome::TaskTerminal);
    assert_eq!(table_count(&store, "action_attempt").await, attempts_before);

    // The already-started attempt keeps its identity and certainty; cancel is
    // not a certainty update and never rewrites it.
    let record = store.load_attempt(attempt).await.unwrap().unwrap();
    assert_eq!(record.attempt, attempt);
    assert_eq!(record.certainty, ActionCertainty::Unknown);
}

#[tokio::test]
async fn steering_before_cancel_stands_and_cancel_is_never_stale() {
    let store = open_store().await;
    let (task, _delegation, _assoc) = seed_workspace_execution(&store).await;
    let advanced = store
        .forward_steering(TaskCommitPremise {
            expected: task,
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
    let TaskCommitOutcome::CommittedAs(advanced) = advanced else {
        panic!("the steering must win first, got {advanced:?}");
    };
    assert_eq!(
        store.cancel_task(task.task).await.unwrap(),
        TaskCancelOutcome::CancelAccepted,
        "cancel is Task-level and is not made stale by a concurrent steering"
    );
    let record = store.load_task(task.task).await.unwrap().unwrap();
    assert_eq!(
        record.task.reference, advanced,
        "the steering forward is not retracted by the later cancel"
    );
    assert_eq!(record.task.progress, TaskProgress::Cancelled);
}

#[tokio::test]
async fn cancel_leaves_started_inference_attempt_and_data_use_unchanged() {
    let store = open_store().await;
    seed_dialogue_consent(&store).await;
    let (task, delegation, _assoc) = seed_workspace_execution(&store).await;
    let claim = task_agent_claim_for(delegation, task);
    let ticket = claim.ticket;
    assert_eq!(
        store.begin_inference_attempt(claim).await.unwrap(),
        AttemptBeginOutcome::Started,
        "the attempt is claimed before the cancel arrives"
    );
    let attempt_rows_before = table_count(&store, "inference_attempt").await;
    let data_use_before = data_use_of(&store, ticket);
    assert!(
        !data_use_before.is_empty(),
        "the claimed attempt recorded its data_use correlation"
    );

    assert_eq!(
        store.cancel_task(task.task).await.unwrap(),
        TaskCancelOutcome::CancelAccepted
    );

    // V-12 §11: already-started inference facts survive cancel untouched.
    assert_eq!(
        table_count(&store, "inference_attempt").await,
        attempt_rows_before
    );
    assert_eq!(data_use_of(&store, ticket), data_use_before);
    let loaded = store
        .load_inference_attempt(ticket)
        .await
        .expect("the attempt stays readable")
        .expect("the attempt row is still there");
    assert_eq!(loaded.ticket, ticket);
    assert_eq!(
        loaded.task_agent.as_ref().map(|premise| premise.delegation),
        Some(delegation.as_raw())
    );
    assert_eq!(progress(&store, task.task).await, TaskProgress::Cancelled);
}

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
    assert_eq!(progress(&store, task.task).await, TaskProgress::Cancelled);
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
