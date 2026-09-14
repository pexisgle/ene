//! Stage 4 erasure-currentness foundation (V21): the canonical Group J
//! erasure-condition store, the AU14 `data_use` coverage compare, and the
//! durable attempt source correlation.
//!
//! Stage 4 has no deletion-operation producer, so these tests seed the
//! canonical tables directly — the same rows the Stage 6 producer will write.
//! No production producer exists for tests.

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
        task_agent: Some(premise),
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
    guard
        .execute(
            "INSERT INTO erasure_condition (operation_id, sweep) VALUES (?1, ?2)",
            params![operation_text, sweep_raw],
        )
        .expect("the condition row must seed");
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
async fn non_task_attempts_record_the_empty_data_use_and_are_not_gated() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    // A condition covering the dialogue consumer's (nonexistent) sources
    // cannot hold it: data_use is empty by construction for non-task uses.
    seed_condition(&store, 1, &[RawId::new()]);
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(InferenceAttempt {
                ticket,
                consumer: ConsumerKind::CompanionLearning,
                capability: CapabilityKind::Dialogue,
                purpose: PurposeKind::DialogueResponse,
                expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
                expected_credential_set: CredentialSetRevision::initial(),
                provider: String::from("openai"),
                model: String::from("dialogue-1"),
                task_agent: None,
            })
            .await,
        Ok(AttemptBeginOutcome::Started),
        "non-task inference keeps its existing path"
    );
    let record = store
        .load_inference_attempt(ticket)
        .await
        .unwrap()
        .expect("the dialogue attempt must read");
    assert_eq!(record.task_agent, None);
    assert_eq!(
        task_table_count(&store, "inference_attempt_data_use"),
        0,
        "a non-task attempt records the empty set, never a fabricated source"
    );
}

// --- V21 migration and backfill ---

struct V20Fixture {
    task: TaskId,
    delegation: DelegationId,
    task_agent_ticket: InferenceTicketId,
    dialogue_ticket: InferenceTicketId,
}

/// Builds a V20-shaped fixture: a Task with an adopted-purpose entry and a
/// delegation, a purpose-only Task Agent attempt, and a dialogue attempt.
///
/// The rows are inserted directly with `data_use_count` absent, then the V21
/// state is removed and `user_version` is lowered, so the reopen exercises the
/// real V20 -> V21 migration.
async fn seed_v20_fixture(path: &std::path::Path) -> V20Fixture {
    let (task, delegation, task_agent_ticket, dialogue_ticket) = {
        let store = Store::open(path).await.expect("a fresh store must open");
        seed_dialogue_consent(&store).await;
        let creation = task_premise(None);
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
        let task_agent_ticket = InferenceTicketId(RawId::new());
        let dialogue_ticket = InferenceTicketId(RawId::new());
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "INSERT INTO inference_attempt (ticket, capability, consumer, purpose, consent_id, consent_rev, credential_set_rev, provider, model, started_at, delegation_id, task_id, task_revision) VALUES (?1, 'dialogue', 'task_agent', 'task_agent_turn', 'consent-1', 1, 0, 'openai', 'dialogue-1', '2026-01-01T00:00:00+00:00', ?2, ?3, ?4)",
                params![
                    crate::codec::encode_id(task_agent_ticket.0),
                    crate::codec::encode_id(delegation.as_raw()),
                    crate::codec::encode_id(created.task.as_raw()),
                    i64::try_from(created.revision.as_u64()).unwrap(),
                ],
            )
            .expect("the v20 task agent attempt must seed");
        guard
            .execute(
                "INSERT INTO inference_attempt (ticket, capability, consumer, purpose, consent_id, consent_rev, credential_set_rev, provider, model, started_at) VALUES (?1, 'dialogue', 'companion_dialogue', 'dialogue_response', 'consent-1', 1, 0, 'openai', 'dialogue-1', '2026-01-01T00:00:00+00:00')",
                params![crate::codec::encode_id(dialogue_ticket.0)],
            )
            .expect("the v20 dialogue attempt must seed");
        (created.task, delegation, task_agent_ticket, dialogue_ticket)
    };
    let conn = rusqlite::Connection::open(path).expect("the rewind must open");
    conn.execute_batch(
        "DELETE FROM inference_attempt_data_use;
         ALTER TABLE inference_attempt DROP COLUMN data_use_count;
         DROP TABLE inference_attempt_data_use;
         DROP TABLE erasure_condition_source;
         DROP TABLE erasure_condition;
         PRAGMA user_version = 20;",
    )
    .expect("the version-20 rewind must apply");
    V20Fixture {
        task,
        delegation,
        task_agent_ticket,
        dialogue_ticket,
    }
}

