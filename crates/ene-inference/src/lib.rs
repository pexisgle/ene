pub mod cost;
pub mod pricing;
pub mod provider;
pub mod usage_query;

use std::future::Future;
use std::pin::Pin;

use cost::UsageEstimate;
use ene_credential::{
    CredentialRef, CredentialRefRepository, CredentialSetRevision, CredentialStore, ScrubbedText,
};
use ene_permission::{
    CapabilityKind, CheckLiveAuthorizationQuery, ConsentRecord, ConsentRepository, ConsentRevision,
    ConsumerKind, DenyCode, EvaluationTracker, InferenceUseCandidate, LiveAuthorizationDecision,
    PurposeKind, UsageCapRef, UsageReservationRef, UsageReservationState, check_live_authorization,
};
use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
use pricing::{PricingCatalog, PricingResolution, PricingSnapshot, PricingSnapshotRef};
use thiserror::Error;

pub use usage_query::{
    ReportedTokenUsage, USAGE_SUMMARY_PAGE_MAX, USAGE_SUMMARY_RANGE_DEFAULT_DAYS,
    USAGE_SUMMARY_RANGE_MAX_DAYS, UsageSummaryCursor, UsageSummaryQuery, UsageSummaryRepository,
    UsageSummaryRow, UsageSummaryStatus,
};

