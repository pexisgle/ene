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
        // The participant rows track the operation's current sweep, so the
        // fixture moves them together with the operation; the reconciliation
        // cursors are part of that same current-sweep state.
        conn.execute(
            "UPDATE deletion_participant SET sweep=?2 WHERE operation_id=?1",
            params![id, i64::MAX],
        )
        .unwrap();
        conn.execute(
            "UPDATE deletion_reconciliation SET sweep=?2 WHERE operation_id=?1",
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
    // The early closure is canonical corruption; restoring it to open lets
    // the sealed A5 completion boundary run steps 1-6 for this operation.
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE erasure_condition SET closed_at=NULL WHERE operation_id=?1",
            [&id],
        )
        .unwrap();
    }
    complete_via_a5(&store, current).await;
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

/// Completes one operation through the sealed A5 boundary.
///
/// The current sweep's exhaustive covered-source reconciliation is driven to
/// completion first, then every required participant is verified through the
/// canonical completion API, and the finalizing transition and the completion
/// commit run. The system-wide mechanical probe must be clean: a call site
/// whose fixture still stores the target must drive that owner's real sweep
/// first, exactly as the production fan-out would.
pub(super) async fn complete_via_a5(store: &Store, current: DeletionOperationRef) {
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
    assert_eq!(
        store
            .complete_deletion_finalizing(current)
            .await
            .expect("the completion commit must answer"),
        DeletionFinalizationOutcome::Completed
    );
}

#[tokio::test]
async fn sweep_advance_moves_source_correlation_to_current_and_deletes_the_old_sweep() {
    // Requirement 1: sweep 1's source correlation moves to sweep 2 and the
    // old sweep keeps no row — unfinished source correlations live only in
    // the current sweep.
    let store = open_memory().await.unwrap();
    let source = RawId::new();
    let probe = crate::codec::encode_id(source);
    let current = admit(&store, "sweep-move", vec![source]).await;
    let id = crate::codec::encode_id(current.operation.as_raw());
    {
        let guard = store.conn.lock().unwrap();
        let count: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM erasure_condition_source WHERE operation_id=?1 AND sweep=1 AND source=?2",
                params![id, probe],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
    let DeletionLifecycleOutcome::Applied(next) = store
        .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
        .await
        .unwrap()
    else {
        panic!("the generation must advance");
    };
    assert_eq!(next.sweep.as_u64(), 2);
    {
        let guard = store.conn.lock().unwrap();
        let old: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM erasure_condition_source WHERE operation_id=?1 AND sweep=1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        let current_count: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM erasure_condition_source WHERE operation_id=?1 AND sweep=2 AND source=?2",
                params![id, probe],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((old, current_count), (0, 1));
        // Historical erasure_condition rows remain as lifecycle/history.
        let conditions: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM erasure_condition WHERE operation_id=?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(conditions, 2);
    }
    // Requirement 5: the current unfinished operation covering the source
    // resolves to the current ErasureConditionRef.
    assert_eq!(
        crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe).unwrap(),
        Some(next.condition())
    );
}

