//! Stage 6 A5: participant completion aggregation, system-wide remainder
//! verification, the finalizing transition, the sealed global completion
//! commit, and the body-free audit (lifecycle §10, §12-§14, §18).
//!
//! These tests drive the production store API. A row that simulates a delayed
//! arrival reaching durable storage is inserted with fixture SQL: the A4
//! acceptance gates refuse every covered write, so the state "a body appeared
//! after the owner verified" is exactly what the A5 remainder probe must
//! collect instead of completing over it.

use super::preservation::{admit, complete_via_a5, mark_all_verified};
use super::*;

use ene_preservation::{
    ConfirmTargetedDeletionOutcome, DeletionAuditStatus, DeletionCompletionSummary,
    DeletionFinalizationOutcome, DeletionLifecycleChange, DeletionLifecycleOutcome,
    DeletionMaterialOutcome, DeletionOperationId, DeletionOperationPhase, DeletionOperationRef,
    DeletionPurpose, DeletionSearchMaterial, DeletionSweepGeneration, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantCompletionOutcome, ParticipantCompletionStatus,
    ParticipantOwnerRef, ParticipantProgress, PreservationRepository as _,
    PreservationTechnicalError, StageTargetedDeletionRequestCommand,
    StageTargetedDeletionRequestOutcome, TargetedDeletionTarget,
};

/// Verifies every required participant of the operation's current sweep
/// through the canonical completion API, without completing the operation.
async fn verify_all(store: &Store, current: DeletionOperationRef) {
    let mut after = None;
    loop {
        let page = store
            .deletion_participants(current.operation, after, 100)
            .await
            .expect("the required snapshot must read");
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        for record in page {
            after = Some(record.participant.owner);
            store
                .record_participant_completion(ParticipantCompletionFact::verified(
                    current.condition(),
                    record.participant.owner,
                    0,
                    WallClockWithTz::now(),
                ))
                .await
                .expect("a verified fact must record");
        }
        if page_len < 100 {
            break;
        }
    }
}

/// Inserts a covered History body that reached durable storage after the
/// owner's verification pass: the delayed arrival the A5 remainder probe must
/// find. The A4 gates refuse the same text through the production append path,
/// so only fixture SQL can produce this remainder.
fn insert_delayed_history(store: &Store, companion: CompanionId, body: &str) {
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO history_message (message_id,companion_id,round_id,role,body,lang,at,presence_generation) VALUES (?1,?2,?3,'owner',?4,'en',?5,1)",
            params![
                crate::codec::encode_id(RawId::new()),
                crate::codec::encode_id(companion.as_raw()),
                crate::codec::encode_id(RawId::new()),
                body,
                WallClockWithTz::now().to_rfc3339(),
            ],
        )
        .unwrap();
}

/// Raw count of one table's rows for this operation.
fn operation_rows(store: &Store, table: &str, current: DeletionOperationRef) -> i64 {
    let id = crate::codec::encode_id(current.operation.as_raw());
    store
        .conn
        .lock()
        .unwrap()
        .query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE operation_id=?1"),
            [&id],
            |row| row.get(0),
        )
        .unwrap()
}

/// The system-wide mechanical remainder probe shared with the completion
/// boundary (test-support path of the same closed surface list).
fn remainder(store: &Store, text: &str) -> u64 {
    let guard = crate::codec::lock_shared(&store.conn);
    crate::erasure::exact_remainder_probe(&guard, text).unwrap()
}

