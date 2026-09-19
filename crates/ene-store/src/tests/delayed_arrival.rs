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
use ene_learning::LearningClaimRef;
use ene_preservation::{
    DeletionOperationPhase, DeletionOperationRef, DeletionSearchMaterial, ErasureConditionRef,
    MechanicalDeletionTarget, ParticipantCompletionFact, ParticipantCompletionOutcome,
    ParticipantOwnerRef, PreservationRepository as _, StartTargetedDeletionCommand,
    StartTargetedDeletionOutcome, TargetedDeletionTarget,
};
use ene_task::{
    DelegationId, TaskAgentEphemeralId, TaskAgentResultArrival, TaskId, TaskResultId, TaskRevision,
};

use super::preservation::{complete_via_a5, enter_finalizing_via_a5};
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
        claim: None,
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
        .append_reply_with_undelivered(command, register_unpresented, None)
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

async fn result_arrival(
    store: &Store,
    delegation: DelegationId,
    body: &str,
) -> TaskAgentResultArrival {
    TaskAgentResultArrival {
        delegation,
        result: TaskResultId::generate(),
        body: scrubbed_result(store, body).await,
    }
}

/// Unwraps a recorded arrival outcome; these fixtures scrub at the store's
/// current revision, so a stale refusal would be a fixture error.
fn expect_recorded(outcome: TaskResultArrivalOutcome) -> TaskResultRecord {
    match outcome {
        TaskResultArrivalOutcome::Recorded(record) => record,
        TaskResultArrivalOutcome::StaleCredentialSet { .. } => {
            panic!("the fixture scrubbed at the current revision")
        }
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
        .append_reply_with_undelivered(reply, true, None)
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
        .append_reply_with_undelivered(reply, true, None)
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

/// Commits one Learning consent row so a formation claim can run through the
/// production inference claim path.
async fn seed_learning_consent(store: &Store) {
    let saved = save_consent(
        store,
        None,
        ConsentRecord {
            capability: CapabilityKind::Learning,
            id: String::from("consent-learning"),
            rev: ConsentRevision::from_u64(1),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            credential_id: String::from("cred-1"),
        },
    )
    .await;
    assert!(
        matches!(saved, ConsentCommitOutcome::Committed { .. }),
        "the learning consent must seed, got {saved:?}"
    );
}

/// Claims one Learning formation attempt through the production repository
/// boundary and returns its ticket (the durable claim the formation carries).
async fn claim_formation(
    store: &Store,
    ticket: InferenceTicketId,
    data_use: Vec<RawId>,
) -> InferenceTicketId {
    let outcome = store
        .begin_inference_attempt(InferenceAttempt {
            ticket,
            consumer: ConsumerKind::CompanionLearning,
            capability: CapabilityKind::Learning,
            purpose: PurposeKind::MemoryFormation,
            expected_consent: (
                String::from("consent-learning"),
                ConsentRevision::from_u64(1),
            ),
            expected_credential_set: CredentialSetRevision::initial(),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            task_agent: None,
            data_use,
            pricing: None,
            usage_estimate: None,
        })
        .await
        .expect("the formation claim must answer");
    assert_eq!(
        outcome,
        AttemptBeginOutcome::Started,
        "the formation claim must start"
    );
    ticket
}

#[tokio::test]
async fn the_admission_hold_probe_uses_the_source_correlation_index() {
    let store = open_memory().await.unwrap();
    // The admission association is bounded by the operation's covered
    // sources: the exact production statement must reach the attempt
    // correlation through its source index instead of scanning every
    // attempt's ordered rows. Boundedness itself is pinned by the behavior
    // tests above; this is the structural guard against a silent rewrite.
    let plan: Vec<String> = {
        let conn = store.conn.lock().unwrap();
        let mut explained = conn
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {}",
                crate::preservation::ASSOCIATE_ATTEMPTS_SQL
            ))
            .unwrap();
        explained
            .query_map(
                params![String::new(), 1_i64, "inference_attempt", String::new()],
                |row| row.get(3),
            )
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert!(!plan.is_empty(), "missing plan for the admission probe");
    assert!(
        plan.iter()
            .any(|line| line.contains("idx_inference_attempt_data_use_source")),
        "the admission probe must use the source-correlation index, plan: {plan:?}"
    );
    assert!(
        !plan
            .iter()
            .any(|line| line.contains("SCAN inference_attempt_data_use")),
        "the admission probe must not scan every attempt correlation, plan: {plan:?}"
    );
}

#[tokio::test]
async fn a_formation_claimed_before_completion_stays_held_after_it() {
    let store = open_memory().await.unwrap();
    let companion = RawId::new();
    let source = RawId::new();
    seed_learning_consent(&store).await;
    // R2 use-first: the formation's provider claim (with its ordered source
    // correlation) commits before the deletion condition.
    let ticket = claim_formation(&store, InferenceTicketId(RawId::new()), vec![source]).await;
    let current = admit(&store, "the secret plan", vec![source], Vec::new()).await;
    // The operation completes while the provider work is still in flight; the
    // condition, its material, and its source rows are gone.
    complete_via_a5(&store, current).await;
    assert!(current_conditions(&store).await.is_empty());

    // The delayed formation arrives with a clean paraphrase: no current
    // condition and no literal match, yet the claim hold still refuses it.
    let record = summary(companion, "a clean paraphrase of the plan", source, source);
    let mut delayed = change(
        &record,
        MemoryTarget::New {
            id: MemoryId::generate(),
        },
        "a clean paraphrase",
        ChangeKind::Initial,
    );
    delayed.claim = Some(LearningClaimRef::from_raw(ticket.0));
    assert_eq!(
        commit(&store, delayed).await,
        MemoryChangeOutcome::HeldForErasure,
        "a claim from before the interval is stale for erasure after completion"
    );
    assert_eq!(learning_rows(&store, companion), 0);
    // The objective attempt fact survives: the refused formation does not
    // erase the provider claim or its ordered correlation.
    let attempt = store
        .load_inference_attempt(ticket)
        .await
        .unwrap()
        .expect("the claimed attempt stays readable");
    assert_eq!(attempt.data_use, vec![source]);

    // A claim committed after completion is a new origin: the hold names the
    // claim, never the text.
    let fresh_source = RawId::new();
    let fresh_ticket =
        claim_formation(&store, InferenceTicketId(RawId::new()), vec![fresh_source]).await;
    let fresh = summary(companion, "a fresh note", fresh_source, fresh_source);
    let mut fresh_change = change(
        &fresh,
        MemoryTarget::New {
            id: MemoryId::generate(),
        },
        "a fresh note",
        ChangeKind::Initial,
    );
    fresh_change.claim = Some(LearningClaimRef::from_raw(fresh_ticket.0));
    assert!(
        matches!(
            commit(&store, fresh_change).await,
            MemoryChangeOutcome::Committed { .. }
        ),
        "a post-completion claim is not held"
    );
    // The fresh Summary evidence and its Memory.
    assert_eq!(learning_rows(&store, companion), 2);
}

// --- Task result arrival / adoption ---

#[tokio::test]
async fn condition_first_collects_a_task_result_body_and_keeps_the_fact() {
    let store = open_memory().await.unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    admit(&store, "the private key", Vec::new(), Vec::new()).await;

    let arrival = result_arrival(&store, delegation, "the report quotes the private key").await;
    let record = expect_recorded(
        store
            .record_task_result_arrival(arrival.clone())
            .await
            .expect("the arrival must record its fact"),
    );
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
    let arrival = result_arrival(&store, delegation, "final report mentions the private key").await;
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
    let retry = expect_recorded(
        store
            .record_task_result_arrival(arrival.clone())
            .await
            .expect("the covered retry must stay an idempotent replay"),
    );
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

#[tokio::test]
async fn a_delegation_claimed_before_completion_collects_its_delayed_result() {
    let store = open_memory().await.unwrap();
    // The task purpose itself carries the target at admission, so the
    // unsealed delegation is associated with the interval even without an
    // explicit source correlation.
    let (_task, delegation) = seed_workspace_execution(&store, "write about the private key").await;
    let current = admit(
        &store,
        "the private key",
        Vec::new(),
        vec![ParticipantOwnerRef::Task],
    )
    .await;
    // The production owner sweep erases the purpose copy; the hold written at
    // admission survives it.
    let participant = crate::TaskErasureParticipant::new(store.clone());
    drive_with_sources(
        &participant,
        current.condition(),
        ParticipantOwnerRef::Task,
        "the private key",
        Vec::new(),
    )
    .await;
    complete_via_a5(&store, current).await;
    assert!(current_conditions(&store).await.is_empty());

    let arrival = result_arrival(&store, delegation, "final report quotes the private key").await;
    let record = expect_recorded(
        store
            .record_task_result_arrival(arrival.clone())
            .await
            .expect("the delayed arrival must record its fact"),
    );
    assert_collected(record.body.text(), "the private key");
    assert_collected(&task_result_body(&store, arrival.result), "the private key");
    assert!(
        store
            .load_delegation_result(delegation)
            .await
            .unwrap()
            .is_some(),
        "the execution seal survives the collected body"
    );
}

#[tokio::test]
async fn a_task_agent_claim_source_associates_its_delegation() {
    let store = open_memory().await.unwrap();
    let (task, delegation) = seed_workspace_execution(&store, "write the report").await;
    // The purpose body is clean; the association comes from the already
    // claimed Task Agent turn's ordered source correlation.
    let saved = save_consent(&store, None, consent_record("consent-1", 1)).await;
    assert!(matches!(saved, ConsentCommitOutcome::Committed { .. }));
    let source = RawId::new();
    let task_agent = TaskAgentAttemptPremise {
        delegation: delegation.as_raw(),
        task: task.task.as_raw(),
        task_revision: RevisionInner::from_u64(task.revision.as_u64()),
        data_use: vec![source],
    };
    assert_eq!(
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
                task_agent: Some(task_agent),
                data_use: vec![source],
                pricing: None,
                usage_estimate: None,
            })
            .await
            .unwrap(),
        AttemptBeginOutcome::Started
    );
    let current = admit(&store, "the private key", vec![source], Vec::new()).await;
    complete_via_a5(&store, current).await;

    let arrival = result_arrival(&store, delegation, "an otherwise clean report").await;
    let record = expect_recorded(
        store
            .record_task_result_arrival(arrival.clone())
            .await
            .expect("the delayed arrival must record its fact"),
    );
    assert_collected(record.body.text(), "the private key");
    assert_collected(&task_result_body(&store, arrival.result), "the private key");
}

