//! Canonical token settlement, attribution, pricing binding, and
//! reproducible cost projection.
use std::sync::Arc;

use ene_inference::cost::{CurrencyCode, TokenRate, UsageCostFact};
use ene_inference::pricing::{PricingCatalogRevision, PricingSnapshot};

use super::*;

/// Seeds (or advances) the dialogue consent for one route and returns the
/// current `(id, rev)` premise a claim must rely on.
async fn route_consent(store: &Store, provider: &str, model: &str) -> (String, ConsentRevision) {
    let current = store
        .load_current(CapabilityKind::Dialogue)
        .await
        .expect("the consent read must answer");
    let (expected, rev) = match &current {
        None => (None, ConsentRevision::from_u64(1)),
        Some(stored) => (
            Some((stored.id.clone(), stored.rev)),
            ConsentRevision::from_u64(stored.rev.as_u64() + 1),
        ),
    };
    let outcome = save_consent(
        store,
        expected,
        ConsentRecord {
            capability: CapabilityKind::Dialogue,
            id: String::from("usage-consent"),
            rev,
            provider: provider.to_owned(),
            model: model.to_owned(),
            credential_id: String::from("openai:main"),
        },
    )
    .await;
    assert!(
        matches!(outcome, ConsentCommitOutcome::Committed { .. }),
        "the route consent must commit, got {outcome:?}"
    );
    let stored = store
        .load_current(CapabilityKind::Dialogue)
        .await
        .expect("the consent read must answer")
        .expect("the committed consent must be current");
    (stored.id, stored.rev)
}

fn attempt_for(
    ticket: InferenceTicketId,
    provider: &str,
    model: &str,
    premise: (String, ConsentRevision),
    pricing: Option<PricingSnapshot>,
) -> InferenceAttempt {
    InferenceAttempt {
        ticket,
        consumer: ConsumerKind::CompanionDialogue,
        capability: CapabilityKind::Dialogue,
        purpose: PurposeKind::DialogueResponse,
        expected_consent: premise,
        expected_credential_set: CredentialSetRevision::initial(),
        provider: provider.to_owned(),
        model: model.to_owned(),
        task_agent: None,
        pricing,
        usage_estimate: None,
    }
}

async fn claim_route(
    store: &Store,
    provider: &str,
    model: &str,
    pricing: Option<PricingSnapshot>,
) -> InferenceTicketId {
    let premise = route_consent(store, provider, model).await;
    let ticket = InferenceTicketId(RawId::new());
    let outcome = store
        .begin_inference_attempt(attempt_for(ticket, provider, model, premise, pricing))
        .await
        .expect("the claim must answer");
    assert_eq!(outcome, AttemptBeginOutcome::Started);
    ticket
}

