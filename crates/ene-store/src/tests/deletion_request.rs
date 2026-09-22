//! Stage 6 A1b: staged Targeted Deletion requests, the Host-local trusted
//! confirmation boundary, the surface mark, and the bounded status read.
//!
//! These tests drive the production store API. Fixture SQL appears only where
//! the slice under test cannot legally produce the state (a terminal operation
//! phase is produced by the sealed A5 completion boundary instead).

use super::*;
use ene_action::{
    ActionAttemptId, ActionAttemptRepository as _, ActionStartOutcome, AttemptCommitPremise,
    OperationKind, RealTargetRef,
};
use ene_companion::RecordResumeActivityCommand;
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

// --- A1c: known source correlation enumeration at first-party admission ---

/// One operation's stored source correlations, in canonical identity order.
fn source_rows(store: &Store, current: DeletionOperationRef) -> Vec<String> {
    let guard = store.conn.lock().unwrap();
    let mut statement = guard
        .prepare(
            "SELECT source FROM erasure_condition_source WHERE operation_id=?1 AND sweep=?2 ORDER BY source",
        )
        .unwrap();
    statement
        .query_map(
            params![
                crate::codec::encode_id(current.operation.as_raw()),
                current.sweep.as_u64() as i64
            ],
            |row| row.get(0),
        )
        .unwrap()
        .collect::<Result<Vec<String>, _>>()
        .unwrap()
}

/// The A5 completion invariant without the A5 authority that does not exist in
/// this slice: every required participant verified for the final sweep, the
/// condition closed, and material, hints, and ALL source rows deleted.
fn complete_fixture(store: &Store, current: DeletionOperationRef) {
    let id = crate::codec::encode_id(current.operation.as_raw());
    let conn = store.conn.lock().unwrap();
    conn.execute(
        "UPDATE deletion_participant SET state='verified',hold_class=NULL,remainder_count=0,reported_at='2026-09-17T01:00:00Z' WHERE operation_id=?1 AND sweep=?2",
        params![id, current.sweep.as_u64() as i64],
    )
    .unwrap();
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
        "DELETE FROM erasure_condition_source WHERE operation_id=?1",
        [&id],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM deletion_reconciliation WHERE operation_id=?1",
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

/// One History message committed through the production append path.
async fn committed_message(
    store: &Store,
    companion: CompanionId,
    generation: PresenceGeneration,
    text: &str,
) -> RawId {
    match store
        .append_message(history_command(companion, generation, text))
        .await
        .expect("the Owner append must answer")
    {
        HistoryAppendOutcome::CommittedAs { message } => message,
        other => panic!("the Owner append must commit, got {other:?}"),
    }
}

#[tokio::test]
async fn first_party_confirmation_enumerates_known_covered_sources() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "private target";

    // Companion-owned bodies: the accepted Owner input carries the target; an
    // unrelated message must never become collateral coverage.
    let message = committed_message(
        &store,
        companion,
        generation,
        &format!("owner said {target}"),
    )
    .await;
    let unrelated = committed_message(&store, companion, generation, "an unrelated note").await;

    // A first-party resume instruction activity whose body carries the target.
    let created = store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("write the report"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            acquired_at: fixture_clock(),
            assignee: AssigneeRef {
                companion: companion.as_raw(),
            },
            workspace: Some(task_workspace("/srv/workspace/ene", None)),
        })
        .await
        .expect("the task must commit");
    let record = store.load_task(created.task).await.unwrap().unwrap();
    let activity = record_activity_id(
        &store,
        RecordResumeActivityCommand {
            companion,
            task: created,
            purpose: record.task.purpose,
            body: format!("continue with {target}"),
            command: RawId::new(),
        },
    )
    .await
    .expect("the activity must record");

    // Derived Learning rows whose stored text carries the target.
    let memory = MemoryId::generate();
    let summary = learning_summary(companion.as_raw(), &format!("summary of {target}"));
    let committed = store
        .commit_memory_change(commit(
            Some(summary.clone()),
            learning_change(
                companion.as_raw(),
                MemoryTarget::New { id: memory },
                &format!("memory of {target}"),
                ChangeKind::Initial,
                false,
            ),
        ))
        .await
        .expect("the memory change must answer");
    assert!(
        matches!(committed, MemoryChangeOutcome::Committed { .. }),
        "the Learning fixture must commit, got {committed:?}"
    );

    // Action and Task-result facts over the delegated execution.
    let assoc = {
        let guard = store.conn.lock().unwrap();
        guard
            .query_row(
                "SELECT assoc_id FROM workspace_assoc WHERE task_id=?1",
                [crate::codec::encode_id(created.task.as_raw())],
                |row| row.get::<_, String>(0),
            )
            .expect("the workspace association must read")
    };
    let assoc = WorkspaceAssocId::from_raw(crate::codec::decode_id(&assoc).unwrap());
    let delegation = DelegationId::generate();
    let delegated = store
        .create_delegation(delegation_premise(
            delegation,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(Some(delegated_workspace(assoc, "/srv/workspace/ene", None))),
        ))
        .await
        .expect("the delegation must answer");
    assert!(matches!(delegated, DelegationOutcome::Delegated(_)));
    let attempt = ActionAttemptId::generate();
    // Platform-independent canonical target that still carries the target text.
    let action_target = std::env::temp_dir()
        .join(format!("ene-a1c-{target}/report.txt"))
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        store
            .insert_attempt_if_current(AttemptCommitPremise {
                attempt,
                delegation: delegation.as_raw(),
                task: created.task.as_raw(),
                task_revision: RevisionInner::from_u64(created.revision.as_u64()),
                workspace: assoc.as_raw(),
                real_target: RealTargetRef::from_canonical_path(action_target),
                operation: OperationKind::Create,
                relied_evaluation: RawId::new(),
            })
            .await
            .expect("the attempt insert must answer"),
        ActionStartOutcome::Started,
        "the fixture records before any condition exists"
    );
    let result = record_result(
        &store,
        delegation,
        &format!("final report mentions {target}"),
    )
    .await;

    // The first-party production path: stage, then the trusted confirmation.
    let request = request_id(&stage(&store, target, DeletionPurpose::Privacy).await);
    let current = started(
        store
            .confirm_targeted_deletion(request, owners())
            .await
            .unwrap(),
    );

    let sources = source_rows(&store, current);
    for (label, id) in [
        ("History message", message),
        ("activity", activity.as_raw()),
        ("Learning summary", summary.id.as_raw()),
        ("Learning memory", memory.as_raw()),
        ("Action attempt", attempt.as_raw()),
        ("Task result", result.result.as_raw()),
    ] {
        assert!(
            sources.contains(&crate::codec::encode_id(id)),
            "the covered {label} identity must be a durable source correlation: {sources:?}"
        );
    }
    assert!(
        !sources.contains(&crate::codec::encode_id(unrelated)),
        "an unrelated body must never be collateral source coverage: {sources:?}"
    );
    assert_eq!(
        sources.len(),
        6,
        "only the covered identities are enumerated: {sources:?}"
    );
}

