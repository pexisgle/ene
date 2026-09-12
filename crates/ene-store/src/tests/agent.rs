//! Inference attempt correlation (V18): Task Agent attribution, claim
//! linearization against steering, and fail-closed reads.

use super::*;

fn task_agent_attempt_premise(delegation: DelegationId, task: TaskRef) -> TaskAgentAttemptPremise {
    TaskAgentAttemptPremise {
        delegation: delegation.as_raw(),
        task: task.task.as_raw(),
        task_revision: RevisionInner::from_u64(task.revision.as_u64()),
    }
}

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

/// Seeds one Task at revision 1 and one delegation bound to it.
async fn seed_delegation(store: &Store) -> (TaskRef, DelegationId) {
    let created = store.create_task(task_premise(None)).await.unwrap();
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
    assert!(
        matches!(outcome, DelegationOutcome::Delegated(_)),
        "the seed delegation must commit, got {outcome:?}"
    );
    (created, delegation)
}

#[tokio::test]
async fn task_agent_claim_records_the_durable_correlation() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation) = seed_delegation(&store).await;
    let premise = task_agent_attempt_premise(delegation, created);
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(ticket, 1, premise))
            .await,
        Ok(AttemptBeginOutcome::Started)
    );
    let record = store
        .load_inference_attempt(ticket)
        .await
        .expect("the attempt row must read")
        .expect("the claimed attempt must exist");
    assert_eq!(record.consumer, ConsumerKind::TaskAgent);
    assert_eq!(record.capability, CapabilityKind::Dialogue);
    assert_eq!(record.purpose, PurposeKind::TaskAgentTurn);
    assert_eq!(
        record.task_agent,
        Some(premise),
        "the durable correlation keeps the delegation and the relied TaskRef"
    );
    // The chain a delayed result walks: ticket -> attempt -> delegation ->
    // delegator. The delegator is read from the delegation row, not copied
    // onto the attempt.
    let loaded_delegation = store
        .load_delegation(delegation)
        .await
        .unwrap()
        .expect("the delegation correspondence must load");
    assert_eq!(loaded_delegation.task, created);
    let task = store
        .load_task(created.task)
        .await
        .unwrap()
        .expect("the task must load");
    assert_eq!(
        loaded_delegation.delegator, task.task.assignee,
        "the attribution walks back to the delegator"
    );
    store
        .record_usage(UsageFact {
            ticket,
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            input_tokens: Some(7),
            output_tokens: Some(3),
            source: UsageSource::Reported,
        })
        .await
        .expect("usage follows the claimed attempt");
    assert_eq!(task_table_count(&store, "usage_fact"), 1);
    assert_eq!(task_table_count(&store, "inference_attempt"), 1);
}

#[tokio::test]
async fn dialogue_attempt_reads_back_without_task_correlation() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
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
                task_agent: None,
            })
            .await,
        Ok(AttemptBeginOutcome::Started)
    );
    let record = store.load_inference_attempt(ticket).await.unwrap().unwrap();
    assert_eq!(record.consumer, ConsumerKind::CompanionDialogue);
    assert_eq!(record.purpose, PurposeKind::DialogueResponse);
    assert_eq!(record.task_agent, None);
}

#[tokio::test]
async fn task_agent_claim_is_stale_after_a_steering_forward() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation) = seed_delegation(&store).await;
    let advanced = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(task_purpose_adoption("moved before the agent turn")),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .unwrap();
    assert!(matches!(advanced, TaskCommitOutcome::CommittedAs(_)));
    let premise = task_agent_attempt_premise(delegation, created);
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(ticket, 1, premise))
            .await,
        Ok(AttemptBeginOutcome::TaskPremiseStale),
        "a moved task revision refuses the start before any provider byte"
    );
    assert_eq!(store.load_inference_attempt(ticket).await, Ok(None));
    assert_eq!(task_table_count(&store, "inference_attempt"), 0);
}

#[tokio::test]
async fn task_agent_claim_is_stale_when_the_delegation_row_is_gone() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let created = store.create_task(task_premise(None)).await.unwrap();
    let premise = TaskAgentAttemptPremise {
        delegation: RawId::new(),
        task: created.task.as_raw(),
        task_revision: RevisionInner::from_u64(created.revision.as_u64()),
    };
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(ticket, 1, premise))
            .await,
        Ok(AttemptBeginOutcome::TaskPremiseStale),
        "a premise naming no delegation row must fail closed before sending"
    );
    assert_eq!(store.load_inference_attempt(ticket).await, Ok(None));
}

