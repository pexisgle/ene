//! Stage 4 erasure-currentness foundation (V21): the canonical Group J
//! erasure-condition store, the AU14 `data_use` coverage compare, and the
//! durable attempt source correlation.
//!
//! Historical/corrupt-state fixtures seed canonical lifecycle rows directly.
//! Admission, atomicity and restart are tested through the owner producer in
//! `preservation`; these tests pin the existing AU14 consumer behavior.

use super::*;
use ene_preservation::{DeletionOperationId, DeletionSweepGeneration, ErasureConditionRef};

fn task_agent_claim(
    ticket: InferenceTicketId,
    consent_rev: u64,
    premise: TaskAgentAttemptPremise,
) -> InferenceAttempt {
    InferenceAttempt {
        ticket,
        consumer: ConsumerKind::TaskAgent,
        capability: CapabilityKind::Dialogue,
        purpose: PurposeKind::TaskAgentTurn,
        expected_consent: (
            String::from("consent-1"),
            ConsentRevision::from_u64(consent_rev),
        ),
        expected_credential_set: CredentialSetRevision::initial(),
        provider: String::from("openai"),
        model: String::from("dialogue-1"),
        data_use: premise.data_use.clone(),
        task_agent: Some(premise),
        pricing: None,
        usage_estimate: None,
    }
}

fn task_agent_premise(
    delegation: DelegationId,
    task: TaskRef,
    data_use: Vec<RawId>,
) -> TaskAgentAttemptPremise {
    TaskAgentAttemptPremise {
        delegation: delegation.as_raw(),
        task: task.task.as_raw(),
        task_revision: RevisionInner::from_u64(task.revision.as_u64()),
        data_use,
    }
}

async fn seed_dialogue_consent(store: &Store) {
    let saved = save_consent(
        store,
        None,
        ConsentRecord {
            capability: CapabilityKind::Dialogue,
            id: String::from("consent-1"),
            rev: ConsentRevision::from_u64(1),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            credential_id: String::from("openai:main"),
        },
    )
    .await;
    assert!(matches!(saved, ConsentCommitOutcome::Committed { .. }));
}

/// Seeds the canonical Group J rows for one condition covering `sources`.
///
/// The physical shape is the logical `erasure_condition`: one condition row
/// and its normalized source correlation. The fixture is test-local because
/// the Stage 6 producer is the only production writer.
fn seed_condition(store: &Store, sweep: u64, sources: &[RawId]) {
    let condition = ErasureConditionRef {
        operation: DeletionOperationId::from_raw(RawId::new()),
        sweep: DeletionSweepGeneration::from_u64(sweep),
    };
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    let operation_text = crate::codec::encode_id(condition.operation.as_raw());
    let sweep_raw = i64::try_from(condition.sweep.as_u64()).expect("the sweep fixture fits i64");
    guard.execute("INSERT INTO deletion_operation (operation_id,sweep,phase,purpose,started_at) VALUES (?1,?2,'active','privacy','2026-09-17T00:00:00Z')", params![operation_text,sweep_raw]).unwrap();
    guard
        .execute(
            "INSERT INTO deletion_search_material (operation_id,exact_text) VALUES (?1,'fixture')",
            [&operation_text],
        )
        .unwrap();
    guard
        .execute(
            "INSERT INTO erasure_condition (operation_id, sweep, opened_at) VALUES (?1, ?2, '2026-09-17T00:00:00Z')",
            params![operation_text, sweep_raw],
        )
        .expect("the condition row must seed");
    // Every operation carries a non-empty required participant snapshot
    // (lifecycle §8); the fixture seeds the same shape admission would.
    guard
        .execute(
            "INSERT INTO deletion_participant (operation_id,participant_owner,state,sweep,erased_count,remainder_count) VALUES (?1,'companion','pending',?2,0,0)",
            params![operation_text, sweep_raw],
        )
        .expect("the participant snapshot must seed");
    for source in sources {
        guard
            .execute(
                "INSERT INTO erasure_condition_source (operation_id, sweep, source) VALUES (?1, ?2, ?3)",
                params![operation_text, sweep_raw, crate::codec::encode_id(*source)],
            )
            .expect("the source coverage row must seed");
    }
}

fn table_exists(store: &Store, table: &str) -> bool {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |row| row.get::<_, i64>(0),
        )
        .expect("the schema probe must read")
        > 0
}

