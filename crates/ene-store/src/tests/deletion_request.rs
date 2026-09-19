//! Stage 6 A1b: staged Targeted Deletion requests, the Host-local trusted
//! confirmation boundary, the surface mark, and the bounded status read.
//!
//! These tests drive the production store API. Fixture SQL appears only where
//! the slice under test cannot legally produce the state (terminal operation
//! phases belong to the A5 completion boundary, which A1/A1b do not expose).

use super::*;
use ene_preservation::*;

fn target(text: &str) -> TargetedDeletionTarget {
    TargetedDeletionTarget {
        mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(text.into())),
        semantic_hints: Vec::new(),
    }
}

async fn stage(
    store: &Store,
    text: &str,
    purpose: DeletionPurpose,
) -> StageTargetedDeletionRequestOutcome {
    store
        .stage_targeted_deletion(StageTargetedDeletionRequestCommand::new(
            target(text),
            purpose,
            WallClockWithTz::now(),
        ))
        .await
        .unwrap()
}

fn request_id(outcome: &StageTargetedDeletionRequestOutcome) -> DeletionRequestId {
    match outcome {
        StageTargetedDeletionRequestOutcome::Staged(request)
        | StageTargetedDeletionRequestOutcome::AlreadyStaged(request)
        | StageTargetedDeletionRequestOutcome::Confirmed(request) => *request,
        other => panic!("expected a staged request, got {other:?}"),
    }
}

fn exact_text(request: &TargetedDeletionRequest) -> &str {
    let MechanicalDeletionTarget::ExactText(material) = &request.target().mechanical;
    material.expose_for_erasure()
}

/// The current product surface's required owners for a fixture operation.
fn owners() -> Vec<ParticipantOwnerRef> {
    vec![
        ParticipantOwnerRef::Companion,
        ParticipantOwnerRef::Learning,
    ]
}

fn started(outcome: ConfirmTargetedDeletionOutcome) -> DeletionOperationRef {
    match outcome {
        ConfirmTargetedDeletionOutcome::Started(current) => current,
        other => panic!("expected a started operation, got {other:?}"),
    }
}

#[tokio::test]
async fn staging_publishes_nothing_and_is_idempotent() {
    let store = open_memory().await.unwrap();
    let before = store.deletion_surface_mark().await.unwrap();
    let staged = stage(&store, "private target", DeletionPurpose::Privacy).await;
    let request = request_id(&staged);
    assert!(
        matches!(staged, StageTargetedDeletionRequestOutcome::Staged(_)),
        "a new scope stages"
    );
    // Staging is inert: no operation, no condition, no coverage.
    assert!(
        store
            .current_erasure_conditions(None, 10)
            .await
            .unwrap()
            .is_empty(),
        "staging must publish no erasure condition"
    );
    assert!(store.deletion_status(None, 10).await.unwrap().is_empty());
    assert_eq!(task_table_count(&store, "deletion_request"), 1);

    // The same scope re-stages the same row; a second owner prompt is never
    // minted for one scope.
    let again = stage(&store, "private target", DeletionPurpose::Privacy).await;
    assert_eq!(
        request_id(&again),
        request,
        "a duplicate stage returns the same durable request"
    );
    assert_eq!(task_table_count(&store, "deletion_request"), 1);

    // The declared purpose is part of the scope, so another purpose is another
    // request (both await the Owner's confirmation).
    let security = stage(&store, "private target", DeletionPurpose::Security).await;
    assert_ne!(request_id(&security), request);
    assert_eq!(task_table_count(&store, "deletion_request"), 2);

    // A blank target is not an admissible scope.
    assert_eq!(
        stage(&store, "   ", DeletionPurpose::Privacy).await,
        StageTargetedDeletionRequestOutcome::NeedsClarification
    );
    assert_eq!(task_table_count(&store, "deletion_request"), 2);

    // The staged scope is readable through the Host-local pending read, with
    // the protected target for the Owner's review. The page is ordered by
    // canonical request identity, not by insertion order.
    let pending = store.pending_targeted_deletions(None, 100).await.unwrap();
    assert_eq!(pending.len(), 2);
    assert!(
        pending.iter().any(|entry| entry.request() == request
            && entry.purpose() == DeletionPurpose::Privacy
            && exact_text(entry) == "private target"),
        "the staged privacy scope is listed for the Owner: {pending:?}"
    );

    // Staging moves the surface mark; the pre-staging mark is stale.
    let after = store.deletion_surface_mark().await.unwrap();
    assert_ne!(before, after, "staging must move the surface mark");
    assert_eq!(
        after,
        store.deletion_surface_mark().await.unwrap(),
        "an unchanged surface keeps its mark"
    );
}