#[tokio::test]
async fn a_held_delegation_refuses_a_delayed_action_after_completion() {
    let store = open_memory().await.unwrap();
    let (task, delegation) = seed_workspace_execution(&store, "write about the private key").await;
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
    let current = admit(
        &store,
        "the private key",
        Vec::new(),
        vec![ParticipantOwnerRef::Task],
    )
    .await;
    let participant = crate::TaskErasureParticipant::new(store.clone());
    drive_with_sources(
        &participant,
        current.condition(),
        ParticipantOwnerRef::Task,
        "the private key",
        Vec::new(),
    )
    .await;
    complete_via_a5(&store, current).await;

    // A delayed Action from the held execution never starts, even though the
    // resolved target itself is clean.
    let target = std::env::temp_dir()
        .join("ene-held-delegation/report.txt")
        .to_string_lossy()
        .into_owned();
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
        "no attempt row exists for a held execution"
    );
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

// --- Dialogue prompt read-set correlation and read-side withholding ---

/// Commits one dialogue consent row so a dialogue claim can run through the
/// production inference claim path.
async fn seed_dialogue_claim_consent(store: &Store) {
    let saved = save_consent(
        store,
        None,
        ConsentRecord {
            capability: CapabilityKind::Dialogue,
            id: String::from("consent-dialogue"),
            rev: ConsentRevision::from_u64(1),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            credential_id: String::from("openai:main"),
        },
    )
    .await;
    assert!(
        matches!(saved, ConsentCommitOutcome::Committed { .. }),
        "the dialogue consent must seed, got {saved:?}"
    );
}

/// Claims one dialogue attempt through the production repository boundary,
/// carrying the assembled prompt's ordered read-set.
async fn claim_dialogue(
    store: &Store,
    ticket: InferenceTicketId,
    data_use: Vec<RawId>,
) -> AttemptBeginOutcome {
    store
        .begin_inference_attempt(InferenceAttempt {
            ticket,
            consumer: ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            purpose: PurposeKind::DialogueResponse,
            expected_consent: (
                String::from("consent-dialogue"),
                ConsentRevision::from_u64(1),
            ),
            expected_credential_set: CredentialSetRevision::initial(),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            task_agent: None,
            data_use,
            pricing: None,
            usage_estimate: None,
        })
        .await
        .expect("the dialogue claim must answer")
}

#[tokio::test]
async fn a_dialogue_claim_records_its_ordered_read_set_and_is_held_by_a_covered_source() {
    let store = open_memory().await.unwrap();
    seed_dialogue_claim_consent(&store).await;
    let first = RawId::new();
    let second = RawId::new();
    let ticket = InferenceTicketId(RawId::new());
    // Order and duplicates are the durable correlation: the prompt read the
    // first Memory twice and the second once.
    assert_eq!(
        claim_dialogue(&store, ticket, vec![first, second, first]).await,
        AttemptBeginOutcome::Started
    );
    let record = store
        .load_inference_attempt(ticket)
        .await
        .unwrap()
        .expect("the dialogue attempt must read");
    assert_eq!(record.task_agent, None);
    assert_eq!(
        record.data_use,
        vec![first, second, first],
        "the ordered read-set survives the claim and the read-back"
    );

    // A condition covering one read source holds a later dialogue claim
    // before any attempt row exists.
    let current = admit(&store, "the private key", vec![first], Vec::new()).await;
    assert_eq!(
        claim_dialogue(&store, InferenceTicketId(RawId::new()), vec![first]).await,
        AttemptBeginOutcome::DataUseHeld,
        "the same gate now sees the covering condition through the read-set"
    );
    // The already-claimed turn is associated with the interval through the
    // same source correlation the admission probe joins on.
    assert!(
        store.inference_claim_held(ticket.0).await.unwrap(),
        "the dialogue claim belongs to the deletion interval"
    );
    assert!(
        !store.inference_claim_held(RawId::new()).await.unwrap(),
        "an unrelated claim is not held"
    );
    assert!(
        current_conditions(&store)
            .await
            .contains(&current.condition())
    );
}

#[tokio::test]
async fn a_dialogue_claim_held_before_completion_refuses_its_delayed_reply_after_it() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = companion_with_generation(&store).await;
    seed_dialogue_claim_consent(&store).await;
    let owner = match append_owner(&store, companion, generation, "an ordinary question").await {
        HistoryAppendOutcome::CommittedAs { message } => message,
        other => panic!("the Owner append must commit, got {other:?}"),
    };
    // R2 use-first: the dialogue provider claim (with the prompt read-set)
    // commits before the deletion condition.
    let source = RawId::new();
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        claim_dialogue(&store, ticket, vec![source]).await,
        AttemptBeginOutcome::Started
    );
    let current = admit(&store, "the private key", vec![source], Vec::new()).await;
    // The operation completes while the provider work is still in flight.
    complete_via_a5(&store, current).await;
    assert!(current_conditions(&store).await.is_empty());

    // The delayed reply is a clean paraphrase: no literal target, no current
    // condition, yet the durable claim hold refuses adoption.
    let mut reply = history_command(companion, generation, "the answer avoids the exact words");
    reply.role = HistoryRole::Companion;
    reply.expected_owner_message = Some(owner);
    let (outcome, registered) = store
        .append_reply_with_undelivered(reply, true, Some(ticket.0))
        .await
        .expect("the delayed reply must answer");
    assert_eq!(
        outcome,
        HistoryAppendOutcome::HeldForErasure,
        "a claim from before the interval is stale for erasure after completion"
    );
    assert!(registered.is_none());
    assert_eq!(history_rows(&store, companion), 1);

    // A fresh claim after completion is a new origin: the hold names the
    // claim, never the text.
    let fresh_ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        claim_dialogue(&store, fresh_ticket, Vec::new()).await,
        AttemptBeginOutcome::Started
    );
    let mut fresh = history_command(companion, generation, "a fresh answer");
    fresh.role = HistoryRole::Companion;
    fresh.expected_owner_message = Some(owner);
    let (outcome, registered) = store
        .append_reply_with_undelivered(fresh, true, Some(fresh_ticket.0))
        .await
        .expect("the fresh reply must answer");
    assert!(
        matches!(outcome, HistoryAppendOutcome::CommittedAs { .. }),
        "a post-completion claim is not held, got {outcome:?}"
    );
    assert!(registered.is_some());
}

