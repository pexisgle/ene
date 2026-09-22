use super::*;
use ene_preservation::*;
use std::sync::Arc;

/// The current product surface's participant snapshot for store fixtures; the
/// Host composition decides it in production, and the store only round-trips it.
fn fixture_participants() -> Vec<ParticipantOwnerRef> {
    vec![
        ParticipantOwnerRef::Companion,
        ParticipantOwnerRef::Learning,
    ]
}

fn command(text: &str, sources: Vec<RawId>) -> StartTargetedDeletionCommand {
    command_with(text, sources, fixture_participants())
}

fn command_with(
    text: &str,
    sources: Vec<RawId>,
    participants: Vec<ParticipantOwnerRef>,
) -> StartTargetedDeletionCommand {
    StartTargetedDeletionCommand::new(
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                text.into(),
            )),
            semantic_hints: vec![],
        },
        DeletionPurpose::Privacy,
        WallClockWithTz::now(),
        sources,
        participants,
    )
}

pub(super) async fn admit(store: &Store, text: &str, sources: Vec<RawId>) -> DeletionOperationRef {
    let outcome = store
        .start_targeted_deletion(command(text, sources).confirmed_for_tests())
        .await
        .unwrap();
    match outcome {
        StartTargetedDeletionOutcome::Started(current) => current,
        other => panic!("unexpected admission: {other:?}"),
    }
}

/// Marks every required participant `verified` for the operation's current
/// sweep through raw fixture SQL.
///
/// Only tests that fabricate a durable `finalizing` shape the A5 boundary owns
/// use this: `validate` refuses `finalizing` without the full verified
/// participant premise, and the fixture must establish that premise
/// explicitly instead of publishing a torn state.
pub(super) fn mark_all_verified(store: &Store, current: DeletionOperationRef) {
    let id = crate::codec::encode_id(current.operation.as_raw());
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE deletion_participant SET state='verified',hold_class=NULL,remainder_count=0,reported_at='2026-09-17T01:00:00Z' WHERE operation_id=?1 AND sweep=?2",
            params![id, current.sweep.as_u64() as i64],
        )
        .unwrap();
}

#[tokio::test]
async fn confirmation_clarification_duplicate_and_conflict_are_distinct() {
    let store = open_memory().await.unwrap();
    let source = RawId::new();
    let request = command("private target", vec![source]);
    assert!(!format!("{request:?}").contains("private target"));
    assert_eq!(
        store
            .start_targeted_deletion(request.clone())
            .await
            .unwrap(),
        StartTargetedDeletionOutcome::ConfirmationRequired
    );
    assert_eq!(
        store
            .start_targeted_deletion(command(" ", vec![]).confirmed_for_tests())
            .await
            .unwrap(),
        StartTargetedDeletionOutcome::NeedsClarification
    );
    let request = request.confirmed_for_tests();
    let StartTargetedDeletionOutcome::Started(current) = store
        .start_targeted_deletion(request.clone())
        .await
        .unwrap()
    else {
        panic!("not started")
    };
    assert_eq!(
        store.start_targeted_deletion(request).await.unwrap(),
        StartTargetedDeletionOutcome::AlreadyCoveredBy(current)
    );
    assert_eq!(
        store
            .start_targeted_deletion(
                command("private target", vec![RawId::new()]).confirmed_for_tests()
            )
            .await
            .unwrap(),
        StartTargetedDeletionOutcome::HeldByOperation(current)
    );
    assert_eq!(
        store.current_erasure_conditions(None, 10).await.unwrap()[0].condition,
        current.condition()
    );
}

