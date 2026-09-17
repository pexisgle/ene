//! Stage 6 A4: current-condition gates at the text-bearing acceptance
//! boundaries (lifecycle §7/§11).
//!
//! Every test drives the production acceptance path of one semantic owner
//! against the canonical store, with a real admitted deletion operation as the
//! condition. The deterministic orderings are the lifecycle §17 R1/R2
//! walkthroughs: condition-first (the write/adopt/present is refused or
//! collected) and use-first (the already-committed fact stays; the delayed
//! body is refused or collected at the receiving boundary). No timing
//! dependence: each order is a straight-line scenario.
//!
//! A1/A2 expose no completion authority; A5 owns the sealed finalizing and
//! completion boundary, and the "operation completed" boundary in these tests
//! runs through it (`complete_via_a5`): every required participant is verified
//! through the canonical API and the durable audit/condition/output commit is
//! the production path. A fixture that still stores the target first drives
//! the owning participant's real sweep, because the system-wide remainder
//! probe refuses to complete over collected target data.

use super::*;

use ene_action::{
    ActionAttemptId, ActionAttemptRepository as _, ActionStartOutcome, AttemptCommitPremise,
    OperationKind, RealTargetRef,
};
use ene_companion::{
    PresentationMark, RecordResumeActivityCommand, ReportStatus, ReportStatusTransition,
    ResumeActivityOutcome,
};
use ene_preservation::{
    DeletionOperationRef, DeletionSearchMaterial, ErasureConditionRef, MechanicalDeletionTarget,
    ParticipantOwnerRef, PreservationRepository as _, StartTargetedDeletionCommand,
    StartTargetedDeletionOutcome, TargetedDeletionTarget,
};
use ene_task::{
    DelegationId, TaskAgentEphemeralId, TaskAgentOutput, TaskAgentResultArrival, TaskId,
    TaskResultId, TaskRevision,
};

use super::preservation::complete_via_a5;
use super::targeted_deletion::drive_with_sources;

fn summary(companion: RawId, content: &str, start: RawId, end: RawId) -> SummaryRecord {
    SummaryRecord {
        id: SummaryId::generate(),
        scope: LearningScope::companion(companion),
        content: content.to_owned(),
        source: SourceRangeRef {
            kind: ExperienceSourceKind::Dialogue,
            start,
            end,
        },
        formed_at: fixture_clock(),
    }
}

fn change(
    summary: &SummaryRecord,
    target: MemoryTarget,
    content: &str,
    kind: ChangeKind,
) -> MemoryChangeCommit {
    MemoryChangeCommit {
        summary: Some(summary.clone()),
        secret_premise: None,
        change: MemoryChange {
            target,
            scope: LearningScope::companion(summary.scope.companion_id()),
            content: content.to_owned(),
            importance: Importance::default(),
            temporal: TemporalMeaning::Enduring,
            change: kind,
            recall_suppressed: false,
            at: fixture_clock(),
        },
    }
}

async fn commit(store: &Store, commit: MemoryChangeCommit) -> MemoryChangeOutcome {
    store
        .commit_memory_change(commit)
        .await
        .expect("the commit must answer")
}

fn admission(
    text: &str,
    sources: Vec<RawId>,
    participants: Vec<ParticipantOwnerRef>,
) -> StartTargetedDeletionCommand {
    StartTargetedDeletionCommand::new(
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                text.to_owned(),
            )),
            semantic_hints: Vec::new(),
        },
        ene_preservation::DeletionPurpose::Privacy,
        fixture_clock(),
        sources,
        participants,
    )
    .confirmed_for_tests()
}

/// Admits one operation against the Companion owner (the default participant
/// of a body-only fixture) plus any additional required owners.
async fn admit(
    store: &Store,
    text: &str,
    sources: Vec<RawId>,
    extra: Vec<ParticipantOwnerRef>,
) -> DeletionOperationRef {
    let mut participants = vec![ParticipantOwnerRef::Companion];
    for owner in extra {
        if !participants.contains(&owner) {
            participants.push(owner);
        }
    }
    match store
        .start_targeted_deletion(admission(text, sources, participants))
        .await
        .expect("admission must commit")
    {
        StartTargetedDeletionOutcome::Started(current) => current,
        other => panic!("the operation must start, got {other:?}"),
    }
}