fn raw_execute(store: &Store, sql: &str, params: impl rusqlite::Params) {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .execute(sql, params)
        .expect("the fixture write must apply");
}

/// Seeds one Task, its adopted-purpose source, and one delegation.
async fn seed_delegated_task(store: &Store) -> (TaskRef, DelegationId, RawId) {
    let creation = task_premise(None);
    let purpose_source = creation.origin.source;
    let created = store.create_task(creation).await.unwrap();
    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            delegation,
            created,
            TaskAgentEphemeralId::generate(),
            delegation_scope(None),
        ))
        .await
        .unwrap();
    assert!(matches!(outcome, DelegationOutcome::Delegated(_)));
    (created, delegation, purpose_source)
}

#[tokio::test]
async fn authoritative_empty_store_allows_the_claim_and_records_ordered_data_use() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation, purpose_source) = seed_delegated_task(&store).await;
    assert!(
        table_exists(&store, "erasure_condition")
            && table_exists(&store, "erasure_condition_source"),
        "the canonical condition store exists in the fresh schema"
    );
    assert_eq!(
        task_table_count(&store, "erasure_condition"),
        0,
        "the authoritative active set is empty by reading the store, not by a sentinel"
    );
    assert_eq!(task_table_count(&store, "erasure_condition_source"), 0);

    let second = RawId::new();
    let data_use = vec![purpose_source, second, purpose_source];
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(
                ticket,
                1,
                task_agent_premise(delegation, created, data_use.clone()),
            ))
            .await,
        Ok(AttemptBeginOutcome::Started),
        "an authoritative empty condition set admits the send"
    );
    let record = store
        .load_inference_attempt(ticket)
        .await
        .unwrap()
        .expect("the claimed attempt must read");
    assert_eq!(
        record
            .task_agent
            .expect("the task agent correlation")
            .data_use,
        data_use,
        "order and duplicates survive the durable round trip"
    );
    assert_eq!(
        task_table_count(&store, "inference_attempt_data_use"),
        3,
        "every logical-input entry keeps its own ordinal row"
    );
}

#[tokio::test]
async fn covering_condition_holds_the_send_without_an_attempt_row_or_child_rows() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation, purpose_source) = seed_delegated_task(&store).await;
    // A condition committed before the claim covers the purpose source.
    seed_condition(&store, 1, &[purpose_source]);

    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(
                ticket,
                1,
                task_agent_premise(delegation, created, vec![purpose_source]),
            ))
            .await,
        Ok(AttemptBeginOutcome::DataUseHeld),
        "a covered source is a data-use hold, not a stale or technical outcome"
    );
    assert_eq!(
        task_table_count(&store, "inference_attempt"),
        0,
        "a held send starts no attempt"
    );
    assert_eq!(task_table_count(&store, "inference_attempt_data_use"), 0);
    assert_eq!(store.load_inference_attempt(ticket).await, Ok(None));
}

#[tokio::test]
async fn unrelated_condition_does_not_hold_another_source() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation, purpose_source) = seed_delegated_task(&store).await;
    // A condition covering a different source never refuses this input.
    seed_condition(&store, 1, &[RawId::new()]);

    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(
                InferenceTicketId(RawId::new()),
                1,
                task_agent_premise(delegation, created, vec![purpose_source]),
            ))
            .await,
        Ok(AttemptBeginOutcome::Started)
    );
}

#[tokio::test]
async fn one_condition_can_cover_many_sources_and_holds_any_of_them() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation, purpose_source) = seed_delegated_task(&store).await;
    let covered = RawId::new();
    seed_condition(&store, 4, &[covered, purpose_source]);

    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(
                InferenceTicketId(RawId::new()),
                1,
                task_agent_premise(delegation, created, vec![covered]),
            ))
            .await,
        Ok(AttemptBeginOutcome::DataUseHeld),
        "one source of a multi-source condition holds the whole send"
    );
}

#[tokio::test]
async fn missing_erasure_store_fails_closed_instead_of_reading_as_clear() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation, purpose_source) = seed_delegated_task(&store).await;
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute_batch("DROP TABLE erasure_condition_source; DROP TABLE erasure_condition;")
            .expect("the store removal must apply");
    }
    let outcome = store
        .begin_inference_attempt(task_agent_claim(
            InferenceTicketId(RawId::new()),
            1,
            task_agent_premise(delegation, created, vec![purpose_source]),
        ))
        .await;
    assert!(
        matches!(
            outcome,
            Err(InferenceTechnicalError::StorageUnavailable { .. })
        ),
        "an unreadable canonical store is a technical failure, got {outcome:?}"
    );
    assert_eq!(
        task_table_count(&store, "inference_attempt"),
        0,
        "no attempt is recorded when currentness cannot be established"
    );
}