#[tokio::test]
async fn covering_condition_ignores_completed_history_and_uses_the_source_index() {
    let store = open_memory().await.unwrap();
    let source = RawId::new();
    let probe = crate::codec::encode_id(source);
    // Requirement 4: build N completed histories that once covered the same
    // source through the production admission path, then completed through the
    // A5 completion boundary (closed current condition, material/hints/source
    // rows removed). History remains as closed erasure_condition rows, but the
    // hot-path input — source rows naming this source — must not grow with
    // the history count. This durable row invariant (not EXPLAIN alone) pins
    // boundedness.
    const HISTORIES: usize = 250;
    for index in 0..HISTORIES {
        let admitted = admit(&store, &format!("history-target-{index}"), vec![source]).await;
        complete_via_a5(&store, admitted).await;
    }
    let source_rows: i64 = {
        let guard = store.conn.lock().unwrap();
        guard
            .query_row(
                "SELECT COUNT(*) FROM erasure_condition_source WHERE source=?1",
                [&probe],
                |r| r.get(0),
            )
            .unwrap()
    };
    let condition_rows = task_table_count(&store, "erasure_condition");
    assert_eq!(
        source_rows, 0,
        "completed history must leave no source rows"
    );
    assert_eq!(
        condition_rows, HISTORIES as i64,
        "historical conditions remain as lifecycle/history"
    );
    // With only completed history, the authoritative answer is the empty
    // set — not corruption, not coverage.
    assert_eq!(
        crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe).unwrap(),
        None
    );
    // An unfinished operation covering the same source still wins, no
    // matter how much completed history exists — and adds exactly one
    // source row, not HISTORIES + 1.
    let active = admit(&store, "current-target", vec![source]).await;
    let source_rows: i64 = {
        let guard = store.conn.lock().unwrap();
        guard
            .query_row(
                "SELECT COUNT(*) FROM erasure_condition_source WHERE source=?1",
                [&probe],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(source_rows, 1);
    assert_eq!(
        crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe).unwrap(),
        Some(active.condition())
    );
    // Completing the current operation returns the answer to empty again
    // (requirement 6: no permanent ban) and restores the zero-row invariant.
    complete_via_a5(&store, active).await;
    let source_rows: i64 = {
        let guard = store.conn.lock().unwrap();
        guard
            .query_row(
                "SELECT COUNT(*) FROM erasure_condition_source WHERE source=?1",
                [&probe],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(source_rows, 0);
    assert_eq!(
        crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe).unwrap(),
        None
    );
    // The same source can cover again after completion: closed conditions
    // never become a keyword ban.
    let again = admit(&store, "current-target-again", vec![source]).await;
    assert_eq!(
        crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe).unwrap(),
        Some(again.condition())
    );
    // Structurally pin the index path as a secondary guard: the exact
    // production statements must reach erasure_condition_source through the
    // source index, never a full scan, so future edits cannot silently
    // reintroduce the historical walk. Boundedness itself is proved by the
    // row counts above, not by this plan alone.
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
async fn completed_operation_with_remaining_source_row_fails_closed() {
    // Requirement 3/6: the A5 completion removes source rows; any remaining
    // source row for a completed operation is canonical corruption.
    let store = open_memory().await.unwrap();
    let source = RawId::new();
    let probe = crate::codec::encode_id(source);
    let current = admit(&store, "completed-leftover", vec![source]).await;
    let id = crate::codec::encode_id(current.operation.as_raw());
    complete_via_a5(&store, current).await;
    {
        // Deliberately re-insert the source row: this is the corruption the A5
        // invariant forbids (a completed operation keeps zero source rows).
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO erasure_condition_source (operation_id,sweep,source) VALUES (?1,?2,?3)",
            params![id, current.sweep.as_u64() as i64, probe],
        )
        .unwrap();
    }
    assert_eq!(
        crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe),
        Err(PreservationTechnicalError::CorruptState)
    );
    assert_eq!(
        store.unfinished_deletions(None, 10).await.unwrap().len(),
        0,
        "completed operations leave the unfinished set"
    );
}