#[tokio::test]
async fn v21_backfills_task_agent_attempts_from_their_replied_purpose_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v21-backfill.db");
    let fixture = seed_v20_fixture(&path).await;
    assert_eq!(read_schema_version(&path), Some(20));

    let reopened = Store::open(&path)
        .await
        .expect("the V21 migration must succeed");
    assert_eq!(read_schema_version(&path), Some(22));
    assert!(
        table_exists(&reopened, "erasure_condition")
            && table_exists(&reopened, "inference_attempt_data_use"),
        "the migration creates the canonical store and the correlation relation"
    );
    let expected_source = reopened
        .load_task(fixture.task)
        .await
        .unwrap()
        .expect("the task survives the migration")
        .context[0]
        .origin
        .source;

    let task_agent = reopened
        .load_inference_attempt(fixture.task_agent_ticket)
        .await
        .unwrap()
        .expect("the task agent attempt must read");
    assert_eq!(task_agent.consumer, ConsumerKind::TaskAgent);
    let correlation = task_agent.task_agent.expect("the correlation");
    assert_eq!(
        correlation.data_use,
        vec![expected_source],
        "the backfill records exactly the relied purpose entry's canonical source"
    );
    assert_eq!(
        correlation.delegation,
        fixture.delegation.as_raw(),
        "the delegation correlation is untouched"
    );

    let dialogue = reopened
        .load_inference_attempt(fixture.dialogue_ticket)
        .await
        .unwrap()
        .expect("the dialogue attempt must read");
    assert_eq!(dialogue.task_agent, None);
    assert_eq!(
        task_table_count(&reopened, "inference_attempt_data_use"),
        1,
        "only the task agent attempt gains a source row; the dialogue attempt stays empty"
    );
}

#[tokio::test]
async fn v21_backfill_fails_closed_on_broken_purpose_correspondence() {
    let cases: [(&str, &str, bool); 8] = [
        (
            "missing purpose entry",
            "DELETE FROM task_context_entry WHERE task_id = ?1 AND item_kind = 'adopted_purpose'",
            false,
        ),
        (
            "ambiguous purpose entries",
            "INSERT INTO task_context_entry (entry_id, task_id, revision, item_kind, purpose_adopted_revision, origin_kind, origin_source, acquired_at) SELECT ?2, task_id, revision, item_kind, purpose_adopted_revision, origin_kind, origin_source, acquired_at FROM task_context_entry WHERE task_id = ?1 AND item_kind = 'adopted_purpose'",
            true,
        ),
        (
            "malformed purpose source",
            "UPDATE task_context_entry SET origin_source = 'not-an-identity' WHERE task_id = ?1 AND item_kind = 'adopted_purpose'",
            false,
        ),
        (
            "snapshot/context pointer mismatch",
            "UPDATE task_context_entry SET purpose_adopted_revision = 99 WHERE task_id = ?1 AND item_kind = 'adopted_purpose'",
            false,
        ),
        (
            "missing task revision snapshot",
            "DELETE FROM task_revision WHERE task_id = ?1",
            false,
        ),
        (
            "malformed origin kind",
            "UPDATE task_context_entry SET origin_kind = 'not-a-known-origin' WHERE task_id = ?1 AND item_kind = 'adopted_purpose'",
            false,
        ),
        (
            "malformed acquired_at",
            "UPDATE task_context_entry SET acquired_at = 'not-a-timestamp' WHERE task_id = ?1 AND item_kind = 'adopted_purpose'",
            false,
        ),
        (
            "NULL-payload duplicate purpose entry",
            "INSERT INTO task_context_entry (entry_id, task_id, revision, item_kind, purpose_adopted_revision, origin_kind, origin_source, acquired_at) SELECT ?2, task_id, revision, item_kind, NULL, origin_kind, origin_source, acquired_at FROM task_context_entry WHERE task_id = ?1 AND item_kind = 'adopted_purpose'",
            true,
        ),
    ];
    for (name, mutation, needs_entry) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v21-backfill-fail.db");
        let fixture = seed_v20_fixture(&path).await;
        {
            let conn = rusqlite::Connection::open(&path).expect("the mutation must open");
            let task_text = crate::codec::encode_id(fixture.task.as_raw());
            let applied = if needs_entry {
                conn.execute(
                    mutation,
                    params![task_text, crate::codec::encode_id(RawId::new())],
                )
            } else {
                conn.execute(mutation, params![task_text])
            };
            applied.unwrap_or_else(|error| panic!("the {name} mutation must apply: {error}"));
        }
        assert!(
            Store::open(&path).await.is_err(),
            "a broken purpose correspondence must abort the migration: {name}"
        );
        assert_eq!(
            read_schema_version(&path),
            Some(20),
            "the failed migration rolls back to the pre-migration version: {name}"
        );
        assert_eq!(
            table_columns(&path, "inference_attempt")
                .iter()
                .filter(|column| column.as_str() == "data_use_count")
                .count(),
            0,
            "the failed backfill rolls back the added column: {name}"
        );
        let tables = schema_table_names(&path);
        assert!(
            !tables.iter().any(|table| {
                table == "inference_attempt_data_use"
                    || table == "erasure_condition"
                    || table == "erasure_condition_source"
            }),
            "the failed migration rolls back the V21 tables: {name}"
        );
    }
}