#[tokio::test]
async fn data_use_read_fails_closed_on_a_corrupt_correlation() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation, purpose_source) = seed_delegated_task(&store).await;
    let cases: [(&str, &str); 4] = [
        (
            "truncated count",
            "UPDATE inference_attempt SET data_use_count = 1 WHERE ticket = ?1",
        ),
        (
            "missing count",
            "UPDATE inference_attempt SET data_use_count = NULL WHERE ticket = ?1",
        ),
        (
            "lost child row",
            "DELETE FROM inference_attempt_data_use WHERE ticket = ?1 AND ordinal = 1",
        ),
        (
            "malformed source",
            "UPDATE inference_attempt_data_use SET source = 'not-an-identity' WHERE ticket = ?1 AND ordinal = 0",
        ),
    ];
    for (name, sql) in cases {
        let ticket = InferenceTicketId(RawId::new());
        assert_eq!(
            store
                .begin_inference_attempt(task_agent_claim(
                    ticket,
                    1,
                    task_agent_premise(delegation, created, vec![purpose_source, RawId::new()],),
                ))
                .await,
            Ok(AttemptBeginOutcome::Started),
            "the {name} fixture must seed a fresh attempt"
        );
        raw_execute(&store, sql, params![crate::codec::encode_id(ticket.0)]);
        assert!(
            matches!(
                store.load_inference_attempt(ticket).await,
                Err(InferenceTechnicalError::StorageUnavailable { .. })
            ),
            "a corrupt data-use correlation must fail closed: {name}"
        );
    }
    // A non-contiguous ordinal is corruption even when the row count matches.
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(
                ticket,
                1,
                task_agent_premise(delegation, created, vec![purpose_source, RawId::new()]),
            ))
            .await,
        Ok(AttemptBeginOutcome::Started)
    );
    raw_execute(
        &store,
        "UPDATE inference_attempt_data_use SET ordinal = 5 WHERE ticket = ?1 AND ordinal = 1",
        params![crate::codec::encode_id(ticket.0)],
    );
    assert!(
        matches!(
            store.load_inference_attempt(ticket).await,
            Err(InferenceTechnicalError::StorageUnavailable { .. })
        ),
        "non-contiguous ordinals must fail closed"
    );
}

#[tokio::test]
async fn condition_committed_after_the_claim_leaves_the_attempt_started() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation, purpose_source) = seed_delegated_task(&store).await;
    let first_ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(
                first_ticket,
                1,
                task_agent_premise(delegation, created, vec![purpose_source]),
            ))
            .await,
        Ok(AttemptBeginOutcome::Started),
        "the claim commits before the condition arrives"
    );
    // The later condition cannot rewrite the already-started attempt fact; it
    // only holds sends that have not claimed yet.
    seed_condition(&store, 2, &[purpose_source]);
    let record = store
        .load_inference_attempt(first_ticket)
        .await
        .unwrap()
        .expect("the started attempt stays durable");
    assert_eq!(
        record.task_agent.expect("the correlation").data_use,
        vec![purpose_source]
    );
    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(
                InferenceTicketId(RawId::new()),
                1,
                task_agent_premise(delegation, created, vec![purpose_source]),
            ))
            .await,
        Ok(AttemptBeginOutcome::DataUseHeld),
        "the same gate sees the later covering condition"
    );
}