/// The A5 completion candidate is the durable participant aggregate: one
/// unfinished participant keeps it false, and neither a local completion nor
/// the aggregate is a global completion.
#[tokio::test]
async fn one_unfinished_participant_is_not_a_global_completion() {
    let store = open_memory().await.unwrap();
    let current = admit(&store, "a5-one-unfinished", vec![]).await;
    store
        .record_participant_completion(ParticipantCompletionFact::verified(
            current.condition(),
            ParticipantOwnerRef::Companion,
            0,
            WallClockWithTz::now(),
        ))
        .await
        .unwrap();
    store
        .record_participant_completion(ParticipantCompletionFact::local_complete(
            current.condition(),
            ParticipantOwnerRef::Learning,
            3,
            0,
            WallClockWithTz::now(),
        ))
        .await
        .unwrap();

    let summary: DeletionCompletionSummary = store
        .deletion_completion_summary(current.operation)
        .await
        .unwrap();
    assert_eq!(
        (
            summary.required,
            summary.verified,
            summary.local_complete,
            summary.in_progress,
            summary.held,
        ),
        (2, 1, 1, 0, 0)
    );
    assert!(!summary.all_verified());
    assert_eq!(
        store.begin_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::NotVerified(summary),
        "one unfinished participant is never a completion"
    );
    assert_eq!(
        store.complete_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::NotFinalizing,
        "no completion commit exists for an unfinished operation"
    );
    let rows = store.unfinished_deletions(None, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].phase, DeletionOperationPhase::Active);
    assert_eq!(
        rows[0].current, current,
        "the identity is never regenerated"
    );
    assert!(
        store
            .deletion_completion_audit(current.operation)
            .await
            .unwrap()
            .is_none(),
        "no audit exists before completion"
    );
}

/// A mechanical remainder refuses finalizing even when every participant
/// claims verification: the probe opens a new sweep, the old verification is
/// stale, and only the real sweep of the new generation completes the
/// operation.
#[tokio::test]
async fn local_completion_and_a_remainder_never_finalize_and_the_new_sweep_does() {
    let store = open_memory().await.unwrap();
    let (companion, _generation) = running_companion(&store).await.unwrap();
    let target = "a5-remainder-target";
    let current = admit(&store, target, vec![]).await;

    // All participants local-complete: not verified, so no finalization even
    // before the remainder is injected.
    for owner in [
        ParticipantOwnerRef::Companion,
        ParticipantOwnerRef::Learning,
    ] {
        store
            .record_participant_completion(ParticipantCompletionFact::local_complete(
                current.condition(),
                owner,
                0,
                0,
                WallClockWithTz::now(),
            ))
            .await
            .unwrap();
    }
    let summary = store
        .deletion_completion_summary(current.operation)
        .await
        .unwrap();
    assert_eq!(
        store.begin_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::NotVerified(summary)
    );

    // A delayed arrival reaches durable storage after the claims: completing
    // over it is impossible.
    insert_delayed_history(&store, companion, &format!("leaked {target}"));
    verify_all(&store, current).await;
    assert!(
        store
            .deletion_completion_summary(current.operation)
            .await
            .unwrap()
            .all_verified()
    );
    let DeletionFinalizationOutcome::RemainderCollected(next) =
        store.begin_deletion_finalizing(current).await.unwrap()
    else {
        panic!("a mechanical remainder must open a new sweep, never finalize");
    };
    assert_eq!(next.sweep.as_u64(), 2);

    // The old generation's verification and results are stale for the new one.
    assert_eq!(
        store.begin_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::StaleSweep
    );
    assert_eq!(
        store
            .record_participant_completion(ParticipantCompletionFact::verified(
                current.condition(),
                ParticipantOwnerRef::Companion,
                0,
                WallClockWithTz::now(),
            ))
            .await
            .unwrap(),
        ParticipantCompletionOutcome::StaleSweep
    );
    let rows = store.unfinished_deletions(None, 10).await.unwrap();
    assert_eq!(rows[0].current, next);
    assert_eq!(rows[0].phase, DeletionOperationPhase::Active);
    let participants = store
        .deletion_participants(next.operation, None, 100)
        .await
        .unwrap();
    assert!(
        participants
            .iter()
            .all(|record| record.progress == ParticipantProgress::Pending),
        "a new generation re-opens every participant"
    );
    assert!(
        matches!(
            store
                .deletion_operation_material(current.operation)
                .await
                .unwrap(),
            DeletionMaterialOutcome::Material(_)
        ),
        "returning to Active happens before any material is destroyed"
    );
    assert_eq!(
        store.current_erasure_conditions(None, 10).await.unwrap()[0].condition,
        next.condition(),
        "the current condition is the new sweep"
    );
    assert!(remainder(&store, target) > 0);

    // The real owner sweep collects the remainder on the new sweep, and only
    // then does the operation complete.
    let participant = crate::CompanionErasureParticipant::new(store.clone());
    let fact = super::targeted_deletion::drive_with_sources(
        &participant,
        next.condition(),
        ParticipantOwnerRef::Companion,
        target,
        Vec::new(),
    )
    .await;
    assert_eq!(
        store.record_participant_completion(fact).await.unwrap(),
        ParticipantCompletionOutcome::Recorded(ParticipantProgress::Verified { sweep: next.sweep })
    );
    store
        .record_participant_completion(ParticipantCompletionFact::verified(
            next.condition(),
            ParticipantOwnerRef::Learning,
            0,
            WallClockWithTz::now(),
        ))
        .await
        .unwrap();
    assert_eq!(
        store.begin_deletion_finalizing(next).await.unwrap(),
        DeletionFinalizationOutcome::Finalizing
    );
    assert_eq!(
        store.complete_deletion_finalizing(next).await.unwrap(),
        DeletionFinalizationOutcome::Completed
    );
    assert_eq!(remainder(&store, target), 0);
    let audit = store
        .deletion_completion_audit(current.operation)
        .await
        .unwrap()
        .expect("completion writes the audit");
    assert_eq!(audit.sweep_count, 2);
    assert_eq!(audit.participants.len(), 2);
    assert!(
        audit
            .participants
            .iter()
            .all(|entry| entry.status == DeletionAuditStatus::Verified)
    );
}