#[tokio::test]
async fn covering_condition_still_fails_closed_on_torn_current_state_for_the_source() {
    // The current-sweep canonical form must preserve the fail-closed
    // contract: every torn shape below is canonical corruption and never
    // reads as Clear, while the probe stays source-scoped (no historical
    // operation walk in the provider-send transaction).
    // 1. Unfinished current condition closed early.
    {
        let store = open_memory().await.unwrap();
        let source = RawId::new();
        let probe = crate::codec::encode_id(source);
        let current = admit(&store, "torn-closed", vec![source]).await;
        let id = crate::codec::encode_id(current.operation.as_raw());
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
    }
    // 2. Source row with no parent condition.
    // 3. Source row with no operation row.
    {
        let store = open_memory().await.unwrap();
        let source = RawId::new();
        let probe = crate::codec::encode_id(source);
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
            Err(PreservationTechnicalError::CorruptState),
            "orphan source without condition or operation must fail closed"
        );
    }
    // 4. Canonical torn advance: operation current sweep = 2, sweep 1 keeps
    // the source correlation while sweep 2 has the current condition without
    // the inherited correlation (requirement 2). Source coverage is
    // cumulative across the advance, so the old-only row is corruption.
    {
        let store = open_memory().await.unwrap();
        let source = RawId::new();
        let probe = crate::codec::encode_id(source);
        let current = admit(&store, "torn-inherit", vec![source]).await;
        let DeletionLifecycleOutcome::Applied(next) = store
            .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
            .await
            .unwrap()
        else {
            panic!("the generation must advance");
        };
        assert_eq!(next.sweep.as_u64(), 2);
        let id = crate::codec::encode_id(next.operation.as_raw());
        {
            let conn = store.conn.lock().unwrap();
            // Simulate the torn write: drop the inherited current row and
            // leave only the old-sweep correlation behind.
            conn.execute(
                "DELETE FROM erasure_condition_source WHERE operation_id=?1 AND sweep=2",
                [&id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO erasure_condition_source (operation_id,sweep,source) VALUES (?1,1,?2)",
                params![id, probe],
            )
            .unwrap();
        }
        assert_eq!(
            crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe),
            Err(PreservationTechnicalError::CorruptState),
            "old-sweep-only correlation without current inheritance must fail closed"
        );
        // The per-operation owner read fails closed on the same shape.
        assert_eq!(
            store.unfinished_deletions(None, 10).await,
            Err(PreservationTechnicalError::CorruptState)
        );
    }
    // 5. Unfinished source row pointing past the operation's current sweep,
    // and an unfinished operation missing its current condition row.
    {
        let store = open_memory().await.unwrap();
        let source = RawId::new();
        let probe = crate::codec::encode_id(source);
        let current = admit(&store, "torn-future", vec![]).await;
        let id = crate::codec::encode_id(current.operation.as_raw());
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO erasure_condition (operation_id,sweep,opened_at) VALUES (?1,2,'2026-09-17T00:00:00Z')",
                [&id],
            )
            .unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO erasure_condition_source (operation_id,sweep,source) VALUES (?1,2,?2)",
                params![id, probe],
            )
            .unwrap();
        assert_eq!(
            crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe),
            Err(PreservationTechnicalError::CorruptState),
            "a future-sweep source row must fail closed"
        );
    }
    {
        let store = open_memory().await.unwrap();
        let source = RawId::new();
        let probe = crate::codec::encode_id(source);
        let current = admit(&store, "torn-missing", vec![source]).await;
        let id = crate::codec::encode_id(current.operation.as_raw());
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM erasure_condition WHERE operation_id=?1 AND sweep=1",
                [&id],
            )
            .unwrap();
        assert_eq!(
            crate::preservation::covering_condition(&store.conn.lock().unwrap(), &probe),
            Err(PreservationTechnicalError::CorruptState),
            "a source row whose current condition is missing must fail closed"
        );
    }
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
    // Only fixture SQL can enter finalizing; the durable premise (all
    // participants verified for the current sweep) must hold for `validate`.
    mark_all_verified(&store, current);
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
        // The new generation resets the walk; a finalizing fixture on it must
        // re-establish the same durable premise the transition would.
        reconcile_to_complete(&store, next).await;
        mark_all_verified(&store, next);
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

#[tokio::test]
async fn admission_snapshots_the_required_participants_and_restart_keeps_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("participant-snapshot.db");
    let store = Store::open(&path).await.unwrap();
    let participants = vec![
        ParticipantOwnerRef::Companion,
        ParticipantOwnerRef::Learning,
        ParticipantOwnerRef::ClientIncarnation(RawId::new()),
    ];
    let current = match store
        .start_targeted_deletion(
            command_with("snapshot-target", vec![], participants).confirmed_for_tests(),
        )
        .await
        .unwrap()
    {
        StartTargetedDeletionOutcome::Started(current) => current,
        other => panic!("unexpected admission: {other:?}"),
    };
    let page = store
        .deletion_participants(current.operation, None, 100)
        .await
        .unwrap();
    assert_eq!(page.len(), 3, "the whole required set is snapshotted");
    assert_eq!(
        page.iter()
            .map(|record| record.participant.owner)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3,
        "the snapshot keeps the exact owners without duplicates"
    );
    for record in &page {
        assert_eq!(record.participant.operation, current.operation);
        assert_eq!(
            record.progress,
            ParticipantProgress::Pending,
            "admission must not claim any participant progress"
        );
        assert_eq!(record.erased_count, 0);
        assert_eq!(record.remainder_count, 0);
        assert!(record.reported_at.is_none());
    }
    // Keyset paging is bounded at the storage boundary.
    let first = store
        .deletion_participants(current.operation, None, 2)
        .await
        .unwrap();
    assert_eq!(first.len(), 2);
    let rest = store
        .deletion_participants(current.operation, Some(first[1].participant.owner), 100)
        .await
        .unwrap();
    assert_eq!(rest.len(), 1);
    drop(store);
    let reopened = Store::open(&path).await.unwrap();
    let after_restart = reopened
        .deletion_participants(current.operation, None, 100)
        .await
        .unwrap();
    assert_eq!(
        after_restart, page,
        "the required set and progress are durable, not memory defaults"
    );
}

