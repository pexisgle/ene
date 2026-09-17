//! Stage 6 B4 host-level tests: the first-party usage / cost / cap surface
//! (`usage-cost-cap` §16/§17).
//!
//! The reads run over the real wire frames and the real domain gate; the
//! fixtures seed usage through the production claim/settlement path, so the
//! read observes exactly what the pipeline writes. Cap mutation goes through
//! the management intent inlet and the permission-owned command.

use super::*;

use crate::serve::WireFrame;
use crate::test_support::memory_handle_with;

use ene_api::v1::refs::{UsageCursorWire, ViewMarkWire};
use ene_api::v1::usage::{
    UsageCapConsumptionView, UsageSummaryPage, UsageSummaryRequest, UsageSummaryResponse,
};
use ene_inference::cost::UsageEstimate;
use ene_inference::pricing::PricingSnapshot;
use ene_inference::{
    InferenceTicketId, TaskAgentAttemptPremise, UsageFact, UsageRepository as _, UsageSource,
};
use ene_permission::{CapabilityKind, ConsentRevision, ConsumerKind, PurposeKind};
use ene_primitive::{CurrencyCode, Money, RawId, RevisionInner, WallClockWithTz};
use ene_task::TaskRepository as _;

/// One usage summary request frame stamped with its connection binding.
fn usage_frame(live: &LiveInput, request: UsageSummaryRequest) -> WireFrame {
    stamped(
        WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("UsageSummaryRequest")),
            ),
            payload: WirePayload::UsageSummaryRequest(request),
        },
        live,
    )
}

/// A filter-less request: the Host applies the period default and the page
/// default.
fn usage_request() -> UsageSummaryRequest {
    UsageSummaryRequest {
        from: None,
        to: None,
        provider: None,
        model: None,
        consumer: None,
        purpose: None,
        status: None,
        cursor: None,
        limit: None,
    }
}

async fn read_usage(
    handle: &HostHandle,
    live: &LiveInput,
    transport: &FakeProviderTransport,
    request: UsageSummaryRequest,
) -> Vec<WireFrame> {
    handle
        .handle_frame(usage_frame(live, request), live.clone(), transport)
        .await
}

fn usage_page_of(responses: &[WireFrame]) -> UsageSummaryPage {
    let Some(WirePayload::UsageSummaryResponse(UsageSummaryResponse::Page(page))) =
        responses.first().map(|frame| &frame.payload)
    else {
        panic!("the usage read must answer a page, got {responses:?}");
    };
    page.clone()
}

fn expect_reject(responses: &[WireFrame], expected: RejectKind) {
    assert!(
        responses.first().is_some_and(|frame| matches!(
            &frame.payload,
            WirePayload::Reject(notice) if notice.kind == expected
        )),
        "expected a {expected:?} rejection, got {responses:?}"
    );
}

/// Seeds (or advances) one capability's consent through the production
/// intent-atomic path and returns the current premise.
async fn usage_consent(
    store: &ene_store::Store,
    capability: CapabilityKind,
    provider: &str,
    model: &str,
) -> (String, ConsentRevision) {
    use ene_permission::{
        ConsentCommitOutcome, ConsentRecord, ConsentRepository as _, IntentFingerprint,
        IntentOutcomeRepository as _, IntentResolution,
    };
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
    let committed = store
        .assign_with_intent(
            expected,
            ConsentRecord {
                capability,
                id: format!("usage-surface-{}-consent", capability.as_str()),
                rev,
                provider: provider.to_owned(),
                model: model.to_owned(),
                credential_id: String::from("openai:main"),
            },
            IntentFingerprint {
                intent_id: RawId::new().as_uuid().to_string(),
                kind: String::from("assign"),
                target: String::from("consent:usage-surface"),
                base: String::from("consent-none"),
                rationale_origin: String::from("management-surface"),
                rationale_quote: None,
            },
        )
        .await
        .expect("the consent write must answer");
    assert!(
        matches!(
            committed,
            IntentResolution::Decided(ConsentCommitOutcome::Committed { .. })
        ),
        "the usage-surface consent must commit, got {committed:?}"
    );
    let stored = store
        .load_current(capability)
        .await
        .expect("the consent read must answer")
        .expect("the committed consent must be current");
    (stored.id, stored.rev)
}

