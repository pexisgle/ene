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
    UsageCapWindow, UsageReservationState,
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

fn unknown_usage(ticket: InferenceTicketId) -> UsageFact {
    UsageFact {
        ticket,
        provider: PROVIDER.to_owned(),
        model: MODEL.to_owned(),
        input_tokens: None,
        cached_input_tokens: None,
        output_tokens: None,
        source: UsageSource::Unknown,
    }
}

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

async fn update_cap(
    store: &Store,
    expected: Option<UsageCapRef>,
    scope: UsageCapScope,
    window: UsageCapWindow,
    limit_micros: u64,
) -> SetUsageCapOutcome {
    store
        .set_usage_cap(SetUsageCapCommand {
            expected,
            scope,
            window,
            limit: Money::from_micros(CurrencyCode::Usd, limit_micros),
        })
        .await
        .expect("the cap write must answer")
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

/// Inserts one reservation row directly, for boundary fixtures that do not
/// travel through admission.
fn insert_reservation(
    store: &Store,
    opened_at: &str,
    upper_bound_micros: i64,
    state: &str,
    committed_micros: Option<i64>,
) {
    let committed_currency = committed_micros.map(|_| "USD");
    crate::codec::lock_shared(&store.conn)
        .execute(
            "INSERT INTO usage_reservation (reservation_id, ticket, provider, model, pricing_snapshot, currency, upper_bound_micros, state, committed_currency, committed_micros, opened_at) VALUES (?1, ?2, ?3, ?4, ?5, 'USD', ?6, ?7, ?8, ?9, ?10)",
            params![
                crate::codec::encode_id(RawId::new()),
                crate::codec::encode_id(RawId::new()),
                PROVIDER,
                MODEL,
                crate::codec::encode_id(RawId::new()),
                upper_bound_micros,
                state,
                committed_currency,
                committed_micros,
                opened_at,
            ],
        )
        .unwrap();
}

fn at(value: &str) -> WallClockWithTz {
    WallClockWithTz::parse_rfc3339(value).expect("the fixture instant parses")
}

fn consumed(
    store: &Store,
    scope: &UsageCapScope,
    window: UsageCapWindow,
    at_value: WallClockWithTz,
) -> u64 {
    let guard = crate::codec::lock_shared(&store.conn);
    match crate::usage_cap::consumed_in_window(&guard, scope, window, CurrencyCode::Usd, at_value)
        .expect("the accounting read must answer")
    {
        crate::usage_cap::CapWindowConsumption::Known(money) => money.micros(),
        crate::usage_cap::CapWindowConsumption::Indeterminate => {
            panic!("the fixture accounting must be representable")
        }
    }
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
async fn daily_and_monthly_windows_are_checked_separately() {
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    stored_cap(
        &store,
        UsageCapScope::System,
        UsageCapWindow::DailyUtc,
        3 * UPPER_BOUND_MICROS,
    )
    .await;
    stored_cap(
        &store,
        UsageCapScope::System,
        UsageCapWindow::MonthlyUtc,
        UPPER_BOUND_MICROS,
    )
    .await;
    let (_, first) = claim_bounded(&store, &premise).await;
    assert_eq!(first, AttemptBeginOutcome::Started);
    let (_, second) = claim_bounded(&store, &premise).await;
    let AttemptBeginOutcome::HeldByCap(held) = &second else {
        panic!("the monthly cap must hold the second call, got {second:?}");
    };
    assert_eq!(held.window(), UsageCapWindow::MonthlyUtc);
}

#[tokio::test]
async fn a_cap_raise_applies_to_the_next_admission_after_the_update_commits() {
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    // One reserved upper bound fits (200 <= 300); two do not (400 > 300).
    let reference = stored_cap(&store, scope.clone(), window, UPPER_BOUND_MICROS + 100).await;
    let (_, first) = claim_bounded(&store, &premise).await;
    assert_eq!(first, AttemptBeginOutcome::Started);
    let (_, second) = claim_bounded(&store, &premise).await;
    assert!(
        matches!(second, AttemptBeginOutcome::HeldByCap(_)),
        "the old cap decides before the update commits"
    );
    let outcome = update_cap(&store, None, scope.clone(), window, 2 * UPPER_BOUND_MICROS).await;
    assert!(
        matches!(outcome, SetUsageCapOutcome::Stale { .. }),
        "a create with no expectation never overwrites an existing cap"
    );
    let (_, third) = claim_bounded(&store, &premise).await;
    assert!(
        matches!(third, AttemptBeginOutcome::HeldByCap(_)),
        "the rejected update must not change the stored limit"
    );
    // The compare-and-set update commits first; the next admission reads the
    // raised cap and fits the second reserved bound (200 + 200 <= 400).
    let raised = update_cap(
        &store,
        Some(reference),
        scope,
        window,
        2 * UPPER_BOUND_MICROS,
    )
    .await;
    assert!(
        matches!(raised, SetUsageCapOutcome::StoredAs(_)),
        "the raised cap must store under the current revision, got {raised:?}"
    );
    let (_, fourth) = claim_bounded(&store, &premise).await;
    assert_eq!(
        fourth,
        AttemptBeginOutcome::Started,
        "the raised cap must decide the next admission"
    );
}

#[tokio::test]
async fn a_cap_lower_holds_after_the_update_commits_first() {
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    let reference = stored_cap(&store, scope.clone(), window, 4 * UPPER_BOUND_MICROS).await;
    let (_, first) = claim_bounded(&store, &premise).await;
    assert_eq!(first, AttemptBeginOutcome::Started);
    // Lowering the cap below the already-consumed amount holds every new send.
    let lowered = update_cap(
        &store,
        Some(reference),
        scope,
        window,
        UPPER_BOUND_MICROS - 1,
    )
    .await;
    assert!(
        matches!(lowered, SetUsageCapOutcome::StoredAs(_)),
        "the current expectation must store the new limit, got {lowered:?}"
    );
    let (_, second) = claim_bounded(&store, &premise).await;
    assert!(
        matches!(second, AttemptBeginOutcome::HeldByCap(_)),
        "the lowered cap must hold the next admission, got {second:?}"
    );
}

#[tokio::test]
async fn a_stale_cap_expectation_writes_nothing() {
    let store = open_memory().await.unwrap();
    let (scope, window) = system_daily();
    let reference = stored_cap(&store, scope.clone(), window, 2 * UPPER_BOUND_MICROS).await;
    // A stale revision (the first stored one is revision 0) writes nothing.
    let stale = update_cap(
        &store,
        Some(UsageCapRef::new(
            reference.id().clone(),
            ene_permission::UsageCapRevision::from_u64(reference.revision().as_u64() + 1),
        )),
        scope.clone(),
        window,
        UPPER_BOUND_MICROS,
    )
    .await;
    let SetUsageCapOutcome::Stale { current } = stale else {
        panic!("a moved revision must answer stale, got {stale:?}");
    };
    assert_eq!(
        current.as_ref().map(|cap| cap.revision()),
        Some(reference.revision()),
        "the stale answer names the current revision"
    );
    let stored: i64 = crate::codec::lock_shared(&store.conn)
        .query_row("SELECT limit_micros FROM usage_cap", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        stored as u64,
        2 * UPPER_BOUND_MICROS,
        "a stale expectation must not rewrite the limit"
    );
}

#[tokio::test]
async fn a_zero_limit_is_not_a_ceiling() {
    let store = open_memory().await.unwrap();
    let (scope, window) = system_daily();
    let outcome = set_cap(&store, scope, window, 0).await;
    assert_eq!(outcome, SetUsageCapOutcome::InvalidLimit);
    let rows: i64 = crate::codec::lock_shared(&store.conn)
        .query_row("SELECT count(*) FROM usage_cap", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0, "an invalid limit writes nothing");
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

#[tokio::test]
async fn unknown_settlement_keeps_the_upper_bound_and_holds_the_next_call() {
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    stored_cap(&store, scope, window, 350).await;
    let (first, outcome) = claim_bounded(&store, &premise).await;
    assert_eq!(outcome, AttemptBeginOutcome::Started);
    store.record_usage(unknown_usage(first)).await.unwrap();
    assert_eq!(
        reserved_row(&store, first),
        Some((
            String::from("committed_unknown"),
            UPPER_BOUND_MICROS as i64,
            None
        )),
        "an uncertain outcome keeps the reserved upper bound, never a zero"
    );
    assert_eq!(
        usage_source(&store, first),
        Some(String::from("unknown")),
        "unknown token usage stays a durable unknown fact"
    );
    let (_, second) = claim_bounded(&store, &premise).await;
    assert!(
        matches!(second, AttemptBeginOutcome::HeldByCap(_)),
        "an unknown external consumption must hold the next call, got {second:?}"
    );
}

#[tokio::test]
async fn a_duplicate_settlement_never_revises_the_terminal_reservation() {
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    stored_cap(&store, scope, window, 350).await;
    let (first, outcome) = claim_bounded(&store, &premise).await;
    assert_eq!(outcome, AttemptBeginOutcome::Started);
    store.record_usage(unknown_usage(first)).await.unwrap();
    store.record_usage(reported_usage(first)).await.unwrap();
    assert_eq!(
        reserved_row(&store, first),
        Some((
            String::from("committed_unknown"),
            UPPER_BOUND_MICROS as i64,
            None
        ))
    );
    assert_eq!(usage_source(&store, first), Some(String::from("unknown")));
}

#[tokio::test]
async fn an_orphaned_reservation_survives_restart_as_committed_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("usage-cap-recovery.db");
    let store = Store::open(&path).await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    stored_cap(&store, scope, window, 350).await;
    let (first, outcome) = claim_bounded(&store, &premise).await;
    assert_eq!(outcome, AttemptBeginOutcome::Started);
    let reference = store
        .load_usage_reservation(first)
        .await
        .unwrap()
        .expect("the claim opened a reservation")
        .reference;
    // Crash before any settlement: dropping the store leaves the reservation
    // `Reserved` and the provider outcome unknowable.
    drop(store);
    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(
        reopened
            .reconcile_orphaned_usage_reservations()
            .await
            .unwrap(),
        1,
        "the orphaned reservation must be re-evaluated once"
    );
    let reservation = reopened
        .load_usage_reservation(first)
        .await
        .unwrap()
        .expect("the reservation survives the restart");
    assert_eq!(reservation.reference, reference);
    assert_eq!(reservation.state, UsageReservationState::CommittedUnknown);
    assert_eq!(
        reservation.upper_bound,
        Money::from_micros(CurrencyCode::Usd, UPPER_BOUND_MICROS)
    );
    assert_eq!(reservation.committed, None);
    let cost = reopened
        .load_usage_cost(first)
        .await
        .unwrap()
        .expect("recovery settles the unknown token usage fact");
    assert_eq!(cost.usage.source, UsageSource::Unknown);
    assert_eq!(
        cost.cost,
        UsageCostFact::Unknown {
            pricing: Some(reservation.pricing)
        },
        "an unknown settlement keeps the pricing basis and no amount"
    );
    let (_, second) = claim_bounded(&reopened, &premise).await;
    assert!(
        matches!(second, AttemptBeginOutcome::HeldByCap(_)),
        "the crashed call's upper bound must keep occupying the cap, got {second:?}"
    );
    assert_eq!(
        reopened
            .reconcile_orphaned_usage_reservations()
            .await
            .unwrap(),
        0,
        "reconciliation is idempotent"
    );
}

#[tokio::test]
async fn a_committed_reported_reservation_still_counts_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("usage-cap-commit.db");
    let store = Store::open(&path).await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    stored_cap(&store, scope, window, 350).await;
    let (first, outcome) = claim_bounded(&store, &premise).await;
    assert_eq!(outcome, AttemptBeginOutcome::Started);
    store.record_usage(reported_usage(first)).await.unwrap();
    drop(store);
    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(
        reopened
            .reconcile_orphaned_usage_reservations()
            .await
            .unwrap(),
        0,
        "a terminal reservation is never re-counted"
    );
    assert_eq!(
        reserved_row(&reopened, first),
        Some((
            String::from("committed_reported"),
            UPPER_BOUND_MICROS as i64,
            Some(REPORTED_TOTAL_MICROS)
        )),
        "the committed actual cost survives the restart"
    );
    let (_, second) = claim_bounded(&reopened, &premise).await;
    assert_eq!(
        second,
        AttemptBeginOutcome::Started,
        "the committed slot stays counted: 128 + 200 <= 350"
    );
}

#[tokio::test]
async fn a_cap_enabled_send_without_a_finite_bound_never_starts() {
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    stored_cap(&store, scope.clone(), window, 10 * UPPER_BOUND_MICROS).await;
    // No reviewed rate: the bound cannot be priced in the cap's currency.
    let (unpriced, outcome) = claim(&store, &premise, None, Some(estimate())).await;
    assert_eq!(outcome, AttemptBeginOutcome::CapIndeterminate);
    // A reviewed rate but no provider estimate: no finite safe bound.
    let (unestimated, outcome) = claim(&store, &premise, Some(pricing()), None).await;
    assert_eq!(outcome, AttemptBeginOutcome::CapIndeterminate);
    assert_eq!(attempt_count(&store), 0);
    assert_eq!(reservation_count(&store), 0);
    assert_eq!(usage_source(&store, unpriced), None);
    assert_eq!(usage_source(&store, unestimated), None);
}

#[tokio::test]
async fn window_accounting_uses_half_open_utc_boundaries() {
    let store = open_memory().await.unwrap();
    // Fixed fixtures around the March 2026 boundaries.
    insert_reservation(
        &store,
        "2026-03-15T00:00:00.000000000Z",
        10,
        "reserved",
        None,
    );
    insert_reservation(
        &store,
        "2026-03-15T23:59:59.999999999Z",
        20,
        "reserved",
        None,
    );
    insert_reservation(
        &store,
        "2026-03-01T00:00:00.000000000Z",
        40,
        "reserved",
        None,
    );
    insert_reservation(
        &store,
        "2026-02-28T23:59:59.999999999Z",
        80,
        "reserved",
        None,
    );
    // The reported actual replaces the upper bound; a released row counts
    // nothing at all.
    insert_reservation(
        &store,
        "2026-03-10T12:00:00.000000000Z",
        500,
        "committed_reported",
        Some(7),
    );
    insert_reservation(
        &store,
        "2026-03-11T12:00:00.000000000Z",
        900,
        "released",
        None,
    );
    let scope = UsageCapScope::System;
    let daily = UsageCapWindow::DailyUtc;
    let monthly = UsageCapWindow::MonthlyUtc;
    assert_eq!(
        consumed(&store, &scope, daily, at("2026-03-15T12:00:00Z")),
        30,
        "the current day counts both of its rows and nothing older"
    );
    assert_eq!(
        consumed(&store, &scope, monthly, at("2026-03-15T12:00:00Z")),
        77,
        "the current month counts every March row (including its reported settlements), never February"
    );
    assert_eq!(
        consumed(&store, &scope, daily, at("2026-03-16T00:00:00Z")),
        0,
        "a daily period excludes its exclusive end instant"
    );
    assert_eq!(
        consumed(&store, &scope, monthly, at("2026-04-01T00:00:00Z")),
        0,
        "a monthly period excludes its exclusive end instant"
    );
    assert_eq!(
        consumed(&store, &scope, monthly, at("2026-03-01T00:00:00Z")),
        77,
        "a period includes its inclusive start instant and everything after it"
    );
    assert_eq!(
        consumed(
            &store,
            &scope,
            monthly,
            at("2026-02-28T23:59:59.999999999Z")
        ),
        80,
        "the instant before the start belongs to the completed month"
    );
    assert_eq!(
        consumed(&store, &scope, daily, at("2026-03-10T12:00:00Z")),
        7,
        "a reported settlement contributes the actual cost, not the upper bound"
    );
    assert_eq!(
        consumed(&store, &scope, daily, at("2026-03-11T12:00:00Z")),
        0,
        "a released reservation contributes nothing"
    );
}

#[tokio::test]
async fn recovery_fails_closed_when_the_reservation_correlation_is_broken() {
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    stored_cap(&store, scope, window, 350).await;
    let (first, outcome) = claim_bounded(&store, &premise).await;
    assert_eq!(outcome, AttemptBeginOutcome::Started);
    // The attempt and the reservation share one transaction, so a vanished
    // attempt row is corruption: recovery must not fabricate a settlement
    // for an unattributable reservation.
    crate::codec::lock_shared(&store.conn)
        .execute(
            "DELETE FROM inference_attempt WHERE ticket = ?1",
            [crate::codec::encode_id(first.0)],
        )
        .unwrap();
    let reconciled = store.reconcile_orphaned_usage_reservations().await;
    assert!(
        matches!(
            reconciled,
            Err(InferenceTechnicalError::StorageUnavailable { .. })
        ),
        "a broken correlation must fail closed, got {reconciled:?}"
    );
    assert_eq!(
        reserved_row(&store, first)
            .expect("the reservation row stays")
            .0,
        "reserved",
        "a failed reconciliation must not half-settle the reservation"
    );
}

#[tokio::test]
async fn older_period_usage_never_blocks_the_current_period() {
    let now = WallClockWithTz::now().as_datetime();
    let yesterday = now - std::time::Duration::from_secs(86_400);
    let previous_month = now - std::time::Duration::from_secs(32 * 86_400);

    // Yesterday is outside the current daily window: it must not consume
    // today's limit.
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    let (scope, window) = system_daily();
    stored_cap(&store, scope, window, UPPER_BOUND_MICROS).await;
    insert_reservation(
        &store,
        &WallClockWithTz::from_datetime(yesterday).to_rfc3339_utc(),
        1_000_000,
        "reserved",
        None,
    );
    let (_, outcome) = claim_bounded(&store, &premise).await;
    assert_eq!(
        outcome,
        AttemptBeginOutcome::Started,
        "a completed day must not consume the current day's limit"
    );

    // A reservation from a previous month is outside the current monthly
    // window even though it is inside no current daily window either.
    let store = open_memory().await.unwrap();
    let premise = route_consent(&store).await;
    stored_cap(
        &store,
        UsageCapScope::System,
        UsageCapWindow::MonthlyUtc,
        UPPER_BOUND_MICROS,
    )
    .await;
    insert_reservation(
        &store,
        &WallClockWithTz::from_datetime(previous_month).to_rfc3339_utc(),
        1_000_000,
        "committed_unknown",
        None,
    );
    let (_, outcome) = claim_bounded(&store, &premise).await;
    assert_eq!(
        outcome,
        AttemptBeginOutcome::Started,
        "a completed month must not consume the current month's limit"
    );
}