#[tokio::test]
async fn dialogue_context_reads_withhold_covered_history_and_memory() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = companion_with_generation(&store).await;
    let tainted =
        match append_owner(&store, companion, generation, "the private key lives here").await {
            HistoryAppendOutcome::CommittedAs { message } => message,
            other => panic!("the tainted append must commit, got {other:?}"),
        };
    let clean = match append_owner(&store, companion, generation, "an unrelated note").await {
        HistoryAppendOutcome::CommittedAs { message } => message,
        other => panic!("the clean append must commit, got {other:?}"),
    };
    let companion_raw = companion.as_raw();
    let record = summary(
        companion_raw,
        "the private key was mentioned",
        RawId::new(),
        RawId::new(),
    );
    let memory = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &record,
                MemoryTarget::New { id: memory },
                "recall the private key",
                ChangeKind::Initial
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));

    // Before the condition both reads hand the target-bearing rows over.
    let before = store
        .load_recent_timeline(companion, 8)
        .await
        .expect("the recent window must read");
    assert!(before.iter().any(|item| item.id == tainted));
    assert!(before.iter().any(|item| item.id == clean));
    let terms = vec![String::from("private")];
    let recalled = store
        .recall_candidates(companion_raw, &terms, 10)
        .await
        .expect("recall must answer");
    assert!(recalled.iter().any(|candidate| candidate.id == memory));

    admit(&store, "the private key", Vec::new(), Vec::new()).await;

    // While the condition is current the dialogue context reads withhold the
    // covered rows and keep the unrelated ones: no covered body reaches a
    // provider input.
    let filtered = store
        .load_recent_timeline(companion, 8)
        .await
        .expect("the filtered window must read");
    assert!(filtered.iter().all(|item| item.id != tainted));
    assert!(filtered.iter().any(|item| item.id == clean));
    let recalled = store
        .recall_candidates(companion_raw, &terms, 10)
        .await
        .expect("filtered recall must answer");
    assert!(
        recalled.iter().all(|candidate| candidate.id != memory),
        "a covered Memory is never offered to a context read"
    );
}

// --- Task Agent execution-local observation provenance (Stage 6 A4) ---
//
// A Task Agent execution observes Action results as execution-local text and
// replays them into later turns. The occurrence ledger records identity and
// correlation only; these tests pin the deletion behavior of that ledger: a
// body-observed workspace source (or an unsurveyable source) associates the
// delegation durably even after the mutable path is rewritten, a covered
// occurrence joins the ordered claim `data_use` coverage, a sealed paraphrase
// of that occurrence is collected by the Task owner sweep, and a body-free
// write confirmation of a clean path keeps adopting.

use ene_task::{TaskAgentObservationId, TaskAgentObservationPremise};

use ene_action::ActionCertainty;

/// Records one read observation through the production repository path,
/// returning its occurrence identity.
async fn record_observation(
    store: &Store,
    delegation: DelegationId,
    attempt: Option<RawId>,
    observed: Option<&str>,
) -> TaskAgentObservationId {
    let observation = TaskAgentObservationId::generate();
    store
        .record_task_agent_observation(TaskAgentObservationPremise {
            observation,
            delegation,
            attempt,
            observed: observed.map(str::to_owned),
            observed_at: fixture_clock(),
        })
        .await
        .expect("the observation must record");
    observation
}

/// Claims one AU5 attempt at `target` through the production start path.
async fn claim_attempt(
    store: &Store,
    task: TaskRef,
    delegation: DelegationId,
    target: &str,
    operation: OperationKind,
) -> RawId {
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
    let attempt = ActionAttemptId::generate();
    let outcome = store
        .insert_attempt_if_current(AttemptCommitPremise {
            attempt,
            delegation: delegation.as_raw(),
            task: task.task.as_raw(),
            task_revision: RevisionInner::from_u64(task.revision.as_u64()),
            workspace: assoc.as_raw(),
            real_target: RealTargetRef::from_canonical_path(target.to_owned()),
            operation,
            relied_evaluation: RawId::new(),
        })
        .await
        .expect("the attempt insert must answer");
    assert_eq!(
        outcome,
        ActionStartOutcome::Started,
        "the {operation:?} attempt must claim"
    );
    attempt.as_raw()
}

/// Claims one AU5 read attempt at `target` through the production start path.
async fn claim_read_attempt(
    store: &Store,
    task: TaskRef,
    delegation: DelegationId,
    target: &str,
) -> RawId {
    claim_attempt(store, task, delegation, target, OperationKind::Read).await
}

fn delegation_hold_rows(store: &Store, delegation: DelegationId) -> i64 {
    use_hold_rows(store, "task_delegation", delegation.as_raw())
}

fn use_hold_rows(store: &Store, kind: &str, use_id: RawId) -> i64 {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM erasure_use_hold WHERE use_kind=?1 AND use_id=?2",
        rusqlite::params![kind, crate::codec::encode_id(use_id)],
        |row| row.get(0),
    )
    .expect("the hold probe must read")
}

fn use_hold_rows_for_operation(
    store: &Store,
    kind: &str,
    use_id: RawId,
    operation: ene_preservation::DeletionOperationId,
) -> i64 {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM erasure_use_hold WHERE use_kind=?1 AND use_id=?2 AND operation_id=?3",
        rusqlite::params![
            kind,
            crate::codec::encode_id(use_id),
            crate::codec::encode_id(operation.as_raw())
        ],
        |row| row.get(0),
    )
    .expect("the operation-specific hold probe must read")
}

fn observation_source_rows(store: &Store, observation: TaskAgentObservationId) -> i64 {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM erasure_condition_source WHERE source=?1",
        [crate::codec::encode_id(observation.as_raw())],
        |row| row.get(0),
    )
    .expect("the source probe must read")
}

fn observation_path(store: &Store, observation: TaskAgentObservationId) -> Option<String> {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT path FROM task_agent_observation WHERE observation_id=?1",
        [crate::codec::encode_id(observation.as_raw())],
        |row| row.get(0),
    )
    .expect("the observation row must read")
}

fn observation_body_observed(store: &Store, observation: TaskAgentObservationId) -> bool {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT body_observed FROM task_agent_observation WHERE observation_id=?1",
        [crate::codec::encode_id(observation.as_raw())],
        |row| row.get(0),
    )
    .expect("the observation row must read")
}

fn action_certainty(store: &Store, attempt: RawId) -> String {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT certainty FROM action_attempt WHERE attempt_id=?1",
        [crate::codec::encode_id(attempt)],
        |row| row.get(0),
    )
    .expect("the attempt row must read")
}

/// Writes one workspace source carrying `content` and returns its path.
fn workspace_source(dir: &tempfile::TempDir, name: &str, content: &str) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, content).expect("the workspace source writes");
    path.to_string_lossy().into_owned()
}

/// Completes the Task participant's real sweep for the current condition.
async fn sweep_task_participant(store: &Store, current: DeletionOperationRef) {
    sweep_task_participant_for(store, current, "the private key").await;
}

async fn sweep_task_participant_for(store: &Store, current: DeletionOperationRef, text: &str) {
    let participant = crate::TaskErasureParticipant::new(store.clone());
    drive_with_sources(
        &participant,
        current.condition(),
        ParticipantOwnerRef::Task,
        text,
        Vec::new(),
    )
    .await;
}

