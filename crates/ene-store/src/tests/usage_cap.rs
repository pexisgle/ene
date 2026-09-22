//! Usage cap admission, reservation lifecycle, settlement, restart
//! reconciliation, and UTC window accounting.
//!
//! The fixtures are deterministic on purpose: the concurrency races use a
//! barrier and two `Store` handles on one database file, and the window
//! boundaries are asserted against fixed instants rather than the clock.
use std::sync::Arc;

use ene_inference::cost::{CurrencyCode, TokenRate, UsageCostFact, UsageEstimate};
use ene_inference::pricing::{PricingCatalogRevision, PricingSnapshot};
use ene_permission::{
    SetUsageCapCommand, SetUsageCapOutcome, UsageCapRef, UsageCapRepository as _, UsageCapScope,
    UsageCapWindow,
};
use ene_primitive::Money;

use super::*;

const PROVIDER: &str = "openai";
const MODEL: &str = "test-model";

/// One reviewed revision: 1 micro per input token, 0.1 per cached input
/// token, 2 micros per output token.
fn pricing() -> PricingSnapshot {
    PricingSnapshot {
        provider: PROVIDER.to_owned(),
        model: MODEL.to_owned(),
        currency: CurrencyCode::Usd,
        input_rate: TokenRate::from_micros_per_million(1_000_000),
        cached_input_rate: TokenRate::from_micros_per_million(100_000),
        output_rate: TokenRate::from_micros_per_million(2_000_000),
        effective_at: WallClockWithTz::parse_rfc3339("2025-06-01T00:00:00Z")
            .expect("the fixture instant parses"),
        source_revision: PricingCatalogRevision::new(1),
    }
}

/// A safe upper bound of exactly 201 micros under [`pricing`]: 100 input
/// tokens at 1 micro plus one micro-unit for the separately rounded input
/// components, plus 50 output tokens at 2 micros.
const UPPER_BOUND_MICROS: u64 = 201;

fn estimate() -> UsageEstimate {
    UsageEstimate {
        input_tokens_upper_bound: 100,
        output_tokens_upper_bound: 50,
    }
}

/// A reported usage that settles to 128 micros under [`pricing`]: 20
/// non-cached input (20) + 80 cached input (8) + 50 output (100). It is
/// deliberately below the reserved upper bound.
fn reported_usage(ticket: InferenceTicketId) -> UsageFact {
    UsageFact {
        ticket,
        provider: PROVIDER.to_owned(),
        model: MODEL.to_owned(),
        input_tokens: Some(100),
        cached_input_tokens: Some(80),
        output_tokens: Some(50),
        source: UsageSource::Reported,
    }
}

const REPORTED_TOTAL_MICROS: i64 = 128;

/// Seeds (or advances) the dialogue consent for the fixture route and returns
/// the current `(id, rev)` premise a claim must rely on.
async fn route_consent(store: &Store) -> (String, ConsentRevision) {
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
            id: String::from("usage-cap-consent"),
            rev,
            provider: PROVIDER.to_owned(),
            model: MODEL.to_owned(),
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
    premise: &(String, ConsentRevision),
    pricing: Option<PricingSnapshot>,
    usage_estimate: Option<UsageEstimate>,
) -> InferenceAttempt {
    InferenceAttempt {
        ticket,
        consumer: ConsumerKind::CompanionDialogue,
        capability: CapabilityKind::Dialogue,
        purpose: PurposeKind::DialogueResponse,
        expected_consent: premise.clone(),
        expected_credential_set: CredentialSetRevision::initial(),
        provider: PROVIDER.to_owned(),
        model: MODEL.to_owned(),
        data_use: Vec::new(),
        task_agent: None,
        pricing,
        usage_estimate,
    }
}

/// Claims a fresh ticket with the fixture bound and returns the ticket.
async fn claim(
    store: &Store,
    premise: &(String, ConsentRevision),
    pricing: Option<PricingSnapshot>,
    usage_estimate: Option<UsageEstimate>,
) -> (InferenceTicketId, AttemptBeginOutcome) {
    let ticket = InferenceTicketId(RawId::new());
    let outcome = store
        .begin_inference_attempt(attempt_for(ticket, premise, pricing, usage_estimate))
        .await
        .expect("the claim must answer");
    (ticket, outcome)
}

/// Claims a fresh ticket carrying the fixture snapshot and estimate.
async fn claim_bounded(
    store: &Store,
    premise: &(String, ConsentRevision),
) -> (InferenceTicketId, AttemptBeginOutcome) {
    claim(store, premise, Some(pricing()), Some(estimate())).await
}