#[tokio::test]
async fn task_agent_claim_rejects_a_premise_that_disagrees_with_the_delegation() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation) = seed_delegation(&store).await;
    // The delegation is bound to revision 1: a premise claiming revision 2
    // for the same delegation is an inconsistent correlation, not a stale
    // revision, and must surface as an unreadable row.
    let premise = TaskAgentAttemptPremise {
        delegation: delegation.as_raw(),
        task: created.task.as_raw(),
        task_revision: RevisionInner::from_u64(2),
    };
    let ticket = InferenceTicketId(RawId::new());
    let outcome = store
        .begin_inference_attempt(task_agent_claim(ticket, 1, premise))
        .await;
    assert!(
        matches!(
            outcome,
            Err(InferenceTechnicalError::StorageUnavailable { .. })
        ),
        "a delegation/premise disagreement is a technical error, got {outcome:?}"
    );
    assert_eq!(store.load_inference_attempt(ticket).await, Ok(None));
}

#[tokio::test]
async fn task_agent_claim_races_a_steering_forward_without_torn_state() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation) = seed_delegation(&store).await;
    let premise = task_agent_attempt_premise(delegation, created);
    let ticket = InferenceTicketId(RawId::new());
    let steering = TaskCommitPremise {
        expected: created,
        new_purpose: Some(task_purpose_adoption("racing agent start")),
        adopted_purpose_entry: TaskContextEntryId::generate(),
        adopted_instruction: None,
    };
    let (steering, claim) = tokio::join!(
        store.forward_steering(steering),
        store.begin_inference_attempt(task_agent_claim(ticket, 1, premise))
    );
    assert!(matches!(steering, Ok(TaskCommitOutcome::CommittedAs(_))));
    let started = match claim.expect("the claim answers a domain outcome") {
        AttemptBeginOutcome::Started => {
            assert!(
                store
                    .load_inference_attempt(ticket)
                    .await
                    .unwrap()
                    .is_some(),
                "a started claim leaves its correlation"
            );
            true
        }
        AttemptBeginOutcome::TaskPremiseStale => {
            assert_eq!(
                store.load_inference_attempt(ticket).await,
                Ok(None),
                "a stale claim leaves no attempt row"
            );
            false
        }
        other => panic!("unexpected claim outcome: {other:?}"),
    };
    assert_eq!(
        task_table_count(&store, "inference_attempt"),
        i64::from(started),
        "the race produces exactly the claimed attempts (zero or one)"
    );
}

#[tokio::test]
async fn inference_attempt_reads_fail_closed_on_corrupt_correlation() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation) = seed_delegation(&store).await;
    let premise = task_agent_attempt_premise(delegation, created);
    let cases = [
        "UPDATE inference_attempt SET task_revision = NULL;",
        "UPDATE inference_attempt SET delegation_id = NULL, task_id = NULL, task_revision = NULL;",
        "UPDATE inference_attempt SET consumer = 'companion_dialogue';",
        "UPDATE inference_attempt SET consumer = 'not_a_consumer';",
        "UPDATE inference_attempt SET purpose = 'not_a_purpose';",
        "UPDATE inference_attempt SET capability = 'not_a_capability';",
    ];
    for (index, sql) in cases.iter().enumerate() {
        let ticket = InferenceTicketId(RawId::new());
        assert_eq!(
            store
                .begin_inference_attempt(task_agent_claim(ticket, 1, premise))
                .await,
            Ok(AttemptBeginOutcome::Started),
            "corruption case {index} must seed a fresh claim"
        );
        {
            let guard = match store.conn.lock() {
                Ok(locked) => locked,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.execute_batch(sql).expect("the corruption must apply");
        }
        assert!(
            matches!(
                store.load_inference_attempt(ticket).await,
                Err(InferenceTechnicalError::StorageUnavailable { .. })
            ),
            "a corrupt attempt row must fail closed after: {sql}"
        );
    }
}