#[tokio::test]
async fn an_observation_of_a_covered_workspace_source_holds_its_delayed_result() {
    let store = open_memory().await.unwrap();
    let files = tempfile::tempdir().unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    let source = workspace_source(&files, "input.txt", "the private key is here");
    let attempt = claim_read_attempt(&store, task, delegation, &source).await;
    let observation = record_observation(
        &store,
        delegation,
        Some(attempt),
        Some("read ok:\nthe private key is here"),
    )
    .await;

    // The admission survey cannot re-read the discarded observation body.
    // The workspace source still carries the target at admission in this
    // fixture, and a body-observed occurrence fails closed either way, so
    // the occurrence is published as a covered source and the execution is
    // associated with the operation.
    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    assert_eq!(delegation_hold_rows(&store, delegation), 1);
    assert_eq!(observation_source_rows(&store, observation), 1);

    // The objective ledger row survives the operation and keeps its
    // correlation: identity, delegation, producing attempt, and path.
    assert_eq!(
        observation_path(&store, observation).as_deref(),
        Some(source.as_str())
    );
    complete_via_a5(&store, current).await;
    assert!(current_conditions(&store).await.is_empty());

    // The delayed final result is a clean paraphrase: the exact-text
    // redaction cannot catch it, so only the durable occurrence
    // correspondence can. The body is collected to the fixed body-free form
    // and the execution still seals.
    let arrival = result_arrival(
        &store,
        delegation,
        "the report summarizes confidential material without quoting it",
    )
    .await;
    let record = expect_recorded(
        store
            .record_task_result_arrival(arrival)
            .await
            .expect("the delayed arrival must record its fact"),
    );
    assert_eq!(record.body.text(), crate::erasure::ERASED_MARKER);
    assert_eq!(
        task_result_body(&store, record.result),
        crate::erasure::ERASED_MARKER
    );
    assert!(
        store
            .load_delegation_result(delegation)
            .await
            .unwrap()
            .is_some(),
        "the execution seal survives the collected body"
    );
}

#[tokio::test]
async fn an_observation_of_a_workspace_source_rewritten_clean_still_holds_its_delayed_result() {
    let store = open_memory().await.unwrap();
    let files = tempfile::tempdir().unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    let source = workspace_source(&files, "input.txt", "the private key is here");
    let attempt = claim_read_attempt(&store, task, delegation, &source).await;
    let observation = record_observation(
        &store,
        delegation,
        Some(attempt),
        Some("read ok:\nthe private key is here"),
    )
    .await;
    assert!(
        observation_body_observed(&store, observation),
        "the occurrence reproduced a target-bearing body"
    );

    // The observed body is discarded; only path/provenance stays durable.
    // Rewriting the mutable path before admission must not let the survey
    // treat the occurrence as clean: current content is not the observed
    // version.
    std::fs::write(&source, "ordinary notes").expect("the workspace source is rewritten clean");
    assert!(
        !std::fs::read_to_string(&source)
            .expect("the rewritten source reads")
            .contains("the private key"),
        "the current workspace read is clean"
    );

    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    assert_eq!(
        observation_source_rows(&store, observation),
        1,
        "the body-observed occurrence stays a deletion-relevant source"
    );
    assert_eq!(
        delegation_hold_rows(&store, delegation),
        1,
        "the owning unsealed delegation is associated despite the clean path"
    );
    complete_via_a5(&store, current).await;
    assert!(current_conditions(&store).await.is_empty());

    let arrival = result_arrival(
        &store,
        delegation,
        "the report summarizes confidential material without quoting it",
    )
    .await;
    let record = expect_recorded(
        store
            .record_task_result_arrival(arrival)
            .await
            .expect("the delayed arrival must record its fact"),
    );
    assert_eq!(record.body.text(), crate::erasure::ERASED_MARKER);
    assert_eq!(
        task_result_body(&store, record.result),
        crate::erasure::ERASED_MARKER
    );

    // A fresh execution after completion is not a permanent ban.
    let (_fresh_task, fresh_delegation) =
        seed_workspace_execution(&store, "write a later report").await;
    let fresh = expect_recorded(
        store
            .record_task_result_arrival(
                result_arrival(&store, fresh_delegation, "a later ordinary report").await,
            )
            .await
            .expect("a post-completion execution may still record"),
    );
    assert_eq!(fresh.body.text(), "a later ordinary report");
}

#[tokio::test]
async fn a_sealed_paraphrase_of_an_observed_workspace_source_is_erased_after_the_source_is_rewritten_clean()
 {
    let store = open_memory().await.unwrap();
    let files = tempfile::tempdir().unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    let source = workspace_source(&files, "input.txt", "the private key is here");
    let attempt = claim_read_attempt(&store, task, delegation, &source).await;
    let observation = record_observation(
        &store,
        delegation,
        Some(attempt),
        Some("read ok:\nthe private key is here"),
    )
    .await;
    assert_eq!(
        store
            .compare_and_set_certainty(
                ActionAttemptId::from_raw(attempt),
                ActionCertainty::Unknown,
                ActionCertainty::ConfirmedSuccess,
                ene_action::EffectGrounds::ObservedAtTarget,
            )
            .await
            .unwrap(),
        ene_action::CertaintyUpdateOutcome::Updated
    );
    let paraphrase = "the report summarizes confidential material without quoting it";
    let arrival = result_arrival(&store, delegation, paraphrase).await;
    let record = expect_recorded(
        store
            .record_task_result_arrival(arrival)
            .await
            .expect("the paraphrase must seal the execution"),
    );
    assert_eq!(record.body.text(), paraphrase);
    assert!(
        store
            .load_delegation_result(delegation)
            .await
            .unwrap()
            .is_some(),
        "the execution is already sealed"
    );

    std::fs::write(&source, "ordinary notes").expect("the workspace source is rewritten clean");
    assert!(
        !std::fs::read_to_string(&source)
            .expect("the rewritten source reads")
            .contains("the private key"),
        "the current workspace read is clean"
    );

    let current = admit(
        &store,
        "the private key",
        Vec::new(),
        vec![ParticipantOwnerRef::Task],
    )
    .await;
    assert_eq!(
        observation_source_rows(&store, observation),
        1,
        "the sealed execution's body-observed occurrence is still a covered source"
    );
    assert_eq!(
        delegation_hold_rows(&store, delegation),
        1,
        "observation provenance associates the sealed execution"
    );

    sweep_task_participant(&store, current).await;
    assert_eq!(
        task_result_body(&store, record.result),
        crate::erasure::ERASED_MARKER,
        "the target-derived paraphrase cannot survive as undeleted personal data"
    );
    assert_eq!(
        action_certainty(&store, attempt),
        "confirmed_success",
        "Action certainty is an objective fact and is never rewritten"
    );
    assert!(
        store
            .load_delegation_result(delegation)
            .await
            .unwrap()
            .is_some(),
        "the execution seal survives the collected body"
    );

    complete_via_a5(&store, current).await;
    let remainder = {
        let guard = crate::codec::lock_shared(&store.conn);
        crate::erasure::exact_remainder_probe(&guard, "the private key")
            .expect("the remainder probe must answer")
    };
    assert_eq!(remainder, 0);

    let (_fresh_task, fresh_delegation) =
        seed_workspace_execution(&store, "write a later report").await;
    let fresh = expect_recorded(
        store
            .record_task_result_arrival(
                result_arrival(&store, fresh_delegation, "a later ordinary report").await,
            )
            .await
            .expect("a post-completion execution may still record"),
    );
    assert_eq!(fresh.body.text(), "a later ordinary report");
}

#[tokio::test]
async fn an_unreadable_observation_source_fails_closed_into_a_hold() {
    let store = open_memory().await.unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    // The producing attempt exists, but its source file does not: a
    // body-observed occurrence cannot prove the discarded body was unrelated
    // to the target, so the occurrence fails closed instead of being assumed
    // clean.
    let missing = std::env::temp_dir()
        .join("ene-stage6-observation-missing/source.txt")
        .to_string_lossy()
        .into_owned();
    let attempt = claim_read_attempt(&store, task, delegation, &missing).await;
    let _observation = record_observation(
        &store,
        delegation,
        Some(attempt),
        Some("read ok:\nsomething was observed here"),
    )
    .await;

    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    assert_eq!(
        delegation_hold_rows(&store, delegation),
        1,
        "an unsurveyable observed source associates the execution"
    );
    complete_via_a5(&store, current).await;

    let arrival = result_arrival(
        &store,
        delegation,
        "a clean paraphrase of unknown provenance",
    )
    .await;
    let record = expect_recorded(
        store
            .record_task_result_arrival(arrival)
            .await
            .expect("the delayed arrival must record its fact"),
    );
    assert_eq!(record.body.text(), crate::erasure::ERASED_MARKER);
}

