//! Bounded first-party usage summary and cap status reads
//! (`usage-cost-cap` §16).
//!
//! The fixtures use the production claim/settlement path so the read sees
//! exactly the rows the pipeline writes. Keyset paging is asserted on
//! deliberately identical `started_at` values, so the `(started_at, ticket)`
//! tiebreak is what keeps the walk lossless.

use ene_inference::cost::{CurrencyCode, TokenRate, UsageCostFact, UsageEstimate};
use ene_inference::pricing::{PricingCatalogRevision, PricingSnapshot};
use ene_inference::{
    ReportedTokenUsage, USAGE_SUMMARY_PAGE_MAX, UsageSummaryCursor, UsageSummaryQuery,
    UsageSummaryRepository as _, UsageSummaryStatus,
};
use ene_permission::{
    SetUsageCapCommand, SetUsageCapOutcome, UsageCapConsumption, UsageCapRef,
    UsageCapRepository as _, UsageCapScope, UsageCapStatusQuery, UsageCapWindow,
};
use ene_primitive::Money;

use super::*;

const PROVIDER: &str = "openai";
const MODEL: &str = "summary-model";

fn at(value: &str) -> WallClockWithTz {
    WallClockWithTz::parse_rfc3339(value).expect("the fixture instant parses")
}

/// 1 micro per input token, 0.1 per cached input, 2 per output.
fn pricing(revision: u64) -> PricingSnapshot {
    PricingSnapshot {
        provider: PROVIDER.to_owned(),
        model: MODEL.to_owned(),
        currency: CurrencyCode::Usd,
        input_rate: TokenRate::from_micros_per_million(1_000_000),
        cached_input_rate: TokenRate::from_micros_per_million(100_000),
        output_rate: TokenRate::from_micros_per_million(2_000_000),
        effective_at: at("2025-06-01T00:00:00Z"),
        source_revision: PricingCatalogRevision::new(revision),
    }
}

async fn seed_consent(
    store: &Store,
    capability: CapabilityKind,
    provider: &str,
    model: &str,
) -> (String, ConsentRevision) {
    let current = store
        .load_current(capability)
        .await
        .expect("the consent read must answer");
    let (expected, rev) = match &current {
        None => (None, ConsentRevision::from_u64(1)),
        Some(stored) => (
            Some((stored.id.clone(), stored.rev)),
            ConsentRevision::from_u64(stored.rev.as_u64() + 1),
        ),
    };
    let saved = save_consent(
        store,
        expected,
        ConsentRecord {
            capability,
            id: format!("usage-query-{}-consent", capability.as_str()),
            rev,
            provider: provider.to_owned(),
            model: model.to_owned(),
            credential_id: String::from("openai:main"),
        },
    )
    .await;
    assert!(matches!(saved, ConsentCommitOutcome::Committed { .. }));
    let stored = store
        .load_current(capability)
        .await
        .expect("the consent read must answer")
        .expect("the committed consent must be current");
    (stored.id, stored.rev)
}

struct ClaimSpec<'a> {
    capability: CapabilityKind,
    consumer: ConsumerKind,
    purpose: PurposeKind,
    provider: &'a str,
    model: &'a str,
    pricing: Option<PricingSnapshot>,
    estimate: Option<UsageEstimate>,
    data_use: Vec<RawId>,
    task_agent: Option<TaskAgentAttemptPremise>,
}

async fn claim(store: &Store, spec: ClaimSpec<'_>) -> InferenceTicketId {
    let premise = seed_consent(store, spec.capability, spec.provider, spec.model).await;
    let ticket = InferenceTicketId(RawId::new());
    let outcome = store
        .begin_inference_attempt(InferenceAttempt {
            ticket,
            consumer: spec.consumer,
            capability: spec.capability,
            purpose: spec.purpose,
            expected_consent: premise,
            expected_credential_set: CredentialSetRevision::initial(),
            provider: spec.provider.to_owned(),
            model: spec.model.to_owned(),
            task_agent: spec.task_agent,
            data_use: spec.data_use.clone(),
            pricing: spec.pricing,
            usage_estimate: spec.estimate,
        })
        .await
        .expect("the claim must answer");
    assert_eq!(
        outcome,
        AttemptBeginOutcome::Started,
        "the claim must start"
    );
    ticket
}