#[tokio::test]
async fn wire_paths_cannot_start_and_confirmation_is_single_use() {
    let store = open_memory().await.unwrap();
    // Two scopes for the same mechanical text (different declared purposes)
    // are staged before either is confirmed: this is how a duplicate scope can
    // still exist once one of them starts.
    let request = request_id(&stage(&store, "leaked key", DeletionPurpose::Security).await);
    let duplicate = request_id(&stage(&store, "leaked key", DeletionPurpose::Privacy).await);

    // Staging alone starts nothing, and the "confirmed" start refuses while no
    // Owner confirmation row exists (the wire has no path to one).
    assert_eq!(
        store
            .start_confirmed_targeted_deletion(request, owners())
            .await
            .unwrap(),
        StartTargetedDeletionOutcome::ConfirmationRequired
    );
    assert!(store.deletion_status(None, 10).await.unwrap().is_empty());

    // The sealed-confirmation entry refuses a command without confirmation.
    let command = StartTargetedDeletionCommand::new(
        target("leaked key"),
        DeletionPurpose::Security,
        WallClockWithTz::now(),
        Vec::new(),
        owners(),
    );
    assert_eq!(
        store.start_targeted_deletion(command).await.unwrap(),
        StartTargetedDeletionOutcome::ConfirmationRequired
    );

    // A confirmation for an unknown request changes nothing.
    assert_eq!(
        store
            .confirm_targeted_deletion(DeletionRequestId::from_raw(RawId::new()), owners())
            .await
            .unwrap(),
        ConfirmTargetedDeletionOutcome::Missing
    );

    // The Host-local confirmation admits exactly one canonical operation.
    let current = started(
        store
            .confirm_targeted_deletion(request, owners())
            .await
            .unwrap(),
    );
    assert_eq!(
        store.current_erasure_conditions(None, 10).await.unwrap()[0].condition,
        current.condition()
    );
    assert_eq!(task_table_count(&store, "deletion_operation"), 1);

    // Duplicate confirmation is idempotent: same request, same single
    // operation, no second condition.
    assert_eq!(
        store
            .confirm_targeted_deletion(request, owners())
            .await
            .unwrap(),
        ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(current)
    );
    assert_eq!(
        store
            .start_confirmed_targeted_deletion(request, owners())
            .await
            .unwrap(),
        StartTargetedDeletionOutcome::AlreadyCoveredBy(current)
    );
    assert_eq!(task_table_count(&store, "deletion_operation"), 1);
    assert_eq!(
        store
            .current_erasure_conditions(None, 100)
            .await
            .unwrap()
            .len(),
        1
    );

    // The request's provenance is durable on the operation row.
    {
        let guard = store.conn.lock().unwrap();
        let recorded: Option<String> = guard
            .query_row(
                "SELECT request_id FROM deletion_operation WHERE operation_id=?1",
                [crate::codec::encode_id(current.operation.as_raw())],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            recorded.as_deref(),
            Some(crate::codec::encode_id(request.as_raw())).as_deref()
        );
    }

    // Confirming the second staged scope whose mechanical target the operation
    // already covers never mints a second operation.
    assert_eq!(
        store
            .confirm_targeted_deletion(duplicate, owners())
            .await
            .unwrap(),
        ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(current)
    );
    assert_eq!(task_table_count(&store, "deletion_operation"), 1);
    assert_eq!(
        store.deletion_status(None, 10).await.unwrap().len(),
        1,
        "the status page still shows the one operation"
    );
    // A later duplicate intent for the covered target stages nothing new (the
    // canonical producer reports the covering operation).
    assert_eq!(
        stage(&store, "leaked key", DeletionPurpose::Security).await,
        StageTargetedDeletionRequestOutcome::AlreadyCoveredBy(current)
    );
    assert_eq!(task_table_count(&store, "deletion_request"), 2);
}