#[tokio::test]
async fn a_write_confirmation_observation_of_a_clean_path_leaves_the_delegation_adoptable() {
    let store = open_memory().await.unwrap();
    let files = tempfile::tempdir().unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    let source = workspace_source(&files, "output.txt", "ordinary notes");
    let attempt = claim_attempt(&store, task, delegation, &source, OperationKind::Create).await;
    // A write confirmation reproduces no workspace content: the occurrence
    // has no observed body, so a currently-clean path can still be proven
    // unrelated to the target.
    let observation = record_observation(&store, delegation, Some(attempt), None).await;
    assert!(!observation_body_observed(&store, observation));
    assert_eq!(observation_source_rows(&store, observation), 0);

    admit(&store, "another private secret", Vec::new(), Vec::new()).await;
    assert_eq!(delegation_hold_rows(&store, delegation), 0);

    assert_eq!(
        store
            .compare_and_set_certainty(
                ActionAttemptId::from_raw(attempt),
                ActionCertainty::Unknown,
                ActionCertainty::ConfirmedSuccess,
                ene_action::EffectGrounds::ObservedAtTarget,
            )
            .await
            .unwrap(),
        ene_action::CertaintyUpdateOutcome::Updated
    );
    let arrival = result_arrival(&store, delegation, "the ordinary report").await;
    let recorded = expect_recorded(
        store
            .record_task_result_arrival(arrival)
            .await
            .expect("the clean arrival must record"),
    );
    let acceptance = store
        .adopt_result(ene_task::TaskResultAdoptionClaim {
            result: recorded.result,
            attempt_refs: vec![attempt],
        })
        .await
        .expect("the clean adoption must answer");
    assert_eq!(
        acceptance,
        ene_task::TaskResultAcceptance::AdoptedAsCompletion(task),
        "a body-free write confirmation of a clean path still completes"
    );
}

#[tokio::test]
async fn an_observation_body_covered_at_mint_is_published_and_held() {
    let store = open_memory().await.unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    // A clean source path: neither the purpose nor the attempt target
    // carries the target. Admission still associates the unsealed read/list
    // execution; the receiving boundary then publishes the occurrence when
    // the observed body itself is covered.
    let source = std::env::temp_dir()
        .join("ene-stage6-observation-clean/source.txt")
        .to_string_lossy()
        .into_owned();
    let attempt = claim_read_attempt(&store, task, delegation, &source).await;
    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    assert_eq!(
        delegation_hold_rows(&store, delegation),
        1,
        "an unsealed body-observing Action is associated at admission by execution identity"
    );

    let observation = record_observation(
        &store,
        delegation,
        Some(attempt),
        Some("read ok:\nthe private key appeared after the condition started"),
    )
    .await;
    assert_eq!(
        delegation_hold_rows(&store, delegation),
        1,
        "the receiving boundary associates a body observed under the current condition"
    );
    assert_eq!(observation_source_rows(&store, observation), 1);
    complete_via_a5(&store, current).await;

    let arrival = result_arrival(&store, delegation, "a later paraphrase of what was read").await;
    let record = expect_recorded(
        store
            .record_task_result_arrival(arrival)
            .await
            .expect("the delayed arrival must record its fact"),
    );
    assert_eq!(record.body.text(), crate::erasure::ERASED_MARKER);
}

#[tokio::test]
async fn a_covered_observation_source_refuses_a_later_task_agent_claim() {
    let store = open_memory().await.unwrap();
    let files = tempfile::tempdir().unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    let source = workspace_source(&files, "input.txt", "the private key is here");
    let attempt = claim_read_attempt(&store, task, delegation, &source).await;
    let observation = record_observation(
        &store,
        delegation,
        Some(attempt),
        Some("read ok:\nthe private key is here"),
    )
    .await;
    admit(&store, "the private key", Vec::new(), Vec::new()).await;

    // The production claim path (AU14) compares the ordered `data_use`
    // against the published covered sources: an observation occurrence that
    // consumed the covered source refuses the send before any attempt row,
    // even though the prompt itself is not compared.
    let saved = save_consent(&store, None, consent_record("consent-observation", 1)).await;
    assert!(matches!(saved, ConsentCommitOutcome::Committed { .. }));
    let ticket = InferenceTicketId(RawId::new());
    let outcome = store
        .begin_inference_attempt(InferenceAttempt {
            ticket,
            consumer: ConsumerKind::TaskAgent,
            capability: CapabilityKind::Dialogue,
            purpose: PurposeKind::TaskAgentTurn,
            expected_consent: (
                String::from("consent-observation"),
                ConsentRevision::from_u64(1),
            ),
            expected_credential_set: CredentialSetRevision::initial(),
            provider: String::from("acme"),
            model: String::from("dialogue-1"),
            task_agent: Some(TaskAgentAttemptPremise {
                delegation: delegation.as_raw(),
                task: task.task.as_raw(),
                task_revision: RevisionInner::from_u64(task.revision.as_u64()),
                data_use: vec![observation.as_raw()],
            }),
            data_use: vec![observation.as_raw()],
            pricing: None,
            usage_estimate: None,
        })
        .await
        .unwrap();
    assert_eq!(
        outcome,
        AttemptBeginOutcome::DataUseHeld,
        "the claim gate sees the covered observation occurrence"
    );
    assert!(
        store
            .load_inference_attempt(ticket)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn reopening_the_store_keeps_the_observation_hold_and_replay() {
    let dir = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let path = dir.path().join("observation-restart.db");
    let store = Store::open(&path).await.unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    let source = workspace_source(&files, "input.txt", "the private key is here");
    let attempt = claim_read_attempt(&store, task, delegation, &source).await;
    let observation = record_observation(
        &store,
        delegation,
        Some(attempt),
        Some("read ok:\nthe private key is here"),
    )
    .await;
    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    assert_eq!(delegation_hold_rows(&store, delegation), 1);
    drop(store);

    // The hold, the source publication, and the occurrence ledger are durable:
    // a restart neither loses the correspondence nor completes the operation.
    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(delegation_hold_rows(&reopened, delegation), 1);
    assert_eq!(observation_source_rows(&reopened, observation), 1);
    assert!(
        current_conditions(&reopened)
            .await
            .contains(&current.condition())
    );

    // Re-recording the same occurrence identity with the same correlation is
    // an idempotent replay, never a second row.
    reopened
        .record_task_agent_observation(TaskAgentObservationPremise {
            observation,
            delegation,
            attempt: Some(attempt),
            observed: Some(String::from("read ok:\nthe private key is here")),
            observed_at: fixture_clock(),
        })
        .await
        .expect("the same-identity replay is idempotent");
    assert_eq!(observation_source_rows(&reopened, observation), 1);

    complete_via_a5(&reopened, current).await;
    let arrival = result_arrival(
        &reopened,
        delegation,
        "a paraphrase that omits the exact words",
    )
    .await;
    let record = expect_recorded(
        reopened
            .record_task_result_arrival(arrival)
            .await
            .expect("the delayed arrival must record its fact"),
    );
    assert_eq!(record.body.text(), crate::erasure::ERASED_MARKER);
}

#[tokio::test]
async fn the_task_participant_sweeps_the_observation_path_copy() {
    let store = open_memory().await.unwrap();
    let files = tempfile::tempdir().unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    // The resolved target path itself carries the target: it is a mechanical
    // body column of the observation ledger, exactly like the Action owner's
    // `real_target` copy.
    let source = workspace_source(&files, "the private key.txt", "ordinary notes");
    let attempt = claim_read_attempt(&store, task, delegation, &source).await;
    let observation = record_observation(
        &store,
        delegation,
        Some(attempt),
        Some("read ok:\nordinary notes"),
    )
    .await;
    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;

    sweep_task_participant(&store, current).await;
    let swept = observation_path(&store, observation).expect("the row survives the sweep");
    assert!(
        !swept.contains("the private key"),
        "the Task owner sweep redacts the occurrence path copy, got {swept}"
    );
    assert!(
        std::path::Path::new(&swept).is_absolute(),
        "the swept path stays a readable canonical locator"
    );
}

#[tokio::test]
async fn an_observation_path_covered_after_admission_is_published_and_held() {
    let store = open_memory().await.unwrap();
    let files = tempfile::tempdir().unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    // The producing attempt's resolved target path carries the target, but
    // the occurrence row does not exist when admission runs. The unsealed
    // read/list Action is still associated by execution identity; the
    // receiving boundary must also publish the recorded path so a consuming
    // claim's data_use names a covered source.
    let source = workspace_source(&files, "the private key.txt", "ordinary notes");
    let attempt = claim_read_attempt(&store, task, delegation, &source).await;
    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    assert_eq!(
        delegation_hold_rows(&store, delegation),
        1,
        "an unsealed body-observing Action is associated at admission; the occurrence row is not required"
    );

    let observation = record_observation(
        &store,
        delegation,
        Some(attempt),
        Some("read ok:\nordinary notes"),
    )
    .await;
    assert_eq!(
        observation_source_rows(&store, observation),
        1,
        "the covered resolved path is published under the occurrence identity"
    );
    assert_eq!(
        delegation_hold_rows(&store, delegation),
        1,
        "the covered resolved path associates the execution"
    );
    // The Task owner sweep redacts the observation path copy and the Action
    // owner sweep redacts the attempt's resolved target copy before
    // completion; the system-wide remainder probe would otherwise collect
    // both as remainders.
    sweep_task_participant(&store, current).await;
    let action_participant = crate::ActionErasureParticipant::new(store.clone());
    drive_with_sources(
        &action_participant,
        current.condition(),
        ParticipantOwnerRef::Action,
        "the private key",
        Vec::new(),
    )
    .await;
    complete_via_a5(&store, current).await;

    let arrival = result_arrival(
        &store,
        delegation,
        "a clean paraphrase that omits the path and the exact words",
    )
    .await;
    let record = expect_recorded(
        store
            .record_task_result_arrival(arrival)
            .await
            .expect("the delayed arrival must record its fact"),
    );
    assert_eq!(record.body.text(), crate::erasure::ERASED_MARKER);
}

fn observation_rows(store: &Store) -> i64 {
    store
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM task_agent_observation", [], |row| {
            row.get(0)
        })
        .expect("the observation count must read")
}

