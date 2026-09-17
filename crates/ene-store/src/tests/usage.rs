//! Canonical token settlement, attribution, and independent-connection races.
use std::sync::Arc;

use super::*;

async fn claim(store: &Store) -> InferenceTicketId {
    let consent = ConsentRecord {
        capability: CapabilityKind::Dialogue,
        id: String::from("usage-consent"),
        rev: ConsentRevision::from_u64(1),
        provider: String::from("openai"),
        model: String::from("test-model"),
        credential_id: String::from("openai:main"),
    };
    save_consent(store, None, consent).await;
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(InferenceAttempt {
                ticket,
                consumer: ConsumerKind::CompanionDialogue,
                capability: CapabilityKind::Dialogue,
                purpose: PurposeKind::DialogueResponse,
                expected_consent: (String::from("usage-consent"), ConsentRevision::from_u64(1)),
                expected_credential_set: CredentialSetRevision::initial(),
                provider: String::from("openai"),
                model: String::from("test-model"),
                task_agent: None,
            })
            .await
            .unwrap(),
        AttemptBeginOutcome::Started
    );
    ticket
}

fn fact(ticket: InferenceTicketId, reported: bool) -> UsageFact {
    UsageFact {
        ticket,
        provider: String::from("openai"),
        model: String::from("test-model"),
        input_tokens: reported.then_some(12),
        cached_input_tokens: reported.then_some(4),
        output_tokens: reported.then_some(3),
        source: if reported {
            UsageSource::Reported
        } else {
            UsageSource::Unknown
        },
    }
}

fn read(
    store: &Store,
    ticket: InferenceTicketId,
) -> (Option<i64>, Option<i64>, Option<i64>, String) {
    crate::codec::lock_shared(&store.conn).query_row(
        "SELECT input_tokens, cached_input_tokens, output_tokens, source FROM usage_fact WHERE ticket = ?1",
        [crate::codec::encode_id(ticket.0)],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).unwrap()
}

#[tokio::test]
async fn unknown_is_null_and_terminal_even_after_duplicate_report() {
    let store = open_memory().await.unwrap();
    let ticket = claim(&store).await;
    store.record_usage(fact(ticket, false)).await.unwrap();
    store.record_usage(fact(ticket, false)).await.unwrap();
    store.record_usage(fact(ticket, true)).await.unwrap();
    assert_eq!(
        read(&store, ticket),
        (None, None, None, String::from("unknown"))
    );
}

#[tokio::test]
async fn invalid_or_unattributable_usage_is_rejected() {
    let store = open_memory().await.unwrap();
    let ticket = claim(&store).await;
    let valid = fact(ticket, true);
    let mut invalid = Vec::new();
    let mut orphan = valid.clone();
    orphan.ticket = InferenceTicketId(RawId::new());
    invalid.push(orphan);
    let mut route = valid.clone();
    route.model = String::from("other-model");
    invalid.push(route);
    let mut partial = valid.clone();
    partial.cached_input_tokens = None;
    invalid.push(partial);
    let mut cache = valid.clone();
    cache.cached_input_tokens = Some(13);
    invalid.push(cache);
    let mut unknown = valid.clone();
    unknown.source = UsageSource::Unknown;
    invalid.push(unknown);
    let mut overflow = valid.clone();
    overflow.input_tokens = Some(u64::MAX);
    invalid.push(overflow);
    for bad in invalid {
        assert!(store.record_usage(bad).await.is_err());
    }
    store.record_usage(valid).await.unwrap();
    assert_eq!(
        read(&store, ticket),
        (Some(12), Some(4), Some(3), String::from("reported"))
    );
}

#[tokio::test]
async fn concurrent_duplicate_settlement_survives_restart_with_attribution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("usage.db");
    let first = Store::open(&path).await.unwrap();
    let second = Store::open(&path).await.unwrap();
    let ticket = claim(&first).await;
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let other = Arc::clone(&barrier);
    let (a, b) = tokio::join!(
        async {
            barrier.wait().await;
            first.record_usage(fact(ticket, true)).await
        },
        async {
            other.wait().await;
            second.record_usage(fact(ticket, true)).await
        }
    );
    a.unwrap();
    b.unwrap();
    drop(first);
    drop(second);
    let reopened = Store::open(&path).await.unwrap();
    reopened.record_usage(fact(ticket, false)).await.unwrap();
    assert_eq!(
        read(&reopened, ticket),
        (Some(12), Some(4), Some(3), String::from("reported"))
    );
    let attempt = reopened
        .load_inference_attempt(ticket)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(attempt.consumer, ConsumerKind::CompanionDialogue);
    assert_eq!(attempt.capability, CapabilityKind::Dialogue);
    assert_eq!(attempt.purpose, PurposeKind::DialogueResponse);
    assert_eq!(attempt.provider, "openai");
    assert_eq!(attempt.model, "test-model");
    let count: i64 = crate::codec::lock_shared(&reopened.conn)
        .query_row("SELECT count(*) FROM usage_fact", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}
