//! Stage 6 A1b: staged Targeted Deletion requests, the Host-local trusted
//! confirmation boundary, the surface mark, and the bounded status read.
//!
//! These tests drive the production store API. Fixture SQL appears only where
//! the slice under test cannot legally produce the state (terminal operation
//! phases belong to the A5 completion boundary, which A1/A1b do not expose).

use super::*;
use ene_action::{
    ActionAttemptId, ActionAttemptRepository as _, ActionStartOutcome, AttemptCommitPremise,
    OperationKind, RealTargetRef,
};
use ene_companion::RecordResumeActivityCommand;
use ene_preservation::*;
use ene_task::{TaskAgentOutput, orchestrate_result_arrival};

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

/// One Task Agent attempt claim under the fixture consent.
async fn claim(
    store: &Store,
    delegation: DelegationId,
    task: TaskRef,
    data_use: Vec<RawId>,
) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
    store
        .begin_inference_attempt(InferenceAttempt {
            ticket: InferenceTicketId(RawId::new()),
            consumer: ConsumerKind::TaskAgent,
            capability: CapabilityKind::Dialogue,
            purpose: PurposeKind::TaskAgentTurn,
            expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
            expected_credential_set: CredentialSetRevision::initial(),
            provider: String::from("acme"),
            model: String::from("dialogue-1"),
            task_agent: Some(TaskAgentAttemptPremise {
                delegation: delegation.as_raw(),
                task: task.task.as_raw(),
                task_revision: RevisionInner::from_u64(task.revision.as_u64()),
                data_use,
            }),
            pricing: None,
            usage_estimate: None,
        })
        .await
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
    let result = orchestrate_result_arrival(
        &store,
        delegation,
        TaskAgentOutput::new(format!("final report mentions {target}")),
    )
    .await
    .expect("the result must record");

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
async fn the_sealed_direct_path_keeps_its_caller_named_sources_only() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target_text = "direct private target";
    let _covered = committed_message(
        &store,
        companion,
        generation,
        &format!("owner said {target_text}"),
    )
    .await;
    // The direct path names its sources explicitly; it must never pick up the
    // enumeration the first-party path adds behind the caller's back.
    let command = StartTargetedDeletionCommand::new(
        target(target_text),
        DeletionPurpose::Privacy,
        WallClockWithTz::now(),
        Vec::new(),
        owners(),
    )
    .confirmed_for_tests();
    let current = match store.start_targeted_deletion(command).await.unwrap() {
        StartTargetedDeletionOutcome::Started(current) => current,
        other => panic!("the direct admission must start, got {other:?}"),
    };
    assert_eq!(
        source_rows(&store, current),
        Vec::<String>::new(),
        "the direct path publishes exactly its caller-provided source set"
    );
}

#[tokio::test]
async fn enumerated_source_holds_a_task_agent_claim_and_unrelated_source_does_not() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "private target";
    let covered = committed_message(
        &store,
        companion,
        generation,
        &format!("owner said {target}"),
    )
    .await;
    let unrelated = committed_message(&store, companion, generation, "an unrelated note").await;

    // The Task Agent's adopted-purpose origin names the covered message: this
    // is the `data_use` source the claim compares.
    let created = store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("write the report"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: covered,
            },
            acquired_at: fixture_clock(),
            assignee: AssigneeRef {
                companion: companion.as_raw(),
            },
            workspace: None,
        })
        .await
        .expect("the task must commit");
    let delegation = DelegationId::generate();
    assert!(matches!(
        store
            .create_delegation(delegation_premise(
                delegation,
                created,
                TaskAgentEphemeralId::generate(),
                delegation_scope(None),
            ))
            .await
            .expect("the delegation must answer"),
        DelegationOutcome::Delegated(_)
    ));
    assert!(matches!(
        save_consent(&store, None, consent_record("consent-1", 1)).await,
        ConsentCommitOutcome::Committed { .. }
    ));

    let request = request_id(&stage(&store, target, DeletionPurpose::Privacy).await);
    let _current = started(
        store
            .confirm_targeted_deletion(request, owners())
            .await
            .unwrap(),
    );

    assert_eq!(
        claim(&store, delegation, created, vec![covered]).await,
        Ok(AttemptBeginOutcome::DataUseHeld),
        "the enumerated source holds the send before any provider byte"
    );
    assert_eq!(
        task_table_count(&store, "inference_attempt"),
        0,
        "a held send starts no attempt and records no data_use"
    );
    assert_eq!(task_table_count(&store, "inference_attempt_data_use"), 0);

    // The same Task premise with an unrelated correlation is admitted: the
    // operation covers exactly its enumerated sources.
    assert_eq!(
        claim(&store, delegation, created, vec![unrelated]).await,
        Ok(AttemptBeginOutcome::Started),
        "an unrelated source must not be held by another source's correlation"
    );
}

#[tokio::test]
async fn enumeration_is_bounded_per_identity_table_and_walks_identity_order() {
    let store = open_memory().await.unwrap();
    let target = "bounded private target";
    let limit = crate::preservation::KNOWN_SOURCE_ENUMERATION_LIMIT as usize;
    // Fixture SQL: the enumeration under test reads durable identities, and
    // one more row than the per-table bound is what the bound is about.
    let mut keys = Vec::new();
    {
        let guard = store.conn.lock().unwrap();
        for _ in 0..limit + 1 {
            let message = RawId::new();
            guard
                .execute(
                    "INSERT INTO history_message (message_id,companion_id,round_id,role,body,lang,at,presence_generation) VALUES (?1,?2,?3,'owner',?4,'en',?5,1)",
                    params![
                        crate::codec::encode_id(message),
                        crate::codec::encode_id(RawId::new()),
                        crate::codec::encode_id(RawId::new()),
                        format!("body {target}"),
                        fixture_clock().to_rfc3339()
                    ],
                )
                .unwrap();
            keys.push(crate::codec::encode_id(message));
        }
    }
    keys.sort();
    let request = request_id(&stage(&store, target, DeletionPurpose::Privacy).await);
    let current = started(
        store
            .confirm_targeted_deletion(request, owners())
            .await
            .unwrap(),
    );
    let sources = source_rows(&store, current);
    assert_eq!(
        sources,
        keys[..limit].to_vec(),
        "the enumeration is bounded to the first {limit} canonical identities"
    );
    assert!(
        !sources.contains(&keys[limit]),
        "identities beyond the per-table bound are not published as coverage; the sweep erases them instead"
    );
    // The statement walks the identity order, so the LIMIT stops the walk
    // instead of sorting a full result set first.
    for identity in crate::preservation::KNOWN_SOURCE_IDENTITIES {
        let sql = crate::preservation::known_source_enumeration_sql(*identity);
        let plan: Vec<String> = {
            let guard = store.conn.lock().unwrap();
            let mut explained = guard.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            explained
                .query_map(params![target, limit as i64], |row| row.get(3))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert!(!plan.is_empty(), "missing plan for {sql}");
        assert!(
            !plan.iter().any(|line| line.contains("TEMP B-TREE")),
            "the bounded enumeration must not sort the full table: {plan:?}"
        );
    }
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