/// Blocker 1: the body-in-memory window after a workspace read and before the
/// occurrence write. Admission must associate the unsealed read/list
/// execution without an observation row, so a delayed write after completion
/// stays old-origin. A later fresh Owner origin of the same string is not a
/// ban.
#[tokio::test]
async fn an_observation_write_parked_across_completion_stays_old_origin() {
    let store = open_memory().await.unwrap();
    let files = tempfile::tempdir().unwrap();
    let (companion, _) = companion_with_generation(&store).await;
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    let source = workspace_source(&files, "input.txt", "the private key is here");
    let attempt = claim_read_attempt(&store, task, delegation, &source).await;
    assert_eq!(
        action_certainty(&store, attempt),
        "unknown",
        "start is an objective fact; certainty is still Unknown"
    );

    store.arm_observation_write_park_for_tests();
    let parked = {
        let store = store.clone();
        tokio::spawn(async move {
            record_observation(
                &store,
                delegation,
                Some(attempt),
                Some("read ok:\nthe private key is here"),
            )
            .await
        })
    };
    store.wait_observation_write_park_for_tests().await;
    assert_eq!(
        observation_rows(&store),
        0,
        "the occurrence must not be durable while the body is still in memory"
    );

    std::fs::write(&source, "ordinary notes").expect("the workspace source is rewritten clean");
    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    assert_eq!(
        delegation_hold_rows(&store, delegation),
        1,
        "the unobserved read/list execution is associated at admission"
    );
    assert_eq!(observation_rows(&store), 0);
    complete_via_a5(&store, current).await;
    assert!(current_conditions(&store).await.is_empty());

    store.release_observation_write_park_for_tests();
    let observation = parked.await.expect("the parked observation write joins");
    assert_eq!(observation_rows(&store), 1);
    assert!(
        observation_body_observed(&store, observation),
        "the ledger records that a workspace body was reproduced"
    );
    assert!(
        store
            .task_delegation_held(delegation.as_raw())
            .await
            .expect("the hold must read"),
        "the hold outlives completion so the delayed body stays old-origin"
    );

    let arrival = result_arrival(
        &store,
        delegation,
        "the report summarizes confidential material without quoting it",
    )
    .await;
    let record = expect_recorded(
        store
            .record_task_result_arrival(arrival)
            .await
            .expect("the delayed arrival must record its fact"),
    );
    assert_eq!(record.body.text(), crate::erasure::ERASED_MARKER);
    assert_eq!(
        action_certainty(&store, attempt),
        "unknown",
        "Action certainty is never rewritten by deletion"
    );

    let fresh = seed_task(&store, companion, "please keep the private key").await;
    let loaded = store
        .load_task(fresh.task)
        .await
        .unwrap()
        .expect("the fresh origin must load");
    assert_eq!(
        loaded.revision.purpose_text.text, "please keep the private key",
        "a post-completion Owner origin of the same string is accepted"
    );
}

/// Blocker 1 remainder: Action start during Finalizing must still hold the
/// execution. Lifecycle §11 collects delayed arrival onto Active / Held /
/// Finalizing; skipping Finalizing would let the parked observation write
/// look like a fresh origin after the completion commit.
#[tokio::test]
async fn an_observation_write_parked_across_finalizing_completion_stays_old_origin() {
    let store = open_memory().await.unwrap();
    let files = tempfile::tempdir().unwrap();
    let (companion, _) = companion_with_generation(&store).await;
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    let source = workspace_source(&files, "input.txt", "ordinary notes");

    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    enter_finalizing_via_a5(&store, current).await;
    assert_eq!(
        delegation_hold_rows(&store, delegation),
        0,
        "purpose/path carry no target, so admission does not hold the execution"
    );

    std::fs::write(&source, "the private key is here").expect("the workspace source is rewritten");
    let attempt = claim_read_attempt(&store, task, delegation, &source).await;
    assert_eq!(
        action_certainty(&store, attempt),
        "unknown",
        "start is an objective fact; certainty is still Unknown"
    );
    assert_eq!(
        delegation_hold_rows(&store, delegation),
        1,
        "a read that starts while Finalizing is associated by execution identity"
    );

    store.arm_observation_write_park_for_tests();
    let parked = {
        let store = store.clone();
        tokio::spawn(async move {
            record_observation(
                &store,
                delegation,
                Some(attempt),
                Some("read ok:\nthe private key is here"),
            )
            .await
        })
    };
    store.wait_observation_write_park_for_tests().await;
    assert_eq!(
        observation_rows(&store),
        0,
        "the occurrence must not be durable while the body is still in memory"
    );

    std::fs::write(&source, "ordinary notes").expect("the workspace source is rewritten clean");
    assert_eq!(
        store
            .complete_deletion_finalizing(current)
            .await
            .expect("the completion commit must answer"),
        ene_preservation::DeletionFinalizationOutcome::Completed
    );
    assert!(current_conditions(&store).await.is_empty());

    store.release_observation_write_park_for_tests();
    let observation = parked.await.expect("the parked observation write joins");
    assert_eq!(observation_rows(&store), 1);
    assert!(
        observation_body_observed(&store, observation),
        "the ledger records that a workspace body was reproduced"
    );
    assert!(
        store
            .task_delegation_held(delegation.as_raw())
            .await
            .expect("the hold must read"),
        "the hold outlives completion so the delayed body stays old-origin"
    );

    let arrival = result_arrival(
        &store,
        delegation,
        "the report summarizes confidential material without quoting it",
    )
    .await;
    let record = expect_recorded(
        store
            .record_task_result_arrival(arrival)
            .await
            .expect("the delayed arrival must record its fact"),
    );
    assert_eq!(record.body.text(), crate::erasure::ERASED_MARKER);
    assert_eq!(
        action_certainty(&store, attempt),
        "unknown",
        "Action certainty is never rewritten by deletion"
    );

    let fresh = seed_task(&store, companion, "please keep the private key").await;
    let loaded = store
        .load_task(fresh.task)
        .await
        .unwrap()
        .expect("the fresh origin must load");
    assert_eq!(
        loaded.revision.purpose_text.text, "please keep the private key",
        "a post-completion Owner origin of the same string is accepted"
    );
}