/// The route attribution of one claimed attempt.
struct UsageRoute<'a> {
    capability: CapabilityKind,
    consumer: ConsumerKind,
    purpose: PurposeKind,
    provider: &'a str,
    model: &'a str,
}

fn dialogue_route<'a>(provider: &'a str, model: &'a str) -> UsageRoute<'a> {
    UsageRoute {
        capability: CapabilityKind::Dialogue,
        consumer: ConsumerKind::CompanionDialogue,
        purpose: PurposeKind::DialogueResponse,
        provider,
        model,
    }
}

fn learning_route<'a>(provider: &'a str, model: &'a str) -> UsageRoute<'a> {
    UsageRoute {
        capability: CapabilityKind::Learning,
        consumer: ConsumerKind::CompanionLearning,
        purpose: PurposeKind::MemoryFormation,
        provider,
        model,
    }
}

fn task_agent_route<'a>(provider: &'a str, model: &'a str) -> UsageRoute<'a> {
    UsageRoute {
        capability: CapabilityKind::Dialogue,
        consumer: ConsumerKind::TaskAgent,
        purpose: PurposeKind::TaskAgentTurn,
        provider,
        model,
    }
}

/// Claims one attempt through the production admission path.
async fn claim_usage(
    store: &ene_store::Store,
    route: UsageRoute<'_>,
    pricing: Option<PricingSnapshot>,
    estimate: Option<UsageEstimate>,
    task_agent: Option<TaskAgentAttemptPremise>,
) -> InferenceTicketId {
    use ene_credential::CredentialSetRevision;
    use ene_inference::{AttemptBeginOutcome, InferenceAttempt, InferenceAttemptRepository as _};

    let UsageRoute {
        capability,
        consumer,
        purpose,
        provider,
        model,
    } = route;
    let expected_consent = usage_consent(store, capability, provider, model).await;
    let ticket = InferenceTicketId(RawId::new());
    let outcome = store
        .begin_inference_attempt(InferenceAttempt {
            ticket,
            consumer,
            capability,
            purpose,
            expected_consent,
            expected_credential_set: CredentialSetRevision::initial(),
            provider: provider.to_owned(),
            model: model.to_owned(),
            task_agent,
            pricing,
            usage_estimate: estimate,
        })
        .await
        .expect("the claim must answer");
    assert_eq!(
        outcome,
        AttemptBeginOutcome::Started,
        "the usage-surface claim must start"
    );
    ticket
}

/// 1 micro per input token, 0.1 per cached input, 2 per output.
fn usage_pricing(provider: &str, model: &str) -> PricingSnapshot {
    use ene_inference::cost::TokenRate;
    use ene_inference::pricing::PricingCatalogRevision;
    PricingSnapshot {
        provider: provider.to_owned(),
        model: model.to_owned(),
        currency: CurrencyCode::Usd,
        input_rate: TokenRate::from_micros_per_million(1_000_000),
        cached_input_rate: TokenRate::from_micros_per_million(100_000),
        output_rate: TokenRate::from_micros_per_million(2_000_000),
        effective_at: WallClockWithTz::parse_rfc3339("2025-06-01T00:00:00Z")
            .expect("the fixture instant parses"),
        source_revision: PricingCatalogRevision::new(1),
    }
}