#[tokio::test]
async fn admission_rollback_leaves_no_partial_publication() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("atomic.db");
    let store = Store::open(&path).await.unwrap();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute_batch("CREATE TRIGGER fail_admission BEFORE INSERT ON erasure_condition_source BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    }
    assert_eq!(
        store
            .start_targeted_deletion(command("sensitive", vec![RawId::new()]).confirmed_for_tests())
            .await,
        Err(PreservationTechnicalError::StorageUnavailable)
    );
    drop(store);
    let store = Store::open(&path).await.unwrap();
    for table in [
        "deletion_operation",
        "deletion_search_material",
        "erasure_condition",
        "erasure_condition_source",
        "deletion_participant",
    ] {
        assert_eq!(task_table_count(&store, table), 0);
    }
    assert!(
        store
            .current_erasure_conditions(None, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn restart_recovers_active_held_finalizing_and_cumulative_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restart.db");
    let store = Store::open(&path).await.unwrap();
    let source = RawId::new();
    let current = admit(&store, "secret", vec![source]).await;
    let finalizing = admit(&store, "other", vec![]).await;
    let active = admit(&store, "active", vec![]).await;
    assert_eq!(
        store
            .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
            .await
            .unwrap(),
        DeletionLifecycleOutcome::Applied(DeletionOperationRef {
            sweep: DeletionSweepGeneration::from_u64(2),
            ..current
        })
    );
    assert_eq!(
        store
            .change_deletion_lifecycle(current, DeletionLifecycleChange::Hold)
            .await
            .unwrap(),
        DeletionLifecycleOutcome::StaleSweep
    );
    let next = DeletionOperationRef {
        sweep: DeletionSweepGeneration::from_u64(2),
        ..current
    };
    store
        .change_deletion_lifecycle(next, DeletionLifecycleChange::Hold)
        .await
        .unwrap();
    // Only fixture SQL can enter finalizing: the A5 completion boundary owns
    // the transition, so the fixture must also establish its durable premise
    // (every required participant verified for the current sweep).
    mark_all_verified(&store, finalizing);
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE deletion_operation SET phase='finalizing' WHERE operation_id=?1",
            [crate::codec::encode_id(finalizing.operation.as_raw())],
        )
        .unwrap();
    drop(store);
    let reopened = Store::open(&path).await.unwrap();
    let rows = reopened.unfinished_deletions(None, 100).await.unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter()
            .any(|r| r.current == active && r.phase == DeletionOperationPhase::Active)
    );
    assert!(
        rows.iter()
            .any(|r| r.current == next && r.phase == DeletionOperationPhase::Held)
    );
    assert!(
        rows.iter()
            .any(|r| r.current == finalizing && r.phase == DeletionOperationPhase::Finalizing)
    );
    assert_eq!(
        crate::preservation::covering_condition(
            &reopened.conn.lock().unwrap(),
            &crate::codec::encode_id(source)
        )
        .unwrap(),
        Some(next.condition())
    );
    assert_eq!(
        reopened
            .current_erasure_conditions(None, 100)
            .await
            .unwrap()
            .len(),
        3
    );
    // Current-sweep canonical: the sweep-1 source row was deleted when sweep
    // 2 inherited it, so only the current sweep carries the correlation.
    assert_eq!(task_table_count(&reopened, "erasure_condition_source"), 1);
    {
        let guard = reopened.conn.lock().unwrap();
        let id = crate::codec::encode_id(next.operation.as_raw());
        let old: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM erasure_condition_source WHERE operation_id=?1 AND sweep=1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        let current_count: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM erasure_condition_source WHERE operation_id=?1 AND sweep=2",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((old, current_count), (0, 1));
    }
    assert_eq!(store_page_count(&reopened).await, 3);
}

async fn store_page_count(store: &Store) -> usize {
    let mut after = None;
    let mut count = 0;
    loop {
        let rows = store.unfinished_deletions(after, 1).await.unwrap();
        if rows.is_empty() {
            return count;
        }
        after = Some(rows[0].current.operation);
        count += 1;
    }
}

#[tokio::test]
async fn concurrent_duplicate_admissions_serialize_on_sqlite_master() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.db");
    let a = Store::open(&path).await.unwrap();
    let b = Store::open(&path).await.unwrap();
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let request = command("racing", vec![RawId::new()]).confirmed_for_tests();
    let other = request.clone();
    let gate = Arc::clone(&barrier);
    let first = tokio::spawn(async move {
        gate.wait().await;
        a.start_targeted_deletion(request).await.unwrap()
    });
    let second = tokio::spawn(async move {
        barrier.wait().await;
        b.start_targeted_deletion(other).await.unwrap()
    });
    let results = [first.await.unwrap(), second.await.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, StartTargetedDeletionOutcome::Started(_)))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, StartTargetedDeletionOutcome::AlreadyCoveredBy(_)))
            .count(),
        1
    );
    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(
        reopened.unfinished_deletions(None, 10).await.unwrap().len(),
        1
    );
}

/// Drives the bounded covered-source reconciliation of one operation's
/// current sweep to completion through the canonical API, exactly as the
/// production fan-out would before demanding any participant.
pub(super) async fn reconcile_to_complete(store: &Store, current: DeletionOperationRef) {
    for _ in 0..64 {
        match store
            .reconcile_deletion_sources(current, DELETION_RECONCILIATION_PAGE_SIZE)
            .await
            .expect("a reconciliation step must answer")
        {
            DeletionReconciliationOutcome::Advanced => {}
            DeletionReconciliationOutcome::Complete
            | DeletionReconciliationOutcome::Finalizing
            | DeletionReconciliationOutcome::Completed => return,
            other => panic!("unexpected reconciliation step: {other:?}"),
        }
    }
    panic!("the exhaustive walk must finish inside the fixture budget");
}

