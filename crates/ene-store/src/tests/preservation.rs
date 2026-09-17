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
async fn current_set_follows_the_current_sweep_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("current-sweep.db");
    let store = Store::open(&path).await.unwrap();
    let mut current = admit(&store, "sweeping", vec![]).await;
    for _ in 0..2 {
        let DeletionLifecycleOutcome::Applied(next) = store
            .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
            .await
            .unwrap()
        else {
            panic!("the generation must advance");
        };
        current = next;
    }
    assert_eq!(current.sweep.as_u64(), 3);
    let current = DeletionOperationRef {
        sweep: DeletionSweepGeneration::from_u64(3),
        ..current
    };
    // Age only the historical sweeps so the current row's opened time is
    // distinguishable from them; the current sweep must win, not sweep 1.
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE erasure_condition SET opened_at='2020-01-01T00:00:00Z' WHERE operation_id=?1 AND sweep<3",
            [crate::codec::encode_id(current.operation.as_raw())],
        )
        .unwrap();
    }
    let current_row = async |store: &Store| -> CurrentErasureCondition {
        store.current_erasure_conditions(None, 10).await.unwrap()[0].clone()
    };
    assert_eq!(current_row(&store).await.condition, current.condition());
    drop(store);
    let reopened = Store::open(&path).await.unwrap();
    let after_restart = current_row(&reopened).await;
    assert_eq!(
        after_restart.condition,
        current.condition(),
        "the current sweep, not a historical one"
    );
    assert!(
        after_restart.opened_at.as_datetime()
            > WallClockWithTz::parse_rfc3339("2020-01-01T00:00:00Z")
                .unwrap()
                .as_datetime()
    );
}

#[tokio::test]
async fn an_orphan_condition_fails_closed_instead_of_reading_as_empty() {
    let store = open_memory().await.unwrap();
    {
        let conn = store.conn.lock().unwrap();
        // A condition row with no deletion_operation parent is torn canonical
        // state: the active set must refuse, never report an authoritative
        // empty set for it.
        conn.execute(
            "INSERT INTO erasure_condition (operation_id,sweep,opened_at) VALUES (?1,1,'2026-09-17T00:00:00Z')",
            [crate::codec::encode_id(RawId::new())],
        )
        .unwrap();
    }
    assert_eq!(
        store.current_erasure_conditions(None, 10).await,
        Err(PreservationTechnicalError::CorruptState)
    );
    assert_eq!(
        store.unfinished_deletions(None, 10).await,
        Err(PreservationTechnicalError::CorruptState)
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

fn complete_operation_fixture(store: &Store, current: DeletionOperationRef) {
    let id = crate::codec::encode_id(current.operation.as_raw());
    let conn = store.conn.lock().unwrap();
    conn.execute(
        "DELETE FROM deletion_search_material WHERE operation_id=?1",
        [&id],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM deletion_semantic_hint WHERE operation_id=?1",
        [&id],
    )
    .unwrap();
    conn.execute(
        "UPDATE erasure_condition SET closed_at='2026-09-17T01:00:00Z' WHERE operation_id=?1 AND sweep=?2",
        params![id, current.sweep.as_u64() as i64],
    )
    .unwrap();
    conn.execute(
        "UPDATE deletion_operation SET phase='completed' WHERE operation_id=?1",
        [&id],
    )
    .unwrap();
}

#[tokio::test]
async fn covering_condition_ignores_completed_history_and_uses_the_source_index() {
    let store = open_memory().await.unwrap();
    let source = RawId::new();
    let probe = crate::codec::encode_id(source);
    // Seed many completed historical operations naming the probed source,
    // each consistent with the validate rules (closed current condition,
    // protected material and hints removed): they must never be enumerated
    // or validated by the covering read.
    {
        let conn = store.conn.lock().unwrap();
        for _ in 0..250 {
            let id = crate::codec::encode_id(RawId::new());
            conn.execute(
                "INSERT INTO deletion_operation (operation_id,sweep,phase,purpose,started_at) VALUES (?1,1,'completed','privacy','2026-01-01T00:00:00Z')",
                [&id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO erasure_condition (operation_id,sweep,opened_at,closed_at) VALUES (?1,1,'2026-01-01T00:00:00Z','2026-09-17T01:00:00Z')",
                [&id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO erasure_condition_source (operation_id,sweep,source) VALUES (?1,1,?2)",
                params![id, probe],
            )
            .unwrap();
        }
    }
    // With only completed history naming the source, the authoritative
    // answer is the empty set — not corruption, not coverage.
    assert_eq!(
        crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe).unwrap(),
        None
    );
    // An unfinished operation covering the same source still wins, no
    // matter how much completed history names it.
    let active = admit(&store, "current-target", vec![source]).await;
    assert_eq!(
        crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe).unwrap(),
        Some(active.condition())
    );
    // Completing the current operation returns the answer to empty again.
    complete_operation_fixture(&store, active);
    assert_eq!(
        crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe).unwrap(),
        None
    );
    // Structurally pin boundedness: the exact production statements must
    // reach erasure_condition_source through the source index, never a full
    // scan, so future edits cannot silently reintroduce the historical walk.
    for statement in [
        crate::preservation::COVERING_CANDIDATE_SQL,
        crate::preservation::COVERING_TORN_PROBE_SQL,
    ] {
        let plan: Vec<String> = {
            let conn = store.conn.lock().unwrap();
            let mut explained = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {statement}"))
                .unwrap();
            explained
                .query_map([probe.as_str()], |row| row.get(3))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert!(!plan.is_empty(), "missing plan for {statement}");
        assert!(
            plan.iter()
                .any(|line| line.contains("idx_erasure_condition_source_source")),
            "covering lookup must use the source index, plan: {plan:?}"
        );
        assert!(
            !plan.iter().any(|line| {
                // "SCAN CONSTANT ROW" is the single constant output row of the
                // OR-combined EXISTS probe, not a table scan.
                line.contains("SCAN") && !line.contains("SCAN CONSTANT ROW")
            }),
            "covering lookup must not scan, plan: {plan:?}"
        );
    }
}

#[tokio::test]
async fn covering_condition_still_fails_closed_on_torn_current_state_for_the_source() {
    // The bounded rewrite must preserve the fail-closed contract: torn
    // current state relevant to the probed source never reads as Clear.
    let store = open_memory().await.unwrap();
    let source = RawId::new();
    let probe = crate::codec::encode_id(source);
    let current = admit(&store, "torn-target", vec![source]).await;
    let id = crate::codec::encode_id(current.operation.as_raw());
    // An unfinished operation whose current condition was closed early is
    // torn, even though no open current condition covers the source.
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE erasure_condition SET closed_at='2026-09-17T01:00:00Z' WHERE operation_id=?1",
            [&id],
        )
        .unwrap();
    assert_eq!(
        crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe),
        Err(PreservationTechnicalError::CorruptState)
    );
    // An orphan source row with no parent condition is torn on its own.
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO erasure_condition_source (operation_id,sweep,source) VALUES (?1,99,?2)",
            params![crate::codec::encode_id(RawId::new()), probe],
        )
        .unwrap();
    assert_eq!(
        crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe),
        Err(PreservationTechnicalError::CorruptState)
    );
}

