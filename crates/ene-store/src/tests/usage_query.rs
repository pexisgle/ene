use ene_inference::cost::{CurrencyCode, TokenRate, UsageCostFact, UsageEstimate};
use ene_inference::pricing::{PricingCatalogRevision, PricingSnapshot};
use ene_inference::{
    ReportedTokenUsage, UsageSummaryQuery, UsageSummaryRepository as _, UsageSummaryStatus,
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
    // one unknown settlement (keeps its 201 upper bound), one still reserved.
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
    let _ = claim(
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
        matches!(refused, AttemptBeginOutcome::HeldByCap),
        "the held status matches the admission, got {refused:?}"
    );
}