/// Seeds one Task plus its delegation for the Task Agent attribution.
async fn seed_delegation(store: &ene_store::Store) -> (ene_task::TaskRef, ene_task::DelegationId) {
    use ene_task::{
        AssigneeRef, DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationScope,
        TaskAgentEphemeralId, TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind,
        TaskCreationPremise, TaskId, TaskPurpose,
    };
    let created = store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("usage surface task"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            acquired_at: WallClockWithTz::now(),
            assignee: AssigneeRef {
                companion: RawId::new(),
            },
            workspace: None,
        })
        .await
        .expect("the task must seed");
    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(DelegationCreationPremise {
            delegation,
            task: created,
            agent: TaskAgentEphemeralId::generate(),
            scope_copy: DelegationScope { workspace: None },
        })
        .await
        .expect("the delegation must seed");
    assert!(
        matches!(outcome, DelegationOutcome::Delegated(_)),
        "the seed delegation must commit, got {outcome:?}"
    );
    (created, delegation)
}

fn task_agent_premise(
    created: ene_task::TaskRef,
    delegation: ene_task::DelegationId,
) -> TaskAgentAttemptPremise {
    TaskAgentAttemptPremise {
        delegation: delegation.as_raw(),
        task: created.task.as_raw(),
        task_revision: RevisionInner::from_u64(created.revision.as_u64()),
        data_use: vec![RawId::new()],
    }
}