/// A crash before the completion commit leaves a durable `Finalizing` marker:
/// restart restores it, the current condition stays effective, the protected
/// material is intact, and only the remaining steps run. A crash after the
/// commit leaves the terminal completed state, never an early closure.
#[tokio::test]
async fn restart_before_and_after_the_completion_commit_never_releases_early() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a5-crash.db");
    let store = Store::open(&path).await.unwrap();
    let source = RawId::new();
    let target = "a5-crash-target";
    let current = admit(&store, target, vec![source]).await;
    verify_all(&store, current).await;
    assert_eq!(
        store.begin_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::Finalizing
    );
    // Crash before the completion commit.
    drop(store);

    let reopened = Store::open(&path).await.unwrap();
    let conditions = reopened.current_erasure_conditions(None, 10).await.unwrap();
    assert_eq!(
        conditions.len(),
        1,
        "an unfinished operation keeps its current condition after restart"
    );
    assert_eq!(conditions[0].condition, current.condition());
    let rows = reopened.unfinished_deletions(None, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].phase, DeletionOperationPhase::Finalizing);
    assert_eq!(rows[0].current, current);
    assert!(
        matches!(
            reopened
                .deletion_operation_material(current.operation)
                .await
                .unwrap(),
            DeletionMaterialOutcome::Material(_)
        ),
        "the verification material survives the crash: re-verification stays possible"
    );
    assert!(
        reopened
            .deletion_completion_audit(current.operation)
            .await
            .unwrap()
            .is_none(),
        "restart never estimates completion"
    );
    // Resume from the durable marker.
    assert_eq!(
        reopened
            .complete_deletion_finalizing(current)
            .await
            .unwrap(),
        DeletionFinalizationOutcome::Completed
    );
    drop(reopened);

    // Crash after the completion commit.
    let reopened = Store::open(&path).await.unwrap();
    assert!(
        reopened
            .current_erasure_conditions(None, 10)
            .await
            .unwrap()
            .is_empty(),
        "completion removes the operation from the current-condition set"
    );
    assert!(
        reopened
            .unfinished_deletions(None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        reopened
            .deletion_operation_material(current.operation)
            .await
            .unwrap(),
        DeletionMaterialOutcome::Destroyed
    );
    assert_eq!(
        reopened
            .complete_deletion_finalizing(current)
            .await
            .unwrap(),
        DeletionFinalizationOutcome::CompletedAlready
    );
    assert_eq!(
        reopened.begin_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::CompletedAlready
    );
    let audit = reopened
        .deletion_completion_audit(current.operation)
        .await
        .unwrap()
        .expect("the completion audit survives restart");
    assert_eq!(audit.operation, current.operation);
    assert_eq!(audit.purpose, DeletionPurpose::Privacy);
    assert_eq!(audit.verified_count(), audit.participants.len() as u64);
    assert_eq!(audit.started_at, rows_started_at(&reopened, current));
}

