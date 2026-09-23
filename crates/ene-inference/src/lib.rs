pub mod cost;
pub mod pricing;
pub mod provider;
pub mod usage_query;

use std::future::Future;
use std::pin::Pin;

use cost::UsageEstimate;
use ene_credential::{
    CredentialRef, CredentialRefRepository, CredentialSetRevision, CredentialStore,
    CredentialTechnicalError, ScrubbedText, available_credential,
};
use ene_permission::{
    CapabilityKind, CheckLiveAuthorizationQuery, ConsentRecord, ConsentRepository, ConsentRevision,
    ConsumerKind, DenyCode, EvaluationTracker, InferenceUseCandidate, LiveAuthorizationDecision,
    PermissionTechnicalError, PurposeKind, check_live_authorization,
};
use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
use pricing::{PricingCatalog, PricingResolution, PricingSnapshot};
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
    /// The safe upper bound of the token usage this adapter's request can
    /// bill (`usage-cost-cap` §8), or `None` when the adapter cannot build a
    /// finite bound.
    ///
    /// The admission converts the estimate into a cap reservation under the
    /// admission pricing snapshot, so it must cover every token the provider
    /// contract can charge: the input bound must be tokenizer-safe for the
    /// request body, and the output bound must be the explicit maximum the
    /// adapter sets on the request itself (not an average). A `None` answer
    /// keeps uncapped sends working and refuses cap-enabled sends
    /// ([`NotSentReason::UsageCapIndeterminate`]) instead of guessing.
    fn usage_estimate(&self, _req: &ProviderRequest) -> Option<UsageEstimate> {
        None
    }

    /// Runs one completion, pushing incremental output to `sink`.
    ///
    /// Deltas are pushed in provider order and the transport stops reading
    /// on [`DeltaFlow::Abort`], reporting [`InferenceTechnicalError::StreamAborted`]:
    /// no delta produced after the consumer stopped is presented, and the
    /// call never completes normally with a delivery gap. The returned
    /// response carries the full text for adoption and History.
    fn complete_streaming<'a>(
        &'a self,
        req: ProviderRequest,
        sink: &'a mut (dyn DeltaSink + Send),
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse, InferenceTechnicalError>> + Send + 'a>>;
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
    /// Persist the first complete settlement for a claimed ticket. Duplicate
    /// arrivals are idempotent; an Unknown settlement is not revised later.
    /// Reject orphan tickets, route mismatches, and inconsistent token facts.
    ///
    /// The same transaction settles the ticket's usage reservation when one
    /// exists: a reported fact commits it as
    /// [`ene_permission::UsageReservationState::CommittedReported`] with the
    /// actual cost derived from the bound pricing snapshot (releasing the
    /// unused reservation), and an unknown fact commits it as
    /// [`ene_permission::UsageReservationState::CommittedUnknown`], which
    /// keeps the reserved upper bound counted against every cap. The first
    /// settlement wins; a duplicate never revises a terminal reservation.
    async fn record_usage(&self, fact: UsageFact) -> Result<(), InferenceTechnicalError>;

    /// Settles every non-terminal usage reservation as
    /// [`ene_permission::UsageReservationState::CommittedUnknown`]
    /// (`usage-cost-cap` §15).
    ///
    /// This is the Host-startup re-evaluation of reservations orphaned by a
    /// crash: external consumption cannot be denied, so the reserved upper
    /// bound stays counted and the ticket settles an Unknown token usage
    /// fact; a crash never releases a reservation and never zeroes usage.
    /// Terminal reservations are not re-counted or re-inserted. The operation
    /// is idempotent.
    async fn reconcile_orphaned_usage_reservations(&self) -> Result<(), InferenceTechnicalError>;
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
    /// A current cap's window consumption plus this request's safe upper
    /// bound would exceed the limit: the caller must NOT issue provider I/O
    /// for this ticket, and neither the attempt nor a reservation is
    /// recorded.
    HeldByCap,
    /// At least one current cap applies to the route, but the claim cannot
    /// construct a finite safe upper bound under it (no reviewed rate, no
    /// provider estimate, a cap currency the request cannot be compared in,
    /// or an unrepresentable sum): the caller must NOT issue provider I/O.
    /// The cap is never treated as satisfied and the bound is never assumed
    /// zero.
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
                if tracker.consume(&authorization, &self.candidate) {
                    Admission::Admitted(Box::new(AuthorizedInference {
                        ticket: InferenceTicketId(RawId::new()),
                        consent: (self.consent.id, self.consent.rev),
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
    let record = consent.load_current(capability).await.map_err(
        |PermissionTechnicalError::StorageUnavailable { reason }| {
            InferenceTechnicalError::StorageUnavailable { reason }
        },
    )?;
    let Some(record) = record else {
        return Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete));
    };
    let credential = available_credential(
        &record.provider,
        &record.credential_id,
        credential_refs,
        credential_store,
    )
    .await
    .map_err(|CredentialTechnicalError::StorageUnavailable { reason }| {
        InferenceTechnicalError::StorageUnavailable { reason }
    })?;
    let Some(credential) = credential else {
        return Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete));
    };
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
    let (provider, model) = (
        authorized.candidate.provider_ref.clone(),
        authorized.candidate.model.clone(),
    );
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
        Ok(AttemptBeginOutcome::HeldByCap) => {
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
        None => unknown_usage(ticket, &provider, &model),
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
    let current = consent.load_current(capability).await.map_err(
        |PermissionTechnicalError::StorageUnavailable { reason }| {
            InferenceTechnicalError::StorageUnavailable { reason }
        },
    )?;
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
        DeltaFlow, DeltaSink, InferenceTechnicalError, ProviderRequest, ProviderResponse,
        ProviderTransport, UsageEstimate,
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
        fn complete_streaming<'a>(
            &'a self,
            _req: ProviderRequest,
            sink: &'a mut (dyn DeltaSink + Send),
        ) -> Pin<
            Box<dyn Future<Output = Result<ProviderResponse, InferenceTechnicalError>> + Send + 'a>,
        > {
            Box::pin(async move {
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
                let response = result?;
                if let DeltaFlow::Abort(reason) = sink.push_delta(&response.text).await {
                    return Err(InferenceTechnicalError::StreamAborted {
                        reason: reason.to_owned(),
                    });
                }
                Ok(response)
            })
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
        AttemptBeginOutcome, AuthorizedInference, DeltaSink, DiscardSink, DispatchAbort,
        InferenceAttempt, InferenceAttemptRecord, InferenceAttemptRepository,
        InferenceDispatchOutcome, InferenceResultArrival, InferenceTechnicalError,
        InferenceTicketId, MAX_INPUT_CHARS, NotSentReason, ProviderRequest, ProviderResponse,
        ProviderTransport, RawUsage, TaskAgentAttemptPremise, UsageEstimate, UsageFact,
        UsageRepository, UsageSource, dispatch_authorized,
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

    struct RecordingAttempts(Mutex<usize>);

    impl InferenceAttemptRepository for RecordingAttempts {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn begin_inference_attempt(
            &self,
            _attempt: InferenceAttempt,
        ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
            *self.0.lock().expect("attempt count lock") += 1;
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

    /// An attempt repository whose claim answers a moved Task Agent premise,
    /// modelling steering landing between admission and the claim.
    struct TaskPremiseStaleAttempts;

    impl InferenceAttemptRepository for TaskPremiseStaleAttempts {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn begin_inference_attempt(
            &self,
            _attempt: InferenceAttempt,
        ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
            Ok(AttemptBeginOutcome::TaskPremiseStale)
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

    /// An attempt repository whose claim finds a covering erasure condition,
    /// modelling a Targeted Deletion condition landing after the History read
    /// but before the send admission.
    struct DataUseHeldAttempts;

    impl InferenceAttemptRepository for DataUseHeldAttempts {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn begin_inference_attempt(
            &self,
            _attempt: InferenceAttempt,
        ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
            Ok(AttemptBeginOutcome::DataUseHeld)
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

    /// An attempt repository whose claim always answers that a cap would be
    /// exceeded, modelling a send the reservation refuses.
    struct HeldByCapAttempts;

    impl InferenceAttemptRepository for HeldByCapAttempts {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn begin_inference_attempt(
            &self,
            _attempt: InferenceAttempt,
        ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
            Ok(AttemptBeginOutcome::HeldByCap)
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

    /// An attempt repository that raises the caller's abort signal from
    /// inside the claim, modelling a stop that lands while the claim commits.
    struct AbortingAttempts(DispatchAbort);

    impl InferenceAttemptRepository for AbortingAttempts {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn begin_inference_attempt(
            &self,
            _attempt: InferenceAttempt,
        ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
            self.0.abort();
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

    /// Usage repository that fails every write, modelling a storage failure
    /// at settlement time.
    struct FailingUsage;

    impl UsageRepository for FailingUsage {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn record_usage(&self, _fact: UsageFact) -> Result<(), InferenceTechnicalError> {
            Err(InferenceTechnicalError::StorageUnavailable {
                reason: String::from("usage store down"),
            })
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn reconcile_orphaned_usage_reservations(
            &self,
        ) -> Result<(), InferenceTechnicalError> {
            Err(InferenceTechnicalError::StorageUnavailable {
                reason: String::from("usage store down"),
            })
        }
    }

    /// Transport that counts calls without performing I/O.
    struct CountingTransport(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl ProviderTransport for CountingTransport {
        fn complete_streaming<'a>(
            &'a self,
            _req: ProviderRequest,
            _sink: &'a mut (dyn DeltaSink + Send),
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<ProviderResponse, InferenceTechnicalError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        fn complete_streaming<'a>(
            &'a self,
            _req: ProviderRequest,
            _sink: &'a mut (dyn DeltaSink + Send),
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<ProviderResponse, InferenceTechnicalError>>
                    + Send
                    + 'a,
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
        async fn reconcile_orphaned_usage_reservations(
            &self,
        ) -> Result<(), InferenceTechnicalError> {
            Ok(())
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
        authorized.candidate.provider_ref = provider.to_owned();
        authorized.candidate.model = model.to_owned();
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
        let held = HeldByCapAttempts;
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
    async fn provider_estimate_travels_with_the_claimed_attempt() {
        let attempts = CapturedAttempts(Mutex::new(Vec::new()));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let estimate = UsageEstimate {
            input_tokens_upper_bound: 120,
            output_tokens_upper_bound: 40,
        };
        let transport =
            FakeProviderTransport::new(String::from("ok"), None).with_usage_estimate(estimate);
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
        assert_eq!(
            claimed[0].usage_estimate,
            Some(estimate),
            "the adapter's safe bound must reach the cap-admission transaction"
        );
    }

    #[tokio::test]
    async fn unpriced_route_claims_without_a_guessed_rate() {
        let attempts = CapturedAttempts(Mutex::new(Vec::new()));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let transport = FakeProviderTransport::new(String::from("ok"), None);
        dispatch_authorized(
            authorized(),
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
        assert!(
            claimed[0].pricing.is_none(),
            "an unreviewed route must claim with no rate, never another model's"
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
    async fn usage_failure_on_completed_call_propagates_not_a_clean_success() {
        let consent = FixedConsent(Some(record(1)));
        let transport = FakeProviderTransport::new(
            String::from("hi there"),
            Some(RawUsage {
                input_tokens: 4,
                cached_input_tokens: 1,
                output_tokens: 2,
            }),
        );
        let result = dispatch_authorized(
            authorized(),
            prompt("hello").await,
            &mut DiscardSink,
            None,
            &consent,
            &StartedAttempts,
            &FailingUsage,
            &transport,
        )
        .await;
        assert!(
            result.is_err(),
            "a claimed call whose settlement fails must not answer Completed"
        );
        assert!(
            matches!(
                result,
                Err(InferenceTechnicalError::StorageUnavailable { .. })
            ),
            "the storage failure propagates: {result:?}"
        );
    }

    #[tokio::test]
    async fn usage_failure_on_transport_error_propagates_not_a_clean_technical_error() {
        let consent = FixedConsent(Some(record(1)));
        let transport = FakeProviderTransport::failing(FakeFailure::ResponseLost);
        let result = dispatch_authorized(
            authorized(),
            prompt("hello").await,
            &mut DiscardSink,
            None,
            &consent,
            &StartedAttempts,
            &FailingUsage,
            &transport,
        )
        .await;
        // The transport error already proves the call may have run, so losing
        // its accounting too would report a technical failure with no durable
        // fact; the settlement failure takes precedence.
        assert!(
            matches!(
                result,
                Err(InferenceTechnicalError::StorageUnavailable { .. })
            ),
            "the usage failure must not be swallowed by the transport error: {result:?}"
        );
    }

    #[tokio::test]
    async fn never_sent_records_no_fact() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let transport = FakeProviderTransport::new(String::from("hi"), None);
        let attempts = RecordingAttempts(Mutex::new(0));
        let result = dispatch_authorized(
            authorized(),
            prompt("x".repeat(MAX_INPUT_CHARS + 1)).await,
            &mut DiscardSink,
            None,
            &consent,
            &attempts,
            &usage,
            &transport,
        )
        .await;
        assert_eq!(
            result.expect("dispatch answers an outcome"),
            InferenceDispatchOutcome::NotSent(NotSentReason::OverLimit)
        );
        assert_eq!(
            *attempts.0.lock().expect("attempt count lock"),
            0,
            "an over-limit input never claims a durable attempt"
        );
        assert!(
            usage.0.lock().expect("usage capture lock").is_empty(),
            "a definitely-never-sent call spends nothing"
        );
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
    async fn missing_usage_maps_to_unknown_not_zero() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let transport = FakeProviderTransport::new(String::from("hi there"), None);
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
        let InferenceDispatchOutcome::Completed { arrival, .. } = outcome else {
            panic!("a provider success completes");
        };
        assert_eq!(arrival.output_text, "hi there");
        assert_eq!(arrival.usage.input_tokens, None);
        assert_eq!(arrival.usage.cached_input_tokens, None);
        assert_eq!(arrival.usage.output_tokens, None);
        assert_eq!(arrival.usage.source, UsageSource::Unknown);
    }

    #[tokio::test]
    async fn explicit_zero_cached_tokens_stays_reported() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        // A provider that decodes cache detail and reports zero hits yields a
        // legitimate reported fact: zero here is evidence, not a filler.
        let transport = FakeProviderTransport::new(
            String::from("hi there"),
            Some(RawUsage {
                input_tokens: 7,
                cached_input_tokens: 0,
                output_tokens: 3,
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
        let InferenceDispatchOutcome::Completed { arrival, .. } = outcome else {
            panic!("a provider success completes");
        };
        assert_eq!(arrival.usage.source, UsageSource::Reported);
        assert_eq!(arrival.usage.cached_input_tokens, Some(0));
    }

    #[tokio::test]
    async fn lost_response_records_unknown_counts() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let transport = FakeProviderTransport::failing(FakeFailure::ResponseLost);
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
        assert!(matches!(result, Err(InferenceTechnicalError::ResponseLost)));
        let facts = usage.0.lock().expect("usage capture lock");
        assert_eq!(
            facts.len(),
            1,
            "a lost response may have run, so it records an unknown fact"
        );
        assert_eq!(facts[0].source, UsageSource::Unknown);
        assert_eq!(facts[0].input_tokens, None);
        assert_eq!(facts[0].cached_input_tokens, None);
        assert_eq!(facts[0].output_tokens, None);
    }

    #[tokio::test]
    async fn task_premise_stale_never_calls_the_provider() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport = CountingTransport(std::sync::Arc::clone(&calls));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let outcome = dispatch_authorized(
            authorized_task_agent(task_agent_premise()),
            prompt("delegated work").await,
            &mut DiscardSink,
            None,
            &consent,
            &TaskPremiseStaleAttempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        assert_eq!(
            outcome,
            InferenceDispatchOutcome::NotSent(NotSentReason::TaskPremiseStale),
            "a moved task premise is a task-stale refusal, not consent staleness"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a task-stale claim must not reach the provider"
        );
        assert!(usage.0.lock().expect("usage capture lock").is_empty());
    }

    #[tokio::test]
    async fn data_use_hold_never_calls_the_provider_and_records_no_fact() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport = CountingTransport(std::sync::Arc::clone(&calls));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let outcome = dispatch_authorized(
            authorized_task_agent(task_agent_premise()),
            prompt("delegated work").await,
            &mut DiscardSink,
            None,
            &consent,
            &DataUseHeldAttempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        assert_eq!(
            outcome,
            InferenceDispatchOutcome::NotSent(NotSentReason::DataUseHeld),
            "a covered source is a data-use hold, not consent or task staleness"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a data-use hold sends zero bytes"
        );
        assert!(usage.0.lock().expect("usage capture lock").is_empty());
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
    async fn dialogue_claim_carries_no_task_agent_correlation() {
        let attempts = CapturedAttempts(Mutex::new(Vec::new()));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let transport = FakeProviderTransport::new(String::from("ok"), None);
        dispatch_authorized(
            authorized(),
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
        assert_eq!(claimed[0].task_agent, None);
    }

    #[tokio::test]
    async fn abort_before_the_claim_claims_nothing_and_records_nothing() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let attempts = RecordingAttempts(Mutex::new(0));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport = CountingTransport(std::sync::Arc::clone(&calls));
        let abort = DispatchAbort::default();
        abort.abort();

        let outcome = dispatch_authorized(
            authorized(),
            prompt("hello").await,
            &mut DiscardSink,
            Some(&abort),
            &consent,
            &attempts,
            &usage,
            &transport,
        )
        .await
        .expect("an abort is a domain outcome");

        assert_eq!(outcome, InferenceDispatchOutcome::Aborted);
        assert_eq!(
            *attempts.0.lock().expect("attempt count lock"),
            0,
            "a pre-claim abort never claims an attempt"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a pre-claim abort never reaches the provider"
        );
        assert!(
            usage.0.lock().expect("usage capture lock").is_empty(),
            "a never-claimed use records no usage fact"
        );
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

    #[tokio::test]
    async fn abort_that_lands_during_the_claim_records_the_unknown_fact() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport = CountingTransport(std::sync::Arc::clone(&calls));
        let abort = DispatchAbort::default();
        let attempts = AbortingAttempts(abort.clone());
        let authorized = authorized();
        let ticket = authorized.ticket;

        let outcome = dispatch_authorized(
            authorized,
            prompt("hello").await,
            &mut DiscardSink,
            Some(&abort),
            &consent,
            &attempts,
            &usage,
            &transport,
        )
        .await
        .expect("an abort is a domain outcome");

        assert_eq!(outcome, InferenceDispatchOutcome::Aborted);
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the stop wins before the provider future is ever polled"
        );
        let facts = usage.0.lock().expect("usage capture lock");
        assert_eq!(
            facts.len(),
            1,
            "the claim that raced the stop still records its accounting"
        );
        assert_eq!(facts[0].ticket, ticket);
        assert_eq!(facts[0].source, UsageSource::Unknown);
    }

    #[tokio::test]
    async fn abort_accounting_failure_is_a_technical_error_not_a_clean_abort() {
        let consent = FixedConsent(Some(record(1)));
        let transport = DroppingTransport::default();
        let abort = DispatchAbort::default();
        let started = std::sync::Arc::clone(&transport.started);
        let dropped = std::sync::Arc::clone(&transport.dropped);
        let mut sink = DiscardSink;

        let mut dispatch = Box::pin(dispatch_authorized(
            authorized(),
            prompt("hello").await,
            &mut sink,
            Some(&abort),
            &consent,
            &StartedAttempts,
            &FailingUsage,
            &transport,
        ));
        let mut started_wait = Box::pin(started.notified());
        tokio::select! {
            () = &mut started_wait => {}
            outcome = &mut dispatch => {
                panic!("the dispatch must wait for the abort, got {outcome:?}");
            }
        }
        abort.abort();
        let result = dispatch.await;
        assert!(
            matches!(
                result,
                Err(InferenceTechnicalError::StorageUnavailable { .. })
            ),
            "a claimed abort whose accounting cannot be written must not answer a clean Aborted"
        );
        assert!(
            dropped.load(std::sync::atomic::Ordering::SeqCst),
            "the abort still dropped the in-flight provider future"
        );
    }

    #[test]
    fn debug_redacts_output_text() {
        let ticket = InferenceTicketId(RawId::new());
        let arrival = InferenceResultArrival {
            ticket,
            output_text: String::from("harbor-sunset-output-probe"),
            usage: UsageFact {
                ticket,
                provider: String::from("acme"),
                model: String::from("dialogue-1"),
                input_tokens: None,
                cached_input_tokens: None,
                output_tokens: None,
                source: UsageSource::Unknown,
            },
        };
        let rendered = format!("{arrival:?}");
        assert!(!rendered.contains("harbor-sunset-output-probe"));
    }
}

#[cfg(test)]
mod admission_tests {
    use super::{
        Admission, InferenceTechnicalError, NotSentReason, PreparedAdmission,
        TaskAgentAttemptPremise, prepare_dialogue_admission, prepare_learning_admission,
        prepare_task_agent_admission,
    };
    use ene_credential::{
        CredentialRef, CredentialRefRepository, CredentialTechnicalError, MemoryCredentialStore,
    };
    use ene_permission::{
        CapabilityKind, ConsentRecord, ConsentRepository, ConsentRevision, PermissionTechnicalError,
    };
    use ene_primitive::{RawId, RevisionInner};

    fn record_for(capability: CapabilityKind) -> ConsentRecord {
        ConsentRecord {
            capability,
            id: String::from("consent-1"),
            rev: ConsentRevision::from_u64(1),
            provider: String::from("acme"),
            model: String::from("shared-1"),
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

    struct FixedRefs(Vec<CredentialRef>);

    impl CredentialRefRepository for FixedRefs {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
            Ok(self.0.clone())
        }
    }

    fn provisioned() -> (MemoryCredentialStore, CredentialRef) {
        let store = MemoryCredentialStore::new();
        let credential = CredentialRef::new("acme", "main").expect("valid test fixture");
        store.insert(credential.clone(), "sk-test-only");
        (store, credential)
    }

    #[tokio::test]
    async fn learning_admission_resolves_the_learning_assignment() {
        let (credential_store, credential) = provisioned();
        let consent = FixedConsent(Some(record_for(CapabilityKind::Learning)));
        let refs = FixedRefs(vec![credential]);
        let prepared =
            prepare_learning_admission(&consent, &refs, &credential_store, Vec::new()).await;
        let PreparedAdmission::Ready(request) = prepared.expect("preparation answers") else {
            panic!("a complete setup must prepare a learning admission");
        };
        let mut tracker = ene_permission::EvaluationTracker::new();
        let Admission::Admitted(authorized) = request.authorize(&mut tracker) else {
            panic!("the learning candidate is inside the closed world");
        };
        assert!(authorized.consent_premise().0 == "consent-1");
    }

    #[tokio::test]
    async fn dialogue_consent_does_not_prepare_a_learning_admission() {
        // Same provider, model, credential, and bearer: only the capability
        // differs, and capability-scoped consent must not be borrowed.
        let (credential_store, credential) = provisioned();
        let consent = FixedConsent(Some(record_for(CapabilityKind::Dialogue)));
        let refs = FixedRefs(vec![credential]);
        let prepared =
            prepare_learning_admission(&consent, &refs, &credential_store, Vec::new()).await;
        assert_eq!(
            prepared,
            Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete)),
            "a dialogue assignment never authorizes learning formation"
        );
    }

    #[tokio::test]
    async fn task_agent_admission_inherits_the_dialogue_consent() {
        let (credential_store, credential) = provisioned();
        let consent = FixedConsent(Some(record_for(CapabilityKind::Dialogue)));
        let refs = FixedRefs(vec![credential]);
        let premise = TaskAgentAttemptPremise {
            delegation: RawId::new(),
            task: RawId::new(),
            task_revision: RevisionInner::from_u64(1),
            data_use: vec![RawId::new()],
        };
        let prepared =
            prepare_task_agent_admission(&consent, &refs, &credential_store, premise.clone()).await;
        let PreparedAdmission::Ready(request) = prepared.expect("preparation answers") else {
            panic!("a complete setup must prepare a task agent admission");
        };
        let mut tracker = ene_permission::EvaluationTracker::new();
        let Admission::Admitted(authorized) = request.authorize(&mut tracker) else {
            panic!("the task agent candidate is inside the closed world");
        };
        assert_eq!(authorized.task_agent.as_ref(), Some(&premise));
        assert_eq!(authorized.consent_premise().0, "consent-1");
    }

    #[tokio::test]
    async fn task_agent_admission_declines_without_the_dialogue_consent() {
        let (credential_store, credential) = provisioned();
        let consent = FixedConsent(Some(record_for(CapabilityKind::Learning)));
        let refs = FixedRefs(vec![credential]);
        let premise = TaskAgentAttemptPremise {
            delegation: RawId::new(),
            task: RawId::new(),
            task_revision: RevisionInner::from_u64(1),
            data_use: vec![RawId::new()],
        };
        let prepared =
            prepare_task_agent_admission(&consent, &refs, &credential_store, premise).await;
        assert_eq!(
            prepared,
            Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete)),
            "task agent admission inherits only the dialogue capability consent"
        );
    }

    #[tokio::test]
    async fn dialogue_admission_still_resolves_its_own_consumer() {
        let (credential_store, credential) = provisioned();
        let consent = FixedConsent(Some(record_for(CapabilityKind::Dialogue)));
        let refs = FixedRefs(vec![credential]);
        let prepared =
            prepare_dialogue_admission(&consent, &refs, &credential_store, Vec::new()).await;
        let PreparedAdmission::Ready(request) = prepared.expect("preparation answers") else {
            panic!("a complete setup must prepare a dialogue admission");
        };
        let mut tracker = ene_permission::EvaluationTracker::new();
        let Admission::Admitted(authorized) = request.authorize(&mut tracker) else {
            panic!("the dialogue candidate is inside the closed world");
        };
        assert!(authorized.consent_premise().0 == "consent-1");
    }

    #[tokio::test]
    async fn learning_admission_declines_without_setup() {
        let (credential_store, _) = provisioned();
        let refs = FixedRefs(Vec::new());
        let prepared =
            prepare_learning_admission(&FixedConsent(None), &refs, &credential_store, Vec::new())
                .await;
        assert_eq!(
            prepared,
            Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete)),
            "no stored consent declines without a tracker decision"
        );
    }

    #[tokio::test]
    async fn learning_admission_declines_when_the_bearer_is_absent() {
        let credential_store = MemoryCredentialStore::new();
        let credential = CredentialRef::new("acme", "main").expect("valid test fixture");
        let refs = FixedRefs(vec![credential]);
        let prepared = prepare_learning_admission(
            &FixedConsent(Some(record_for(CapabilityKind::Learning))),
            &refs,
            &credential_store,
            Vec::new(),
        )
        .await;
        assert_eq!(
            prepared,
            Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete)),
            "a registered ref without a bearer is incomplete setup"
        );
    }

    #[tokio::test]
    async fn learning_preparation_never_returns_inference_errors_as_outcomes() {
        // A store failure is `Err`, never a fabricated decline; both consumers
        // share the rule.
        let (credential_store, credential) = provisioned();
        let refs = FixedRefs(vec![credential]);
        let failing = FailingConsent;
        let result =
            prepare_learning_admission(&failing, &refs, &credential_store, Vec::new()).await;
        assert!(matches!(
            result,
            Err(InferenceTechnicalError::StorageUnavailable { .. })
        ));
    }

    struct FailingConsent;

    impl ConsentRepository for FailingConsent {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn load_current(
            &self,
            _capability: CapabilityKind,
        ) -> Result<Option<ConsentRecord>, PermissionTechnicalError> {
            Err(PermissionTechnicalError::StorageUnavailable {
                reason: String::from("consent store down"),
            })
        }
    }
}