/// Immediate-writer dual of the Finalizing hold: when the completion commit
/// lands first, a later read/list start is a post-closure origin and must
/// not inherit a hold from the closed operation.
#[tokio::test]
async fn a_read_started_after_finalizing_completion_is_a_fresh_origin() {
    let store = open_memory().await.unwrap();
    let files = tempfile::tempdir().unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    let source = workspace_source(&files, "input.txt", "ordinary notes");

    let current = admit(&store, "the private key", Vec::new(), Vec::new()).await;
    enter_finalizing_via_a5(&store, current).await;
    assert_eq!(
        store
            .complete_deletion_finalizing(current)
            .await
            .expect("the completion commit must answer"),
        ene_preservation::DeletionFinalizationOutcome::Completed
    );
    assert!(current_conditions(&store).await.is_empty());

    std::fs::write(&source, "the private key is here").expect("the workspace source is rewritten");
    let attempt = claim_read_attempt(&store, task, delegation, &source).await;
    assert_eq!(
        action_certainty(&store, attempt),
        "unknown",
        "start is an objective fact; certainty is still Unknown"
    );
    assert_eq!(
        delegation_hold_rows(&store, delegation),
        0,
        "a read that starts after completion is a fresh origin"
    );
    assert!(
        !store
            .task_delegation_held(delegation.as_raw())
            .await
            .expect("the hold must read"),
        "the closed operation must not hold a post-completion start"
    );
}

/// P1-2: one sealed paraphrase of a body that carried two exact targets must
/// correspond to both unfinished operations, so driving only B still collects
/// the result. A retry of the same operation must not duplicate the row.
#[tokio::test]
async fn one_delegation_corresponds_to_two_unfinished_operations() {
    let store = open_memory().await.unwrap();
    let files = tempfile::tempdir().unwrap();
    let (_task, delegation) = seed_workspace_execution(&store, "write the report").await;
    let task = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .unwrap()
        .task;
    let target_a = "alpha-deletion-token";
    let target_b = "beta-deletion-token";
    let source = workspace_source(
        &files,
        "input.txt",
        &format!("{target_a} and {target_b} live in this file"),
    );
    let attempt = claim_read_attempt(&store, task, delegation, &source).await;
    let _observation = record_observation(
        &store,
        delegation,
        Some(attempt),
        Some(&format!(
            "read ok:\n{target_a} and {target_b} live in this file"
        )),
    )
    .await;
    let paraphrase = "the report restates both confidential items without quoting either";
    let arrival = result_arrival(&store, delegation, paraphrase).await;
    let record = expect_recorded(
        store
            .record_task_result_arrival(arrival)
            .await
            .expect("the paraphrase must seal the execution"),
    );
    assert_eq!(record.body.text(), paraphrase);
    assert!(
        !record.body.text().contains(target_a) && !record.body.text().contains(target_b),
        "the sealed result must omit both exact texts"
    );

    let op_a = admit(
        &store,
        target_a,
        Vec::new(),
        vec![ParticipantOwnerRef::Task],
    )
    .await;
    let op_b = admit(
        &store,
        target_b,
        Vec::new(),
        vec![ParticipantOwnerRef::Task],
    )
    .await;
    assert_eq!(
        use_hold_rows(&store, "task_delegation", delegation.as_raw()),
        2,
        "the same delegation corresponds to both operations"
    );
    assert_eq!(
        use_hold_rows_for_operation(
            &store,
            "task_delegation",
            delegation.as_raw(),
            op_a.operation
        ),
        1
    );
    assert_eq!(
        use_hold_rows_for_operation(
            &store,
            "task_delegation",
            delegation.as_raw(),
            op_b.operation
        ),
        1
    );
    {
        let guard = crate::codec::lock_shared(&store.conn);
        assert!(
            crate::preservation::held_use_for_operation(
                &guard,
                crate::preservation::USE_KIND_TASK_DELEGATION,
                delegation.as_raw(),
                &crate::codec::encode_id(op_b.operation.as_raw()),
            )
            .expect("the operation-specific hold must read"),
            "operation-specific lookup finds B independently of A"
        );
    }

    sweep_task_participant_for(&store, op_b, target_b).await;
    assert_eq!(
        task_result_body(&store, record.result),
        crate::erasure::ERASED_MARKER,
        "driving only B still collects the semantically derived result"
    );

    complete_via_a5(&store, op_b).await;
    assert_eq!(
        use_hold_rows_for_operation(
            &store,
            "task_delegation",
            delegation.as_raw(),
            op_a.operation
        ),
        1,
        "B's completion must not drop A's association"
    );
    assert_eq!(
        use_hold_rows_for_operation(
            &store,
            "task_delegation",
            delegation.as_raw(),
            op_b.operation
        ),
        1,
        "the hold outlives B's completion"
    );

    // A retry of the same operation's association must not duplicate the row.
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
             VALUES ('task_delegation', ?1, ?2, ?3)",
            rusqlite::params![
                crate::codec::encode_id(delegation.as_raw()),
                crate::codec::encode_id(op_b.operation.as_raw()),
                fixture_clock().to_rfc3339()
            ],
        )
        .expect("a retried association insert must run");
    }
    assert_eq!(
        use_hold_rows_for_operation(
            &store,
            "task_delegation",
            delegation.as_raw(),
            op_b.operation
        ),
        1,
        "retrying the same operation must not duplicate the correspondence"
    );

    complete_via_a5(&store, op_a).await;
    assert_eq!(
        use_hold_rows(&store, "task_delegation", delegation.as_raw()),
        2,
        "A's completion must not drop B's historical association"
    );
}

/// P1-2 remainder: one inference claim whose data_use covers two targets is
/// associated with both operations.
#[tokio::test]
async fn one_inference_claim_corresponds_to_two_unfinished_operations() {
    let store = open_memory().await.unwrap();
    seed_learning_consent(&store).await;
    let (companion, generation) = companion_with_generation(&store).await;
    let target_a = "claim-alpha-token";
    let target_b = "claim-beta-token";
    let source_a = match append_owner(&store, companion, generation, target_a).await {
        HistoryAppendOutcome::CommittedAs { message } => message,
        other => panic!("the first source must commit, got {other:?}"),
    };
    let source_b = match append_owner(&store, companion, generation, target_b).await {
        HistoryAppendOutcome::CommittedAs { message } => message,
        other => panic!("the second source must commit, got {other:?}"),
    };
    let ticket = InferenceTicketId(RawId::new());
    claim_formation(&store, ticket, vec![source_a, source_b]).await;

    let op_a = admit(&store, target_a, vec![source_a], Vec::new()).await;
    let op_b = admit(&store, target_b, vec![source_b], Vec::new()).await;
    assert_eq!(
        use_hold_rows(&store, "inference_attempt", ticket.0),
        2,
        "the same claim corresponds to both operations"
    );
    assert_eq!(
        use_hold_rows_for_operation(&store, "inference_attempt", ticket.0, op_a.operation),
        1
    );
    assert_eq!(
        use_hold_rows_for_operation(&store, "inference_attempt", ticket.0, op_b.operation),
        1
    );
    assert!(
        store
            .inference_claim_held(ticket.0)
            .await
            .expect("the boolean hold must read"),
        "held_use is true when any operation corresponds"
    );
}