/// The operation's durable `started_at`, read raw for the audit comparison.
fn rows_started_at(store: &Store, current: DeletionOperationRef) -> WallClockWithTz {
    let id = crate::codec::encode_id(current.operation.as_raw());
    let raw: String = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT started_at FROM deletion_operation WHERE operation_id=?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    WallClockWithTz::parse_rfc3339(&raw).unwrap()
}

/// The completion audit is body-free: the target body, a registered secret,
/// and a prompt body are all absent from every audit column, while the
/// protected search material itself is destroyed.
#[tokio::test]
async fn completion_audit_carries_no_body_secret_or_prompt_text() {
    let store = open_memory().await.unwrap();
    let (companion, _generation) = running_companion(&store).await.unwrap();
    let target = "a5-audit-target";
    let secret = "a5-registered-secret-value";
    let prompt = "a5-provider-prompt-body";
    // Fixture text that is not the operation's target: it stays in its owner's
    // rows and must never leak into the audit.
    insert_delayed_history(&store, companion, secret);
    insert_delayed_history(&store, companion, prompt);
    // The target itself is stored and collected by the real owner sweep.
    insert_delayed_history(&store, companion, &format!("body with {target}"));
    let current = admit(&store, target, vec![]).await;
    let participant = crate::CompanionErasureParticipant::new(store.clone());
    let fact = super::targeted_deletion::drive_with_sources(
        &participant,
        current.condition(),
        ParticipantOwnerRef::Companion,
        target,
        Vec::new(),
    )
    .await;
    assert_eq!(fact.status(), ParticipantCompletionStatus::Verified);
    store.record_participant_completion(fact).await.unwrap();
    store
        .record_participant_completion(ParticipantCompletionFact::verified(
            current.condition(),
            ParticipantOwnerRef::Learning,
            0,
            WallClockWithTz::now(),
        ))
        .await
        .unwrap();
    assert_eq!(
        store.begin_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::Finalizing
    );
    assert_eq!(
        store.complete_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::Completed
    );
    assert_eq!(
        remainder(&store, target),
        0,
        "the target is gone from the whole store, not just the audit"
    );
    let scan = store.conn.lock().unwrap();
    let leaked = |needle: &str| -> i64 {
        scan.query_row(
            "SELECT
               (SELECT COUNT(*) FROM deletion_completion_audit
                WHERE instr(operation_id, ?1) > 0 OR instr(purpose, ?1) > 0
                   OR instr(started_at, ?1) > 0 OR instr(completed_at, ?1) > 0)
             + (SELECT COUNT(*) FROM deletion_audit_participant
                WHERE instr(participant_owner, ?1) > 0 OR instr(final_state, ?1) > 0)",
            [needle],
            |row| row.get(0),
        )
        .unwrap()
    };
    for needle in [target, secret, prompt] {
        assert_eq!(leaked(needle), 0, "no audit column may carry {needle}");
    }
}