#[tokio::test]
async fn an_empty_or_duplicate_required_participant_set_is_refused() {
    let store = open_memory().await.unwrap();
    assert_eq!(
        store
            .start_targeted_deletion(
                command_with("no-participants", vec![], vec![]).confirmed_for_tests()
            )
            .await,
        Err(PreservationTechnicalError::InvalidParticipantSet),
        "an operation with no required participant could never be verified"
    );
    assert_eq!(
        store
            .start_targeted_deletion(
                command_with(
                    "duplicate-participants",
                    vec![],
                    vec![
                        ParticipantOwnerRef::Companion,
                        ParticipantOwnerRef::Companion
                    ],
                )
                .confirmed_for_tests(),
            )
            .await,
        Err(PreservationTechnicalError::InvalidParticipantSet)
    );
    assert_eq!(task_table_count(&store, "deletion_operation"), 0);
    assert_eq!(task_table_count(&store, "deletion_participant"), 0);
}

#[tokio::test]
async fn duplicate_admission_requires_the_existing_snapshot_to_cover_the_request() {
    let store = open_memory().await.unwrap();
    let source = RawId::new();
    let current = admit(&store, "covered-scope", vec![source]).await;
    // The fixture snapshot covers Companion and Learning. A duplicate request
    // with a subset is idempotent; a request needing an owner outside the
    // durable snapshot is a live-operation conflict, never a silent widening.
    let subset = command_with(
        "covered-scope",
        vec![source],
        vec![ParticipantOwnerRef::Companion],
    )
    .confirmed_for_tests();
    assert_eq!(
        store.start_targeted_deletion(subset).await.unwrap(),
        StartTargetedDeletionOutcome::AlreadyCoveredBy(current)
    );
    let wider = command_with(
        "covered-scope",
        vec![source],
        vec![
            ParticipantOwnerRef::Companion,
            ParticipantOwnerRef::ClientIncarnation(RawId::new()),
        ],
    )
    .confirmed_for_tests();
    assert_eq!(
        store.start_targeted_deletion(wider).await.unwrap(),
        StartTargetedDeletionOutcome::HeldByOperation(current)
    );
    let rows = store
        .deletion_participants(current.operation, None, 100)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "the snapshot is not widened in place");
}