#[tokio::test]
async fn confirmed_request_survives_a_crash_between_confirmation_and_admission() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recovery.db");
    let store = Store::open(&path).await.unwrap();
    let request = request_id(&stage(&store, "recover me", DeletionPurpose::Privacy).await);
    // Fixture SQL: only the confirmation write can produce this state, and the
    // separation it models is exactly the crash window after that write.
    {
        let guard = store.conn.lock().unwrap();
        guard
            .execute(
                "INSERT INTO deletion_confirmation (request_id,confirmed_at) VALUES (?1,?2)",
                params![
                    crate::codec::encode_id(request.as_raw()),
                    WallClockWithTz::now().to_rfc3339()
                ],
            )
            .unwrap();
    }
    drop(store);

    let reopened = Store::open(&path).await.unwrap();
    // The staged row already carries the confirmation: staging reports the
    // owed canonical admission instead of minting a second request.
    assert_eq!(
        stage(&reopened, "recover me", DeletionPurpose::Privacy).await,
        StageTargetedDeletionRequestOutcome::Confirmed(request)
    );
    let started = reopened
        .start_confirmed_targeted_deletion(request, owners())
        .await
        .unwrap();
    let StartTargetedDeletionOutcome::Started(current) = started else {
        panic!("the durable confirmation must still admit after a restart: {started:?}");
    };
    assert_eq!(
        reopened.current_erasure_conditions(None, 10).await.unwrap()[0].condition,
        current.condition()
    );
    assert!(
        reopened
            .pending_targeted_deletions(None, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn status_read_covers_terminal_phases_and_torn_state_fails_closed() {
    let store = open_memory().await.unwrap();
    let request = request_id(&stage(&store, "terminal phases", DeletionPurpose::Privacy).await);
    let _ = store
        .confirm_targeted_deletion(request, owners())
        .await
        .unwrap();
    let operation = store.deletion_status(None, 10).await.unwrap()[0].current;

    // Only fixture SQL can enter finalizing/completed: the A5 completion
    // boundary owns those transitions, and A1/A1b expose no completion
    // authority. The status read must still show them when they exist.
    for (phase, expected) in [
        ("finalizing", DeletionOperationPhase::Finalizing),
        ("completed", DeletionOperationPhase::Completed),
    ] {
        {
            let guard = store.conn.lock().unwrap();
            let id = crate::codec::encode_id(operation.operation.as_raw());
            if phase == "finalizing" {
                guard
                    .execute(
                        "UPDATE deletion_operation SET phase=?1 WHERE operation_id=?2",
                        params![phase, id],
                    )
                    .unwrap();
            } else {
                // The A5 invariant: a completed operation keeps a closed
                // condition, no protected material, hints, or sources, and
                // every required participant verified for the final sweep.
                guard
                    .execute(
                        "UPDATE erasure_condition SET closed_at=?1 WHERE operation_id=?2",
                        params![WallClockWithTz::now().to_rfc3339(), id],
                    )
                    .unwrap();
                guard
                    .execute(
                        "DELETE FROM deletion_search_material WHERE operation_id=?1",
                        [&id],
                    )
                    .unwrap();
                guard
                    .execute(
                        "UPDATE deletion_participant SET state='verified', hold_class=NULL, remainder_count=0, reported_at=?1 WHERE operation_id=?2",
                        params![WallClockWithTz::now().to_rfc3339(), id],
                    )
                    .unwrap();
                guard
                    .execute(
                        "UPDATE deletion_operation SET phase='completed' WHERE operation_id=?1",
                        [&id],
                    )
                    .unwrap();
            }
        }
        let status = store.deletion_status(None, 10).await.unwrap();
        assert_eq!(
            status.first().map(|record| record.phase),
            Some(expected),
            "the status read reports the {phase} phase"
        );
        if expected == DeletionOperationPhase::Completed {
            assert!(
                store
                    .unfinished_deletions(None, 10)
                    .await
                    .unwrap()
                    .is_empty(),
                "a completed operation is not unfinished"
            );
        }
    }

    // A confirmation row without its request is torn canonical state: the
    // surface mark fails closed instead of publishing a mark over it.
    {
        let guard = store.conn.lock().unwrap();
        guard
            .execute(
                "INSERT INTO deletion_confirmation (request_id,confirmed_at) VALUES (?1,?2)",
                params![
                    crate::codec::encode_id(RawId::new()),
                    WallClockWithTz::now().to_rfc3339()
                ],
            )
            .unwrap();
    }
    assert_eq!(
        store.deletion_surface_mark().await,
        Err(PreservationTechnicalError::CorruptState)
    );
}

#[tokio::test]
async fn operation_request_provenance_must_match_its_journal_row() {
    let store = open_memory().await.unwrap();
    let request = request_id(&stage(&store, "provenance", DeletionPurpose::Privacy).await);
    let _ = started(
        store
            .confirm_targeted_deletion(request, owners())
            .await
            .unwrap(),
    );
    // Tamper: the staged scope text no longer matches the operation's
    // protected material. Every operation-scoped read fails closed.
    {
        let guard = store.conn.lock().unwrap();
        guard
            .execute(
                "UPDATE deletion_request SET exact_text='other' WHERE request_id=?1",
                [crate::codec::encode_id(request.as_raw())],
            )
            .unwrap();
    }
    assert_eq!(
        store.current_erasure_conditions(None, 10).await,
        Err(PreservationTechnicalError::CorruptState)
    );
    assert_eq!(
        store.deletion_status(None, 10).await,
        Err(PreservationTechnicalError::CorruptState)
    );
    assert_eq!(
        store.unfinished_deletions(None, 10).await,
        Err(PreservationTechnicalError::CorruptState)
    );
}

#[tokio::test]
async fn store_bounds_the_request_and_status_pages() {
    let store = open_memory().await.unwrap();
    let _ = stage(&store, "bound one", DeletionPurpose::Privacy).await;
    let _ = stage(&store, "bound two", DeletionPurpose::Security).await;
    for limit in [0, 101] {
        assert_eq!(
            store.pending_targeted_deletions(None, limit).await,
            Err(PreservationTechnicalError::InvalidLimit)
        );
        assert_eq!(
            store.deletion_status(None, limit).await,
            Err(PreservationTechnicalError::InvalidLimit)
        );
    }
    let first = store.pending_targeted_deletions(None, 1).await.unwrap();
    assert_eq!(first.len(), 1);
    let second = store
        .pending_targeted_deletions(Some(first[0].request()), 1)
        .await
        .unwrap();
    assert_eq!(second.len(), 1);
    assert!(second[0].request().as_raw() != first[0].request().as_raw());
}