async fn current_conditions(store: &Store) -> Vec<ErasureConditionRef> {
    store
        .current_erasure_conditions(None, 100)
        .await
        .expect("the canonical current set must read")
        .into_iter()
        .map(|condition| condition.condition)
        .collect()
}

fn count_rows(store: &Store, sql: &str, key: &str) -> i64 {
    let conn = store.conn.lock().unwrap();
    conn.query_row(sql, [key], |row| row.get(0))
        .expect("the fixture count must read")
}

fn history_rows(store: &Store, companion: CompanionId) -> i64 {
    count_rows(
        store,
        "SELECT COUNT(*) FROM history_message WHERE companion_id=?1",
        &crate::codec::encode_id(companion.as_raw()),
    )
}

fn undelivered_rows(store: &Store, companion: CompanionId) -> i64 {
    count_rows(
        store,
        "SELECT COUNT(*) FROM undelivered WHERE companion_id=?1",
        &crate::codec::encode_id(companion.as_raw()),
    )
}

fn learning_rows(store: &Store, companion: RawId) -> i64 {
    let key = crate::codec::encode_id(companion);
    let conn = store.conn.lock().unwrap();
    let summaries: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM learning_summary WHERE companion_id=?1",
            [&key],
            |row| row.get(0),
        )
        .unwrap();
    let memories: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM learning_memory WHERE companion_id=?1",
            [&key],
            |row| row.get(0),
        )
        .unwrap();
    summaries + memories
}

fn task_purpose_text(store: &Store, task: TaskId) -> String {
    count_text(
        store,
        "SELECT purpose_text FROM task WHERE task_id=?1",
        &crate::codec::encode_id(task.as_raw()),
    )
}

fn task_result_body(store: &Store, result: TaskResultId) -> String {
    count_text(
        store,
        "SELECT body FROM task_result WHERE result_id=?1",
        &crate::codec::encode_id(result.as_raw()),
    )
}

fn count_text(store: &Store, sql: &str, key: &str) -> String {
    let conn = store.conn.lock().unwrap();
    conn.query_row(sql, [key], |row| row.get(0))
        .expect("the fixture text must read")
}

async fn companion_with_generation(store: &Store) -> (CompanionId, PresenceGeneration) {
    let companion = store
        .ensure_running_companion()
        .await
        .expect("the companion must exist");
    let attribution = store
        .load_attribution(companion.as_raw())
        .await
        .expect("the attribution must load")
        .expect("the attribution must exist");
    (companion, attribution.generation)
}

async fn append_owner(
    store: &Store,
    companion: CompanionId,
    generation: PresenceGeneration,
    text: &str,
) -> HistoryAppendOutcome {
    store
        .append_message(history_command(companion, generation, text))
        .await
        .expect("the Owner append must answer")
}

async fn append_reply_with_body(
    store: &Store,
    companion: CompanionId,
    generation: PresenceGeneration,
    text: &str,
    register_unpresented: bool,
) -> (HistoryAppendOutcome, Option<ene_companion::UndeliveredRef>) {
    let mut command = history_command(companion, generation, text);
    command.role = HistoryRole::Companion;
    store
        .append_reply_with_undelivered(command, register_unpresented)
        .await
        .expect("the reply append must answer")
}

async fn seed_task(store: &Store, companion: CompanionId, purpose: &str) -> TaskRef {
    let workspace = task_workspace("/srv/workspace/ene", None);
    store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: purpose.to_owned(),
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
            workspace: Some(workspace),
        })
        .await
        .expect("the task must commit")
}

async fn seed_delegation(store: &Store, task: TaskRef) -> DelegationId {
    // The delegation copies the boundary the Task already carries; the read
    // below resolves the committed association by task identity.
    let assoc = {
        let key = crate::codec::encode_id(task.task.as_raw());
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT assoc_id FROM workspace_assoc WHERE task_id=?1",
            [&key],
            |row| row.get::<_, String>(0),
        )
        .expect("the committed association must read")
    };
    let assoc = WorkspaceAssocId::from_raw(crate::codec::decode_id(&assoc).unwrap());
    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            delegation,
            task,
            TaskAgentEphemeralId::generate(),
            delegation_scope(Some(delegated_workspace(assoc, "/srv/workspace/ene", None))),
        ))
        .await
        .expect("the delegation must answer");
    assert!(
        matches!(outcome, DelegationOutcome::Delegated(_)),
        "the delegation must commit, got {outcome:?}"
    );
    delegation
}