async fn set_cap(
    store: &Store,
    scope: UsageCapScope,
    window: UsageCapWindow,
    limit_micros: u64,
) -> SetUsageCapOutcome {
    store
        .set_usage_cap(SetUsageCapCommand {
            expected: None,
            scope,
            window,
            limit: Money::from_micros(CurrencyCode::Usd, limit_micros),
        })
        .await
        .expect("the cap write must answer")
}

async fn stored_cap(
    store: &Store,
    scope: UsageCapScope,
    window: UsageCapWindow,
    limit_micros: u64,
) -> UsageCapRef {
    match set_cap(store, scope, window, limit_micros).await {
        SetUsageCapOutcome::StoredAs(reference) => reference,
        other => panic!("the cap create must store, got {other:?}"),
    }
}

fn system_daily() -> (UsageCapScope, UsageCapWindow) {
    (UsageCapScope::System, UsageCapWindow::DailyUtc)
}

fn reserved_row(store: &Store, ticket: InferenceTicketId) -> Option<(String, i64, Option<i64>)> {
    crate::codec::lock_shared(&store.conn)
        .query_row(
            "SELECT state, upper_bound_micros, committed_micros FROM usage_reservation WHERE ticket = ?1",
            [crate::codec::encode_id(ticket.0)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .unwrap()
}

fn reservation_count(store: &Store) -> i64 {
    crate::codec::lock_shared(&store.conn)
        .query_row("SELECT count(*) FROM usage_reservation", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn attempt_count(store: &Store) -> i64 {
    crate::codec::lock_shared(&store.conn)
        .query_row("SELECT count(*) FROM inference_attempt", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn usage_source(store: &Store, ticket: InferenceTicketId) -> Option<String> {
    crate::codec::lock_shared(&store.conn)
        .query_row(
            "SELECT source FROM usage_fact WHERE ticket = ?1",
            [crate::codec::encode_id(ticket.0)],
            |row| row.get(0),
        )
        .optional()
        .unwrap()
}

#[tokio::test]
async fn an_uncapped_route_claims_without_a_reservation() {
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (_, outcome) = claim_bounded(&store, &premise).await;
    assert_eq!(outcome, AttemptBeginOutcome::Started);
    assert_eq!(
        reservation_count(&store),
        0,
        "no cap applies, so there is no limit to protect with a reservation"
    );
}

#[tokio::test]
async fn a_cap_refusal_creates_no_attempt_reservation_or_usage() {
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    // The limit is below the 200-micro upper bound: the very first claim is
    // already held, and nothing may be recorded for it.
    stored_cap(&store, scope, window, 199).await;
    let (ticket, outcome) = claim_bounded(&store, &premise).await;
    let AttemptBeginOutcome::HeldByCap(held) = &outcome else {
        panic!("a cap below the upper bound must hold the send, got {outcome:?}");
    };
    assert_eq!(held.scope(), &UsageCapScope::System);
    assert_eq!(held.window(), UsageCapWindow::DailyUtc);
    assert_eq!(reservation_count(&store), 0);
    assert_eq!(attempt_count(&store), 0, "a held claim creates no attempt");
    assert_eq!(
        usage_source(&store, ticket),
        None,
        "a refusal that never reached the provider records no actual usage"
    );
}

#[tokio::test]
async fn concurrent_claims_for_one_remaining_slot_start_at_most_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("usage-cap-race.db");
    let first = Store::open(&path).await.unwrap();
    let second = Store::open(&path).await.unwrap();
    let premise = route_consent(&first).await;
    // Exactly one 200-micro reservation fits.
    let (scope, window) = system_daily();
    stored_cap(&first, scope, window, UPPER_BOUND_MICROS).await;
    let first_ticket = InferenceTicketId(RawId::new());
    let second_ticket = InferenceTicketId(RawId::new());
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let other = Arc::clone(&barrier);
    let (a, b) = tokio::join!(
        async {
            barrier.wait().await;
            first
                .begin_inference_attempt(attempt_for(
                    first_ticket,
                    &premise,
                    Some(pricing()),
                    Some(estimate()),
                ))
                .await
        },
        async {
            other.wait().await;
            second
                .begin_inference_attempt(attempt_for(
                    second_ticket,
                    &premise,
                    Some(pricing()),
                    Some(estimate()),
                ))
                .await
        }
    );
    let a = a.expect("the first claim must answer");
    let b = b.expect("the second claim must answer");
    let started = [&a, &b]
        .iter()
        .filter(|outcome| matches!(outcome, AttemptBeginOutcome::Started))
        .count();
    let held = [&a, &b]
        .iter()
        .filter(|outcome| matches!(outcome, AttemptBeginOutcome::HeldByCap(_)))
        .count();
    assert_eq!(
        (started, held),
        (1, 1),
        "exactly one concurrent claim may consume the remaining slot: {a:?} / {b:?}"
    );
    assert_eq!(reservation_count(&first), 1);
    assert_eq!(attempt_count(&first), 1);
    let held_ticket = if matches!(a, AttemptBeginOutcome::HeldByCap(_)) {
        first_ticket
    } else {
        second_ticket
    };
    assert_eq!(reserved_row(&first, held_ticket), None);
    assert_eq!(usage_source(&first, held_ticket), None);
}

#[tokio::test]
async fn the_smaller_of_provider_and_system_caps_is_enforced() {
    // System is the binding cap.
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    stored_cap(&store, scope.clone(), window, UPPER_BOUND_MICROS).await;
    stored_cap(
        &store,
        UsageCapScope::Provider(PROVIDER.to_owned()),
        window,
        3 * UPPER_BOUND_MICROS,
    )
    .await;
    let (_, first) = claim_bounded(&store, &premise).await;
    assert_eq!(first, AttemptBeginOutcome::Started);
    let (_, second) = claim_bounded(&store, &premise).await;
    let AttemptBeginOutcome::HeldByCap(held) = &second else {
        panic!("the system cap must hold the second call, got {second:?}");
    };
    assert_eq!(held.scope(), &UsageCapScope::System);

    // Provider is the binding cap.
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    stored_cap(&store, scope, window, 3 * UPPER_BOUND_MICROS).await;
    stored_cap(
        &store,
        UsageCapScope::Provider(PROVIDER.to_owned()),
        window,
        UPPER_BOUND_MICROS,
    )
    .await;
    let (_, first) = claim_bounded(&store, &premise).await;
    assert_eq!(first, AttemptBeginOutcome::Started);
    let (_, second) = claim_bounded(&store, &premise).await;
    let AttemptBeginOutcome::HeldByCap(held) = &second else {
        panic!("the provider cap must hold the second call, got {second:?}");
    };
    assert_eq!(held.scope(), &UsageCapScope::Provider(PROVIDER.to_owned()));

    // Another provider's cap never applies to this route.
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    stored_cap(&store, scope, window, 3 * UPPER_BOUND_MICROS).await;
    stored_cap(
        &store,
        UsageCapScope::Provider(String::from("acme")),
        window,
        UPPER_BOUND_MICROS,
    )
    .await;
    let (_, first) = claim_bounded(&store, &premise).await;
    assert_eq!(
        first,
        AttemptBeginOutcome::Started,
        "another provider's cap must not hold this route"
    );
}

#[tokio::test]
async fn reported_settlement_counts_actual_and_releases_the_unused_reservation() {
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    // One reserved upper bound plus the actual committed cost fits
    // (200 + 128 = 328 <= 350), but two reserved bounds would not
    // (400 > 350): only a settlement that released the unused reservation
    // lets the second call through.
    stored_cap(&store, scope, window, 350).await;
    let (first, outcome) = claim_bounded(&store, &premise).await;
    assert_eq!(outcome, AttemptBeginOutcome::Started);
    assert_eq!(
        reserved_row(&store, first),
        Some((String::from("reserved"), UPPER_BOUND_MICROS as i64, None))
    );
    store.record_usage(reported_usage(first)).await.unwrap();
    assert_eq!(
        reserved_row(&store, first),
        Some((
            String::from("committed_reported"),
            UPPER_BOUND_MICROS as i64,
            Some(REPORTED_TOTAL_MICROS)
        )),
        "the actual committed cost replaces the reserved upper bound"
    );
    let UsageCostFact::Reported(cost) = store
        .load_usage_cost(first)
        .await
        .unwrap()
        .expect("the ticket is settled")
        .cost
    else {
        panic!("the reported facts must project a reported cost");
    };
    assert_eq!(cost.total.micros(), REPORTED_TOTAL_MICROS as u64);
    let (_, second) = claim_bounded(&store, &premise).await;
    assert_eq!(
        second,
        AttemptBeginOutcome::Started,
        "the unused reservation amount must be released by the settlement"
    );
}
