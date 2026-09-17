use super::*;
use ene_preservation::*;
use std::sync::Arc;

fn command(text: &str, sources: Vec<RawId>) -> StartTargetedDeletionCommand {
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
    // Only fixture SQL can enter finalizing: no A1 production completion authority.
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
    assert_eq!(task_table_count(&reopened, "erasure_condition_source"), 2);
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
async fn generation_exhaustion_is_durable_and_cannot_resume() {
    let store = open_memory().await.unwrap();
    let current = admit(&store, "max", vec![]).await;
    let id = crate::codec::encode_id(current.operation.as_raw());
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE deletion_operation SET sweep=?2 WHERE operation_id=?1",
            params![id, i64::MAX],
        )
        .unwrap();
        conn.execute(
            "UPDATE erasure_condition SET sweep=?2 WHERE operation_id=?1",
            params![id, i64::MAX],
        )
        .unwrap();
    }
    let current = DeletionOperationRef {
        sweep: DeletionSweepGeneration::from_u64(i64::MAX as u64),
        ..current
    };
    let held = DeletionLifecycleOutcome::Held(DeletionHoldReason::GenerationExhausted);
    assert_eq!(
        store
            .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
            .await
            .unwrap(),
        held
    );
    assert_eq!(
        store
            .change_deletion_lifecycle(current, DeletionLifecycleChange::Resume)
            .await
            .unwrap(),
        held
    );
    assert_eq!(
        store.unfinished_deletions(None, 10).await.unwrap()[0].hold,
        Some(DeletionHoldReason::GenerationExhausted)
    );
    assert_eq!(
        store.current_erasure_conditions(None, 10).await.unwrap()[0].condition,
        current.condition()
    );
}

#[tokio::test]
async fn closed_completed_condition_is_not_a_permanent_ban_and_corruption_fails_closed() {
    let store = open_memory().await.unwrap();
    let source = RawId::new();
    let current = admit(&store, "temporary", vec![source]).await;
    let id = crate::codec::encode_id(current.operation.as_raw());
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE erasure_condition SET closed_at='2026-09-17T01:00:00Z' WHERE operation_id=?1",
            [&id],
        )
        .unwrap();
    }
    assert_eq!(
        store.current_erasure_conditions(None, 10).await,
        Err(PreservationTechnicalError::CorruptState)
    );
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM deletion_search_material WHERE operation_id=?1",
            [&id],
        )
        .unwrap();
        conn.execute(
            "UPDATE deletion_operation SET phase='completed' WHERE operation_id=?1",
            [&id],
        )
        .unwrap();
    }
    assert!(
        store
            .current_erasure_conditions(None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .unfinished_deletions(None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        crate::preservation::covering_condition(
            &store.conn.lock().unwrap(),
            &crate::codec::encode_id(source)
        )
        .unwrap(),
        None
    );
    assert_eq!(
        store
            .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
            .await
            .unwrap(),
        DeletionLifecycleOutcome::Completed
    );
    let fresh = admit(&store, "temporary", vec![RawId::new()]).await;
    assert_ne!(fresh.operation, current.operation);
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