#[tokio::test]
async fn enumerated_sources_survive_restart_and_sweep_and_clear_on_completion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("known-sources.db");
    let store = Store::open(&path).await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "restart private target";
    let message = committed_message(
        &store,
        companion,
        generation,
        &format!("owner said {target}"),
    )
    .await;
    let probe = crate::codec::encode_id(message);
    let request = request_id(&stage(&store, target, DeletionPurpose::Privacy).await);
    let current = started(
        store
            .confirm_targeted_deletion(request, owners())
            .await
            .unwrap(),
    );
    assert_eq!(source_rows(&store, current), vec![probe.clone()]);
    drop(store);

    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(
        crate::preservation::covering_condition(&reopened.conn.lock().unwrap(), &probe).unwrap(),
        Some(current.condition()),
        "the enumerated correlation is durable canonical coverage after restart"
    );
    let DeletionLifecycleOutcome::Applied(next) = reopened
        .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
        .await
        .unwrap()
    else {
        panic!("the generation must advance");
    };
    assert_eq!(next.sweep.as_u64(), 2);
    assert_eq!(
        source_rows(&reopened, next),
        vec![probe.clone()],
        "NextSweep copies the enumerated correlation into the current sweep"
    );
    {
        let guard = reopened.conn.lock().unwrap();
        let superseded: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM erasure_condition_source WHERE operation_id=?1 AND sweep=1",
                [crate::codec::encode_id(next.operation.as_raw())],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(superseded, 0, "the superseded sweep keeps no source row");
    }
    assert_eq!(
        crate::preservation::covering_condition(&reopened.conn.lock().unwrap(), &probe).unwrap(),
        Some(next.condition())
    );
    complete_fixture(&reopened, next);
    assert_eq!(
        task_table_count(&reopened, "erasure_condition_source"),
        0,
        "a completed operation keeps zero source rows"
    );
    assert_eq!(
        crate::preservation::covering_condition(&reopened.conn.lock().unwrap(), &probe).unwrap(),
        None,
        "a completed operation is not a permanent keyword ban"
    );
}