#[tokio::test]
async fn held_unavailable_next_sweep_is_rejected_and_only_resume_releases_the_hold() {
    // The hold signals an unrecovered impediment: advancing the generation
    // underneath it would publish a new current condition without a recovery
    // decision and silently flip the phase that A4/A5 participant sweep
    // tracking observes. NextSweep is therefore rejected with no writes and
    // no generation advance — symmetric with the GenerationExhausted
    // rejection — and an explicit Resume stays the only path back to Active.
    let store = open_memory().await.unwrap();
    let current = admit(&store, "held-target", vec![]).await;
    let DeletionLifecycleOutcome::Applied(held) = store
        .change_deletion_lifecycle(current, DeletionLifecycleChange::Hold)
        .await
        .unwrap()
    else {
        panic!("hold must apply");
    };
    assert_eq!(held, current);
    assert_eq!(
        store
            .change_deletion_lifecycle(held, DeletionLifecycleChange::NextSweep)
            .await
            .unwrap(),
        DeletionLifecycleOutcome::Held(DeletionHoldReason::Unavailable)
    );
    let rows = store.unfinished_deletions(None, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].current, held);
    assert_eq!(rows[0].phase, DeletionOperationPhase::Held);
    assert_eq!(rows[0].hold, Some(DeletionHoldReason::Unavailable));
    let DeletionLifecycleOutcome::Applied(resumed) = store
        .change_deletion_lifecycle(held, DeletionLifecycleChange::Resume)
        .await
        .unwrap()
    else {
        panic!("resume must apply");
    };
    assert_eq!(resumed, current);
    let DeletionLifecycleOutcome::Applied(next) = store
        .change_deletion_lifecycle(resumed, DeletionLifecycleChange::NextSweep)
        .await
        .unwrap()
    else {
        panic!("the generation must advance after resume");
    };
    assert_eq!(next.sweep.as_u64(), 2);
    let rows = store.unfinished_deletions(None, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].current, next);
    assert_eq!(rows[0].phase, DeletionOperationPhase::Active);
    assert_eq!(rows[0].hold, None);
}

#[tokio::test]
async fn finalizing_next_sweep_returns_to_active_on_remainder_and_stays_finalizing_without_material()
 {
    // Designed remainder path (§5: Finalizing--remainder-->Active): with
    // search material present a new sweep generation issues and the operation
    // returns to Active; once the material is destroyed the operation can no
    // longer restart and stays Finalizing.
    let store = open_memory().await.unwrap();
    let current = admit(&store, "remainder-target", vec![]).await;
    let id = crate::codec::encode_id(current.operation.as_raw());
    // Only fixture SQL can enter finalizing: no A1 production completion authority.
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE deletion_operation SET phase='finalizing' WHERE operation_id=?1",
            [&id],
        )
        .unwrap();
    let DeletionLifecycleOutcome::Applied(next) = store
        .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
        .await
        .unwrap()
    else {
        panic!("remainder must reopen a new sweep");
    };
    assert_eq!(next.sweep.as_u64(), 2);
    let rows = store.unfinished_deletions(None, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].current, next);
    assert_eq!(rows[0].phase, DeletionOperationPhase::Active);
    assert_eq!(rows[0].hold, None);
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE deletion_operation SET phase='finalizing' WHERE operation_id=?1",
            [&id],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM deletion_search_material WHERE operation_id=?1",
            [&id],
        )
        .unwrap();
    }
    assert_eq!(
        store
            .change_deletion_lifecycle(next, DeletionLifecycleChange::NextSweep)
            .await
            .unwrap(),
        DeletionLifecycleOutcome::Finalizing
    );
    let rows = store.unfinished_deletions(None, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].current, next);
    assert_eq!(rows[0].phase, DeletionOperationPhase::Finalizing);
}