async fn seed_workspace_execution(store: &Store, purpose: &str) -> (TaskRef, DelegationId) {
    let (companion, _) = companion_with_generation(store).await;
    let task = seed_task(store, companion, purpose).await;
    let delegation = seed_delegation(store, task).await;
    (task, delegation)
}

fn result_arrival(delegation: DelegationId, body: &str) -> TaskAgentResultArrival {
    TaskAgentResultArrival {
        delegation,
        result: TaskResultId::generate(),
        body: TaskAgentOutput::new(body.to_owned()),
    }
}

// --- Dialogue History append / reply adoption ---

#[tokio::test]
async fn condition_first_refuses_a_dialogue_append_without_leaving_a_row() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = companion_with_generation(&store).await;
    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    assert_eq!(current_conditions(&store).await, vec![current.condition()]);

    let outcome = append_owner(&store, companion, generation, "please keep the private key").await;
    assert_eq!(
        outcome,
        HistoryAppendOutcome::HeldForErasure,
        "a covered Owner append is a domain hold, not a storage error"
    );
    assert_eq!(history_rows(&store, companion), 0);
    assert_eq!(undelivered_rows(&store, companion), 0);

    // A covered reply is refused the same way: no row, no undelivered
    // registration, no round.
    let (outcome, registered) =
        append_reply_with_body(&store, companion, generation, "about the private key", true).await;
    assert_eq!(outcome, HistoryAppendOutcome::HeldForErasure);
    assert!(registered.is_none());
    assert_eq!(history_rows(&store, companion), 0);
    assert_eq!(undelivered_rows(&store, companion), 0);
}

#[tokio::test]
async fn append_first_then_condition_refuses_the_delayed_reply() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = companion_with_generation(&store).await;
    // R2: the Owner turn commits first; the deletion condition starts while
    // the provider call is in flight.
    let owner = match append_owner(&store, companion, generation, "an ordinary question").await {
        HistoryAppendOutcome::CommittedAs { message } => message,
        other => panic!("the Owner append must commit, got {other:?}"),
    };
    let current = admit(&store, "the leaked token", vec![owner], Vec::new()).await;

    // The delayed reply restates the target: refused by the mechanical text.
    let mut reply = history_command(companion, generation, "here is the leaked token");
    reply.role = HistoryRole::Companion;
    reply.expected_owner_message = Some(owner);
    let (outcome, registered) = store
        .append_reply_with_undelivered(reply, true)
        .await
        .expect("the reply append must answer");
    assert_eq!(outcome, HistoryAppendOutcome::HeldForErasure);
    assert!(registered.is_none());

    // A paraphrase without the target is refused through the relied Owner
    // input's source correlation: the reply derives from a covered turn.
    let mut reply = history_command(companion, generation, "the answer avoids the exact words");
    reply.role = HistoryRole::Companion;
    reply.expected_owner_message = Some(owner);
    let (outcome, _) = store
        .append_reply_with_undelivered(reply, true)
        .await
        .expect("the reply append must answer");
    assert_eq!(outcome, HistoryAppendOutcome::HeldForErasure);
    assert_eq!(history_rows(&store, companion), 1);

    // Only the pre-condition Owner row remains; the condition is still
    // current (no completion happened here).
    assert!(
        current_conditions(&store)
            .await
            .contains(&current.condition())
    );
}

#[tokio::test]
async fn a_replayed_covered_command_does_not_re_materialize_the_history_row() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = companion_with_generation(&store).await;
    let command = CommandId(RawId::new());
    let text = "resend the private key";
    let mut first = history_command_with_ids(companion, generation, text, Some(command), None);
    first.local_id = Some(String::from("local-1"));
    assert!(matches!(
        store.append_message(first.clone()).await.unwrap(),
        HistoryAppendOutcome::CommittedAs { .. }
    ));
    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;

    // The owner sweep erases the committed row: the Client's local copy is
    // the only remaining source, and a reconnect replays it.
    let participant = crate::CompanionErasureParticipant::new(store.clone());
    drive_with_sources(
        &participant,
        current.condition(),
        ParticipantOwnerRef::Companion,
        "the private key",
        Vec::new(),
    )
    .await;
    assert_eq!(history_rows(&store, companion), 0);

    let replay = store
        .append_message(first)
        .await
        .expect("the replay must answer");
    assert_eq!(
        replay,
        HistoryAppendOutcome::HeldForErasure,
        "a replayed local copy must not re-create the erased row"
    );
    assert_eq!(history_rows(&store, companion), 0);
}