async fn claim_dialogue(store: &Store, spec_pricing: Option<PricingSnapshot>) -> InferenceTicketId {
    claim(
        store,
        ClaimSpec {
            capability: CapabilityKind::Dialogue,
            consumer: ConsumerKind::CompanionDialogue,
            purpose: PurposeKind::DialogueResponse,
            provider: PROVIDER,
            model: MODEL,
            pricing: spec_pricing,
            estimate: None,
            data_use: Vec::new(),
            task_agent: None,
        },
    )
    .await
}

async fn claim_learning(store: &Store) -> InferenceTicketId {
    claim(
        store,
        ClaimSpec {
            capability: CapabilityKind::Learning,
            consumer: ConsumerKind::CompanionLearning,
            purpose: PurposeKind::MemoryFormation,
            provider: PROVIDER,
            model: MODEL,
            pricing: None,
            estimate: None,
            // A Learning formation always names at least the messages its
            // prompt read; an empty correlation is refused at the claim.
            data_use: vec![RawId::new()],
            task_agent: None,
        },
    )
    .await
}

async fn claim_task_agent(store: &Store) -> InferenceTicketId {
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
    let data_use = vec![RawId::new()];
    claim(
        store,
        ClaimSpec {
            capability: CapabilityKind::Dialogue,
            consumer: ConsumerKind::TaskAgent,
            purpose: PurposeKind::TaskAgentTurn,
            provider: PROVIDER,
            model: MODEL,
            pricing: None,
            estimate: None,
            data_use: data_use.clone(),
            task_agent: Some(TaskAgentAttemptPremise {
                delegation: delegation.as_raw(),
                task: created.task.as_raw(),
                task_revision: RevisionInner::from_u64(created.revision.as_u64()),
                data_use,
            }),
        },
    )
    .await
}

fn reported_fact(ticket: InferenceTicketId, input: u64, cached: u64, output: u64) -> UsageFact {
    UsageFact {
        ticket,
        provider: PROVIDER.to_owned(),
        model: MODEL.to_owned(),
        input_tokens: Some(input),
        cached_input_tokens: Some(cached),
        output_tokens: Some(output),
        source: UsageSource::Reported,
    }
}

async fn query(
    store: &Store,
    from: &str,
    to: &str,
    limit: u32,
) -> Vec<ene_inference::UsageSummaryRow> {
    store
        .query_usage_summary(UsageSummaryQuery {
            from: at(from),
            to: at(to),
            provider: None,
            model: None,
            consumer: None,
            purpose: None,
            status: None,
            after: None,
            limit,
        })
        .await
        .expect("the usage summary read must answer")
}

async fn set_system_cap(store: &Store, window: UsageCapWindow, limit_micros: u64) -> UsageCapRef {
    let outcome = store
        .set_usage_cap(SetUsageCapCommand {
            expected: None,
            scope: UsageCapScope::System,
            window,
            limit: Money::from_micros(CurrencyCode::Usd, limit_micros),
        })
        .await
        .expect("the cap write must answer");
    match outcome {
        SetUsageCapOutcome::StoredAs(reference) => reference,
        other => panic!("the cap must store, got {other:?}"),
    }
}

fn attempt_count(store: &Store) -> i64 {
    crate::codec::lock_shared(&store.conn)
        .query_row("SELECT count(*) FROM usage_fact", [], |row| row.get(0))
        .unwrap()
}