async fn claim(store: &Store) -> InferenceTicketId {
    claim_route(store, "openai", "test-model", None).await
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

fn snapshot(revision: u64, input: u64, cached_input: u64, output: u64) -> PricingSnapshot {
    snapshot_for(
        "openai",
        "test-model",
        revision,
        input,
        cached_input,
        output,
    )
}

fn snapshot_for(
    provider: &str,
    model: &str,
    revision: u64,
    input: u64,
    cached_input: u64,
    output: u64,
) -> PricingSnapshot {
    PricingSnapshot {
        provider: provider.to_owned(),
        model: model.to_owned(),
        currency: CurrencyCode::Usd,
        input_rate: TokenRate::from_micros_per_million(input),
        cached_input_rate: TokenRate::from_micros_per_million(cached_input),
        output_rate: TokenRate::from_micros_per_million(output),
        effective_at: WallClockWithTz::parse_rfc3339("2025-06-01T00:00:00Z")
            .expect("the fixture instant parses"),
        source_revision: PricingCatalogRevision::new(revision),
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

fn stored_binding(store: &Store, ticket: InferenceTicketId) -> Option<String> {
    crate::codec::lock_shared(&store.conn)
        .query_row(
            "SELECT pricing_snapshot FROM usage_fact WHERE ticket = ?1",
            [crate::codec::encode_id(ticket.0)],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

fn attempt_binding(store: &Store, ticket: InferenceTicketId) -> Option<String> {
    crate::codec::lock_shared(&store.conn)
        .query_row(
            "SELECT pricing_snapshot FROM inference_attempt WHERE ticket = ?1",
            [crate::codec::encode_id(ticket.0)],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

fn pricing_row_count(store: &Store) -> i64 {
    crate::codec::lock_shared(&store.conn)
        .query_row("SELECT count(*) FROM pricing_snapshot", [], |row| {
            row.get(0)
        })
        .unwrap()
}

async fn settled_cost(store: &Store, ticket: InferenceTicketId) -> UsageCostFact {
    let record = store
        .load_usage_cost(ticket)
        .await
        .expect("the cost read must answer")
        .expect("the ticket is settled");
    assert_eq!(record.usage.ticket, ticket);
    record.cost
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

#[tokio::test]
async fn reported_cost_has_three_components_and_total() {
    let store = open_memory().await.unwrap();
    let pricing = snapshot(1, 2_000_000, 500_000, 8_000_000);
    let reference = pricing.reference().to_text();
    let ticket = claim_route(&store, "openai", "test-model", Some(pricing)).await;
    store.record_usage(fact(ticket, true)).await.unwrap();
    let UsageCostFact::Reported(cost) = settled_cost(&store, ticket).await else {
        panic!("a reported usage under a reviewed rate must project Reported");
    };
    // non_cached = 12 - 4 = 8 * 2.0 = 16 micros; cached = 4 * 0.5 = 2;
    // output = 3 * 8.0 = 24. The cached subset is never charged at the
    // normal input rate on top: 12 * 2.0 would have been 24.
    assert_eq!(cost.input.micros(), 16);
    assert_eq!(cost.cached_input.micros(), 2);
    assert_eq!(cost.output.micros(), 24);
    assert_eq!(cost.total.micros(), 42);
    assert_eq!(cost.total.currency(), CurrencyCode::Usd);
    assert_eq!(cost.pricing.to_text(), reference);
    // The attempt and the settled fact bind the same durable snapshot.
    assert_eq!(attempt_binding(&store, ticket), Some(reference.clone()));
    assert_eq!(stored_binding(&store, ticket), Some(reference));
    assert_eq!(pricing_row_count(&store), 1);
}

#[tokio::test]
async fn a_new_catalog_revision_does_not_reprice_a_settled_fact() {
    let store = open_memory().await.unwrap();
    let old = snapshot(1, 2_000_000, 500_000, 8_000_000);
    let old_reference = old.reference();
    let first = claim_route(&store, "openai", "test-model", Some(old)).await;
    store.record_usage(fact(first, true)).await.unwrap();
    // The current catalog moves to revision 2 with cheaper rates. Only the
    // call admitted after the move may resolve it.
    let new = snapshot(2, 100_000, 50_000, 400_000);
    let new_reference = new.reference();
    assert_ne!(old_reference, new_reference);
    let second = claim_route(&store, "openai", "test-model", Some(new)).await;
    store.record_usage(fact(second, true)).await.unwrap();
    let UsageCostFact::Reported(first_cost) = settled_cost(&store, first).await else {
        panic!("the first fact stays Reported");
    };
    let UsageCostFact::Reported(second_cost) = settled_cost(&store, second).await else {
        panic!("the second fact stays Reported");
    };
    assert_eq!(first_cost.pricing, old_reference);
    assert_eq!(first_cost.total.micros(), 42);
    assert_eq!(second_cost.pricing, new_reference);
    // 8 * 0.1 -> 1, 4 * 0.05 -> 1, 3 * 0.4 -> 2 (each component rounds up).
    assert_eq!(second_cost.total.micros(), 4);
    assert_ne!(first_cost.total, second_cost.total);
    assert_eq!(stored_binding(&store, first), Some(old_reference.to_text()));
    assert_eq!(
        pricing_row_count(&store),
        2,
        "one immutable row per revision"
    );
}

#[tokio::test]
async fn historical_cost_survives_restart_with_the_bound_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pricing.db");
    let store = Store::open(&path).await.unwrap();
    let pricing = snapshot(1, 2_000_000, 500_000, 8_000_000);
    let reference = pricing.reference();
    let ticket = claim_route(&store, "openai", "test-model", Some(pricing)).await;
    store.record_usage(fact(ticket, true)).await.unwrap();
    drop(store);
    let reopened = Store::open(&path).await.unwrap();
    let UsageCostFact::Reported(cost) = settled_cost(&reopened, ticket).await else {
        panic!("the persisted cost must project Reported");
    };
    assert_eq!(cost.pricing, reference);
    assert_eq!(cost.input.micros(), 16);
    assert_eq!(cost.cached_input.micros(), 2);
    assert_eq!(cost.output.micros(), 24);
    assert_eq!(cost.total.micros(), 42);
}

#[tokio::test]
async fn unknown_or_unpriced_settle_unknown_never_zero() {
    let store = open_memory().await.unwrap();
    // An unreviewed route claims no rate; the reported token spend must not
    // become a guessed or zero amount.
    let unpriced = claim_route(&store, "openai", "test-model", None).await;
    store.record_usage(fact(unpriced, true)).await.unwrap();
    assert_eq!(
        settled_cost(&store, unpriced).await,
        UsageCostFact::Unknown { pricing: None }
    );
    // A reviewed rate with unknown token counts keeps the rate reference but
    // still settles no amount.
    let pricing = snapshot(1, 2_000_000, 500_000, 8_000_000);
    let reference = pricing.reference();
    let unknown_usage = claim_route(&store, "openai", "test-model", Some(pricing)).await;
    store
        .record_usage(fact(unknown_usage, false))
        .await
        .unwrap();
    assert_eq!(
        settled_cost(&store, unknown_usage).await,
        UsageCostFact::Unknown {
            pricing: Some(reference)
        }
    );
    // An un-settled claimed attempt is `None`, distinct from Unknown.
    let pending = claim_route(&store, "openai", "test-model", None).await;
    assert_eq!(store.load_usage_cost(pending).await.unwrap(), None);
}

#[tokio::test]
async fn snapshot_route_must_match_the_attempt_route() {
    let store = open_memory().await.unwrap();
    for (provider, model, pricing) in [
        (
            "openai",
            "test-model",
            snapshot_for("openai", "other-model", 1, 1_000_000, 500_000, 2_000_000),
        ),
        (
            "openai",
            "test-model",
            snapshot_for("acme", "test-model", 1, 1_000_000, 500_000, 2_000_000),
        ),
    ] {
        let premise = route_consent(&store, provider, model).await;
        let ticket = InferenceTicketId(RawId::new());
        let outcome = store
            .begin_inference_attempt(attempt_for(ticket, provider, model, premise, Some(pricing)))
            .await;
        assert!(
            outcome.is_err(),
            "a snapshot for another route must never be bound to the attempt"
        );
        assert_eq!(
            attempt_binding(&store, ticket),
            None,
            "the refused claim must not bind anything"
        );
    }
    assert_eq!(pricing_row_count(&store), 0);
}

#[tokio::test]
async fn different_models_never_share_a_rate() {
    let store = open_memory().await.unwrap();
    let first = claim_route(
        &store,
        "openai",
        "model-a",
        Some(snapshot_for(
            "openai", "model-a", 1, 1_000_000, 500_000, 2_000_000,
        )),
    )
    .await;
    let mut first_fact = fact(first, true);
    first_fact.model = String::from("model-a");
    store.record_usage(first_fact).await.unwrap();
    let second = claim_route(
        &store,
        "openai",
        "model-b",
        Some(snapshot_for(
            "openai", "model-b", 1, 9_000_000, 4_500_000, 18_000_000,
        )),
    )
    .await;
    let mut second_fact = fact(second, true);
    second_fact.model = String::from("model-b");
    store.record_usage(second_fact).await.unwrap();
    let UsageCostFact::Reported(first_cost) = settled_cost(&store, first).await else {
        panic!("model-a must project its own rates");
    };
    let UsageCostFact::Reported(second_cost) = settled_cost(&store, second).await else {
        panic!("model-b must project its own rates");
    };
    assert_eq!(first_cost.total.micros(), 16);
    assert_eq!(second_cost.total.micros(), 144);
    assert_ne!(first_cost.pricing, second_cost.pricing);
}

#[tokio::test]
async fn cost_read_refuses_tampered_pricing_binding() {
    let store = open_memory().await.unwrap();
    let first = snapshot(1, 2_000_000, 500_000, 8_000_000);
    let first_ticket = claim_route(&store, "openai", "test-model", Some(first)).await;
    store.record_usage(fact(first_ticket, true)).await.unwrap();
    let second = snapshot(2, 100_000, 50_000, 400_000);
    let second_reference = second.reference().to_text();
    let second_ticket = claim_route(&store, "openai", "test-model", Some(second)).await;
    store.record_usage(fact(second_ticket, true)).await.unwrap();
    // Repointing the settled fact at revision 2's valid row would silently
    // reprice history; the attempt binding still names revision 1, so the
    // read refuses instead of projecting the other revision's rates.
    crate::codec::lock_shared(&store.conn)
        .execute(
            "UPDATE usage_fact SET pricing_snapshot = ?1 WHERE ticket = ?2",
            params![second_reference, crate::codec::encode_id(first_ticket.0)],
        )
        .unwrap();
    assert!(
        store.load_usage_cost(first_ticket).await.is_err(),
        "a swapped binding must fail closed, never reprice"
    );
    assert!(store.load_usage_cost(second_ticket).await.is_ok());
}

#[tokio::test]
async fn cost_read_refuses_edited_snapshot_content_and_dangling_references() {
    let store = open_memory().await.unwrap();
    let pricing = snapshot(1, 2_000_000, 500_000, 8_000_000);
    let reference = pricing.reference().to_text();
    let ticket = claim_route(&store, "openai", "test-model", Some(pricing)).await;
    store.record_usage(fact(ticket, true)).await.unwrap();
    // A rate edited under the same reference no longer derives that
    // reference, so the row is unreadable.
    crate::codec::lock_shared(&store.conn)
        .execute(
            "UPDATE pricing_snapshot SET output_rate = output_rate + 1 WHERE id = ?1",
            params![reference],
        )
        .unwrap();
    assert!(store.load_usage_cost(ticket).await.is_err());
    // A reference with no row at all is also a technical error.
    crate::codec::lock_shared(&store.conn)
        .execute(
            "UPDATE usage_fact SET pricing_snapshot = ?1 WHERE ticket = ?2",
            params![
                String::from("00000000-0000-4000-8000-000000000000"),
                crate::codec::encode_id(ticket.0)
            ],
        )
        .unwrap();
    assert!(store.load_usage_cost(ticket).await.is_err());
}

#[tokio::test]
async fn cost_read_refuses_unrepresentable_amounts() {
    let store = open_memory().await.unwrap();
    // Absurd but storable rates and counts: their exact product does not fit
    // the money representation, so the projection fails closed rather than
    // wrapping or saturating to zero.
    let pricing = snapshot(
        1,
        1_000_000_000_000_000_000,
        1_000_000_000_000_000_000,
        1_000_000_000_000_000_000,
    );
    let ticket = claim_route(&store, "openai", "test-model", Some(pricing)).await;
    store
        .record_usage(UsageFact {
            ticket,
            provider: String::from("openai"),
            model: String::from("test-model"),
            input_tokens: Some(9_000_000_000_000_000_000),
            cached_input_tokens: Some(0),
            output_tokens: Some(0),
            source: UsageSource::Reported,
        })
        .await
        .unwrap();
    let result = store.load_usage_cost(ticket).await;
    assert!(
        matches!(
            result,
            Err(InferenceTechnicalError::CostProjectionFailed { .. })
        ),
        "an unrepresentable amount must fail closed, got {result:?}"
    );
}