// --- Learning Summary / Memory formation ---

#[tokio::test]
async fn condition_first_refuses_a_learning_formation() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    admit(&store, "the secret plan", Vec::new(), Vec::new()).await;
    let record = summary(
        companion,
        "summary of the secret plan",
        RawId::new(),
        RawId::new(),
    );
    let outcome = commit(
        &store,
        change(
            &record,
            MemoryTarget::New {
                id: MemoryId::generate(),
            },
            "recall the secret plan",
            ChangeKind::Initial,
        ),
    )
    .await;
    assert_eq!(outcome, MemoryChangeOutcome::HeldForErasure);
    assert_eq!(learning_rows(&store, companion), 0);
}

#[tokio::test]
async fn formation_first_then_condition_refuses_the_delayed_formation() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let start = RawId::new();
    let end = RawId::new();
    let first = summary(companion, "ordinary evidence", start, end);
    let memory = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &first,
                MemoryTarget::New { id: memory },
                "an ordinary recall",
                ChangeKind::Initial
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));
    // R2: the deletion starts after the first formation committed; a delayed
    // formation computed against the same (covered) evidence arrives.
    let current = admit(&store, "the secret plan", vec![start, end], Vec::new()).await;

    let delayed = summary(companion, "ordinary evidence", start, end);
    let outcome = commit(
        &store,
        change(
            &delayed,
            MemoryTarget::Existing {
                id: memory,
                expected_revision: MemoryRevision::initial(),
            },
            "an ordinary recall, restated",
            ChangeKind::Refined,
        ),
    )
    .await;
    assert_eq!(
        outcome,
        MemoryChangeOutcome::HeldForErasure,
        "a formation derived from a covered source is held even without the exact text"
    );

    // A target-bearing body is held by the mechanical text as well.
    let textual = summary(
        companion,
        "summary of the secret plan",
        RawId::new(),
        RawId::new(),
    );
    assert_eq!(
        commit(
            &store,
            change(
                &textual,
                MemoryTarget::New {
                    id: MemoryId::generate()
                },
                "fresh text",
                ChangeKind::Initial
            ),
        )
        .await,
        MemoryChangeOutcome::HeldForErasure
    );
    assert_eq!(current_conditions(&store).await, vec![current.condition()]);
}

// --- Task result arrival / adoption ---

#[tokio::test]
async fn condition_first_collects_a_task_result_body_and_keeps_the_fact() {
    let store = open_memory().await.unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    admit(&store, "the private key", Vec::new(), Vec::new()).await;

    let arrival = result_arrival(delegation, "the report quotes the private key");
    let record = store
        .record_task_result_arrival(arrival.clone())
        .await
        .expect("the arrival must record its fact");
    assert_collected(record.body.text(), "the private key");
    assert_collected(&task_result_body(&store, arrival.result), "the private key");
    // The objective fact survives: the delegation is sealed and the result is
    // readable, only its body was collected.
    assert!(
        store
            .load_delegation_result(delegation)
            .await
            .unwrap()
            .is_some()
    );
    let stored = store
        .load_task_result(arrival.result)
        .await
        .unwrap()
        .unwrap();
    assert_collected(stored.body.text(), "the private key");
}