/// Drives one operation to the durable `finalizing` marker through the sealed
/// A5 begin step, without the completion commit.
///
/// The current sweep's exhaustive covered-source reconciliation is driven to
/// completion first, then every required participant is verified through the
/// canonical completion API. The system-wide mechanical probe must be clean:
/// a call site whose fixture still stores the target must drive that owner's
/// real sweep first, exactly as the production fan-out would.
pub(super) async fn enter_finalizing_via_a5(store: &Store, current: DeletionOperationRef) {
    reconcile_to_complete(store, current).await;
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
            let outcome = store
                .record_participant_completion(ParticipantCompletionFact::verified(
                    current.condition(),
                    record.participant.owner,
                    0,
                    WallClockWithTz::now(),
                ))
                .await
                .expect("a verified fact must record");
            assert!(
                matches!(outcome, ParticipantCompletionOutcome::Recorded(_)),
                "the fixture fact must apply: {outcome:?}"
            );
        }
        if page_len < 100 {
            break;
        }
    }
    assert_eq!(
        store
            .begin_deletion_finalizing(current)
            .await
            .expect("the finalizing transition must answer"),
        DeletionFinalizationOutcome::Finalizing
    );
}

/// Completes one operation through the sealed A5 boundary.
///
/// [`enter_finalizing_via_a5`] then the completion commit. The durable
/// audit/condition/output commit is the production path.
pub(super) async fn complete_via_a5(store: &Store, current: DeletionOperationRef) {
    enter_finalizing_via_a5(store, current).await;
    assert_eq!(
        store
            .complete_deletion_finalizing(current)
            .await
            .expect("the completion commit must answer"),
        DeletionFinalizationOutcome::Completed
    );
}

#[tokio::test]
async fn completed_operation_requires_every_participant_verified_for_the_final_sweep() {
    let store = open_memory().await.unwrap();
    let current = admit(&store, "completed-participants", vec![]).await;
    let id = crate::codec::encode_id(current.operation.as_raw());
    // A canonical completion through the sealed boundary: verified
    // participants, closed condition, removed material, durable audit.
    complete_via_a5(&store, current).await;
    // The participant table alone answers the completion invariant for A5.
    let rows = store
        .deletion_participants(current.operation, None, 100)
        .await
        .unwrap();
    assert!(
        rows.iter().all(|record| record.progress.is_verified()),
        "a completed operation has every participant verified"
    );
    assert_eq!(
        store
            .deletion_operation_material(current.operation)
            .await
            .unwrap(),
        DeletionMaterialOutcome::Destroyed,
        "completion destroys the protected material"
    );
    // A completed operation whose participant row is not verified is torn
    // canonical state; reads fail closed instead of reporting the snapshot.
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE deletion_participant SET state='held',hold_class='failed',reported_at='2026-09-17T02:00:00Z' WHERE operation_id=?1 AND participant_owner='companion'",
            [&id],
        )
        .unwrap();
    }
    assert_eq!(
        store
            .deletion_participants(current.operation, None, 100)
            .await,
        Err(PreservationTechnicalError::CorruptState)
    );
    assert_eq!(
        store
            .begin_participant_demand(current.condition(), ParticipantOwnerRef::Companion)
            .await,
        Err(PreservationTechnicalError::CorruptState),
        "the torn completion fails closed on every participant path"
    );
    // A canonical completion instead answers the participant paths with the
    // completed domain outcome.
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE deletion_participant SET state='verified',hold_class=NULL,remainder_count=0 WHERE operation_id=?1 AND participant_owner='companion'",
            [&id],
        )
        .unwrap();
    }
    assert_eq!(
        store
            .begin_participant_demand(current.condition(), ParticipantOwnerRef::Companion)
            .await,
        Ok(ParticipantDemandOutcome::Completed)
    );
    assert_eq!(
        store
            .record_participant_completion(ParticipantCompletionFact::verified(
                current.condition(),
                ParticipantOwnerRef::Companion,
                1,
                WallClockWithTz::now(),
            ))
            .await,
        Ok(ParticipantCompletionOutcome::Completed)
    );
}