#[tokio::test]
async fn condition_and_data_use_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("erasure-restart.db");
    let (ticket, data_use, condition_source, delegation, created) = {
        let store = Store::open(&path).await.unwrap();
        seed_dialogue_consent(&store).await;
        let (created, delegation, purpose_source) = seed_delegated_task(&store).await;
        let unrelated = RawId::new();
        let condition_source = RawId::new();
        // The durable condition covers a source outside this claim, so the
        // claim is admitted and the condition itself is what must survive.
        seed_condition(&store, 3, &[condition_source]);
        let ticket = InferenceTicketId(RawId::new());
        // Order and duplicates must both survive: [purpose, unrelated,
        // purpose] is the exact durable correlation the read must reproduce.
        let data_use = vec![purpose_source, unrelated, purpose_source];
        assert_eq!(
            store
                .begin_inference_attempt(task_agent_claim(
                    ticket,
                    1,
                    task_agent_premise(delegation, created, data_use.clone()),
                ))
                .await,
            Ok(AttemptBeginOutcome::Started)
        );
        (ticket, data_use, condition_source, delegation, created)
    };
    let reopened = Store::open(&path).await.expect("reopen must succeed");
    let record = reopened
        .load_inference_attempt(ticket)
        .await
        .expect("the correlation must read after restart")
        .expect("the claimed attempt survives restart");
    assert_eq!(
        record.task_agent.expect("the correlation").data_use,
        data_use,
        "the ordered data_use survives restart"
    );
    assert_eq!(
        reopened
            .begin_inference_attempt(task_agent_claim(
                InferenceTicketId(RawId::new()),
                1,
                task_agent_premise(delegation, created, vec![condition_source]),
            ))
            .await,
        Ok(AttemptBeginOutcome::DataUseHeld),
        "the canonical current-condition state survives restart"
    );
    assert_eq!(
        task_table_count(&reopened, "erasure_condition_source"),
        1,
        "the canonical condition state survives restart"
    );
}

#[tokio::test]
async fn dialogue_attempts_record_the_empty_data_use_and_are_not_gated() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    // A condition covering the dialogue consumer's (nonexistent) sources
    // cannot hold it: a dialogue attempt carries no correlation.
    seed_condition(&store, 1, &[RawId::new()]);
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(InferenceAttempt {
                ticket,
                consumer: ConsumerKind::CompanionDialogue,
                capability: CapabilityKind::Dialogue,
                purpose: PurposeKind::DialogueResponse,
                expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
                expected_credential_set: CredentialSetRevision::initial(),
                provider: String::from("openai"),
                model: String::from("dialogue-1"),
                data_use: Vec::new(),
                task_agent: None,
                pricing: None,
                usage_estimate: None,
            })
            .await,
        Ok(AttemptBeginOutcome::Started),
        "dialogue keeps its existing path"
    );
    let record = store
        .load_inference_attempt(ticket)
        .await
        .unwrap()
        .expect("the dialogue attempt must read");
    assert_eq!(record.task_agent, None);
    assert!(record.data_use.is_empty());
    assert_eq!(
        task_table_count(&store, "inference_attempt_data_use"),
        0,
        "a dialogue attempt records the empty set, never a fabricated source"
    );
}

#[tokio::test]
async fn learning_formation_claim_is_gated_by_its_source_correlation() {
    let store = open_memory().await.unwrap();
    let saved = save_consent(
        &store,
        None,
        ConsentRecord {
            capability: CapabilityKind::Learning,
            id: String::from("consent-learning"),
            rev: ConsentRevision::from_u64(1),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            credential_id: String::from("openai:main"),
        },
    )
    .await;
    assert!(matches!(saved, ConsentCommitOutcome::Committed { .. }));
    let covered = RawId::new();
    let clear = RawId::new();
    seed_condition(&store, 1, &[covered]);
    let claim = |ticket: InferenceTicketId, data_use: Vec<RawId>| InferenceAttempt {
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
        data_use,
        task_agent: None,
        pricing: None,
        usage_estimate: None,
    };
    // A formation whose prompt read a covered source is held before any
    // provider byte and before the attempt row exists.
    assert_eq!(
        store
            .begin_inference_attempt(claim(InferenceTicketId(RawId::new()), vec![covered]))
            .await,
        Ok(AttemptBeginOutcome::DataUseHeld)
    );
    // Any covered source holds the whole claim, and a held claim leaves no
    // attempt row and no provider byte.
    let held_ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(claim(held_ticket, vec![clear, covered]))
            .await,
        Ok(AttemptBeginOutcome::DataUseHeld),
        "any covered source holds the whole claim"
    );
    assert_eq!(
        store.load_inference_attempt(held_ticket).await.unwrap(),
        None
    );
    // A formation whose correlation is not covered claims normally and
    // records its ordered data_use.
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(claim(ticket, vec![clear]))
            .await,
        Ok(AttemptBeginOutcome::Started)
    );
    let record = store
        .load_inference_attempt(ticket)
        .await
        .unwrap()
        .expect("the learning attempt must read");
    assert_eq!(record.consumer, ConsumerKind::CompanionLearning);
    assert_eq!(record.task_agent, None);
    assert_eq!(record.data_use, vec![clear]);
}