/// The whole B4 read surface over one wire: dialogue / learning / Task Agent
/// attribution, Reserved vs Unknown vs Reported states, cost components, and
/// the cap slots with their opaque marks. The read changes nothing durable.
#[tokio::test]
async fn usage_summary_reads_attribution_states_costs_and_caps() {
    use ene_permission::{
        SetUsageCapCommand, SetUsageCapOutcome, UsageCapRepository as _, UsageCapScope,
        UsageCapStatusQuery, UsageCapWindow, UsageReservationState,
    };

    let Some((handle, _dir)) = memory_handle("usage-surface-read").await else {
        panic!("the handle must open");
    };
    let store = handle.store.clone();
    let provider = "openai";
    let model = "surface-model";
    let pricing = usage_pricing(provider, model);
    let estimate = UsageEstimate {
        input_tokens_upper_bound: 100,
        output_tokens_upper_bound: 50,
    };
    assert!(matches!(
        store
            .set_usage_cap(SetUsageCapCommand {
                expected: None,
                scope: UsageCapScope::System,
                window: UsageCapWindow::DailyUtc,
                limit: Money::from_micros(CurrencyCode::Usd, 1_000),
            })
            .await
            .unwrap(),
        SetUsageCapOutcome::StoredAs(_)
    ));
    let dialogue = claim_usage(
        &store,
        dialogue_route(provider, model),
        Some(pricing.clone()),
        Some(estimate),
        None,
    )
    .await;
    let learning = claim_usage(
        &store,
        learning_route(provider, model),
        Some(pricing.clone()),
        Some(estimate),
        None,
    )
    .await;
    let (created, delegation) = seed_delegation(&store).await;
    let task_agent = claim_usage(
        &store,
        task_agent_route(provider, model),
        Some(pricing.clone()),
        Some(estimate),
        Some(task_agent_premise(created, delegation)),
    )
    .await;
    assert_eq!(
        store
            .record_usage(UsageFact {
                ticket: dialogue,
                provider: provider.to_owned(),
                model: model.to_owned(),
                input_tokens: Some(1_000),
                cached_input_tokens: Some(200),
                output_tokens: Some(500),
                source: UsageSource::Reported,
            })
            .await,
        Ok(())
    );
    assert_eq!(
        store
            .record_usage(UsageFact {
                ticket: learning,
                provider: provider.to_owned(),
                model: model.to_owned(),
                input_tokens: None,
                cached_input_tokens: None,
                output_tokens: None,
                source: UsageSource::Unknown,
            })
            .await,
        Ok(())
    );
    let reservation_before = store
        .load_usage_reservation(task_agent)
        .await
        .expect("the reservation read must answer")
        .expect("the task agent claim reserved under the cap");
    assert_eq!(reservation_before.state, UsageReservationState::Reserved);

    let live = paired_input("usage-surface-read");
    let transport = fake_transport();
    let responses = read_usage(&handle, &live, &transport, usage_request()).await;
    let page = usage_page_of(&responses);
    assert_eq!(page.rows.len(), 3, "all three consumers are attributed");
    let dialogue_row = page
        .rows
        .iter()
        .find(|row| row.consumer == "companion_dialogue")
        .expect("the dialogue row is attributed");
    assert_eq!(dialogue_row.purpose, "dialogue_response");
    assert_eq!(dialogue_row.provider, provider);
    assert_eq!(dialogue_row.model, model);
    assert_eq!(dialogue_row.status, "reported");
    let tokens = dialogue_row.tokens.expect("reported tokens travel");
    assert_eq!(tokens.input_tokens, 1_000);
    assert_eq!(tokens.cached_input_tokens, 200);
    assert_eq!(tokens.output_tokens, 500);
    let cost = dialogue_row
        .cost
        .clone()
        .expect("the cost components travel");
    assert_eq!(cost.input.micros, 800);
    assert_eq!(cost.cached_input.micros, 20);
    assert_eq!(cost.output.micros, 1_000);
    assert_eq!(cost.total.micros, 1_820);
    assert_eq!(cost.total.currency, "USD");
    let learning_row = page
        .rows
        .iter()
        .find(|row| row.consumer == "companion_learning")
        .expect("the learning row is attributed");
    assert_eq!(learning_row.status, "unknown");
    assert_eq!(learning_row.purpose, "memory_formation");
    assert!(learning_row.tokens.is_none());
    assert!(learning_row.cost.is_none(), "unknown cost is never zero");
    let task_row = page
        .rows
        .iter()
        .find(|row| row.consumer == "task_agent")
        .expect("the task agent row is attributed");
    assert_eq!(task_row.purpose, "task_agent_turn");
    assert_eq!(task_row.status, "reserved");
    assert!(task_row.tokens.is_none());
    assert!(task_row.cost.is_none());
    assert_eq!(
        task_row.reserved.as_ref().map(|money| money.micros),
        Some(201),
        "the reserved upper bound stays visible"
    );

    // The cap section always names the system slots (with their marks); the
    // stored system daily cap carries the durable breakdown.
    let system = page
        .caps
        .iter()
        .find(|cap| cap.scope == "system" && cap.window == "daily_utc")
        .expect("the system daily slot is always named");
    assert_eq!(system.mark, "usage-cap-system-daily_utc-rev-0");
    let stored = system.stored.as_ref().expect("the system cap is stored");
    assert_eq!(stored.limit.micros, 1_000);
    let UsageCapConsumptionView::Known {
        reserved,
        committed_reported,
        committed_unknown,
        consumed,
        remaining,
        held,
    } = &stored.consumption
    else {
        panic!("the fixture consumption is comparable");
    };
    assert_eq!(reserved.micros, 201, "the reserved upper bound is counted");
    assert_eq!(committed_reported.micros, 1_820);
    assert_eq!(committed_unknown.micros, 201);
    assert_eq!(consumed.micros, 2_222);
    assert_eq!(remaining.micros, 0);
    assert!(*held, "consumption above the limit is held");

    // The read changed nothing durable: the reserved ticket is still
    // reserved, no usage fact was settled, and no cap revision moved.
    let reservation_after = store
        .load_usage_reservation(task_agent)
        .await
        .expect("the reservation read must answer")
        .expect("the reservation stays durable");
    assert_eq!(reservation_after, reservation_before);
    assert_eq!(
        store
            .load_usage_cost(task_agent)
            .await
            .expect("the cost read must answer"),
        None,
        "the read settles no usage fact for the reserved ticket"
    );
    let status = store
        .load_usage_cap_status(UsageCapStatusQuery {
            provider: None,
            at: WallClockWithTz::now(),
        })
        .await
        .unwrap();
    assert_eq!(
        status[0].cap.revision().as_u64(),
        0,
        "the first stored revision is the owner's first revision"
    );
    assert!(page.evaluated_at.ends_with('Z'));
}