#[tokio::test]
async fn participant_progress_is_durable_and_verified_is_terminal_for_the_sweep() {
    let store = open_memory().await.unwrap();
    let current = admit(&store, "participant-progress", vec![]).await;
    let condition = current.condition();
    let owner = ParticipantOwnerRef::Companion;
    assert_eq!(
        store
            .begin_participant_demand(condition, owner)
            .await
            .unwrap(),
        ParticipantDemandOutcome::Marked(ParticipantProgress::Running {
            sweep: current.sweep
        })
    );
    // The same command is idempotent: a retry after a crash re-marks Running.
    assert_eq!(
        store
            .begin_participant_demand(condition, owner)
            .await
            .unwrap(),
        ParticipantDemandOutcome::Marked(ParticipantProgress::Running {
            sweep: current.sweep
        })
    );
    let at = WallClockWithTz::now();
    assert_eq!(
        store
            .record_participant_completion(ParticipantCompletionFact::more_work(
                condition, owner, 2, 5, at,
            ))
            .await
            .unwrap(),
        ParticipantCompletionOutcome::Recorded(ParticipantProgress::Running {
            sweep: current.sweep
        })
    );
    assert_eq!(
        store
            .record_participant_completion(ParticipantCompletionFact::local_complete(
                condition, owner, 5, 0, at,
            ))
            .await
            .unwrap(),
        ParticipantCompletionOutcome::Recorded(ParticipantProgress::LocalComplete {
            sweep: current.sweep,
        })
    );
    // Local completion is not verification: the participant row is still not
    // verified, so nothing may treat this sweep as finished.
    let local = participant_row(&store, current.operation, owner).await;
    assert_eq!(
        local.progress,
        ParticipantProgress::LocalComplete {
            sweep: current.sweep
        }
    );
    assert!(!local.progress.is_verified());
    assert_eq!(
        store
            .record_participant_completion(ParticipantCompletionFact::verified(
                condition, owner, 5, at,
            ))
            .await
            .unwrap(),
        ParticipantCompletionOutcome::Recorded(ParticipantProgress::Verified {
            sweep: current.sweep
        })
    );
    let verified = participant_row(&store, current.operation, owner).await;
    assert_eq!(
        verified.progress,
        ParticipantProgress::Verified {
            sweep: current.sweep
        }
    );
    assert_eq!(verified.erased_count, 5);
    assert_eq!(verified.remainder_count, 0);
    assert_eq!(verified.reported_at, Some(at));
    // A repeated verification is idempotent, and verification is terminal for
    // the sweep: a later downgrading report cannot reopen it.
    assert_eq!(
        store
            .record_participant_completion(ParticipantCompletionFact::verified(
                condition, owner, 5, at,
            ))
            .await
            .unwrap(),
        ParticipantCompletionOutcome::Recorded(ParticipantProgress::Verified {
            sweep: current.sweep
        })
    );
    assert_eq!(
        store
            .record_participant_completion(ParticipantCompletionFact::more_work(
                condition, owner, 0, 9, at,
            ))
            .await
            .unwrap(),
        ParticipantCompletionOutcome::AlreadyVerified
    );
    assert_eq!(
        store
            .begin_participant_demand(condition, owner)
            .await
            .unwrap(),
        ParticipantDemandOutcome::AlreadyVerified,
        "a verified participant is never re-demanded"
    );
    let unchanged = participant_row(&store, current.operation, owner).await;
    assert_eq!(
        unchanged.progress,
        ParticipantProgress::Verified {
            sweep: current.sweep
        }
    );
    assert_eq!(unchanged.erased_count, 5);
}

/// Reads one participant row for a store test.
async fn participant_row(
    store: &Store,
    operation: DeletionOperationId,
    owner: ParticipantOwnerRef,
) -> DeletionParticipantRecord {
    store
        .deletion_participants(operation, None, 100)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.participant.owner == owner)
        .expect("the required participant row must exist")
}