#[tokio::test]
async fn result_first_then_condition_redacts_and_a_retry_stays_idempotent() {
    let store = open_memory().await.unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let arrival = result_arrival(delegation, "final report mentions the private key");
    store
        .record_task_result_arrival(arrival.clone())
        .await
        .expect("the pre-condition arrival must record");
    assert_eq!(
        task_result_body(&store, arrival.result),
        "final report mentions the private key"
    );

    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    // The owner sweep collects the committed body (A3); the retry of the same
    // arrival then compares against the collected form and stays idempotent
    // instead of re-introducing the body.
    let participant = crate::TaskErasureParticipant::new(store.clone());
    drive_with_sources(
        &participant,
        current.condition(),
        ParticipantOwnerRef::Task,
        "the private key",
        Vec::new(),
    )
    .await;
    assert_collected(&task_result_body(&store, arrival.result), "the private key");
    let retry = store
        .record_task_result_arrival(arrival.clone())
        .await
        .expect("the covered retry must stay an idempotent replay");
    assert_collected(retry.body.text(), "the private key");
    assert_collected(&task_result_body(&store, arrival.result), "the private key");

    // After completion the same identity still never re-writes the body: the
    // stored collected form disagrees with the incoming raw text, so the
    // retry fails closed instead of resurrecting the target.
    complete_via_a5(&store, current).await;
    let result = arrival.result;
    assert!(
        store.record_task_result_arrival(arrival).await.is_err(),
        "a post-completion retry never rewrites the collected body"
    );
    assert_collected(&task_result_body(&store, result), "the private key");
}

/// The collected form of a covered body: the mechanical target is gone and
/// the fixed marker records where it was removed.
fn assert_collected(text: &str, target: &str) {
    assert!(
        !text.contains(target),
        "the collected body must not carry the target: {text}"
    );
    assert!(
        text.contains("[erased]"),
        "the collected body keeps the fixed body-free marker: {text}"
    );
}

#[tokio::test]
async fn condition_first_holds_a_steering_and_keeps_the_revision() {
    let store = open_memory().await.unwrap();
    let (companion, _) = companion_with_generation(&store).await;
    let task = seed_task(&store, companion, "write the report").await;
    admit(&store, "the private key", Vec::new(), Vec::new()).await;

    let record = store.load_task(task.task).await.unwrap().unwrap();
    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: task,
            new_purpose: Some(ene_task::TaskPurposeAdoptionPremise {
                purpose: TaskPurpose {
                    text: String::from("write about the private key"),
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
        .expect("the steering must answer");
    assert_eq!(outcome, TaskCommitOutcome::HeldForErasure);
    let reloaded = store.load_task(task.task).await.unwrap().unwrap();
    assert_eq!(reloaded.task.reference, record.task.reference);
    assert_eq!(task_purpose_text(&store, task.task), "write the report");

    // A steering whose instruction source is covered is held even when the
    // purpose text is unrelated.
    let source = RawId::new();
    let task = seed_task(&store, companion, "write the report").await;
    admit(&store, "another target", vec![source], Vec::new()).await;
    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: task,
            new_purpose: None,
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: Some(ene_task::TaskInstructionAdoptionPremise {
                entry: TaskContextEntryId::generate(),
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source,
                },
                acquired_at: fixture_clock(),
            }),
        })
        .await
        .expect("the steering must answer");
    assert_eq!(outcome, TaskCommitOutcome::HeldForErasure);
    assert_eq!(task_purpose_text(&store, task.task), "write the report");
}