/// The operation-lifetime search material is readable while the operation is
/// unfinished (so verification can always be re-established) and is destroyed
/// by the completion commit: no row can reconstruct the target afterwards,
/// including the staged request's protected text.
#[tokio::test]
async fn completion_destroys_the_search_material_unrecoverably() {
    let store = open_memory().await.unwrap();
    let target = "a5-material-target";
    let staged = store
        .stage_targeted_deletion(StageTargetedDeletionRequestCommand::new(
            TargetedDeletionTarget {
                mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                    target.to_owned(),
                )),
                semantic_hints: Vec::new(),
            },
            DeletionPurpose::Privacy,
            WallClockWithTz::now(),
        ))
        .await
        .unwrap();
    let request = match staged {
        StageTargetedDeletionRequestOutcome::Staged(request) => request,
        other => panic!("the fixture request must stage: {other:?}"),
    };
    let current = match store
        .confirm_targeted_deletion(request, vec![ParticipantOwnerRef::Companion])
        .await
        .unwrap()
    {
        ConfirmTargetedDeletionOutcome::Started(current) => current,
        other => panic!("the fixture operation must start: {other:?}"),
    };
    assert!(matches!(
        store
            .deletion_operation_material(current.operation)
            .await
            .unwrap(),
        DeletionMaterialOutcome::Material(_)
    ));
    verify_all(&store, current).await;
    assert_eq!(
        store.begin_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::Finalizing
    );
    assert!(
        matches!(
            store
                .deletion_operation_material(current.operation)
                .await
                .unwrap(),
            DeletionMaterialOutcome::Material(_)
        ),
        "Finalizing keeps the material until the completion commit"
    );
    assert_eq!(
        store.complete_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::Completed
    );
    assert_eq!(
        store
            .deletion_operation_material(current.operation)
            .await
            .unwrap(),
        DeletionMaterialOutcome::Destroyed
    );
    assert_eq!(
        operation_rows(&store, "deletion_search_material", current),
        0
    );
    assert_eq!(operation_rows(&store, "deletion_semantic_hint", current), 0);
    assert_eq!(
        operation_rows(&store, "erasure_condition_source", current),
        0
    );
    let request_text: Option<String> = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT exact_text FROM deletion_request WHERE request_id=?1",
            [crate::codec::encode_id(request.as_raw())],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        request_text, None,
        "the staged request's protected text is destroyed at completion"
    );
    let audit = store
        .deletion_completion_audit(current.operation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(audit.erased_count, 0);
    assert_eq!(audit.sweep_count, 1);
}

/// A completed operation ends the text's meaning as a deletion target: the
/// same string is a fresh origin for reads, appends, and a new admission.
#[tokio::test]
async fn a_completed_operation_is_not_a_permanent_ban() {
    let store = open_memory().await.unwrap();
    let target = "a5-new-origin";
    let current = admit(&store, target, vec![]).await;
    complete_via_a5(&store, current).await;
    assert!(
        crate::preservation::covering_text(
            &store.conn.lock().unwrap(),
            &format!("text with {target}")
        )
        .unwrap()
        .is_none(),
        "a closed condition no longer covers the text"
    );
    let (companion, generation) = running_companion(&store).await.unwrap();
    assert!(matches!(
        store
            .append_message(history_command(companion, generation, target))
            .await
            .unwrap(),
        HistoryAppendOutcome::CommittedAs { .. }
    ));
    let fresh = admit(&store, target, vec![]).await;
    assert_ne!(fresh.operation, current.operation);
    assert_eq!(
        store.current_erasure_conditions(None, 10).await.unwrap()[0].condition,
        fresh.condition(),
        "a new operation with the same text is a new origin, not a duplicate"
    );
}