/// One stored reservation row as `(state, upper_bound_micros, committed_micros)`.
fn reservation_row(store: &Store, ticket: InferenceTicketId) -> Option<(String, i64, Option<i64>)> {
    crate::codec::lock_shared(&store.conn)
        .query_row(
            "SELECT state, upper_bound_micros, committed_micros FROM usage_reservation WHERE ticket = ?1",
            [crate::codec::encode_id(ticket.0)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .unwrap()
}

fn cap_revision(store: &Store) -> i64 {
    crate::codec::lock_shared(&store.conn)
        .query_row(
            "SELECT revision FROM usage_cap WHERE scope = 'system' AND provider = '' AND window = 'daily_utc'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

/// Rewrites one claimed attempt's admission instant. The read filters and
/// orders on this column, so a test can place rows deterministically without
/// sleeping.
fn set_started_at(store: &Store, ticket: InferenceTicketId, text: &str) {
    let changed = crate::codec::lock_shared(&store.conn)
        .execute(
            "UPDATE inference_attempt SET started_at = ?2 WHERE ticket = ?1",
            params![crate::codec::encode_id(ticket.0), text],
        )
        .unwrap();
    assert_eq!(changed, 1);
}

#[tokio::test]
async fn summary_attributes_dialogue_learning_and_task_agent() {
    let store = open_memory().await.unwrap();
    let route_pricing = pricing(1);
    let dialogue = claim_dialogue(&store, Some(route_pricing.clone())).await;
    let learning = claim_learning(&store).await;
    let task_agent = claim_task_agent(&store).await;
    let dialogue_cost = store
        .record_usage(reported_fact(dialogue, 1_000, 200, 500))
        .await;
    assert_eq!(dialogue_cost, Ok(()));
    assert_eq!(
        store.record_usage(reported_fact(learning, 10, 0, 2)).await,
        Ok(())
    );
    assert_eq!(
        store
            .record_usage(reported_fact(task_agent, 20, 5, 7))
            .await,
        Ok(())
    );

    let rows = query(&store, "2026-01-01T00:00:00Z", "2027-01-01T00:00:00Z", 50).await;
    assert_eq!(rows.len(), 3, "all three consumers are attributed");
    let by_ticket = |ticket: InferenceTicketId| {
        rows.iter()
            .find(|row| row.ticket == ticket)
            .expect("the ticket must be listed")
    };
    let dialogue_row = by_ticket(dialogue);
    assert_eq!(dialogue_row.provider, PROVIDER);
    assert_eq!(dialogue_row.model, MODEL);
    assert_eq!(dialogue_row.consumer, ConsumerKind::CompanionDialogue);
    assert_eq!(dialogue_row.purpose, PurposeKind::DialogueResponse);
    assert_eq!(dialogue_row.status, UsageSummaryStatus::Reported);
    assert_eq!(
        dialogue_row.tokens,
        Some(ReportedTokenUsage {
            input_tokens: 1_000,
            cached_input_tokens: 200,
            output_tokens: 500,
        })
    );
    // non_cached 800 at 1 micro + cached 200 at 0.1 + output 500 at 2.
    let Some(UsageCostFact::Reported(cost)) = &dialogue_row.cost else {
        panic!("the reported row must project its cost");
    };
    assert_eq!(cost.input, Money::from_micros(CurrencyCode::Usd, 800));
    assert_eq!(cost.cached_input, Money::from_micros(CurrencyCode::Usd, 20));
    assert_eq!(cost.output, Money::from_micros(CurrencyCode::Usd, 1_000));
    assert_eq!(cost.total, Money::from_micros(CurrencyCode::Usd, 1_820));
    let learning_row = by_ticket(learning);
    assert_eq!(learning_row.consumer, ConsumerKind::CompanionLearning);
    assert_eq!(learning_row.purpose, PurposeKind::MemoryFormation);
    let task_row = by_ticket(task_agent);
    assert_eq!(task_row.consumer, ConsumerKind::TaskAgent);
    assert_eq!(task_row.purpose, PurposeKind::TaskAgentTurn);
    assert_eq!(
        task_row.tokens,
        Some(ReportedTokenUsage {
            input_tokens: 20,
            cached_input_tokens: 5,
            output_tokens: 7,
        })
    );

    // The consumer filter narrows to exactly one attribution group, and the
    // provider/model filters narrow the same rows.
    let filtered = store
        .query_usage_summary(UsageSummaryQuery {
            from: at("2026-01-01T00:00:00Z"),
            to: at("2027-01-01T00:00:00Z"),
            provider: Some(PROVIDER.to_owned()),
            model: Some(MODEL.to_owned()),
            consumer: Some(ConsumerKind::TaskAgent),
            purpose: Some(PurposeKind::TaskAgentTurn),
            status: Some(UsageSummaryStatus::Reported),
            after: None,
            limit: 50,
        })
        .await
        .unwrap();
    assert_eq!(
        filtered.iter().map(|row| row.ticket).collect::<Vec<_>>(),
        vec![task_agent]
    );
}

#[tokio::test]
async fn reported_unknown_and_reserved_are_distinct_states() {
    let store = open_memory().await.unwrap();
    set_system_cap(&store, UsageCapWindow::DailyUtc, 1_000_000).await;
    let estimate = UsageEstimate {
        input_tokens_upper_bound: 100,
        output_tokens_upper_bound: 50,
    };
    let reserved = claim(
        &store,
        ClaimSpec {
            capability: CapabilityKind::Dialogue,
            consumer: ConsumerKind::CompanionDialogue,
            purpose: PurposeKind::DialogueResponse,
            provider: PROVIDER,
            model: MODEL,
            pricing: Some(pricing(1)),
            estimate: Some(estimate),
            data_use: Vec::new(),
            task_agent: None,
        },
    )
    .await;
    let unknown = claim(
        &store,
        ClaimSpec {
            capability: CapabilityKind::Dialogue,
            consumer: ConsumerKind::CompanionDialogue,
            purpose: PurposeKind::DialogueResponse,
            provider: PROVIDER,
            model: MODEL,
            pricing: Some(pricing(1)),
            estimate: Some(estimate),
            data_use: Vec::new(),
            task_agent: None,
        },
    )
    .await;
    let reported = claim(
        &store,
        ClaimSpec {
            capability: CapabilityKind::Dialogue,
            consumer: ConsumerKind::CompanionDialogue,
            purpose: PurposeKind::DialogueResponse,
            provider: PROVIDER,
            model: MODEL,
            pricing: Some(pricing(1)),
            estimate: Some(estimate),
            data_use: Vec::new(),
            task_agent: None,
        },
    )
    .await;
    assert_eq!(
        store
            .record_usage(UsageFact {
                ticket: unknown,
                provider: PROVIDER.to_owned(),
                model: MODEL.to_owned(),
                input_tokens: None,
                cached_input_tokens: None,
                output_tokens: None,
                source: UsageSource::Unknown,
            })
            .await,
        Ok(())
    );
    assert_eq!(
        store
            .record_usage(reported_fact(reported, 100, 0, 50))
            .await,
        Ok(())
    );

    let rows = query(&store, "2026-01-01T00:00:00Z", "2027-01-01T00:00:00Z", 50).await;
    assert_eq!(rows.len(), 3);
    let status_of = |ticket: InferenceTicketId| {
        rows.iter()
            .find(|row| row.ticket == ticket)
            .expect("the ticket must be listed")
            .status
    };
    assert_eq!(status_of(reserved), UsageSummaryStatus::Reserved);
    assert_eq!(status_of(unknown), UsageSummaryStatus::Unknown);
    assert_eq!(status_of(reported), UsageSummaryStatus::Reported);
    let reserved_row = rows.iter().find(|row| row.ticket == reserved).unwrap();
    assert_eq!(reserved_row.tokens, None, "no token count is invented");
    assert_eq!(reserved_row.cost, None, "no cost is invented");
    assert_eq!(
        reserved_row.reserved,
        Some(Money::from_micros(CurrencyCode::Usd, 201)),
        "the reserved upper bound stays visible while unsettled"
    );
    let unknown_row = rows.iter().find(|row| row.ticket == unknown).unwrap();
    assert_eq!(unknown_row.tokens, None);
    assert_eq!(
        unknown_row.cost,
        Some(UsageCostFact::Unknown {
            pricing: Some(pricing(1).reference())
        })
    );
    assert_eq!(
        unknown_row.reserved,
        Some(Money::from_micros(CurrencyCode::Usd, 201)),
        "unknown keeps the upper bound counted against the cap"
    );
    let reported_row = rows.iter().find(|row| row.ticket == reported).unwrap();
    assert_eq!(
        reported_row.reserved, None,
        "reported counts its actual cost"
    );

    for (status, expected) in [
        (UsageSummaryStatus::Reported, reported),
        (UsageSummaryStatus::Unknown, unknown),
        (UsageSummaryStatus::Reserved, reserved),
    ] {
        let filtered = store
            .query_usage_summary(UsageSummaryQuery {
                from: at("2026-01-01T00:00:00Z"),
                to: at("2027-01-01T00:00:00Z"),
                provider: None,
                model: None,
                consumer: None,
                purpose: None,
                status: Some(status),
                after: None,
                limit: 50,
            })
            .await
            .unwrap();
        assert_eq!(
            filtered.iter().map(|row| row.ticket).collect::<Vec<_>>(),
            vec![expected],
            "the {:?} filter names exactly its state",
            status
        );
    }
}

#[tokio::test]
async fn bounded_pages_walk_every_row_without_loss_or_duplication() {
    let store = open_memory().await.unwrap();
    let mut tickets = Vec::new();
    for index in 0..60_u64 {
        let ticket = claim_dialogue(&store, None).await;
        assert_eq!(
            store
                .record_usage(reported_fact(ticket, index + 1, 0, 1))
                .await,
            Ok(())
        );
        tickets.push(ticket);
    }
    // One identical instant for every row: only the ticket tiebreak keeps the
    // keyset walk lossless.
    let instant = at("2026-09-01T00:00:00Z").to_rfc3339_utc();
    for ticket in &tickets {
        set_started_at(&store, *ticket, &instant);
    }

    let mut seen = Vec::new();
    let mut after: Option<UsageSummaryCursor> = None;
    loop {
        let page = store
            .query_usage_summary(UsageSummaryQuery {
                from: at("2026-08-01T00:00:00Z"),
                to: at("2026-10-01T00:00:00Z"),
                provider: None,
                model: None,
                consumer: None,
                purpose: None,
                status: None,
                after,
                limit: 10,
            })
            .await
            .unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 10, "the SQL bound holds");
        for window in page.windows(2) {
            assert!(
                (
                    window[0].started_at.as_datetime(),
                    window[0].ticket.0.as_uuid()
                ) >= (
                    window[1].started_at.as_datetime(),
                    window[1].ticket.0.as_uuid()
                ),
                "rows are newest first with the ticket tiebreak"
            );
        }
        let last = page.last().unwrap();
        after = Some(UsageSummaryCursor {
            started_at: last.started_at,
            ticket: last.ticket,
        });
        seen.extend(page.into_iter().map(|row| row.ticket));
    }
    assert_eq!(seen.len(), 60, "every row is walked exactly once");
    let mut unique = seen.clone();
    unique.sort_by_key(|ticket| ticket.0.as_uuid());
    unique.dedup();
    assert_eq!(unique.len(), 60, "no row is duplicated across pages");
    assert_eq!(
        unique.len(),
        tickets.len(),
        "the walk covers every claimed ticket"
    );
    for ticket in &tickets {
        assert!(unique.contains(ticket), "the walk loses no row");
    }

    // The owner clamp bounds the SQL read even for an oversized request.
    let clamped = query(
        &store,
        "2026-08-01T00:00:00Z",
        "2026-10-01T00:00:00Z",
        10_000,
    )
    .await;
    assert_eq!(clamped.len(), USAGE_SUMMARY_PAGE_MAX as usize);

    // An empty or inverted range is an empty page, not an unbounded read.
    let inverted = query(&store, "2026-10-01T00:00:00Z", "2026-08-01T00:00:00Z", 10).await;
    assert!(inverted.is_empty());
    // The effective lower bound is clamped to the maximum span.
    let clamped_range = store
        .query_usage_summary(UsageSummaryQuery {
            from: at("2000-01-01T00:00:00Z"),
            to: at("2026-12-01T00:00:00Z"),
            provider: None,
            model: None,
            consumer: None,
            purpose: None,
            status: None,
            after: None,
            limit: 5,
        })
        .await
        .unwrap();
    assert_eq!(
        clamped_range.len(),
        5,
        "the range clamp keeps the most recent span readable in one page"
    );
}

#[tokio::test]
async fn read_settles_nothing_and_mutates_no_cap_or_pricing() {
    let store = open_memory().await.unwrap();
    set_system_cap(&store, UsageCapWindow::DailyUtc, 1_000_000).await;
    let ticket = claim(
        &store,
        ClaimSpec {
            capability: CapabilityKind::Dialogue,
            consumer: ConsumerKind::CompanionDialogue,
            purpose: PurposeKind::DialogueResponse,
            provider: PROVIDER,
            model: MODEL,
            pricing: Some(pricing(1)),
            estimate: Some(UsageEstimate {
                input_tokens_upper_bound: 100,
                output_tokens_upper_bound: 50,
            }),
            data_use: Vec::new(),
            task_agent: None,
        },
    )
    .await;
    let revision_before = cap_revision(&store);
    let facts_before = attempt_count(&store);
    let reservation_before = reservation_row(&store, ticket);
    assert_eq!(
        reservation_before,
        Some((
            String::from("reserved"),
            crate::codec::encode_u64(201).unwrap(),
            None
        )),
        "the fixture leaves the reservation non-terminal"
    );

    // Two reads (the second paging past the head) change nothing.
    let first = query(&store, "2026-01-01T00:00:00Z", "2027-01-01T00:00:00Z", 1).await;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].status, UsageSummaryStatus::Reserved);
    let after = UsageSummaryCursor {
        started_at: first[0].started_at,
        ticket: first[0].ticket,
    };
    store
        .query_usage_summary(UsageSummaryQuery {
            from: at("2026-01-01T00:00:00Z"),
            to: at("2027-01-01T00:00:00Z"),
            provider: None,
            model: None,
            consumer: None,
            purpose: None,
            status: None,
            after: Some(after),
            limit: 1,
        })
        .await
        .unwrap();

    assert_eq!(
        cap_revision(&store),
        revision_before,
        "the read does not bump a cap revision"
    );
    assert_eq!(
        attempt_count(&store),
        facts_before,
        "the read settles no usage fact"
    );
    assert_eq!(
        reservation_row(&store, ticket),
        reservation_before,
        "the read leaves the non-terminal reservation exactly as it was"
    );
}

#[tokio::test]
async fn cap_status_breaks_down_consumption_and_reflects_admission() {
    let store = open_memory().await.unwrap();
    set_system_cap(&store, UsageCapWindow::DailyUtc, 1_000).await;
    let estimate = UsageEstimate {
        input_tokens_upper_bound: 100,
        output_tokens_upper_bound: 50,
    };
    let mut claims = Vec::new();
    for _ in 0..3 {
        claims.push(
            claim(
                &store,
                ClaimSpec {
                    capability: CapabilityKind::Dialogue,
                    consumer: ConsumerKind::CompanionDialogue,
                    purpose: PurposeKind::DialogueResponse,
                    provider: PROVIDER,
                    model: MODEL,
                    pricing: Some(pricing(1)),
                    estimate: Some(estimate),
                    data_use: Vec::new(),
                    task_agent: None,
                },
            )
            .await,
        );
    }
    // One reported settlement (actual cost 100: 100 input at 1 micro),
    // one unknown settlement (keeps its 200 upper bound), one still reserved.
    assert_eq!(
        store
            .record_usage(reported_fact(claims[0], 100, 0, 0))
            .await,
        Ok(())
    );
    assert_eq!(
        store
            .record_usage(UsageFact {
                ticket: claims[1],
                provider: PROVIDER.to_owned(),
                model: MODEL.to_owned(),
                input_tokens: None,
                cached_input_tokens: None,
                output_tokens: None,
                source: UsageSource::Unknown,
            })
            .await,
        Ok(())
    );
    let status = store
        .load_usage_cap_status(UsageCapStatusQuery {
            provider: None,
            at: WallClockWithTz::now(),
        })
        .await
        .expect("the cap status read must answer");
    assert_eq!(status.len(), 1, "only the stored system daily cap exists");
    let cap = &status[0];
    assert_eq!(cap.cap.scope(), &UsageCapScope::System);
    assert_eq!(cap.cap.window(), UsageCapWindow::DailyUtc);
    assert_eq!(
        cap.cap.limit(),
        Money::from_micros(CurrencyCode::Usd, 1_000)
    );
    let UsageCapConsumption::Known {
        reserved,
        committed_reported,
        committed_unknown,
        consumed,
        remaining,
        held,
    } = &cap.consumption
    else {
        panic!("the fixture consumption is comparable");
    };
    assert_eq!(*reserved, Money::from_micros(CurrencyCode::Usd, 201));
    assert_eq!(
        *committed_reported,
        Money::from_micros(CurrencyCode::Usd, 100)
    );
    assert_eq!(
        *committed_unknown,
        Money::from_micros(CurrencyCode::Usd, 201)
    );
    assert_eq!(*consumed, Money::from_micros(CurrencyCode::Usd, 502));
    assert_eq!(*remaining, Money::from_micros(CurrencyCode::Usd, 498));
    assert!(!held, "502 of 1000 is not held");

    // The status and the admission agree: under this consumption a new
    // 201-micro reservation fits (703 <= 1000) ...
    let fitting = claim(
        &store,
        ClaimSpec {
            capability: CapabilityKind::Dialogue,
            consumer: ConsumerKind::CompanionDialogue,
            purpose: PurposeKind::DialogueResponse,
            provider: PROVIDER,
            model: MODEL,
            pricing: Some(pricing(1)),
            estimate: Some(estimate),
            data_use: Vec::new(),
            task_agent: None,
        },
    )
    .await;
    assert_ne!(
        fitting, claims[0],
        "a distinct ticket claims under the headroom"
    );
    // ... and once the cap is lowered below the current consumption, the
    // status is held with zero remaining and admission refuses.
    let current = store
        .load_usage_cap_status(UsageCapStatusQuery {
            provider: None,
            at: WallClockWithTz::now(),
        })
        .await
        .unwrap()
        .remove(0);
    let lowered = store
        .set_usage_cap(SetUsageCapCommand {
            expected: Some(current.cap.reference()),
            scope: UsageCapScope::System,
            window: UsageCapWindow::DailyUtc,
            limit: Money::from_micros(CurrencyCode::Usd, 600),
        })
        .await
        .unwrap();
    assert!(matches!(lowered, SetUsageCapOutcome::StoredAs(_)));
    let held_status = store
        .load_usage_cap_status(UsageCapStatusQuery {
            provider: None,
            at: WallClockWithTz::now(),
        })
        .await
        .unwrap()
        .remove(0);
    let UsageCapConsumption::Known {
        consumed,
        remaining,
        held,
        ..
    } = &held_status.consumption
    else {
        panic!("the fixture consumption is comparable");
    };
    assert_eq!(*consumed, Money::from_micros(CurrencyCode::Usd, 703));
    assert_eq!(*remaining, Money::zero(CurrencyCode::Usd));
    assert!(held, "consumption above the lowered limit is held");
    let refused = store
        .begin_inference_attempt(InferenceAttempt {
            ticket: InferenceTicketId(RawId::new()),
            consumer: ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            purpose: PurposeKind::DialogueResponse,
            expected_consent: store
                .load_current(CapabilityKind::Dialogue)
                .await
                .unwrap()
                .map(|record| (record.id, record.rev))
                .expect("the dialogue consent is current"),
            expected_credential_set: CredentialSetRevision::initial(),
            provider: PROVIDER.to_owned(),
            model: MODEL.to_owned(),
            data_use: Vec::new(),
            task_agent: None,
            pricing: Some(pricing(1)),
            usage_estimate: Some(estimate),
        })
        .await
        .unwrap();
    assert!(
        matches!(refused, AttemptBeginOutcome::HeldByCap(_)),
        "the held status matches the admission, got {refused:?}"
    );
}

#[tokio::test]
async fn cap_status_filters_system_and_one_provider() {
    let store = open_memory().await.unwrap();
    set_system_cap(&store, UsageCapWindow::MonthlyUtc, 5_000).await;
    let provider_cap = store
        .set_usage_cap(SetUsageCapCommand {
            expected: None,
            scope: UsageCapScope::Provider(String::from(PROVIDER)),
            window: UsageCapWindow::DailyUtc,
            limit: Money::from_micros(CurrencyCode::Usd, 300),
        })
        .await
        .unwrap();
    assert!(matches!(provider_cap, SetUsageCapOutcome::StoredAs(_)));
    let other = store
        .set_usage_cap(SetUsageCapCommand {
            expected: None,
            scope: UsageCapScope::Provider(String::from("other-provider")),
            window: UsageCapWindow::DailyUtc,
            limit: Money::from_micros(CurrencyCode::Usd, 900),
        })
        .await
        .unwrap();
    assert!(matches!(other, SetUsageCapOutcome::StoredAs(_)));

    let all = store
        .load_usage_cap_status(UsageCapStatusQuery {
            provider: None,
            at: WallClockWithTz::now(),
        })
        .await
        .unwrap();
    assert_eq!(
        all.len(),
        3,
        "every stored cap is reported without a filter"
    );
    let filtered = store
        .load_usage_cap_status(UsageCapStatusQuery {
            provider: Some(String::from(PROVIDER)),
            at: WallClockWithTz::now(),
        })
        .await
        .unwrap();
    assert_eq!(filtered.len(), 2, "system plus the named provider only");
    assert!(filtered.iter().any(|status| {
        status.cap.scope() == &UsageCapScope::Provider(String::from(PROVIDER))
            && status.cap.window() == UsageCapWindow::DailyUtc
    }));
    assert!(filtered.iter().any(|status| {
        status.cap.scope() == &UsageCapScope::System
            && status.cap.window() == UsageCapWindow::MonthlyUtc
    }));
    assert!(
        !filtered
            .iter()
            .any(|status| status.cap.scope()
                == &UsageCapScope::Provider(String::from("other-provider"))),
        "another provider's cap is not mixed into the filtered read"
    );
}

#[tokio::test]
async fn usage_summary_plan_uses_the_started_index_and_limit() {
    let store = open_memory().await.unwrap();
    let plan: Vec<String> = crate::codec::lock_shared(&store.conn)
        .prepare(&format!(
            "EXPLAIN QUERY PLAN {}",
            crate::inference::SQL_SELECT_USAGE_SUMMARY
        ))
        .unwrap()
        .query_map(
            params![
                at("2026-01-01T00:00:00Z").to_rfc3339_utc(),
                at("2027-01-01T00:00:00Z").to_rfc3339_utc(),
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                Option::<String>::None,
                crate::codec::encode_u64(10).unwrap()
            ],
            |row| row.get::<_, String>(3),
        )
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let joined = plan.join("\n");
    assert!(
        joined.contains("idx_inference_attempt_started"),
        "the keyset page reads the started index, plan: {joined}"
    );
    assert!(
        !joined.contains("SCAN a"),
        "no full scan of the attempt table, plan: {joined}"
    );
}