/// Limits, filters, and cursors are owner-bounded and typed: malformed
/// fields are `UnsupportedFieldValue`, a reused cursor under another premise
/// (or on another connection) is `StaleBaseView`, and a full page walks.
#[tokio::test]
async fn usage_summary_limits_filters_and_cursors_are_typed() {
    let Some((handle, _dir)) = memory_handle("usage-surface-bounds").await else {
        panic!("the handle must open");
    };
    let store = handle.store.clone();
    let provider = "openai";
    let model = "bounds-model";
    let pricing = usage_pricing(provider, model);
    // 51 rows: one more than the default page, so the default read must hand
    // out a cursor and the walk must not lose the 51st row.
    for index in 0..51_u64 {
        let ticket = claim_usage(
            &store,
            dialogue_route(provider, model),
            Some(pricing.clone()),
            None,
            None,
        )
        .await;
        assert_eq!(
            store
                .record_usage(UsageFact {
                    ticket,
                    provider: provider.to_owned(),
                    model: model.to_owned(),
                    input_tokens: Some(index + 1),
                    cached_input_tokens: Some(0),
                    output_tokens: Some(1),
                    source: UsageSource::Reported,
                })
                .await,
            Ok(())
        );
    }
    let live = paired_input("usage-surface-bounds");
    let transport = fake_transport();

    // Malformed fields are typed rejections, never empty pages.
    for request in [
        UsageSummaryRequest {
            limit: Some(0),
            ..usage_request()
        },
        UsageSummaryRequest {
            limit: Some(51),
            ..usage_request()
        },
        UsageSummaryRequest {
            consumer: Some(String::from("nobody")),
            ..usage_request()
        },
        UsageSummaryRequest {
            purpose: Some(String::from("nobody")),
            ..usage_request()
        },
        UsageSummaryRequest {
            status: Some(String::from("settled")),
            ..usage_request()
        },
        UsageSummaryRequest {
            from: Some(String::from("yesterday")),
            ..usage_request()
        },
        UsageSummaryRequest {
            to: Some(String::from("not-a-clock")),
            ..usage_request()
        },
    ] {
        let responses = read_usage(&handle, &live, &transport, request).await;
        expect_reject(&responses, RejectKind::UnsupportedFieldValue);
    }

    // The default page is 50 rows; the 51st arrives on the cursor page and
    // no row is lost or repeated across the walk.
    let first = usage_page_of(&read_usage(&handle, &live, &transport, usage_request()).await);
    assert_eq!(first.rows.len(), 50);
    let cursor = first
        .next_cursor
        .clone()
        .expect("a full page hands out a cursor");
    let second = usage_page_of(
        &read_usage(
            &handle,
            &live,
            &transport,
            UsageSummaryRequest {
                cursor: Some(cursor.clone()),
                ..usage_request()
            },
        )
        .await,
    );
    assert_eq!(second.rows.len(), 1);
    assert!(
        second.next_cursor.is_none(),
        "an unfinished page hands out no further cursor"
    );
    let mut walked: Vec<u64> = first
        .rows
        .iter()
        .chain(&second.rows)
        .map(|row| row.tokens.expect("reported").input_tokens)
        .collect();
    assert_eq!(walked.len(), 51, "the walk covers every row");
    walked.sort_unstable();
    walked.dedup();
    assert_eq!(walked.len(), 51, "the walk repeats no row");
    assert_eq!(walked.first(), Some(&1));
    assert_eq!(walked.last(), Some(&51));

    // A cursor reused under another filter premise is stale, never a
    // silently different result set.
    let stale = read_usage(
        &handle,
        &live,
        &transport,
        UsageSummaryRequest {
            consumer: Some(String::from("companion_learning")),
            cursor: Some(cursor.clone()),
            ..usage_request()
        },
    )
    .await;
    assert!(
        matches!(
            stale.first().map(|frame| &frame.payload),
            Some(WirePayload::UsageSummaryResponse(
                UsageSummaryResponse::StaleBaseView { .. }
            ))
        ),
        "a premise-mismatched cursor is stale, got {stale:?}"
    );
    // Another connection's cursor is equally unresolvable.
    let foreign = paired_input("usage-surface-bounds-other");
    let foreign_answer = read_usage(
        &handle,
        &foreign,
        &transport,
        UsageSummaryRequest {
            cursor: Some(UsageCursorWire(cursor.0.clone())),
            ..usage_request()
        },
    )
    .await;
    assert!(
        matches!(
            foreign_answer.first().map(|frame| &frame.payload),
            Some(WirePayload::UsageSummaryResponse(
                UsageSummaryResponse::StaleBaseView { .. }
            ))
        ),
        "another connection's cursor is stale, got {foreign_answer:?}"
    );

    // A filter with no matching rows is a genuine empty page.
    let empty = usage_page_of(
        &read_usage(
            &handle,
            &live,
            &transport,
            UsageSummaryRequest {
                consumer: Some(String::from("companion_learning")),
                ..usage_request()
            },
        )
        .await,
    );
    assert!(empty.rows.is_empty());
    // A provider filter names the provider's cap slots even with no cap
    // stored, so an intent can echo the none-state mark.
    let provider_slots = usage_page_of(
        &read_usage(
            &handle,
            &live,
            &transport,
            UsageSummaryRequest {
                provider: Some(String::from("openai")),
                ..usage_request()
            },
        )
        .await,
    );
    assert!(provider_slots.caps.iter().any(|cap| {
        cap.scope == "provider"
            && cap.provider.as_deref() == Some("openai")
            && cap.window == "monthly_utc"
            && cap.mark == "usage-cap-provider-openai-monthly_utc-none"
            && cap.stored.is_none()
    }));
}

