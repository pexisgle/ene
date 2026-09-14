//! Sealed-but-unadopted result re-evaluation: the bounded candidate read, the
//! re-derived adoption claim, and the re-run of the existing adoption gate
//! after certainty settlement or a stop between AU15a and AU15b.
//!
//! The durable checks reproduce the slice F contract: the re-evaluation never
//! re-executes provider or Action work, never invents a second adoption
//! master, and keeps the `adopt_result` gate as the only completion decision.

use super::*;

use ene_action::{
    ActionAttemptRepository as _, ActionCertainty, ActionStartOutcome, EffectGrounds,
};
use ene_task::{
    TaskProgress, TaskResultAcceptance, TaskResultAdoptionClaim, reevaluate_result_adoption,
};

use super::task_result::{
    claim, finalize, raw_exec, seed_workspace_execution, settle, start_attempt,
};

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
async fn a_withheld_result_completes_once_every_blocker_settles() {
    let store = open_memory().await.unwrap();
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let first = start_attempt(&store, delegation, task, assoc, "first.txt").await;
    let second = start_attempt(&store, delegation, task, assoc, "second.txt").await;
    let result = finalize(&store, delegation, "done").await;

    let withheld = store
        .adopt_result(claim(result.result, &[first, second]))
        .await
        .unwrap();
    let TaskResultAcceptance::WithheldByEffectFacts { attempts } = withheld else {
        panic!("two unknown attempts must withhold, got {withheld:?}");
    };
    assert_eq!(attempts.len(), 2);

    // One blocker settles: the re-evaluation re-reads current facts and stays
    // withheld with only the remaining blocker, without a busy loop or a
    // second adoption state.
    settle(
        &store,
        first,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let still = reevaluate_result_adoption(&store, result.result)
        .await
        .unwrap();
    assert_eq!(
        still,
        TaskResultAcceptance::WithheldByEffectFacts {
            attempts: vec![second.as_raw()]
        }
    );
    assert_eq!(progress(&store, task.task).await, TaskProgress::InProgress);

    // The last blocker settles: the same sealed result completes.
    settle(
        &store,
        second,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    assert_eq!(
        reevaluate_result_adoption(&store, result.result)
            .await
            .unwrap(),
        TaskResultAcceptance::AdoptedAsCompletion(task)
    );
    assert_eq!(progress(&store, task.task).await, TaskProgress::Completed);
}

#[tokio::test]
async fn a_result_sealed_before_a_stop_is_recovered_after_reopen() {
    let directory = tempfile::tempdir().expect("store directory");
    let path = directory.path().join("reevaluation.db");
    let (task, delegation, _assoc, result) = {
        let store = Store::open(&path).await.expect("the fresh store must open");
        let (task, delegation, assoc) = seed_workspace_execution(&store).await;
        let attempt = start_attempt(&store, delegation, task, assoc, "input.txt").await;
        settle(
            &store,
            attempt,
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget,
        )
        .await;
        // AU15a commits; the stop happens before AU15b ever runs.
        let result = finalize(&store, delegation, "recovered report").await;
        assert!(result.adopted_revision.is_none());
        (task, delegation, assoc, result)
    };

    let reopened = Store::open(&path).await.expect("the reopen must succeed");
    let candidates = reopened.list_unadopted_results(16).await.unwrap();
    assert_eq!(
        candidates,
        vec![result.result],
        "the sealed, unadopted result is the bounded recovery candidate"
    );
    assert_eq!(
        reevaluate_result_adoption(&reopened, result.result)
            .await
            .unwrap(),
        TaskResultAcceptance::AdoptedAsCompletion(task)
    );
    assert_eq!(
        progress(&reopened, task.task).await,
        TaskProgress::Completed
    );
    // Recovery never re-executes the execution: the one Action attempt and
    // its delegation correspondence stay exactly as committed.
    assert_eq!(task_table_count(&reopened, "action_attempt"), 1);
    assert_eq!(task_table_count(&reopened, "delegation"), 1);
    let stored = reopened
        .load_delegation_result(delegation)
        .await
        .unwrap()
        .expect("the sealed result stays readable");
    assert_eq!(stored.result, result.result);
    assert_eq!(stored.body.text(), "recovered report");
    assert_eq!(stored.adopted_revision, Some(task.revision));
    assert_eq!(stored.attempt_refs.len(), 1);
}

#[tokio::test]
async fn a_cancelled_late_result_stays_recorded_to_the_original_only() {
    let store = open_memory().await.unwrap();
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "input.txt").await;
    settle(
        &store,
        attempt,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    assert_eq!(
        store.cancel_task(task.task).await.unwrap(),
        ene_task::TaskCancelOutcome::CancelAccepted
    );
    let result = finalize(&store, delegation, "late body").await;

    assert_eq!(
        reevaluate_result_adoption(&store, result.result)
            .await
            .unwrap(),
        TaskResultAcceptance::RecordedToOriginalOnly,
        "cancel is absorbing: a late result never completes the Task"
    );
    assert_eq!(progress(&store, task.task).await, TaskProgress::Cancelled);
    let stored = store
        .load_task_result(result.result)
        .await
        .unwrap()
        .expect("the late result stays durable");
    assert!(stored.adopted_revision.is_none());
}

#[tokio::test]
async fn a_moved_revision_result_stays_recorded_to_the_original_only() {
    let store = open_memory().await.unwrap();
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let attempt = start_attempt(&store, delegation, task, assoc, "input.txt").await;
    settle(
        &store,
        attempt,
        ActionCertainty::ConfirmedSuccess,
        EffectGrounds::ObservedAtTarget,
    )
    .await;
    let result = finalize(&store, delegation, "old revision body").await;

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
    assert!(matches!(advanced, TaskCommitOutcome::CommittedAs(_)));

    assert_eq!(
        reevaluate_result_adoption(&store, result.result)
            .await
            .unwrap(),
        TaskResultAcceptance::RecordedToOriginalOnly,
        "a result of a superseded revision is not adopted into the current one"
    );
    assert_eq!(progress(&store, task.task).await, TaskProgress::InProgress);
}

#[tokio::test]
async fn duplicate_and_concurrent_reevaluation_converge_idempotently() {
    let store = open_memory().await.unwrap();
    let (task, delegation, _assoc) = seed_workspace_execution(&store).await;
    let result = finalize(&store, delegation, "done").await;

    let (first, second) = tokio::join!(
        reevaluate_result_adoption(&store, result.result),
        reevaluate_result_adoption(&store, result.result)
    );
    assert_eq!(
        first.unwrap(),
        TaskResultAcceptance::AdoptedAsCompletion(task)
    );
    assert_eq!(
        second.unwrap(),
        TaskResultAcceptance::AdoptedAsCompletion(task)
    );
    // A later duplicate converges on the same completed unit.
    assert_eq!(
        reevaluate_result_adoption(&store, result.result)
            .await
            .unwrap(),
        TaskResultAcceptance::AdoptedAsCompletion(task)
    );
    assert_eq!(task_table_count(&store, "task_result"), 1);
    assert_eq!(progress(&store, task.task).await, TaskProgress::Completed);
}

#[tokio::test]
async fn the_derived_claim_matches_the_durable_execution_lifetime() {
    let store = open_memory().await.unwrap();
    let (task, delegation, assoc) = seed_workspace_execution(&store).await;
    let missing = ene_task::TaskResultId::generate();
    assert_eq!(
        store.load_result_adoption_claim(missing).await.unwrap(),
        None
    );

    let first = start_attempt(&store, delegation, task, assoc, "first.txt").await;
    let result = finalize(&store, delegation, "done").await;
    let claim = store
        .load_result_adoption_claim(result.result)
        .await
        .unwrap()
        .expect("the stored result derives a claim");
    assert_eq!(claim.result, result.result);
    assert_eq!(claim.attempt_refs, vec![first.as_raw()]);

    // The execution is sealed, so a new attempt is refused; the authoritative
    // sealed set never grows and the derived claim stays exactly it.
    let refused = store
        .insert_attempt_if_current(super::task_result::attempt_premise(
            ene_action::ActionAttemptId::generate(),
            delegation,
            task,
            assoc,
            "second.txt",
        ))
        .await
        .unwrap();
    assert_eq!(refused, ActionStartOutcome::ExecutionSealed);
    let claim = store
        .load_result_adoption_claim(result.result)
        .await
        .unwrap()
        .expect("the stored result still derives a claim");
    assert_eq!(claim.attempt_refs, vec![first.as_raw()]);
}

#[tokio::test]
async fn a_corrupted_result_delegation_correspondence_fails_closed() {
    let store = open_memory().await.unwrap();
    let (_task, delegation, _assoc) = seed_workspace_execution(&store).await;
    let result = finalize(&store, delegation, "done").await;
    raw_exec(
        &store,
        "UPDATE delegation SET task_revision = task_revision + 1",
    );
    assert!(
        matches!(
            store.load_result_adoption_claim(result.result).await,
            Err(TaskTechnicalError::StorageUnavailable { .. })
        ),
        "a disagreement between the result and its delegation is not rounded to stale"
    );
}

#[tokio::test]
async fn the_unadopted_listing_excludes_adopted_results_and_respects_the_limit() {
    let store = open_memory().await.unwrap();
    let (first_task, first_delegation, _) = seed_workspace_execution(&store).await;
    let adopted = finalize(&store, first_delegation, "first").await;
    assert_eq!(
        store
            .adopt_result(TaskResultAdoptionClaim {
                result: adopted.result,
                attempt_refs: Vec::new(),
            })
            .await
            .unwrap(),
        TaskResultAcceptance::AdoptedAsCompletion(first_task)
    );
    let (_second_task, second_delegation, _) = seed_workspace_execution(&store).await;
    let pending = finalize(&store, second_delegation, "second").await;

    let listed = store.list_unadopted_results(8).await.unwrap();
    assert_eq!(listed, vec![pending.result]);
    assert_eq!(
        store.list_unadopted_results(0).await.unwrap(),
        Vec::new(),
        "the limit bounds the rows read"
    );
}