#[tokio::test]
async fn holds_are_distinct_durable_incomplete_outcomes() {
    let store = open_memory().await.unwrap();
    let participants = vec![
        ParticipantOwnerRef::Companion,
        ParticipantOwnerRef::Learning,
        ParticipantOwnerRef::Task,
    ];
    let current = match store
        .start_targeted_deletion(
            command_with("held-target", vec![], participants).confirmed_for_tests(),
        )
        .await
        .unwrap()
    {
        StartTargetedDeletionOutcome::Started(current) => current,
        other => panic!("unexpected admission: {other:?}"),
    };
    let condition = current.condition();
    let at = WallClockWithTz::now();
    // A participant that erased some items and then hit a hold keeps its last
    // reported counts: a hold carries no usable counts of its own.
    assert_eq!(
        store
            .record_participant_completion(ParticipantCompletionFact::more_work(
                condition,
                ParticipantOwnerRef::Companion,
                4,
                2,
                at,
            ))
            .await
            .unwrap(),
        ParticipantCompletionOutcome::Recorded(ParticipantProgress::Running {
            sweep: current.sweep,
        })
    );
    for (owner, reason) in [
        (
            ParticipantOwnerRef::Companion,
            ParticipantHoldClass::Unavailable,
        ),
        (ParticipantOwnerRef::Learning, ParticipantHoldClass::Failed),
        (ParticipantOwnerRef::Task, ParticipantHoldClass::Unsupported),
    ] {
        assert_eq!(
            store
                .record_participant_completion(ParticipantCompletionFact::held(
                    condition, owner, reason, at,
                ))
                .await
                .unwrap(),
            ParticipantCompletionOutcome::Recorded(ParticipantProgress::Held {
                sweep: current.sweep,
                reason,
            })
        );
    }
    let rows = store
        .deletion_participants(current.operation, None, 100)
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);
    for record in &rows {
        // Held is durable, distinct, and never verified: no completion
        // candidate can be derived while a participant is held (§10).
        assert!(record.progress.hold_reason().is_some());
        assert!(!record.progress.is_verified());
    }
    let companion = rows
        .iter()
        .find(|record| record.participant.owner == ParticipantOwnerRef::Companion)
        .expect("the companion row must exist");
    assert_eq!(
        (companion.erased_count, companion.remainder_count),
        (4, 2),
        "a hold preserves the last reported counts"
    );
    // A fact for an owner outside the durable snapshot never registers lazily.
    assert_eq!(
        store
            .record_participant_completion(ParticipantCompletionFact::verified(
                condition,
                ParticipantOwnerRef::Presence,
                1,
                at,
            ))
            .await
            .unwrap(),
        ParticipantCompletionOutcome::NotRequired
    );
    assert_eq!(
        store
            .begin_participant_demand(condition, ParticipantOwnerRef::Presence)
            .await
            .unwrap(),
        ParticipantDemandOutcome::NotRequired
    );
    assert_eq!(
        store
            .deletion_participants(current.operation, None, 100)
            .await
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn stale_generation_reports_and_demands_never_update_current_state() {
    let store = open_memory().await.unwrap();
    let first = admit(&store, "stale-generation", vec![]).await;
    let DeletionLifecycleOutcome::Applied(second) = store
        .change_deletion_lifecycle(first, DeletionLifecycleChange::NextSweep)
        .await
        .unwrap()
    else {
        panic!("the generation must advance");
    };
    // A new sweep reopens every participant: old-sweep verification never
    // counts for the new one, while the owner set itself is unchanged.
    let reset = store
        .deletion_participants(second.operation, None, 100)
        .await
        .unwrap();
    assert_eq!(reset.len(), fixture_participants().len());
    for record in &reset {
        assert_eq!(record.progress, ParticipantProgress::Pending);
        assert!(record.reported_at.is_none());
    }
    assert_eq!(
        store
            .begin_participant_demand(second.condition(), ParticipantOwnerRef::Companion)
            .await
            .unwrap(),
        ParticipantDemandOutcome::Marked(ParticipantProgress::Running {
            sweep: second.sweep
        })
    );
    let at = WallClockWithTz::now();
    assert_eq!(
        store
            .record_participant_completion(ParticipantCompletionFact::verified(
                first.condition(),
                ParticipantOwnerRef::Companion,
                9,
                at,
            ))
            .await
            .unwrap(),
        ParticipantCompletionOutcome::StaleSweep,
        "an older generation must not advance the current sweep"
    );
    assert_eq!(
        store
            .begin_participant_demand(first.condition(), ParticipantOwnerRef::Companion)
            .await
            .unwrap(),
        ParticipantDemandOutcome::StaleSweep
    );
    let current = participant_row(&store, second.operation, ParticipantOwnerRef::Companion).await;
    assert_eq!(
        current.progress,
        ParticipantProgress::Running {
            sweep: second.sweep
        }
    );
    assert_eq!(current.erased_count, 0);
    assert_eq!(current.remainder_count, 0);
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

#[tokio::test]
async fn material_read_returns_the_protected_target_and_current_sweep_sources() {
    let store = open_memory().await.unwrap();
    let source = RawId::new();
    let other = RawId::new();
    let current = admit(&store, "material-target", vec![source, other]).await;
    let DeletionMaterialOutcome::Material(material) = store
        .deletion_operation_material(current.operation)
        .await
        .unwrap()
    else {
        panic!("an active operation keeps its material");
    };
    let MechanicalDeletionTarget::ExactText(exact) = &material.target().mechanical;
    assert_eq!(exact.expose_for_erasure(), "material-target");
    assert_eq!(material.sources().len(), 2);
    assert!(material.sources().contains(&source));
    assert!(material.sources().contains(&other));
    let DeletionLifecycleOutcome::Applied(next) = store
        .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
        .await
        .unwrap()
    else {
        panic!("the generation must advance");
    };
    // The correlation moves to the current sweep, so the material read stays
    // complete after a generation advance.
    let DeletionMaterialOutcome::Material(next_material) = store
        .deletion_operation_material(next.operation)
        .await
        .unwrap()
    else {
        panic!("the new sweep keeps the material");
    };
    assert_eq!(next_material.sources().len(), 2);
    // A finalizing operation that already ran its material wipe reads as
    // Destroyed, never as corrupt and never as protected material.
    {
        // The generation advance reset the walk; the fabricated finalizing
        // shape must re-establish the whole durable premise.
        reconcile_to_complete(&store, next).await;
        mark_all_verified(&store, next);
        let conn = store.conn.lock().unwrap();
        let id = crate::codec::encode_id(next.operation.as_raw());
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
            .deletion_operation_material(next.operation)
            .await
            .unwrap(),
        DeletionMaterialOutcome::Destroyed
    );
    assert_eq!(
        store
            .deletion_operation_material(DeletionOperationId::from_raw(RawId::new()))
            .await
            .unwrap(),
        DeletionMaterialOutcome::Missing
    );
}

#[tokio::test]
async fn missing_or_foreign_participant_rows_fail_closed() {
    // 1. An operation whose required participant snapshot disappeared is torn
    // canonical state: the unfinished set, the current-condition set, and the
    // source-coverage hot path all fail closed instead of reading it as an
    // incomplete-but-valid operation.
    {
        let store = open_memory().await.unwrap();
        let source = RawId::new();
        let current = admit(&store, "missing-snapshot", vec![source]).await;
        let id = crate::codec::encode_id(current.operation.as_raw());
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM deletion_participant WHERE operation_id=?1",
                [&id],
            )
            .unwrap();
        assert_eq!(
            store.unfinished_deletions(None, 10).await,
            Err(PreservationTechnicalError::CorruptState)
        );
        assert_eq!(
            store.current_erasure_conditions(None, 10).await,
            Err(PreservationTechnicalError::CorruptState)
        );
        assert_eq!(
            crate::preservation::covering_condition(
                &store.conn.lock().unwrap(),
                &crate::codec::encode_id(source)
            ),
            Err(PreservationTechnicalError::CorruptState)
        );
        assert_eq!(
            store
                .deletion_participants(current.operation, None, 10)
                .await,
            Err(PreservationTechnicalError::CorruptState)
        );
    }
    // 2. A participant row outside the operation's current sweep cannot be
    // read as current progress.
    {
        let store = open_memory().await.unwrap();
        let current = admit(&store, "foreign-sweep", vec![]).await;
        let id = crate::codec::encode_id(current.operation.as_raw());
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE deletion_participant SET sweep=sweep+1 WHERE operation_id=?1",
                [&id],
            )
            .unwrap();
        assert_eq!(
            store.unfinished_deletions(None, 10).await,
            Err(PreservationTechnicalError::CorruptState)
        );
        assert_eq!(
            store
                .deletion_participants(current.operation, None, 10)
                .await,
            Err(PreservationTechnicalError::CorruptState)
        );
    }
    // 3. An unknown operation is never an authoritative empty participant set.
    let store = open_memory().await.unwrap();
    let unknown = DeletionOperationId::from_raw(RawId::new());
    assert_eq!(
        store.deletion_participants(unknown, None, 10).await,
        Err(PreservationTechnicalError::UnknownOperation)
    );
    assert_eq!(
        store.deletion_participants(unknown, None, 0).await,
        Err(PreservationTechnicalError::InvalidLimit)
    );
}