#[tokio::test]
async fn a_carried_purpose_under_a_condition_is_materialized_body_free() {
    let store = open_memory().await.unwrap();
    let (companion, _) = companion_with_generation(&store).await;
    let task = seed_task(&store, companion, "write about the private key").await;
    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;

    let outcome = store
        .forward_steering(TaskCommitPremise {
            expected: task,
            new_purpose: None,
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .expect("the carry-forward steering must answer");
    let TaskCommitOutcome::CommittedAs(next) = outcome else {
        panic!("the carry-forward must commit, got {outcome:?}");
    };
    assert_eq!(next.revision.as_u64(), TaskRevision::initial().as_u64() + 1);
    assert_collected(&task_purpose_text(&store, task.task), "the private key");
    assert_eq!(current_conditions(&store).await, vec![current.condition()]);
}

// --- Action attempt / settlement ---

#[tokio::test]
async fn condition_first_refuses_an_action_attempt_on_a_covered_target() {
    let store = open_memory().await.unwrap();
    let (task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let assoc = {
        let key = crate::codec::encode_id(task.task.as_raw());
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT assoc_id FROM workspace_assoc WHERE task_id=?1",
            [&key],
            |row| row.get::<_, String>(0),
        )
        .expect("the association must read")
    };
    let assoc = WorkspaceAssocId::from_raw(crate::codec::decode_id(&assoc).unwrap());
    let target = std::env::temp_dir()
        .join("ene-a4-private-key/report.txt")
        .to_string_lossy()
        .into_owned();
    admit(&store, "ene-a4-private-key", Vec::new(), Vec::new()).await;

    let attempt = ActionAttemptId::generate();
    let outcome = store
        .insert_attempt_if_current(AttemptCommitPremise {
            attempt,
            delegation: delegation.as_raw(),
            task: task.task.as_raw(),
            task_revision: RevisionInner::from_u64(task.revision.as_u64()),
            workspace: assoc.as_raw(),
            real_target: RealTargetRef::from_canonical_path(target),
            operation: OperationKind::Create,
            relied_evaluation: RawId::new(),
        })
        .await
        .expect("the attempt insert must answer");
    assert_eq!(outcome, ActionStartOutcome::HeldForErasure);
    assert!(
        store.load_attempt(attempt).await.unwrap().is_none(),
        "no attempt row exists for a held target"
    );
}

// --- Undelivered presentation start and ACK ---

#[tokio::test]
async fn condition_first_holds_a_presentation_start_and_ack() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = companion_with_generation(&store).await;
    let (outcome, registered) =
        append_reply_with_body(&store, companion, generation, "the private key", true).await;
    assert!(matches!(outcome, HistoryAppendOutcome::CommittedAs { .. }));
    let entry = registered.expect("the reply registers an undelivered entry");

    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    let mark_start = PresentationMark {
        round: RawId::new(),
        presented: false,
    };
    assert_eq!(
        store
            .compare_and_mark_reported_sync(entry.id, ReportStatus::Pending, mark_start)
            .unwrap(),
        ReportStatusTransition::HeldForErasure,
        "a covered item is never claimed as a presentation start"
    );
    let mark_ack = PresentationMark {
        round: RawId::new(),
        presented: true,
    };
    assert_eq!(
        store
            .compare_and_mark_reported_sync(entry.id, ReportStatus::Pending, mark_ack)
            .unwrap(),
        ReportStatusTransition::HeldForErasure,
        "a covered item is never confirmed presented"
    );
    let status = {
        let conn = store.conn.lock().unwrap();
        let key = crate::codec::encode_id(entry.id.as_raw());
        conn.query_row(
            "SELECT status FROM undelivered WHERE undelivered_id=?1",
            [&key],
            |row| row.get::<_, String>(0),
        )
        .expect("the status must read")
    };
    assert_eq!(status, "pending");

    // Completion requires the collected remainder to be gone: the Companion
    // owner's real bounded sweep removes the covered History body and the
    // dangling reporting reference, exactly as the production fan-out would.
    let participant = crate::CompanionErasureParticipant::new(store.clone());
    drive_with_sources(
        &participant,
        current.condition(),
        ParticipantOwnerRef::Companion,
        "the private key",
        Vec::new(),
    )
    .await;
    complete_via_a5(&store, current).await;

    // After completion the same text is a new origin: a fresh reply commits
    // and its own reporting transitions proceed — the closed condition is not
    // a permanent ban.
    let (outcome, registered) =
        append_reply_with_body(&store, companion, generation, "the private key", true).await;
    assert!(matches!(outcome, HistoryAppendOutcome::CommittedAs { .. }));
    let fresh = registered.expect("the fresh reply registers an undelivered entry");
    assert_eq!(
        store
            .compare_and_mark_reported_sync(fresh.id, ReportStatus::Pending, mark_start)
            .unwrap(),
        ReportStatusTransition::MarkedPresentationUnknown
    );
}

// --- Resume instruction activity ---

#[tokio::test]
async fn condition_first_holds_a_resume_instruction_activity() {
    let store = open_memory().await.unwrap();
    let (companion, _) = companion_with_generation(&store).await;
    let task = seed_task(&store, companion, "write the report").await;
    let record = store.load_task(task.task).await.unwrap().unwrap();
    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;

    let command = RawId::new();
    assert_eq!(
        record_activity_id(
            &store,
            RecordResumeActivityCommand {
                companion,
                task,
                purpose: record.task.purpose,
                body: String::from("continue with the private key"),
                command,
            },
        )
        .await,
        Err(CompanionTechnicalError::StorageUnavailable {
            reason: String::from("activity held for erasure"),
        })
    );
    let activities = {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM activity_record WHERE companion_id=?1",
            [crate::codec::encode_id(companion.as_raw())],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(activities, 0);

    // A fresh instruction after completion records normally.
    complete_via_a5(&store, current).await;
    let recorded = store
        .record_resume_activity(RecordResumeActivityCommand {
            companion,
            task,
            purpose: record.task.purpose,
            body: String::from("a fresh resume instruction"),
            command,
        })
        .await
        .expect("the post-completion activity must record");
    let ResumeActivityOutcome::Recorded(activity) = recorded else {
        panic!("the post-completion activity must record, got {recorded:?}");
    };
    let loaded = store
        .load_activity(activity)
        .await
        .unwrap()
        .expect("the activity must exist");
    assert_eq!(loaded.body, "a fresh resume instruction");
}

// --- Restart and completion boundaries ---

#[tokio::test]
async fn restart_does_not_avoid_the_condition() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a4-restart.db");
    let store = Store::open(&path).await.unwrap();
    let (companion, generation) = companion_with_generation(&store).await;
    admit(&store, "the private key", Vec::new(), Vec::new()).await;
    assert_eq!(
        append_owner(&store, companion, generation, "carrying the private key").await,
        HistoryAppendOutcome::HeldForErasure
    );
    drop(store);

    let reopened = Store::open(&path).await.unwrap();
    let (companion, generation) = companion_with_generation(&reopened).await;
    assert_eq!(current_conditions(&reopened).await.len(), 1);
    assert_eq!(
        append_owner(&reopened, companion, generation, "carrying the private key").await,
        HistoryAppendOutcome::HeldForErasure,
        "a restart restores the active condition from durable state"
    );
    assert_eq!(history_rows(&reopened, companion), 0);
}

#[tokio::test]
async fn fresh_owner_input_after_completion_is_a_new_origin() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = companion_with_generation(&store).await;
    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    assert_eq!(
        append_owner(&store, companion, generation, "about the private key").await,
        HistoryAppendOutcome::HeldForErasure
    );
    complete_via_a5(&store, current).await;
    assert!(current_conditions(&store).await.is_empty());

    let outcome = append_owner(&store, companion, generation, "about the private key").await;
    assert!(
        matches!(outcome, HistoryAppendOutcome::CommittedAs { .. }),
        "a closed condition does not cover a fresh Owner input, got {outcome:?}"
    );
    assert_eq!(history_rows(&store, companion), 1);

    // The same time boundary holds for a fresh formation: the closed
    // condition no longer covers the new evidence.
    let companion_raw = companion.as_raw();
    let record = summary(
        companion_raw,
        "about the private key",
        RawId::new(),
        RawId::new(),
    );
    let desired = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &record,
                MemoryTarget::New { id: desired },
                "a fresh recognition",
                ChangeKind::Initial
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));
}