#[tokio::test]
async fn task_agent_claim_is_stale_when_the_inherited_consent_moves() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let (created, delegation) = seed_delegation(&store).await;
    let moved = save_consent(
        &store,
        Some((String::from("consent-1"), ConsentRevision::from_u64(1))),
        ConsentRecord {
            capability: CapabilityKind::Dialogue,
            id: String::from("consent-1"),
            rev: ConsentRevision::from_u64(2),
            provider: String::from("openai"),
            model: String::from("dialogue-2"),
            credential_id: String::from("openai:main"),
        },
    )
    .await;
    assert!(matches!(moved, ConsentCommitOutcome::Committed { .. }));
    let premise = task_agent_attempt_premise(delegation, created);
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(ticket, 1, premise))
            .await,
        Ok(AttemptBeginOutcome::Stale),
        "a moved inherited consent refuses the start before any provider byte"
    );
    assert_eq!(store.load_inference_attempt(ticket).await, Ok(None));
}

#[tokio::test]
async fn multiple_delegations_keep_distinct_attempt_correlations() {
    let store = open_memory().await.unwrap();
    seed_dialogue_consent(&store).await;
    let created = store.create_task(task_premise(None)).await.unwrap();
    let first = DelegationId::generate();
    let second = DelegationId::generate();
    for delegation in [first, second] {
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
    }
    let first_ticket = InferenceTicketId(RawId::new());
    let second_ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(
                first_ticket,
                1,
                task_agent_attempt_premise(first, created),
            ))
            .await,
        Ok(AttemptBeginOutcome::Started)
    );
    assert_eq!(
        store
            .begin_inference_attempt(task_agent_claim(
                second_ticket,
                1,
                task_agent_attempt_premise(second, created),
            ))
            .await,
        Ok(AttemptBeginOutcome::Started)
    );
    let first_record = store
        .load_inference_attempt(first_ticket)
        .await
        .unwrap()
        .unwrap();
    let second_record = store
        .load_inference_attempt(second_ticket)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first_record.task_agent.map(|premise| premise.delegation),
        Some(first.as_raw()),
        "each attempt keeps its own delegation attribution"
    );
    assert_eq!(
        second_record.task_agent.map(|premise| premise.delegation),
        Some(second.as_raw())
    );
    store
        .record_usage(UsageFact {
            ticket: first_ticket,
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            input_tokens: Some(1),
            output_tokens: Some(1),
            source: UsageSource::Reported,
        })
        .await
        .expect("usage for the first delegation records");
    store
        .record_usage(UsageFact {
            ticket: second_ticket,
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            input_tokens: Some(2),
            output_tokens: Some(2),
            source: UsageSource::Reported,
        })
        .await
        .expect("usage for the second delegation records");
    assert_eq!(
        task_table_count(&store, "usage_fact"),
        2,
        "parallel delegations keep distinct usage rows"
    );
}

#[tokio::test]
async fn task_agent_correlation_survives_reopen_without_replay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("attempt-restart.db");
    let ticket = InferenceTicketId(RawId::new());
    let premise;
    {
        let store = Store::open(&path).await.unwrap();
        seed_dialogue_consent(&store).await;
        let (created, delegation) = seed_delegation(&store).await;
        premise = task_agent_attempt_premise(delegation, created);
        assert_eq!(
            store
                .begin_inference_attempt(task_agent_claim(ticket, 1, premise))
                .await,
            Ok(AttemptBeginOutcome::Started)
        );
    }
    let reopened = Store::open(&path).await.expect("reopen must succeed");
    let record = reopened
        .load_inference_attempt(ticket)
        .await
        .expect("the correlation must read after restart")
        .expect("the claimed attempt survives restart");
    assert_eq!(record.consumer, ConsumerKind::TaskAgent);
    assert_eq!(record.task_agent, Some(premise));
    assert_eq!(
        task_table_count(&reopened, "inference_attempt"),
        1,
        "reopen replays nothing"
    );
}