pub const MAX_INPUT_CHARS: usize = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InferenceTicketId(pub RawId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentAttemptPremise {
    pub delegation: RawId,
    pub task: RawId,
    pub task_revision: RevisionInner,
    pub data_use: Vec<RawId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotSentReason {
    EvaluationConsumed,
    SetupIncomplete,
    ConsentStale,
    NotInAllowlist,
    OverLimit,
    TaskPremiseStale,
    DataUseHeld,
    UsageCapReached,
    UsageCapIndeterminate,
}

#[derive(Clone, PartialEq, Eq)]
pub struct InferenceResultArrival {
    pub ticket: InferenceTicketId,
    pub output_text: String,
    pub usage: UsageFact,
}

impl core::fmt::Debug for InferenceResultArrival {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InferenceResultArrival")
            .field("ticket", &self.ticket)
            .field("output_text", &"<redacted>")
            .field("usage", &self.usage)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageSource {
    Reported,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageFact {
    pub ticket: InferenceTicketId,
    pub provider: String,
    pub model: String,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub source: UsageSource,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InferenceTechnicalError {
    #[error("provider transport failed: {0}")]
    ProviderTransportFailed(String),
    #[error("stream aborted: {reason}")]
    StreamAborted { reason: String },
    #[error("http client build failed")]
    HttpClientBuildFailed,
    #[error("provider response lost")]
    ResponseLost,
    #[error("pricing catalog unavailable")]
    PricingCatalogUnavailable,
    #[error("usage cost projection failed: {reason}")]
    CostProjectionFailed { reason: String },
    #[error("inference storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProviderRequest {
    pub model: String,
    pub credential: CredentialRef,
    pub input: String,
}

impl core::fmt::Debug for ProviderRequest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ProviderRequest")
            .field("model", &self.model)
            .field("credential", &self.credential)
            .field("input", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProviderResponse {
    pub text: String,
    pub usage: Option<RawUsage>,
}

impl core::fmt::Debug for ProviderResponse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ProviderResponse")
            .field("text", &"<redacted>")
            .field("usage", &self.usage)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RawUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

pub trait ProviderTransport: Send + Sync {
    fn complete(
        &self,
        req: ProviderRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse, InferenceTechnicalError>> + Send + '_>>;

    fn usage_estimate(&self, _req: &ProviderRequest) -> Option<UsageEstimate> {
        None
    }

    fn complete_streaming<'a>(
        &'a self,
        req: ProviderRequest,
        sink: &'a mut (dyn DeltaSink + Send),
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse, InferenceTechnicalError>> + Send + 'a>>
    {
        Box::pin(async move {
            let response = self.complete(req).await?;
            if let DeltaFlow::Abort(reason) = sink.push_delta(&response.text).await {
                return Err(InferenceTechnicalError::StreamAborted {
                    reason: reason.to_owned(),
                });
            }
            Ok(response)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaFlow {
    Continue,
    Abort(&'static str),
}

pub trait DeltaSink: Send {
    fn push_delta<'a>(
        &'a mut self,
        delta: &'a str,
    ) -> Pin<Box<dyn Future<Output = DeltaFlow> + Send + 'a>>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DiscardSink;

impl DeltaSink for DiscardSink {
    fn push_delta<'a>(
        &'a mut self,
        _delta: &'a str,
    ) -> Pin<Box<dyn Future<Output = DeltaFlow> + Send + 'a>> {
        Box::pin(async move { DeltaFlow::Continue })
    }
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait UsageRepository: Send + Sync {
    async fn record_usage(&self, fact: UsageFact) -> Result<(), InferenceTechnicalError>;

    async fn load_usage_cost(
        &self,
        ticket: InferenceTicketId,
    ) -> Result<Option<UsageCostRecord>, InferenceTechnicalError>;

    async fn load_usage_reservation(
        &self,
        ticket: InferenceTicketId,
    ) -> Result<Option<UsageReservation>, InferenceTechnicalError>;

    async fn reconcile_orphaned_usage_reservations(&self) -> Result<u64, InferenceTechnicalError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageReservation {
    pub reference: UsageReservationRef,
    pub ticket: InferenceTicketId,
    pub provider: String,
    pub model: String,
    pub pricing: PricingSnapshotRef,
    pub upper_bound: cost::Money,
    pub state: UsageReservationState,
    pub committed: Option<cost::Money>,
    pub opened_at: WallClockWithTz,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageCostRecord {
    pub usage: UsageFact,
    pub cost: cost::UsageCostFact,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceAttempt {
    pub ticket: InferenceTicketId,
    pub consumer: ConsumerKind,
    pub capability: CapabilityKind,
    pub purpose: PurposeKind,
    pub expected_consent: (String, ConsentRevision),
    pub expected_credential_set: CredentialSetRevision,
    pub provider: String,
    pub model: String,
    pub task_agent: Option<TaskAgentAttemptPremise>,
    pub data_use: Vec<RawId>,
    pub pricing: Option<PricingSnapshot>,
    pub usage_estimate: Option<UsageEstimate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceAttemptRecord {
    pub ticket: InferenceTicketId,
    pub consumer: ConsumerKind,
    pub capability: CapabilityKind,
    pub purpose: PurposeKind,
    pub provider: String,
    pub model: String,
    pub task_agent: Option<TaskAgentAttemptPremise>,
    pub data_use: Vec<RawId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AttemptBeginOutcome {
    Started,
    Stale,
    TaskPremiseStale,
    DataUseHeld,
    HeldByCap(UsageCapRef),
    CapIndeterminate,
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait InferenceAttemptRepository: Send + Sync {
    async fn begin_inference_attempt(
        &self,
        attempt: InferenceAttempt,
    ) -> Result<AttemptBeginOutcome, InferenceTechnicalError>;

    async fn load_inference_attempt(
        &self,
        ticket: InferenceTicketId,
    ) -> Result<Option<InferenceAttemptRecord>, InferenceTechnicalError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Authorized; the single-use decision is already consumed.
    Admitted(Box<AuthorizedInference>),
    Declined(NotSentReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionRequest {
    candidate: InferenceUseCandidate,
    consent: ConsentRecord,
    credential: CredentialRef,
    task_agent: Option<TaskAgentAttemptPremise>,
    data_use: Vec<RawId>,
}

impl AdmissionRequest {
    #[must_use]
    pub fn authorize(self, tracker: &mut EvaluationTracker) -> Admission {
        let query = CheckLiveAuthorizationQuery {
            candidate: self.candidate.clone(),
            expected_consent: Some((self.consent.id.clone(), self.consent.rev)),
        };
        match check_live_authorization(&query, Some(&self.consent), tracker) {
            LiveAuthorizationDecision::AllowForThisUse(authorization) => {
                if tracker.consume(&authorization, &self.candidate.fingerprint()) {
                    Admission::Admitted(Box::new(AuthorizedInference {
                        ticket: InferenceTicketId(RawId::new()),
                        consent: (self.consent.id, self.consent.rev),
                        provider: self.consent.provider,
                        model: self.consent.model,
                        credential: self.credential,
                        candidate: self.candidate,
                        task_agent: self.task_agent,
                        data_use: self.data_use,
                    }))
                } else {
                    Admission::Declined(NotSentReason::EvaluationConsumed)
                }
            }
            LiveAuthorizationDecision::Deny(code) => Admission::Declined(not_sent_for_deny(code)),
            LiveAuthorizationDecision::NeedsRevalidation => {
                Admission::Declined(NotSentReason::ConsentStale)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedAdmission {
    Ready(Box<AdmissionRequest>),
    Declined(NotSentReason),
}

pub async fn prepare_dialogue_admission(
    consent: &impl ConsentRepository,
    credential_refs: &impl CredentialRefRepository,
    credential_store: &impl CredentialStore,
    data_use: Vec<RawId>,
) -> Result<PreparedAdmission, InferenceTechnicalError> {
    prepare_admission(
        consent,
        credential_refs,
        credential_store,
        AdmissionBinding {
            consumer: ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            purpose: PurposeKind::DialogueResponse,
            task_agent: None,
            data_use,
        },
    )
    .await
}

pub async fn prepare_learning_admission(
    consent: &impl ConsentRepository,
    credential_refs: &impl CredentialRefRepository,
    credential_store: &impl CredentialStore,
    data_use: Vec<RawId>,
) -> Result<PreparedAdmission, InferenceTechnicalError> {
    prepare_admission(
        consent,
        credential_refs,
        credential_store,
        AdmissionBinding {
            consumer: ConsumerKind::CompanionLearning,
            capability: CapabilityKind::Learning,
            purpose: PurposeKind::MemoryFormation,
            task_agent: None,
            data_use,
        },
    )
    .await
}

pub async fn prepare_task_agent_admission(
    consent: &impl ConsentRepository,
    credential_refs: &impl CredentialRefRepository,
    credential_store: &impl CredentialStore,
    task_agent: TaskAgentAttemptPremise,
) -> Result<PreparedAdmission, InferenceTechnicalError> {
    let data_use = task_agent.data_use.clone();
    prepare_admission(
        consent,
        credential_refs,
        credential_store,
        AdmissionBinding {
            consumer: ConsumerKind::TaskAgent,
            capability: CapabilityKind::Dialogue,
            purpose: PurposeKind::TaskAgentTurn,
            task_agent: Some(task_agent),
            data_use,
        },
    )
    .await
}

struct AdmissionBinding {
    consumer: ConsumerKind,
    capability: CapabilityKind,
    purpose: PurposeKind,
    task_agent: Option<TaskAgentAttemptPremise>,
    data_use: Vec<RawId>,
}

async fn prepare_admission(
    consent: &impl ConsentRepository,
    credential_refs: &impl CredentialRefRepository,
    credential_store: &impl CredentialStore,
    binding: AdmissionBinding,
) -> Result<PreparedAdmission, InferenceTechnicalError> {
    let AdmissionBinding {
        consumer,
        capability,
        purpose,
        task_agent,
        data_use,
    } = binding;
    let record = consent.load_current(capability).await.map_err(|_| {
        InferenceTechnicalError::StorageUnavailable {
            reason: String::from("load consent"),
        }
    })?;
    let Some(record) = record else {
        return Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete));
    };
    let known_refs = credential_refs.list_refs().await.map_err(|_| {
        InferenceTechnicalError::StorageUnavailable {
            reason: String::from("list credential refs"),
        }
    })?;
    let Some(credential) = known_refs
        .iter()
        .find(|known| known.provider() == record.provider && known.id() == record.credential_id)
        .cloned()
    else {
        return Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete));
    };
    if !credential_store.contains(&credential) {
        return Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete));
    }
    let candidate = InferenceUseCandidate {
        consumer,
        capability,
        provider_ref: record.provider.clone(),
        model: record.model.clone(),
        purpose,
    };
    Ok(PreparedAdmission::Ready(Box::new(AdmissionRequest {
        candidate,
        consent: record,
        credential,
        task_agent,
        data_use,
    })))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedInference {
    ticket: InferenceTicketId,
    consent: (String, ConsentRevision),
    provider: String,
    model: String,
    credential: CredentialRef,
    candidate: InferenceUseCandidate,
    task_agent: Option<TaskAgentAttemptPremise>,
    data_use: Vec<RawId>,
}

impl AuthorizedInference {
    #[must_use]
    pub fn consent_premise(&self) -> (&str, u64) {
        (&self.consent.0, self.consent.1.as_u64())
    }

    #[must_use]
    pub fn ticket(&self) -> InferenceTicketId {
        self.ticket
    }

    #[must_use]
    pub fn task_agent_premise(&self) -> Option<&TaskAgentAttemptPremise> {
        self.task_agent.as_ref()
    }

    #[must_use]
    pub fn data_use(&self) -> &[RawId] {
        &self.data_use
    }
}

#[derive(Clone, Default)]
pub struct DispatchAbort {
    inner: std::sync::Arc<DispatchAbortInner>,
}

#[derive(Default)]
struct DispatchAbortInner {
    aborted: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl DispatchAbort {
    pub fn abort(&self) {
        self.inner
            .aborted
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.notify.notify_one();
    }

    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.inner.aborted.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn aborted(&self) {
        while !self.is_aborted() {
            self.inner.notify.notified().await;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceDispatchOutcome {
    Completed {
        arrival: InferenceResultArrival,
        adopted: bool,
    },
    NotSent(NotSentReason),
    Aborted,
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait InferenceExecutor: Send + Sync {
    async fn admit_dialogue(
        &self,
        data_use: Vec<RawId>,
    ) -> Result<Admission, InferenceTechnicalError>;

    async fn admit_learning(
        &self,
        data_use: Vec<RawId>,
    ) -> Result<Admission, InferenceTechnicalError>;

    async fn admit_task_agent(
        &self,
        task_agent: TaskAgentAttemptPremise,
    ) -> Result<Admission, InferenceTechnicalError>;

    async fn dispatch(
        &self,
        authorized: AuthorizedInference,
        prompt: ScrubbedText,
        sink: &mut (dyn DeltaSink + Send),
        abort: Option<&DispatchAbort>,
    ) -> Result<InferenceDispatchOutcome, InferenceTechnicalError>;
}

#[expect(
    clippy::too_many_arguments,
    reason = "the owner boundary takes each repository it must sequence in order; a parameter struct would only restate the same wiring"
)]
pub async fn dispatch_authorized(
    authorized: AuthorizedInference,
    prompt: ScrubbedText,
    sink: &mut (dyn DeltaSink + Send),
    abort: Option<&DispatchAbort>,
    consent: &impl ConsentRepository,
    attempts: &impl InferenceAttemptRepository,
    usage: &impl UsageRepository,
    transport: &impl ProviderTransport,
) -> Result<InferenceDispatchOutcome, InferenceTechnicalError> {
    if abort.is_some_and(DispatchAbort::is_aborted) {
        return Ok(InferenceDispatchOutcome::Aborted);
    }
    if prompt.text().chars().count() > MAX_INPUT_CHARS {
        return Ok(InferenceDispatchOutcome::NotSent(NotSentReason::OverLimit));
    }
    let ticket = authorized.ticket;
    let consumer = authorized.candidate.consumer;
    let capability = authorized.candidate.capability;
    let purpose = authorized.candidate.purpose;
    let (consent_id, consent_rev) = (authorized.consent.0.clone(), authorized.consent.1);
    let (provider, model) = (authorized.provider.clone(), authorized.model.clone());
    let credential_set = prompt.credential_set();
    let credential = authorized.credential;
    let task_agent = authorized.task_agent;
    let data_use = authorized.data_use;
    let pricing = match PricingCatalog::first_party()
        .map_err(|_| InferenceTechnicalError::PricingCatalogUnavailable)?
        .resolve(&provider, &model, WallClockWithTz::now())
    {
        PricingResolution::Priced(snapshot) => Some(snapshot),
        PricingResolution::Unpriced => None,
    };
    let request = ProviderRequest {
        model: model.clone(),
        credential,
        input: prompt.into_text(),
    };
    let usage_estimate = transport.usage_estimate(&request);
    match attempts
        .begin_inference_attempt(InferenceAttempt {
            ticket,
            consumer,
            capability,
            purpose,
            expected_consent: (consent_id.clone(), consent_rev),
            expected_credential_set: credential_set,
            provider: provider.clone(),
            model: model.clone(),
            task_agent,
            data_use,
            pricing,
            usage_estimate,
        })
        .await
    {
        Ok(AttemptBeginOutcome::Started) => {}
        Ok(AttemptBeginOutcome::Stale) => {
            return Ok(InferenceDispatchOutcome::NotSent(
                NotSentReason::ConsentStale,
            ));
        }
        Ok(AttemptBeginOutcome::TaskPremiseStale) => {
            return Ok(InferenceDispatchOutcome::NotSent(
                NotSentReason::TaskPremiseStale,
            ));
        }
        Ok(AttemptBeginOutcome::DataUseHeld) => {
            return Ok(InferenceDispatchOutcome::NotSent(
                NotSentReason::DataUseHeld,
            ));
        }
        Ok(AttemptBeginOutcome::HeldByCap(_)) => {
            return Ok(InferenceDispatchOutcome::NotSent(
                NotSentReason::UsageCapReached,
            ));
        }
        Ok(AttemptBeginOutcome::CapIndeterminate) => {
            return Ok(InferenceDispatchOutcome::NotSent(
                NotSentReason::UsageCapIndeterminate,
            ));
        }
        Err(error) => return Err(error),
    }
    let response = if let Some(abort) = abort {
        tokio::select! {
            biased;
            () = abort.aborted() => {
                usage
                    .record_usage(unknown_usage(ticket, &provider, &model))
                    .await?;
                return Ok(InferenceDispatchOutcome::Aborted);
            }
            response = transport.complete_streaming(request, sink) => response,
        }
    } else {
        transport.complete_streaming(request, sink).await
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            usage
                .record_usage(unknown_usage(ticket, &provider, &model))
                .await?;
            return Err(error);
        }
    };
    let fact = match response
        .usage
        .filter(|raw| raw.cached_input_tokens <= raw.input_tokens)
    {
        Some(raw) => UsageFact {
            ticket,
            provider: provider.clone(),
            model: model.clone(),
            input_tokens: Some(raw.input_tokens),
            cached_input_tokens: Some(raw.cached_input_tokens),
            output_tokens: Some(raw.output_tokens),
            source: UsageSource::Reported,
        },
        None => UsageFact {
            ticket,
            provider,
            model,
            input_tokens: None,
            cached_input_tokens: None,
            output_tokens: None,
            source: UsageSource::Unknown,
        },
    };
    let arrival = InferenceResultArrival {
        ticket,
        output_text: response.text,
        usage: fact,
    };
    usage.record_usage(arrival.usage.clone()).await?;
    let adopted = consent_matches(consent, capability, &consent_id, consent_rev).await?;
    Ok(InferenceDispatchOutcome::Completed { arrival, adopted })
}

async fn consent_matches(
    consent: &impl ConsentRepository,
    capability: CapabilityKind,
    id: &str,
    revision: ConsentRevision,
) -> Result<bool, InferenceTechnicalError> {
    let current = consent.load_current(capability).await.map_err(|_| {
        InferenceTechnicalError::StorageUnavailable {
            reason: String::from("load consent"),
        }
    })?;
    let Some(current) = current else {
        return Ok(false);
    };
    Ok(current.id == id && current.rev == revision)
}

fn unknown_usage(ticket: InferenceTicketId, provider: &str, model: &str) -> UsageFact {
    UsageFact {
        ticket,
        provider: provider.to_string(),
        model: model.to_string(),
        input_tokens: None,
        cached_input_tokens: None,
        output_tokens: None,
        source: UsageSource::Unknown,
    }
}

fn not_sent_for_deny(code: DenyCode) -> NotSentReason {
    match code {
        DenyCode::ConsentStale => NotSentReason::ConsentStale,
        DenyCode::NotInAllowlist => NotSentReason::NotInAllowlist,
    }
}

pub mod fake {
    use std::future::Future;
    use std::pin::Pin;

    use super::RawUsage;
    use super::{
        InferenceTechnicalError, ProviderRequest, ProviderResponse, ProviderTransport,
        UsageEstimate,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum FakeFailure {
        Transport(String),
        ResponseLost,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct FakeProviderTransport {
        pub text: String,
        pub usage: Option<RawUsage>,
        pub fail: Option<FakeFailure>,
        pub usage_estimate: Option<UsageEstimate>,
    }

    impl FakeProviderTransport {
        #[must_use]
        pub fn new(text: String, usage: Option<RawUsage>) -> Self {
            Self {
                text,
                usage,
                fail: None,
                usage_estimate: None,
            }
        }

        #[must_use]
        pub fn failing(fail: FakeFailure) -> Self {
            Self {
                text: String::new(),
                usage: None,
                fail: Some(fail),
                usage_estimate: None,
            }
        }

        #[must_use]
        pub fn with_usage_estimate(mut self, estimate: UsageEstimate) -> Self {
            self.usage_estimate = Some(estimate);
            self
        }
    }

    impl ProviderTransport for FakeProviderTransport {
        fn complete(
            &self,
            _req: ProviderRequest,
        ) -> Pin<
            Box<dyn Future<Output = Result<ProviderResponse, InferenceTechnicalError>> + Send + '_>,
        > {
            let result = if let Some(fail) = &self.fail {
                Err(match fail {
                    FakeFailure::Transport(reason) => {
                        InferenceTechnicalError::ProviderTransportFailed(reason.clone())
                    }
                    FakeFailure::ResponseLost => InferenceTechnicalError::ResponseLost,
                })
            } else {
                Ok(ProviderResponse {
                    text: self.text.clone(),
                    usage: self.usage,
                })
            };
            Box::pin(async move { result })
        }

        fn usage_estimate(&self, _req: &ProviderRequest) -> Option<UsageEstimate> {
            self.usage_estimate
        }
    }
}

#[cfg(test)]
mod dispatch_tests {
    use std::sync::Mutex;

    use super::fake::{FakeFailure, FakeProviderTransport};
    use super::{
        AttemptBeginOutcome, AuthorizedInference, DiscardSink, DispatchAbort, InferenceAttempt,
        InferenceAttemptRecord, InferenceAttemptRepository, InferenceDispatchOutcome,
        InferenceResultArrival, InferenceTechnicalError, InferenceTicketId, MAX_INPUT_CHARS,
        NotSentReason, ProviderRequest, ProviderResponse, ProviderTransport, RawUsage,
        TaskAgentAttemptPremise, UsageCostRecord, UsageEstimate, UsageFact, UsageRepository,
        UsageReservation, UsageSource, dispatch_authorized,
    };
    use ene_credential::{CredentialRef, CredentialSetRevision, ScrubbedText};
    use ene_permission::{
        CapabilityKind, ConsentRecord, ConsentRepository, ConsentRevision, ConsumerKind,
        InferenceUseCandidate, PermissionTechnicalError, PurposeKind,
    };
    use ene_primitive::{RawId, RevisionInner};

    fn record(revision: u64) -> ConsentRecord {
        ConsentRecord {
            capability: CapabilityKind::Dialogue,
            id: String::from("consent-1"),
            rev: ConsentRevision::from_u64(revision),
            provider: String::from("acme"),
            model: String::from("dialogue-1"),
            credential_id: String::from("acme:main"),
        }
    }

    struct FixedConsent(Option<ConsentRecord>);

    impl ConsentRepository for FixedConsent {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn load_current(
            &self,
            capability: CapabilityKind,
        ) -> Result<Option<ConsentRecord>, PermissionTechnicalError> {
            Ok(self
                .0
                .clone()
                .filter(|consent| consent.capability == capability))
        }
    }

    struct StartedAttempts;

    impl InferenceAttemptRepository for StartedAttempts {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn begin_inference_attempt(
            &self,
            _attempt: InferenceAttempt,
        ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
            Ok(AttemptBeginOutcome::Started)
        }

        async fn load_inference_attempt(
            &self,
            _ticket: InferenceTicketId,
        ) -> Result<Option<InferenceAttemptRecord>, InferenceTechnicalError> {
            Ok(None)
        }
    }

    struct StaleAttempts;

    impl InferenceAttemptRepository for StaleAttempts {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn begin_inference_attempt(
            &self,
            _attempt: InferenceAttempt,
        ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
            Ok(AttemptBeginOutcome::Stale)
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn load_inference_attempt(
            &self,
            _ticket: InferenceTicketId,
        ) -> Result<Option<InferenceAttemptRecord>, InferenceTechnicalError> {
            Ok(None)
        }
    }

    struct HeldByCapAttempts(ene_permission::UsageCapRef);

    impl InferenceAttemptRepository for HeldByCapAttempts {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn begin_inference_attempt(
            &self,
            _attempt: InferenceAttempt,
        ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
            Ok(AttemptBeginOutcome::HeldByCap(self.0.clone()))
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn load_inference_attempt(
            &self,
            _ticket: InferenceTicketId,
        ) -> Result<Option<InferenceAttemptRecord>, InferenceTechnicalError> {
            Ok(None)
        }
    }

    struct CapIndeterminateAttempts;

    impl InferenceAttemptRepository for CapIndeterminateAttempts {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn begin_inference_attempt(
            &self,
            _attempt: InferenceAttempt,
        ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
            Ok(AttemptBeginOutcome::CapIndeterminate)
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn load_inference_attempt(
            &self,
            _ticket: InferenceTicketId,
        ) -> Result<Option<InferenceAttemptRecord>, InferenceTechnicalError> {
            Ok(None)
        }
    }

    struct CapturedAttempts(Mutex<Vec<InferenceAttempt>>);

    impl InferenceAttemptRepository for CapturedAttempts {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn begin_inference_attempt(
            &self,
            attempt: InferenceAttempt,
        ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
            self.0.lock().expect("attempt capture lock").push(attempt);
            Ok(AttemptBeginOutcome::Started)
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn load_inference_attempt(
            &self,
            _ticket: InferenceTicketId,
        ) -> Result<Option<InferenceAttemptRecord>, InferenceTechnicalError> {
            Ok(None)
        }
    }

    struct CountingTransport(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl ProviderTransport for CountingTransport {
        fn complete(
            &self,
            _req: ProviderRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<ProviderResponse, InferenceTechnicalError>>
                    + Send
                    + '_,
            >,
        > {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async move {
                Ok(ProviderResponse {
                    text: String::new(),
                    usage: None,
                })
            })
        }
    }

    #[derive(Default)]
    struct DroppingTransport {
        started: std::sync::Arc<tokio::sync::Notify>,
        dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    struct DropProbe(std::sync::Arc<std::sync::atomic::AtomicBool>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl ProviderTransport for DroppingTransport {
        fn complete(
            &self,
            _req: ProviderRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<ProviderResponse, InferenceTechnicalError>>
                    + Send
                    + '_,
            >,
        > {
            let started = std::sync::Arc::clone(&self.started);
            let dropped = std::sync::Arc::clone(&self.dropped);
            Box::pin(async move {
                let _probe = DropProbe(dropped);
                started.notify_one();
                std::future::pending::<()>().await;
                unreachable!("the aborted provider future is dropped, never completed")
            })
        }
    }

    struct CapturedUsage(Mutex<Vec<UsageFact>>);

    impl UsageRepository for CapturedUsage {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn record_usage(&self, fact: UsageFact) -> Result<(), InferenceTechnicalError> {
            self.0.lock().expect("usage capture lock").push(fact);
            Ok(())
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn load_usage_cost(
            &self,
            _ticket: InferenceTicketId,
        ) -> Result<Option<UsageCostRecord>, InferenceTechnicalError> {
            Ok(None)
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn load_usage_reservation(
            &self,
            _ticket: InferenceTicketId,
        ) -> Result<Option<UsageReservation>, InferenceTechnicalError> {
            Ok(None)
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn reconcile_orphaned_usage_reservations(
            &self,
        ) -> Result<u64, InferenceTechnicalError> {
            Ok(0)
        }
    }

    struct ScrubRefs(ene_credential::CredentialSetRevision);

    impl ene_credential::CredentialRefRepository for ScrubRefs {
        #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
        async fn list_refs(
            &self,
        ) -> Result<Vec<ene_credential::CredentialRef>, ene_credential::CredentialTechnicalError>
        {
            Ok(Vec::new())
        }
    }

    impl ene_credential::CredentialSetRepository for ScrubRefs {
        #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
        async fn current_set_revision(
            &self,
        ) -> Result<ene_credential::CredentialSetRevision, ene_credential::CredentialTechnicalError>
        {
            Ok(self.0)
        }
    }

    async fn scrub_fixture(
        text: &str,
        revision: ene_credential::CredentialSetRevision,
    ) -> ScrubbedText {
        use ene_credential::SecretScrubber as _;
        ene_credential::CredentialScrubber {
            refs: &ScrubRefs(revision),
            store: &ene_credential::MemoryCredentialStore::new(),
        }
        .scrub(text)
        .await
        .expect("fixture registry is readable")
    }
    async fn prompt(text: impl Into<String>) -> ScrubbedText {
        scrub_fixture(&text.into(), CredentialSetRevision::initial()).await
    }

    fn authorized() -> AuthorizedInference {
        let consent = record(1);
        AuthorizedInference {
            ticket: InferenceTicketId(RawId::new()),
            consent: (consent.id, consent.rev),
            provider: consent.provider,
            model: consent.model,
            credential: CredentialRef::new("acme", "main").expect("valid test fixture"),
            candidate: InferenceUseCandidate {
                consumer: ConsumerKind::CompanionDialogue,
                capability: CapabilityKind::Dialogue,
                provider_ref: String::from("acme"),
                model: String::from("dialogue-1"),
                purpose: PurposeKind::DialogueResponse,
            },
            task_agent: None,
            data_use: Vec::new(),
        }
    }

    fn task_agent_premise() -> TaskAgentAttemptPremise {
        TaskAgentAttemptPremise {
            delegation: RawId::new(),
            task: RawId::new(),
            task_revision: RevisionInner::from_u64(1),
            data_use: vec![RawId::new(), RawId::new()],
        }
    }

    fn authorized_task_agent(premise: TaskAgentAttemptPremise) -> AuthorizedInference {
        let consent = record(1);
        let data_use = premise.data_use.clone();
        AuthorizedInference {
            ticket: InferenceTicketId(RawId::new()),
            consent: (consent.id, consent.rev),
            provider: consent.provider,
            model: consent.model,
            credential: CredentialRef::new("acme", "main").expect("valid test fixture"),
            candidate: InferenceUseCandidate {
                consumer: ConsumerKind::TaskAgent,
                capability: CapabilityKind::Dialogue,
                provider_ref: String::from("acme"),
                model: String::from("dialogue-1"),
                purpose: PurposeKind::TaskAgentTurn,
            },
            task_agent: Some(premise),
            data_use,
        }
    }

    fn authorized_route(provider: &str, model: &str) -> AuthorizedInference {
        let mut authorized = authorized();
        authorized.provider = provider.to_owned();
        authorized.model = model.to_owned();
        authorized
    }

    #[tokio::test]
    async fn priced_route_claims_with_the_reviewed_snapshot() {
        let attempts = CapturedAttempts(Mutex::new(Vec::new()));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let transport = FakeProviderTransport::new(String::from("ok"), None);
        dispatch_authorized(
            authorized_route("openai", "gpt-4o"),
            prompt("hello").await,
            &mut DiscardSink,
            None,
            &consent,
            &attempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        let claimed = attempts.0.lock().expect("attempt capture lock");
        assert_eq!(claimed.len(), 1);
        let pricing = claimed[0]
            .pricing
            .as_ref()
            .expect("a reviewed route claims under its snapshot");
        assert_eq!(pricing.provider, "openai");
        assert_eq!(pricing.model, "gpt-4o");
        assert_eq!(pricing.input_rate.micros_per_million(), 2_500_000);
        assert_eq!(pricing.cached_input_rate.micros_per_million(), 1_250_000);
        assert_eq!(pricing.output_rate.micros_per_million(), 10_000_000);
        assert_eq!(
            pricing.source_revision,
            crate::pricing::FIRST_PARTY_REVISION
        );
    }

    #[tokio::test]
    async fn held_and_indeterminate_cap_admissions_stay_distinct_refusals() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport = CountingTransport(std::sync::Arc::clone(&calls));
        let held = HeldByCapAttempts(ene_permission::UsageCapRef::new(
            ene_permission::UsageCapId::new(
                ene_permission::UsageCapScope::System,
                ene_permission::UsageCapWindow::DailyUtc,
            ),
            ene_permission::UsageCapRevision::from_u64(1),
        ));
        let outcome = dispatch_authorized(
            authorized(),
            prompt("hello").await,
            &mut DiscardSink,
            None,
            &consent,
            &held,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        assert_eq!(
            outcome,
            InferenceDispatchOutcome::NotSent(NotSentReason::UsageCapReached),
            "a cap-exceeded refusal never reads as consent or erasure staleness"
        );
        let outcome = dispatch_authorized(
            authorized(),
            prompt("hello").await,
            &mut DiscardSink,
            None,
            &consent,
            &CapIndeterminateAttempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        assert_eq!(
            outcome,
            InferenceDispatchOutcome::NotSent(NotSentReason::UsageCapIndeterminate),
            "an unprovable bound is its own typed refusal, never held-by-cap or zero"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "neither cap refusal may reach the provider"
        );
        assert!(
            usage.0.lock().expect("usage capture lock").is_empty(),
            "a refused send records no usage fact"
        );
    }

    #[tokio::test]
    async fn transport_failure_records_unknown_counts() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let transport = FakeProviderTransport::failing(FakeFailure::Transport("down".to_owned()));
        let result = dispatch_authorized(
            authorized(),
            prompt("hello").await,
            &mut DiscardSink,
            None,
            &consent,
            &StartedAttempts,
            &usage,
            &transport,
        )
        .await;
        assert!(result.is_err(), "transport failure propagates");
        let facts = usage.0.lock().expect("usage capture lock");
        assert_eq!(facts.len(), 1, "an uncertain attempt records one fact");
        assert_eq!(facts[0].source, UsageSource::Unknown);
        assert_eq!(facts[0].input_tokens, None);
        assert_eq!(facts[0].cached_input_tokens, None);
        assert_eq!(facts[0].output_tokens, None);
    }

    #[tokio::test]
    async fn stale_credential_set_never_calls_the_provider() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport = CountingTransport(std::sync::Arc::clone(&calls));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let outcome = dispatch_authorized(
            authorized(),
            prompt("the key is sk-new").await,
            &mut DiscardSink,
            None,
            &consent,
            &StaleAttempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        assert_eq!(
            outcome,
            InferenceDispatchOutcome::NotSent(NotSentReason::ConsentStale)
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a stale credential-set premise must not reach the provider"
        );
        assert!(usage.0.lock().expect("usage capture lock").is_empty());
    }

    #[tokio::test]
    async fn completed_records_reported_counts_even_when_adoption_moves() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(2)));
        let transport = FakeProviderTransport::new(
            String::from("hi there"),
            Some(RawUsage {
                input_tokens: 4,
                cached_input_tokens: 1,
                output_tokens: 2,
            }),
        );
        let outcome = dispatch_authorized(
            authorized(),
            prompt("hello").await,
            &mut DiscardSink,
            None,
            &consent,
            &StartedAttempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        let InferenceDispatchOutcome::Completed { arrival, adopted } = outcome else {
            panic!("a provider success completes");
        };
        assert!(!adopted, "the moved consent refuses adoption");
        assert_eq!(arrival.output_text, "hi there");
        assert_eq!(arrival.usage.source, UsageSource::Reported);
        let facts = usage.0.lock().expect("usage capture lock");
        assert_eq!(facts.len(), 1, "the reported fact is kept");
        assert_eq!(facts[0].input_tokens, Some(4));
        assert_eq!(facts[0].cached_input_tokens, Some(1));
        assert_eq!(facts[0].output_tokens, Some(2));
    }

    #[tokio::test]
    async fn task_agent_claim_carries_the_durable_correlation() {
        let attempts = CapturedAttempts(Mutex::new(Vec::new()));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let transport = FakeProviderTransport::new(String::from("ok"), None);
        let premise = task_agent_premise();
        let outcome = dispatch_authorized(
            authorized_task_agent(premise.clone()),
            prompt("delegated work").await,
            &mut DiscardSink,
            None,
            &consent,
            &attempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        assert!(matches!(
            outcome,
            InferenceDispatchOutcome::Completed { adopted: true, .. }
        ));
        let claimed = attempts.0.lock().expect("attempt capture lock");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].consumer, ConsumerKind::TaskAgent);
        assert_eq!(claimed[0].purpose, PurposeKind::TaskAgentTurn);
        assert_eq!(claimed[0].capability, CapabilityKind::Dialogue);
        assert_eq!(
            claimed[0].task_agent,
            Some(premise),
            "the claim carries the delegation/task/relied-revision and the ordered data_use"
        );
        let facts = usage.0.lock().expect("usage capture lock");
        assert_eq!(facts.len(), 1, "the task agent turn records its usage");
        assert_eq!(facts[0].ticket, claimed[0].ticket);
        assert_eq!(
            facts[0].source,
            UsageSource::Unknown,
            "a provider without reported counts records unknown, never zero"
        );
        assert_eq!(facts[0].input_tokens, None);
        assert_eq!(facts[0].cached_input_tokens, None);
        assert_eq!(facts[0].output_tokens, None);
    }

    #[tokio::test]
    async fn abort_during_the_provider_wait_records_unknown_usage_and_drops_the_call() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let transport = DroppingTransport::default();
        let abort = DispatchAbort::default();
        let started = std::sync::Arc::clone(&transport.started);
        let dropped = std::sync::Arc::clone(&transport.dropped);
        let authorized = authorized();
        let ticket = authorized.ticket;
        let mut sink = DiscardSink;

        let mut dispatch = Box::pin(dispatch_authorized(
            authorized,
            prompt("hello").await,
            &mut sink,
            Some(&abort),
            &consent,
            &StartedAttempts,
            &usage,
            &transport,
        ));
        let mut started_wait = Box::pin(started.notified());
        tokio::select! {
            () = &mut started_wait => {}
            outcome = &mut dispatch => {
                panic!("the dispatch must wait for the abort, got {outcome:?}");
            }
        }
        assert!(
            !dropped.load(std::sync::atomic::Ordering::SeqCst),
            "the provider future is still running before the abort"
        );
        abort.abort();
        let outcome = dispatch.await.expect("the abort answers an outcome");
        assert_eq!(outcome, InferenceDispatchOutcome::Aborted);
        assert!(
            dropped.load(std::sync::atomic::Ordering::SeqCst),
            "the abort dropped the in-flight provider future"
        );
        let facts = usage.0.lock().expect("usage capture lock");
        assert_eq!(facts.len(), 1, "the claimed attempt keeps its accounting");
        assert_eq!(facts[0].ticket, ticket);
        assert_eq!(facts[0].source, UsageSource::Unknown);
        assert_eq!(facts[0].input_tokens, None);
        assert_eq!(facts[0].cached_input_tokens, None);
        assert_eq!(facts[0].output_tokens, None);
    }
}