/// Cap mutation goes through the permission-owned command: a fresh read's
/// mark applies once, a reused mark is a typed stale, a foreign mark never
/// creates, an invalid limit clarifies, and a superseded/unpaired connection
/// cannot mutate at all.
#[tokio::test]
async fn usage_cap_mutation_applies_once_and_rechecks_currentness() {
    use ene_permission::{
        SetUsageCapCommand, SetUsageCapOutcome, UsageCapConsumption, UsageCapRepository as _,
        UsageCapStatusQuery,
    };

    let Some((handle, _dir)) = memory_handle("usage-surface-cap").await else {
        panic!("the handle must open");
    };
    let live = paired_input("usage-surface-cap");
    let transport = fake_transport();
    let page = usage_page_of(&read_usage(&handle, &live, &transport, usage_request()).await);
    let system_daily = page
        .caps
        .iter()
        .find(|cap| cap.scope == "system" && cap.window == "daily_utc")
        .expect("the none-state system slot is named");
    assert!(system_daily.stored.is_none());
    assert_eq!(system_daily.mark, "usage-cap-system-daily_utc-none");

    // Create through the shared target grammar and the read mark.
    let intent_id = CommandWireId(RawId::new().as_uuid());
    let applied = handle
        .handle_frame(
            management_intent_frame_full(
                &live,
                ManagementIntentKind::ManageRuleConsentCap,
                "cap:system:daily_utc:USD:1000",
                &system_daily.mark,
                RationaleOrigin::ManagementSurface,
                None,
                intent_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(
        outcome_of(&applied),
        ManagementOutcome::StoredAsRuleView {
            revision: ViewMarkWire(String::from("usage-cap-system-daily_utc-rev-0")),
        }
    );
    async fn status(handle: &HostHandle) -> Vec<ene_permission::UsageCapStatus> {
        handle
            .store
            .load_usage_cap_status(UsageCapStatusQuery {
                provider: None,
                at: WallClockWithTz::now(),
            })
            .await
            .expect("the cap status read must answer")
    }
    let stored = status(&handle).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].cap.limit(),
        Money::from_micros(CurrencyCode::Usd, 1_000)
    );
    assert!(matches!(
        stored[0].consumption,
        UsageCapConsumption::Known { .. }
    ));

    // An exact retry of the same id replays the stored outcome: no second
    // write and no revision bump.
    let replayed = handle
        .handle_frame(
            management_intent_frame_full(
                &live,
                ManagementIntentKind::ManageRuleConsentCap,
                "cap:system:daily_utc:USD:1000",
                &system_daily.mark,
                RationaleOrigin::ManagementSurface,
                None,
                intent_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(outcome_of(&replayed), outcome_of(&applied));
    assert_eq!(status(&handle).await[0].cap.revision().as_u64(), 0);

    // A reused stale mark answers StaleBaseView with the current mark and
    // overwrites nothing.
    let stale = handle
        .handle_frame(
            management_intent_frame_full(
                &live,
                ManagementIntentKind::ManageRuleConsentCap,
                "cap:system:daily_utc:USD:9999",
                &system_daily.mark,
                RationaleOrigin::ManagementSurface,
                None,
                CommandWireId(RawId::new().as_uuid()),
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(
        outcome_of(&stale),
        ManagementOutcome::StaleBaseView {
            current: ViewMarkWire(String::from("usage-cap-system-daily_utc-rev-0")),
        }
    );
    assert_eq!(
        status(&handle).await[0].cap.limit(),
        Money::from_micros(CurrencyCode::Usd, 1_000),
        "the stale intent overwrites nothing"
    );

    // A mark from another grammar is face-stale: it never creates and never
    // deletes.
    let face_stale = handle
        .handle_frame(
            management_intent_frame_full(
                &live,
                ManagementIntentKind::ManageRuleConsentCap,
                "cap:provider:openai:monthly_utc:USD:500",
                "consent-dialogue-rev-3",
                RationaleOrigin::ManagementSurface,
                None,
                CommandWireId(RawId::new().as_uuid()),
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(
        outcome_of(&face_stale),
        ManagementOutcome::StaleBaseView {
            current: ViewMarkWire(String::from("usage-cap-provider-openai-monthly_utc-none")),
        }
    );
    assert_eq!(
        status(&handle).await.len(),
        1,
        "the face-stale intent creates no cap"
    );

    // A zero limit is the owner's InvalidLimit, surfaced as clarification.
    let invalid = handle
        .handle_frame(
            management_intent_frame_full(
                &live,
                ManagementIntentKind::ManageRuleConsentCap,
                "cap:system:monthly_utc:USD:0",
                "usage-cap-system-monthly_utc-none",
                RationaleOrigin::ManagementSurface,
                None,
                CommandWireId(RawId::new().as_uuid()),
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert_eq!(outcome_of(&invalid), ManagementOutcome::NeedsClarification);

    // The owner command itself still applies outside the intent inlet (the
    // Host composition and its own tests use it directly).
    assert!(matches!(
        handle
            .store
            .set_usage_cap(SetUsageCapCommand {
                expected: None,
                scope: ene_permission::UsageCapScope::System,
                window: ene_permission::UsageCapWindow::MonthlyUtc,
                limit: Money::from_micros(CurrencyCode::Usd, 10),
            })
            .await
            .unwrap(),
        SetUsageCapOutcome::StoredAs(_)
    ));

    // A superseded connection is refused by the domain gate: neither the
    // read nor the mutation reaches an owner.
    let superseded = {
        let mut state = live.clone();
        state.phase = ConnectionPhase::Superseded;
        state
    };
    let refused = read_usage(&handle, &superseded, &transport, usage_request()).await;
    expect_reject(&refused, RejectKind::StaleConnection);
    let refused_intent = handle
        .handle_frame(
            management_intent_frame_full(
                &superseded,
                ManagementIntentKind::ManageRuleConsentCap,
                "cap:system:daily_utc:USD:123",
                "usage-cap-system-daily_utc-rev-0",
                RationaleOrigin::ManagementSurface,
                None,
                CommandWireId(RawId::new().as_uuid()),
            ),
            superseded,
            &transport,
        )
        .await;
    expect_reject(&refused_intent, RejectKind::StaleConnection);
    assert_eq!(
        status(&handle)
            .await
            .iter()
            .find(|status| status.cap.window() == ene_permission::UsageCapWindow::DailyUtc)
            .map(|status| status.cap.limit()),
        Some(Money::from_micros(CurrencyCode::Usd, 1_000)),
        "the refused intent changed nothing"
    );

    // An unauthenticated connection is closed without a domain answer.
    let unpaired = unpaired_input();
    let closed = read_usage(&handle, &unpaired, &transport, usage_request()).await;
    assert_eq!(
        closed.len(),
        1,
        "the unauthenticated read answers the terminal close only"
    );
    assert_eq!(
        closed[0].payload.message_type(),
        "DisconnectNotice",
        "an unauthenticated connection learns nothing"
    );
}

/// The read surface never carries a body or a secret: with a secret API key
/// in the credential store and a body in History, the serialized usage
/// response contains neither.
#[tokio::test]
async fn usage_summary_response_never_carries_bodies_or_secrets() {
    const SECRET: &str = "sk-usage-surface-secret-3f7a";
    const BODY: &str = "the private owner words that must not leak";

    let credential = ene_credential::CredentialRef::new("openai", "main")
        .expect("the fixture credential ref is valid");
    let Some((handle, _dir)) = memory_handle_with("usage-surface-secret", |store| {
        store.insert(credential, SECRET)
    })
    .await
    else {
        panic!("the handle must open");
    };
    let store = handle.store.clone();
    let provider = "openai";
    let model = "surface-secret-model";
    let ticket = claim_usage(
        &store,
        dialogue_route(provider, model),
        Some(usage_pricing(provider, model)),
        None,
        None,
    )
    .await;
    assert_eq!(
        store
            .record_usage(UsageFact {
                ticket,
                provider: provider.to_owned(),
                model: model.to_owned(),
                input_tokens: Some(3),
                cached_input_tokens: Some(1),
                output_tokens: Some(2),
                source: UsageSource::Reported,
            })
            .await,
        Ok(())
    );
    // A History body rides the same database; the usage read must not echo
    // it (bodies belong to their own filtered surfaces only).
    use ene_companion::{CompanionRepository as _, HistoryRepository as _};
    use ene_presence::PresenceRepository as _;
    let Some(companion) = store.ensure_running_companion().await.ok() else {
        panic!("the running companion must exist");
    };
    let Some(attribution) = store.load_attribution(companion.as_raw()).await.unwrap() else {
        panic!("the presence attribution must exist");
    };
    assert!(
        store
            .append_message(ene_companion::AppendHistoryCommand {
                companion,
                round: RawId::new(),
                role: ene_companion::HistoryRole::Owner,
                text: String::from(BODY),
                lang: String::from("en"),
                at: WallClockWithTz::now(),
                expected_generation: attribution.generation,
                expected_consent: None,
                expected_credential_set: None,
                expected_owner_message: None,
                command_id: None,
                round_wire: Some(RawId::new().as_uuid().to_string()),
                round_intent: None,
                incarnation: Some((1, 2)),
                local_id: None,
            })
            .await
            .is_ok(),
        "the history body must store"
    );

    let live = paired_input("usage-surface-secret");
    let transport = fake_transport();
    let page = usage_page_of(&read_usage(&handle, &live, &transport, usage_request()).await);
    let json = serde_json::to_string(&page).expect("the page serializes");
    assert!(
        !json.contains(SECRET),
        "no credential value travels the usage surface"
    );
    assert!(
        !json.contains(BODY),
        "no history body travels the usage surface"
    );
}