#[tokio::test]
async fn inference_attempt_migration_backfills_consumer_and_purpose() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("attempt-migration.db");
    let dialogue_ticket = InferenceTicketId(RawId::new());
    let learning_ticket = InferenceTicketId(RawId::new());
    {
        let store = Store::open(&path).await.expect("a fresh store must open");
        assert_eq!(read_schema_version(&path), Some(18));
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "INSERT INTO inference_attempt (ticket, capability, consent_id, consent_rev, provider, model, started_at) VALUES (?1, 'dialogue', 'c1', 1, 'p', 'm', '2026-01-01T00:00:00+00:00'), (?2, 'learning', 'c2', 1, 'p', 'm', '2026-01-01T00:00:00+00:00')",
                params![
                    crate::codec::encode_id(dialogue_ticket.0),
                    crate::codec::encode_id(learning_ticket.0),
                ],
            )
            .expect("the pre-V18 rows must seed");
    }
    {
        let conn = rusqlite::Connection::open(&path).expect("the rewind must open");
        conn.execute_batch(
            "ALTER TABLE inference_attempt DROP COLUMN consumer;
             ALTER TABLE inference_attempt DROP COLUMN purpose;
             ALTER TABLE inference_attempt DROP COLUMN credential_set_rev;
             ALTER TABLE inference_attempt DROP COLUMN delegation_id;
             ALTER TABLE inference_attempt DROP COLUMN task_id;
             ALTER TABLE inference_attempt DROP COLUMN task_revision;
             PRAGMA user_version = 17;",
        )
        .expect("the version-17 rewind must apply");
    }
    assert_eq!(read_schema_version(&path), Some(17));
    assert!(
        !table_columns(&path, "inference_attempt").contains(&String::from("consumer")),
        "the rewind drops the correlation columns"
    );

    let reopened = Store::open(&path)
        .await
        .expect("the V18 migration must succeed");
    assert_eq!(read_schema_version(&path), Some(18));
    let columns = table_columns(&path, "inference_attempt");
    for column in [
        "consumer",
        "purpose",
        "credential_set_rev",
        "delegation_id",
        "task_id",
        "task_revision",
    ] {
        assert!(
            columns.contains(&String::from(column)),
            "the migrated attempt table has {column}"
        );
    }
    let dialogue = reopened
        .load_inference_attempt(dialogue_ticket)
        .await
        .unwrap()
        .expect("the backfilled dialogue attempt must read");
    assert_eq!(dialogue.consumer, ConsumerKind::CompanionDialogue);
    assert_eq!(dialogue.purpose, PurposeKind::DialogueResponse);
    assert_eq!(dialogue.task_agent, None);
    let learning = reopened
        .load_inference_attempt(learning_ticket)
        .await
        .unwrap()
        .expect("the backfilled learning attempt must read");
    assert_eq!(learning.consumer, ConsumerKind::CompanionLearning);
    assert_eq!(learning.purpose, PurposeKind::MemoryFormation);
    assert_eq!(learning.task_agent, None);
}

#[tokio::test]
async fn inference_attempt_migration_fails_closed_on_an_unknown_capability() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("attempt-migration-unknown.db");
    {
        let store = Store::open(&path).await.expect("a fresh store must open");
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "INSERT INTO inference_attempt (ticket, capability, consent_id, consent_rev, provider, model, started_at) VALUES (?1, 'mystery', 'c1', 1, 'p', 'm', '2026-01-01T00:00:00+00:00')",
                params![crate::codec::encode_id(RawId::new())],
            )
            .expect("the unknown-capability row must seed");
    }
    {
        let conn = rusqlite::Connection::open(&path).expect("the rewind must open");
        conn.execute_batch(
            "ALTER TABLE inference_attempt DROP COLUMN consumer;
             ALTER TABLE inference_attempt DROP COLUMN purpose;
             ALTER TABLE inference_attempt DROP COLUMN credential_set_rev;
             ALTER TABLE inference_attempt DROP COLUMN delegation_id;
             ALTER TABLE inference_attempt DROP COLUMN task_id;
             ALTER TABLE inference_attempt DROP COLUMN task_revision;
             PRAGMA user_version = 17;",
        )
        .expect("the version-17 rewrite must apply");
    }
    assert!(
        Store::open(&path).await.is_err(),
        "an unknown stored capability aborts the backfill instead of inventing an attribution"
    );
    assert_eq!(
        read_schema_version(&path),
        Some(17),
        "the failed migration rolls back to the pre-migration version"
    );
    assert!(
        !table_columns(&path, "inference_attempt").contains(&String::from("consumer")),
        "the failed migration also rolls back the added columns"
    );
}
