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

use ene_preservation::{
    DeletionOperationRef, DeletionSearchMaterial, ErasureConditionRef, MechanicalDeletionTarget,
    ParticipantOwnerRef, PreservationRepository as _, StartTargetedDeletionCommand,
    StartTargetedDeletionOutcome, TargetedDeletionTarget,
};
use ene_task::{DelegationId, TaskAgentEphemeralId, TaskAgentResultArrival, TaskId, TaskResultId};

use super::preservation::complete_via_a5;
use super::targeted_deletion::drive_with_sources;

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

// --- Learning Summary / Memory formation ---

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

// --- Action attempt / settlement ---

// --- Undelivered presentation start and ACK ---

// --- Resume instruction activity ---

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

// --- Dialogue prompt read-set correlation and read-side withholding ---

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