/// P1-1 store path: a formation identity published before the Learning claim
/// stays old-origin after the covering History is swept and the operation
/// completes. A genuine post-completion source of the same string is not held.
#[tokio::test]
async fn a_learning_formation_taken_off_the_queue_stays_old_origin_after_completion() {
    let store = open_memory().await.unwrap();
    seed_learning_consent(&store).await;
    let (companion, generation) = companion_with_generation(&store).await;
    let target = "queued-formation-canary";
    let source = match append_owner(
        &store,
        companion,
        generation,
        &format!("please remember {target}"),
    )
    .await
    {
        HistoryAppendOutcome::CommittedAs { message } => message,
        other => panic!("the source must commit, got {other:?}"),
    };

    let formation = store
        .begin_learning_formation(companion.as_raw(), vec![source])
        .await
        .expect("the formation identity must publish");
    let attempts: i64 = {
        let conn = store.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM inference_attempt", [], |row| {
            row.get(0)
        })
        .expect("the attempt count must read")
    };
    assert_eq!(attempts, 0, "the Learning claim must not exist yet");

    let current = admit(&store, target, vec![source], Vec::new()).await;
    assert_eq!(
        use_hold_rows(&store, "learning_formation", formation),
        1,
        "admission associates the in-flight formation"
    );

    let companion_participant = crate::CompanionErasureParticipant::new(store.clone());
    drive_with_sources(
        &companion_participant,
        current.condition(),
        ParticipantOwnerRef::Companion,
        target,
        Vec::new(),
    )
    .await;
    complete_via_a5(&store, current).await;
    assert!(current_conditions(&store).await.is_empty());
    assert!(
        store
            .learning_formation_must_refuse(formation)
            .await
            .expect("the refuse check must read"),
        "the hold outlives completion so the stale transcript stays old-origin"
    );

    let stale_ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(InferenceAttempt {
                ticket: stale_ticket,
                consumer: ConsumerKind::CompanionLearning,
                capability: CapabilityKind::Learning,
                purpose: PurposeKind::MemoryFormation,
                expected_consent: (
                    String::from("consent-learning"),
                    ConsentRevision::from_u64(1),
                ),
                expected_credential_set: CredentialSetRevision::initial(),
                provider: String::from("openai"),
                model: String::from("dialogue-1"),
                task_agent: None,
                data_use: vec![source],
                pricing: None,
                usage_estimate: None,
            })
            .await
            .expect("the stale claim must answer"),
        AttemptBeginOutcome::DataUseHeld,
        "a delayed claim from the popped candidate must not start"
    );

    store
        .settle_learning_formation(formation)
        .await
        .expect("the stale identity settles");

    let fresh = match append_owner(
        &store,
        companion,
        generation,
        &format!("please remember {target} again"),
    )
    .await
    {
        HistoryAppendOutcome::CommittedAs { message } => message,
        other => panic!("the fresh origin must commit, got {other:?}"),
    };
    let fresh_formation = store
        .begin_learning_formation(companion.as_raw(), vec![fresh])
        .await
        .expect("the fresh formation identity must publish");
    assert!(
        !store
            .learning_formation_must_refuse(fresh_formation)
            .await
            .expect("the fresh refuse check must read"),
        "a post-completion Owner origin is a new formation, not a ban"
    );
    claim_formation(&store, InferenceTicketId(RawId::new()), vec![fresh]).await;
}

#[tokio::test]
async fn a_host_transient_arrival_after_verified_opens_a_new_sweep() {
    let store = open_memory().await.unwrap();
    let current = admit(
        &store,
        "secret body",
        Vec::new(),
        vec![ParticipantOwnerRef::HostTransient],
    )
    .await;
    assert_eq!(
        store
            .record_participant_completion(ParticipantCompletionFact::verified(
                current.condition(),
                ParticipantOwnerRef::HostTransient,
                0,
                fixture_clock(),
            ))
            .await
            .expect("the verified fact must record"),
        ParticipantCompletionOutcome::Recorded(ene_preservation::ParticipantProgress::Verified {
            sweep: current.sweep,
        })
    );
    assert_eq!(
        store
            .note_host_transient_learning_arrival(vec![current.operation])
            .await
            .expect("the arrival must publish"),
        crate::HostTransientArrivalOutcome::SweepOpened
    );
    let unfinished = store
        .unfinished_deletions(None, 100)
        .await
        .expect("unfinished operations read");
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].phase, DeletionOperationPhase::Active);
    assert_ne!(unfinished[0].current.sweep, current.sweep);
    let host = store
        .deletion_participants(unfinished[0].current.operation, None, 100)
        .await
        .expect("participant rows read")
        .into_iter()
        .find(|record| record.participant.owner == ParticipantOwnerRef::HostTransient)
        .expect("HostTransient remains required");
    assert!(
        !host.progress.is_verified(),
        "the previous verification must not count for the new sweep"
    );
}

#[tokio::test]
async fn a_host_transient_arrival_during_finalizing_returns_to_active() {
    let store = open_memory().await.unwrap();
    let current = admit(
        &store,
        "secret body",
        Vec::new(),
        vec![ParticipantOwnerRef::HostTransient],
    )
    .await;
    enter_finalizing_via_a5(&store, current).await;
    assert_eq!(
        store
            .note_host_transient_learning_arrival(vec![current.operation])
            .await
            .expect("the arrival must publish"),
        crate::HostTransientArrivalOutcome::SweepOpened
    );
    let unfinished = store
        .unfinished_deletions(None, 100)
        .await
        .expect("unfinished operations read");
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].phase, DeletionOperationPhase::Active);
    assert_ne!(unfinished[0].current.sweep, current.sweep);
}

#[tokio::test]
async fn a_host_transient_arrival_after_completion_does_not_reopen() {
    let store = open_memory().await.unwrap();
    let current = admit(
        &store,
        "secret body",
        Vec::new(),
        vec![ParticipantOwnerRef::HostTransient],
    )
    .await;
    complete_via_a5(&store, current).await;
    assert_eq!(
        store
            .note_host_transient_learning_arrival(vec![current.operation])
            .await
            .expect("the arrival must publish"),
        crate::HostTransientArrivalOutcome::Unchanged
    );
    let unfinished = store
        .unfinished_deletions(None, 100)
        .await
        .expect("unfinished operations read");
    assert!(
        unfinished.is_empty(),
        "the completed operation stays closed"
    );
}

#[tokio::test]
async fn a_host_transient_arrival_invalidates_only_the_named_operation() {
    let store = open_memory().await.unwrap();
    let a = admit(
        &store,
        "secret-a",
        Vec::new(),
        vec![ParticipantOwnerRef::HostTransient],
    )
    .await;
    let b = admit(
        &store,
        "secret-b",
        Vec::new(),
        vec![ParticipantOwnerRef::HostTransient],
    )
    .await;
    for current in [a, b] {
        assert_eq!(
            store
                .record_participant_completion(ParticipantCompletionFact::verified(
                    current.condition(),
                    ParticipantOwnerRef::HostTransient,
                    0,
                    fixture_clock(),
                ))
                .await
                .expect("the verified fact must record"),
            ParticipantCompletionOutcome::Recorded(
                ene_preservation::ParticipantProgress::Verified {
                    sweep: current.sweep,
                }
            )
        );
    }
    assert_eq!(
        store
            .note_host_transient_learning_arrival(vec![a.operation])
            .await
            .expect("the A arrival must publish"),
        crate::HostTransientArrivalOutcome::SweepOpened
    );
    let unfinished = store
        .unfinished_deletions(None, 100)
        .await
        .expect("unfinished operations read");
    let record_a = unfinished
        .iter()
        .find(|record| record.current.operation == a.operation)
        .expect("A stays unfinished");
    let record_b = unfinished
        .iter()
        .find(|record| record.current.operation == b.operation)
        .expect("B stays unfinished");
    assert_eq!(record_a.phase, DeletionOperationPhase::Active);
    assert_ne!(record_a.current.sweep, a.sweep);
    assert_eq!(record_b.phase, DeletionOperationPhase::Active);
    assert_eq!(record_b.current.sweep, b.sweep);
    let host_b = store
        .deletion_participants(b.operation, None, 100)
        .await
        .expect("B participants read")
        .into_iter()
        .find(|record| record.participant.owner == ParticipantOwnerRef::HostTransient)
        .expect("B HostTransient remains required");
    assert!(
        host_b.progress.is_verified(),
        "an unrelated operation must keep its HostTransient verification"
    );

    assert_eq!(
        store
            .note_host_transient_learning_arrival(vec![b.operation])
            .await
            .expect("the B arrival must publish"),
        crate::HostTransientArrivalOutcome::SweepOpened
    );
    let unfinished = store
        .unfinished_deletions(None, 100)
        .await
        .expect("unfinished operations read");
    let record_b = unfinished
        .iter()
        .find(|record| record.current.operation == b.operation)
        .expect("B stays unfinished");
    assert_ne!(record_b.current.sweep, b.sweep);
    let host_b = store
        .deletion_participants(b.operation, None, 100)
        .await
        .expect("B participants read")
        .into_iter()
        .find(|record| record.participant.owner == ParticipantOwnerRef::HostTransient)
        .expect("B HostTransient remains required");
    assert!(!host_b.progress.is_verified());
}
