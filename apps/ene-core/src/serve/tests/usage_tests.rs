//! Stage 6 B4 host-level tests: the first-party usage / cost / cap surface
//! (`usage-cost-cap` §16/§17).
//!
//! The reads run over the real wire frames and the real domain gate; the
//! fixtures seed usage through the production claim/settlement path, so the
//! read observes exactly what the pipeline writes.

use super::*;

use crate::serve::WireFrame;
use crate::test_support::memory_handle_with;

use ene_api::v1::usage::{UsageSummaryPage, UsageSummaryRequest, UsageSummaryResponse};
use ene_inference::cost::UsageEstimate;
use ene_inference::pricing::PricingSnapshot;
use ene_inference::{
    InferenceTicketId, TaskAgentAttemptPremise, UsageFact, UsageRepository as _, UsageSource,
};
use ene_permission::{CapabilityKind, ConsentRevision, ConsumerKind, PurposeKind};
use ene_primitive::{CurrencyCode, RawId, WallClockWithTz};

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
                base: String::from("consent-dialogue-none"),
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
            data_use: match (&task_agent, consumer) {
                (Some(premise), _) => premise.data_use.clone(),
                // A Learning formation always names at least the messages its
                // prompt read; the claim refuses an empty correlation.
                (None, ConsumerKind::CompanionLearning) => vec![RawId::new()],
                (None, _) => Vec::new(),
            },
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

    let live = live_input("usage-surface-secret");
    let transport = fake_transport();
    let page = usage_page_of(&read_usage(&handle, &live, &transport, usage_request()).await);
    assert_eq!(page.rows.len(), 1, "the seeded usage row must travel");
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