#[tokio::test]
async fn torn_or_unreadable_current_state_fails_closed_at_the_boundary() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = companion_with_generation(&store).await;
    admit(&store, "the private key", Vec::new(), Vec::new()).await;
    {
        // An open condition whose operation row is gone is torn canonical
        // state: the boundary must not read it as "not covering".
        let conn = store.conn.lock().unwrap();
        conn.execute("DELETE FROM deletion_operation", ()).unwrap();
    }
    assert!(
        store
            .append_message(history_command(companion, generation, "ordinary text"))
            .await
            .is_err(),
        "an unreadable current-condition set refuses instead of accepting"
    );

    // A finalizing operation whose protected material was already wiped has
    // no readable target: writes are refused (fail closed), not accepted.
    let store = open_memory().await.unwrap();
    let (companion, generation) = companion_with_generation(&store).await;
    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    {
        // The finalizing premise (every required participant verified) plus
        // the wiped material is the unreadable-target shape A5 owns.
        let id = crate::codec::encode_id(current.operation.as_raw());
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE deletion_participant SET state='verified',hold_class=NULL,remainder_count=0,reported_at='2026-09-17T01:00:00Z' WHERE operation_id=?1",
            [&id],
        )
        .unwrap();
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
            .append_message(history_command(companion, generation, "ordinary text"))
            .await
            .unwrap(),
        HistoryAppendOutcome::HeldForErasure,
        "an unreadable target covers every body"
    );
}