/// A completion audit can never be written for an operation that is not
/// `Finalizing`, and a completed operation's own condition is terminal.
#[tokio::test]
async fn completion_only_commits_from_finalizing_and_only_advances_a_matching_ref() {
    let store = open_memory().await.unwrap();
    let current = admit(&store, "a5-phases", vec![]).await;
    assert_eq!(
        store.complete_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::NotFinalizing
    );
    let stale = DeletionOperationRef {
        operation: current.operation,
        sweep: DeletionSweepGeneration::from_u64(7),
    };
    assert_eq!(
        store.begin_deletion_finalizing(stale).await.unwrap(),
        DeletionFinalizationOutcome::StaleSweep
    );
    assert_eq!(
        store.complete_deletion_finalizing(stale).await.unwrap(),
        DeletionFinalizationOutcome::StaleSweep
    );
    verify_all(&store, current).await;
    assert_eq!(
        store.begin_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::Finalizing
    );
    // Idempotent marker: repeating the begin step changes nothing.
    assert_eq!(
        store.begin_deletion_finalizing(current).await.unwrap(),
        DeletionFinalizationOutcome::Finalizing
    );
    let rows = store.unfinished_deletions(None, 10).await.unwrap();
    assert_eq!(rows[0].phase, DeletionOperationPhase::Finalizing);
    assert!(
        store
            .deletion_completion_summary(current.operation)
            .await
            .unwrap()
            .all_verified()
    );
}

/// The audited erased count accumulates every sweep of the operation, not just
/// the final generation: a sweep that erased and was superseded still appears
/// in the objective completion metadata.
#[tokio::test]
async fn the_audit_erased_count_accumulates_across_sweeps() {
    let store = open_memory().await.unwrap();
    let current = admit(&store, "a5-erased-total", vec![]).await;
    for (owner, erased, remainder) in [
        (ParticipantOwnerRef::Companion, 5, 2),
        (ParticipantOwnerRef::Learning, 3, 1),
    ] {
        store
            .record_participant_completion(ParticipantCompletionFact::local_complete(
                current.condition(),
                owner,
                erased,
                remainder,
                WallClockWithTz::now(),
            ))
            .await
            .unwrap();
    }
    let DeletionLifecycleOutcome::Applied(next) = store
        .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
        .await
        .unwrap()
    else {
        panic!("the generation must advance");
    };
    verify_all(&store, next).await;
    assert_eq!(
        store.begin_deletion_finalizing(next).await.unwrap(),
        DeletionFinalizationOutcome::Finalizing
    );
    assert_eq!(
        store.complete_deletion_finalizing(next).await.unwrap(),
        DeletionFinalizationOutcome::Completed
    );
    let audit = store
        .deletion_completion_audit(current.operation)
        .await
        .unwrap()
        .expect("completion writes the audit");
    assert_eq!(audit.sweep_count, 2);
    assert_eq!(
        audit.erased_count, 8,
        "the superseded sweep's erased rows stay in the audit"
    );
}

/// `mark_all_verified` and the aggregate agree: the summary counts what the
/// durable participant rows say for the current sweep, nothing inferred.
#[tokio::test]
async fn the_completion_summary_is_the_durable_participant_aggregate() {
    let store = open_memory().await.unwrap();
    let current = admit(&store, "a5-summary", vec![]).await;
    let summary = store
        .deletion_completion_summary(current.operation)
        .await
        .unwrap();
    assert_eq!(
        (summary.required, summary.verified, summary.held),
        (2, 0, 0)
    );
    assert!(!summary.all_verified());
    assert!(summary.is_well_formed());
    mark_all_verified(&store, current);
    let summary = store
        .deletion_completion_summary(current.operation)
        .await
        .unwrap();
    assert_eq!((summary.required, summary.verified), (2, 2));
    assert!(summary.all_verified());
    assert!(summary.is_well_formed());
    assert_eq!(summary.sweep, current.sweep);
    assert_eq!(
        store
            .deletion_completion_summary(DeletionOperationId::from_raw(RawId::new()))
            .await,
        Err(PreservationTechnicalError::UnknownOperation)
    );
}
