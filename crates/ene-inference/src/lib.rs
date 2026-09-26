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
    CredentialRotated,
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
    fn usage_estimate(&self, _req: &ProviderRequest) -> Option<UsageEstimate> {
        None
    }

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
    async fn record_usage(&self, fact: UsageFact) -> Result<(), InferenceTechnicalError>;

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
    HeldByCap,
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
    Admitted(Box<AuthorizedInference>),
    Declined(NotSentReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionRequest {
    candidate: InferenceUseCandidate,
    consent: ConsentRecord,
    credential: CredentialRef,
    credential_version: Option<u64>,
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
                        credential_version: self.credential_version,
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
    // Capture which published version this admission is bound to, so dispatch can
    // name the cause when the version moved, and can fail a send whose credential
    // has since gone away rather than discovering it inside the transport.
    let credential_version = credential_store.published_version(&credential).map_err(
        |CredentialTechnicalError::StorageUnavailable { reason }| {
            InferenceTechnicalError::StorageUnavailable { reason }
        },
    )?;
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
        credential_version,
        task_agent,
        data_use,
    })))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedInference {
    ticket: InferenceTicketId,
    consent: (String, ConsentRevision),
    credential: CredentialRef,
    credential_version: Option<u64>,
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

struct DispatchAbortInner {
    aborted: std::sync::atomic::AtomicBool,
    notify: tokio::sync::watch::Sender<bool>,
    children: std::sync::Mutex<Vec<std::sync::Weak<DispatchAbortInner>>>,
}

impl Default for DispatchAbortInner {
    fn default() -> Self {
        let (notify, _) = tokio::sync::watch::channel(false);
        Self {
            aborted: std::sync::atomic::AtomicBool::new(false),
            notify,
            children: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl DispatchAbort {
    /// Returns a child signal that aborts when either source aborts.
    #[must_use]
    pub fn linked_to(&self, other: &Self) -> Self {
        let linked = Self::default();
        self.inner.link(&linked.inner);
        other.inner.link(&linked.inner);
        linked
    }

    pub fn abort(&self) {
        DispatchAbortInner::abort(std::sync::Arc::clone(&self.inner));
    }

    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.inner.aborted.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub async fn aborted(&self) {
        let mut notify = self.inner.notify.subscribe();
        loop {
            let observed = *notify.borrow_and_update();
            if observed || self.is_aborted() {
                return;
            }
            if notify.changed().await.is_err() {
                return;
            }
        }
    }
}

impl DispatchAbortInner {
    fn link(self: &std::sync::Arc<Self>, child: &std::sync::Arc<Self>) {
        let abort_child = {
            let mut children = self
                .children
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            children.retain(|entry| entry.strong_count() != 0);
            children.push(std::sync::Arc::downgrade(child));
            self.aborted.load(std::sync::atomic::Ordering::SeqCst)
        };
        if abort_child {
            Self::abort(std::sync::Arc::clone(child));
        }
    }

    fn abort(self: std::sync::Arc<Self>) {
        if self.aborted.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let _previous = self.notify.send_replace(true);
        let children = {
            let mut children = self
                .children
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *children)
        };
        for child in children
            .into_iter()
            .filter_map(|entry| std::sync::Weak::upgrade(&entry))
        {
            Self::abort(child);
        }
    }
}

pub type InferenceClaimFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = tokio::sync::OwnedMutexGuard<()>> + Send + 'static>,
>;

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

    async fn dispatch_with_claim_scope(
        &self,
        authorized: AuthorizedInference,
        prompt: ScrubbedText,
        sink: &mut (dyn DeltaSink + Send),
        abort: Option<&DispatchAbort>,
        acquire_claim_scope: Box<dyn FnOnce() -> InferenceClaimFuture + Send>,
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
    credential_store: &impl CredentialStore,
    consent: &impl ConsentRepository,
    attempts: &impl InferenceAttemptRepository,
    usage: &impl UsageRepository,
    transport: &impl ProviderTransport,
) -> Result<InferenceDispatchOutcome, InferenceTechnicalError> {
    dispatch_authorized_inner(
        authorized,
        prompt,
        sink,
        abort,
        None,
        credential_store,
        consent,
        attempts,
        usage,
        transport,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "the owner boundary takes each repository it must sequence in order; a parameter struct would only restate the same wiring"
)]
pub async fn dispatch_authorized_with_claim_scope(
    authorized: AuthorizedInference,
    prompt: ScrubbedText,
    sink: &mut (dyn DeltaSink + Send),
    abort: Option<&DispatchAbort>,
    acquire_claim_scope: Box<dyn FnOnce() -> InferenceClaimFuture + Send>,
    credential_store: &impl CredentialStore,
    consent: &impl ConsentRepository,
    attempts: &impl InferenceAttemptRepository,
    usage: &impl UsageRepository,
    transport: &impl ProviderTransport,
) -> Result<InferenceDispatchOutcome, InferenceTechnicalError> {
    dispatch_authorized_inner(
        authorized,
        prompt,
        sink,
        abort,
        Some(acquire_claim_scope),
        credential_store,
        consent,
        attempts,
        usage,
        transport,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "the owner boundary takes each repository it must sequence in order; a parameter struct would only restate the same wiring"
)]
async fn dispatch_authorized_inner(
    authorized: AuthorizedInference,
    prompt: ScrubbedText,
    sink: &mut (dyn DeltaSink + Send),
    abort: Option<&DispatchAbort>,
    acquire_claim_scope: Option<Box<dyn FnOnce() -> InferenceClaimFuture + Send>>,
    credential_store: &impl CredentialStore,
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
    let admitted_credential_version = authorized.credential_version;
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
    let claim_scope = if let Some(acquire) = acquire_claim_scope {
        let scope = acquire().await;
        if abort.is_some_and(DispatchAbort::is_aborted) {
            return Ok(InferenceDispatchOutcome::Aborted);
        }
        Some(scope)
    } else {
        None
    };
    // If the credential rotated after this dispatch was admitted, the value the
    // transport would read now is one the consent, permission and cost checks
    // never saw. Refuse before the claim so the refusal stays a pre-claim
    // NotSent: no attempt row and no usage reservation are created for a send
    // that is provably 0 bytes.
    let current_credential_version = credential_store
        .published_version(&request.credential)
        .map_err(|CredentialTechnicalError::StorageUnavailable { reason }| {
            InferenceTechnicalError::StorageUnavailable { reason }
        })?;
    if current_credential_version != admitted_credential_version {
        return Ok(InferenceDispatchOutcome::NotSent(
            NotSentReason::CredentialRotated,
        ));
    }
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
    drop(claim_scope);
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

#[cfg(test)]
mod fake {
    use std::future::Future;
    use std::pin::Pin;

    use super::RawUsage;
    use super::{
        DeltaFlow, DeltaSink, InferenceTechnicalError, ProviderRequest, ProviderResponse,
        ProviderTransport, UsageEstimate,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum FakeFailure {
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
    use std::sync::{Arc, Mutex};

    use super::fake::{FakeFailure, FakeProviderTransport};
    use super::{
        Admission, AttemptBeginOutcome, AuthorizedInference, DeltaSink, DiscardSink, DispatchAbort,
        InferenceAttempt, InferenceAttemptRecord, InferenceAttemptRepository,
        InferenceDispatchOutcome, InferenceResultArrival, InferenceTechnicalError,
        InferenceTicketId, MAX_INPUT_CHARS, NotSentReason, PreparedAdmission, ProviderRequest,
        ProviderResponse, ProviderTransport, RawUsage, TaskAgentAttemptPremise, UsageEstimate,
        UsageFact, UsageRepository, UsageSource, dispatch_authorized, prepare_dialogue_admission,
        unknown_usage,
    };
    use ene_credential::{CredentialRef, CredentialSetRevision, ScrubbedText};
    use ene_permission::{
        CapabilityKind, ConsentRecord, ConsentRepository, ConsentRevision, ConsumerKind,
        EvaluationTracker, InferenceUseCandidate, PermissionTechnicalError, PurposeKind,
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

    struct RefusedAttempts {
        outcome: AttemptBeginOutcome,
        begin_calls: Mutex<usize>,
    }

    impl RefusedAttempts {
        fn new(outcome: AttemptBeginOutcome) -> Self {
            Self {
                outcome,
                begin_calls: Mutex::new(0),
            }
        }
    }

    impl InferenceAttemptRepository for RefusedAttempts {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn begin_inference_attempt(
            &self,
            _attempt: InferenceAttempt,
        ) -> Result<AttemptBeginOutcome, InferenceTechnicalError> {
            *self.begin_calls.lock().expect("attempt call lock") += 1;
            Ok(self.outcome.clone())
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

    struct FailingUsage(Mutex<usize>);

    impl UsageRepository for FailingUsage {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn record_usage(&self, _fact: UsageFact) -> Result<(), InferenceTechnicalError> {
            *self.0.lock().expect("usage call lock") += 1;
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

    struct OneRef(ene_credential::CredentialRef);

    impl ene_credential::CredentialRefRepository for OneRef {
        #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
        async fn list_refs(
            &self,
        ) -> Result<Vec<ene_credential::CredentialRef>, ene_credential::CredentialTechnicalError>
        {
            Ok(vec![self.0.clone()])
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

    /// A store holding the credential `authorized()` admits against. Each call
    /// builds a fresh one so no test can contaminate another through shared
    /// interior state.
    fn creds() -> ene_credential::MemoryCredentialStore {
        let store = ene_credential::MemoryCredentialStore::new();
        store.insert(
            CredentialRef::new("acme", "main").expect("valid test fixture"),
            "test-bearer",
        );
        store
    }

    fn authorized() -> AuthorizedInference {
        let consent = record(1);
        AuthorizedInference {
            ticket: InferenceTicketId(RawId::new()),
            consent: (consent.id, consent.rev),
            credential: CredentialRef::new("acme", "main").expect("valid test fixture"),
            credential_version: None,
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
            credential_version: None,
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

    /// Admission must record the version the store actually has, not a constant.
    /// If that capture were dropped or hardcoded, every versioned send would be
    /// refused while this suite stayed green, so the capture and the positive
    /// case are both pinned here: the admitted object carries the store's
    /// version, and an unrotated dispatch still sends.
    #[tokio::test]
    async fn admission_captures_the_stores_published_version() {
        use ene_credential::{
            CredentialStore as _, MemoryVersionedStore, VersionedCredentialStore,
        };

        let cred = CredentialRef::new("acme", "main").expect("valid test fixture");
        let store = MemoryVersionedStore::new();
        store
            .put_version(&cred, 7, "value")
            .expect("candidate version");
        store.activate(store.prepare_snapshot(&cred, 7).expect("snapshot"));
        let published = store
            .published_version(&cred)
            .expect("the store reports its version");

        let prepared = prepare_dialogue_admission(
            &FixedConsent(Some(record(1))),
            &OneRef(cred.clone()),
            &store,
            Vec::new(),
        )
        .await
        .expect("admission prepares");
        let PreparedAdmission::Ready(request) = prepared else {
            panic!("a stored credential must admit, got {prepared:?}");
        };
        let mut tracker = EvaluationTracker::default();
        let Admission::Admitted(authorized) = request.authorize(&mut tracker) else {
            panic!("the fixture must authorize the admitted request");
        };
        assert_eq!(
            authorized.credential_version, published,
            "the admitted object must carry the version the store published"
        );

        let attempts = CapturedAttempts(Mutex::new(Vec::new()));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport = CountingTransport(std::sync::Arc::clone(&calls));
        let outcome = dispatch_authorized(
            *authorized,
            prompt("hello").await,
            &mut DiscardSink,
            None,
            &store,
            &FixedConsent(Some(record(1))),
            &attempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        assert!(
            matches!(outcome, InferenceDispatchOutcome::Completed { .. }),
            "an unrotated versioned credential must still send, got {outcome:?}"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the provider must be contacted when nothing rotated"
        );
    }

    /// A rotation between admission and the provider call must stop the send.
    /// Authenticating with the new value would use a secret the consent,
    /// permission and cost checks never saw, so the dispatch reports a distinct
    /// not-sent outcome and the provider is never contacted at all.
    #[tokio::test]
    async fn a_rotation_after_admission_refuses_the_send() {
        use ene_credential::{
            CredentialStore as _, MemoryVersionedStore, VersionedCredentialStore,
        };

        let cred = CredentialRef::new("acme", "main").expect("valid test fixture");
        let store = MemoryVersionedStore::new();
        for version in [1_u64, 2] {
            store
                .put_version(&cred, version, "value")
                .expect("candidate version");
        }
        store.activate(store.prepare_snapshot(&cred, 1).expect("snapshot"));

        let mut authorized = authorized();
        authorized.credential = cred.clone();
        authorized.credential_version = store
            .published_version(&cred)
            .expect("the store reports its published version");

        // The rotation lands after the admission captured its version.
        store.activate(store.prepare_snapshot(&cred, 2).expect("snapshot"));

        let attempts = CapturedAttempts(Mutex::new(Vec::new()));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport = CountingTransport(std::sync::Arc::clone(&calls));
        let outcome = dispatch_authorized(
            authorized,
            prompt("hello").await,
            &mut DiscardSink,
            None,
            &store,
            &FixedConsent(Some(record(1))),
            &attempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");

        assert_eq!(
            outcome,
            InferenceDispatchOutcome::NotSent(NotSentReason::CredentialRotated),
            "a rotated credential must refuse the send rather than authenticate with the new value"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the provider must not be contacted under a rotated credential"
        );
        assert!(
            attempts.0.lock().expect("attempt capture lock").is_empty(),
            "a pre-claim refusal must not create an attempt row"
        );
        assert!(
            usage.0.lock().expect("usage capture lock").is_empty(),
            "a pre-claim refusal must not record usage or leave a reservation"
        );
    }

    #[tokio::test]
    async fn claimed_attempt_carries_route_pricing_and_estimate() {
        let consent = FixedConsent(Some(record(1)));

        let attempts = CapturedAttempts(Mutex::new(Vec::new()));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let transport = FakeProviderTransport::new(String::from("ok"), None);
        dispatch_authorized(
            authorized_route("openai", "gpt-4o"),
            prompt("hello").await,
            &mut DiscardSink,
            None,
            &creds(),
            &consent,
            &attempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        let claimed = attempts.0.lock().expect("attempt capture lock").clone();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].usage_estimate, None);
        let pricing = claimed[0]
            .pricing
            .as_ref()
            .expect("a reviewed route claims under its snapshot");
        assert_eq!(pricing.provider, "openai");
        assert_eq!(pricing.model, "gpt-4o");
        assert_eq!(pricing.currency, crate::cost::CurrencyCode::Usd);
        assert_eq!(pricing.input_rate.micros_per_million(), 2_500_000);
        assert_eq!(pricing.cached_input_rate.micros_per_million(), 1_250_000);
        assert_eq!(pricing.output_rate.micros_per_million(), 10_000_000);
        assert_eq!(
            pricing.source_revision,
            crate::pricing::FIRST_PARTY_REVISION
        );

        let attempts = CapturedAttempts(Mutex::new(Vec::new()));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
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
            &creds(),
            &consent,
            &attempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        let claimed = attempts.0.lock().expect("attempt capture lock").clone();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].usage_estimate, Some(estimate));
        assert!(claimed[0].pricing.is_some());

        let attempts = CapturedAttempts(Mutex::new(Vec::new()));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let transport = FakeProviderTransport::new(String::from("ok"), None);
        dispatch_authorized(
            authorized(),
            prompt("hello").await,
            &mut DiscardSink,
            None,
            &creds(),
            &consent,
            &attempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        let claimed = attempts.0.lock().expect("attempt capture lock").clone();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].pricing, None);
        assert_eq!(claimed[0].usage_estimate, None);
    }

    #[tokio::test]
    async fn pre_send_refusals_keep_typed_authority_and_zero_side_effects() {
        #[derive(Clone, Copy)]
        enum Case {
            OverLimit,
            ConsentStale,
            TaskPremiseStale,
            DataUseHeld,
            UsageCapReached,
            UsageCapIndeterminate,
        }

        for case in [
            Case::OverLimit,
            Case::ConsentStale,
            Case::TaskPremiseStale,
            Case::DataUseHeld,
            Case::UsageCapReached,
            Case::UsageCapIndeterminate,
        ] {
            let attempts = RefusedAttempts::new(match case {
                Case::OverLimit => AttemptBeginOutcome::Started,
                Case::ConsentStale => AttemptBeginOutcome::Stale,
                Case::TaskPremiseStale => AttemptBeginOutcome::TaskPremiseStale,
                Case::DataUseHeld => AttemptBeginOutcome::DataUseHeld,
                Case::UsageCapReached => AttemptBeginOutcome::HeldByCap,
                Case::UsageCapIndeterminate => AttemptBeginOutcome::CapIndeterminate,
            });
            let usage = CapturedUsage(Mutex::new(Vec::new()));
            let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let transport = CountingTransport(std::sync::Arc::clone(&calls));
            let consent = FixedConsent(Some(record(1)));
            let (authorized, input, expected, expected_begin_calls) = match case {
                Case::OverLimit => (
                    authorized(),
                    "x".repeat(MAX_INPUT_CHARS + 1),
                    NotSentReason::OverLimit,
                    0,
                ),
                Case::ConsentStale => (
                    authorized(),
                    String::from("the key is sk-new"),
                    NotSentReason::ConsentStale,
                    1,
                ),
                Case::TaskPremiseStale => (
                    authorized_task_agent(task_agent_premise()),
                    String::from("delegated work"),
                    NotSentReason::TaskPremiseStale,
                    1,
                ),
                Case::DataUseHeld => (
                    authorized_task_agent(task_agent_premise()),
                    String::from("delegated work"),
                    NotSentReason::DataUseHeld,
                    1,
                ),
                Case::UsageCapReached => (
                    authorized(),
                    String::from("hello"),
                    NotSentReason::UsageCapReached,
                    1,
                ),
                Case::UsageCapIndeterminate => (
                    authorized(),
                    String::from("hello"),
                    NotSentReason::UsageCapIndeterminate,
                    1,
                ),
            };
            let outcome = dispatch_authorized(
                authorized,
                prompt(input).await,
                &mut DiscardSink,
                None,
                &creds(),
                &consent,
                &attempts,
                &usage,
                &transport,
            )
            .await
            .expect("dispatch answers an outcome");
            assert_eq!(outcome, InferenceDispatchOutcome::NotSent(expected));
            let begin_calls = *attempts.begin_calls.lock().expect("attempt call lock");
            assert_eq!(begin_calls, expected_begin_calls);
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
            let recorded = usage.0.lock().expect("usage capture lock").clone();
            assert!(recorded.is_empty());
        }
    }

    #[tokio::test]
    async fn usage_accounting_failure_preempts_underlying_outcome() {
        #[derive(Clone, Copy)]
        enum Underlying {
            Completed,
            ResponseLost,
        }

        for underlying in [Underlying::Completed, Underlying::ResponseLost] {
            let consent = FixedConsent(Some(record(1)));
            let usage = FailingUsage(Mutex::new(0));
            let transport = match underlying {
                Underlying::Completed => FakeProviderTransport::new(
                    String::from("hi there"),
                    Some(RawUsage {
                        input_tokens: 4,
                        cached_input_tokens: 1,
                        output_tokens: 2,
                    }),
                ),
                Underlying::ResponseLost => {
                    FakeProviderTransport::failing(FakeFailure::ResponseLost)
                }
            };
            let result = dispatch_authorized(
                authorized(),
                prompt("hello").await,
                &mut DiscardSink,
                None,
                &creds(),
                &consent,
                &StartedAttempts,
                &usage,
                &transport,
            )
            .await;
            assert!(matches!(
                result,
                Err(InferenceTechnicalError::StorageUnavailable { .. })
            ));
            let usage_calls = *usage.0.lock().expect("usage call lock");
            assert_eq!(usage_calls, 1);
        }
    }

    #[tokio::test]
    async fn completed_usage_survives_adoption_movement_and_missing_stays_unknown() {
        for (consent_revision, raw_usage, expected_source, expected_counts, expected_adopted) in [
            (
                2,
                Some(RawUsage {
                    input_tokens: 4,
                    cached_input_tokens: 1,
                    output_tokens: 2,
                }),
                UsageSource::Reported,
                Some((4, 1, 2)),
                false,
            ),
            (1, None, UsageSource::Unknown, None, true),
            (
                1,
                Some(RawUsage {
                    input_tokens: 7,
                    cached_input_tokens: 0,
                    output_tokens: 3,
                }),
                UsageSource::Reported,
                Some((7, 0, 3)),
                true,
            ),
        ] {
            let usage = CapturedUsage(Mutex::new(Vec::new()));
            let consent = FixedConsent(Some(record(consent_revision)));
            let transport = FakeProviderTransport::new(String::from("hi there"), raw_usage);
            let outcome = dispatch_authorized(
                authorized(),
                prompt("hello").await,
                &mut DiscardSink,
                None,
                &creds(),
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
            assert_eq!(arrival.output_text, "hi there");
            assert_eq!(arrival.usage.source, expected_source);
            assert_eq!(adopted, expected_adopted);
            match expected_counts {
                Some((input, cached, output)) => {
                    assert_eq!(arrival.usage.input_tokens, Some(input));
                    assert_eq!(arrival.usage.cached_input_tokens, Some(cached));
                    assert_eq!(arrival.usage.output_tokens, Some(output));
                }
                None => {
                    assert_eq!(arrival.usage.input_tokens, None);
                    assert_eq!(arrival.usage.cached_input_tokens, None);
                    assert_eq!(arrival.usage.output_tokens, None);
                }
            }
            let facts = usage.0.lock().expect("usage capture lock").clone();
            assert_eq!(facts.as_slice(), &[arrival.usage]);
        }
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
            &creds(),
            &consent,
            &StartedAttempts,
            &usage,
            &transport,
        )
        .await;
        assert!(matches!(result, Err(InferenceTechnicalError::ResponseLost)));
        let facts = usage.0.lock().expect("usage capture lock").clone();
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
    async fn claim_attribution_matches_consumer_and_task_correlation() {
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
            &creds(),
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
        let claimed = attempts.0.lock().expect("attempt capture lock").clone();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].consumer, ConsumerKind::TaskAgent);
        assert_eq!(claimed[0].purpose, PurposeKind::TaskAgentTurn);
        assert_eq!(claimed[0].capability, CapabilityKind::Dialogue);
        assert_eq!(claimed[0].task_agent, Some(premise));
        let facts = usage.0.lock().expect("usage capture lock").clone();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].ticket, claimed[0].ticket);
        assert_eq!(facts[0].source, UsageSource::Unknown);
        assert_eq!(facts[0].input_tokens, None);
        assert_eq!(facts[0].cached_input_tokens, None);
        assert_eq!(facts[0].output_tokens, None);
        let attempts = CapturedAttempts(Mutex::new(Vec::new()));
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let transport = FakeProviderTransport::new(String::from("ok"), None);
        dispatch_authorized(
            authorized(),
            prompt("hello").await,
            &mut DiscardSink,
            None,
            &creds(),
            &consent,
            &attempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        let claimed = attempts.0.lock().expect("attempt capture lock").clone();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].consumer, ConsumerKind::CompanionDialogue);
        assert_eq!(claimed[0].purpose, PurposeKind::DialogueResponse);
        assert_eq!(claimed[0].capability, CapabilityKind::Dialogue);
        assert_eq!(claimed[0].task_agent, None);
    }

    #[tokio::test]
    async fn one_abort_wakes_every_registered_waiter() {
        let abort = DispatchAbort::default();
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
        let first = {
            let abort = abort.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                abort.aborted().await;
            })
        };
        let second = {
            let abort = abort.clone();
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                abort.aborted().await;
            })
        };
        barrier.wait().await;
        abort.abort();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            first.await.expect("first abort waiter joins");
            second.await.expect("second abort waiter joins");
        })
        .await
        .expect("both waiters must wake");
    }

    #[test]
    fn linked_abort_propagates_without_relabelling_its_source() {
        let task_cancel = DispatchAbort::default();
        let host_shutdown = DispatchAbort::default();
        let dispatch_abort = task_cancel.linked_to(&host_shutdown);

        host_shutdown.abort();
        assert!(dispatch_abort.is_aborted());
        assert!(!task_cancel.is_aborted());

        let other_cancel = DispatchAbort::default();
        let other_dispatch_abort = other_cancel.linked_to(&DispatchAbort::default());
        other_cancel.abort();
        assert!(other_dispatch_abort.is_aborted());
    }

    #[tokio::test]
    async fn abort_before_the_claim_claims_nothing_and_records_nothing() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let attempts = RecordingAttempts(Mutex::new(0));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport = CountingTransport(std::sync::Arc::clone(&calls));
        let task_cancel = DispatchAbort::default();
        let host_shutdown = DispatchAbort::default();
        let abort = task_cancel.linked_to(&host_shutdown);
        host_shutdown.abort();

        let outcome = dispatch_authorized(
            authorized(),
            prompt("hello").await,
            &mut DiscardSink,
            Some(&abort),
            &creds(),
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
    async fn abort_after_claim_records_unknown_and_stops_or_skips_transport() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let transport = DroppingTransport::default();
        let abort = DispatchAbort::default();
        let started = std::sync::Arc::clone(&transport.started);
        let dropped = std::sync::Arc::clone(&transport.dropped);
        let authorized_input = authorized();
        let ticket = authorized_input.ticket;
        let mut sink = DiscardSink;

        let cred_store = creds();
        let mut dispatch = Box::pin(dispatch_authorized(
            authorized_input,
            prompt("hello").await,
            &mut sink,
            Some(&abort),
            &cred_store,
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
        assert!(!dropped.load(std::sync::atomic::Ordering::SeqCst));
        abort.abort();
        let outcome = dispatch.await.expect("the abort answers an outcome");
        assert_eq!(outcome, InferenceDispatchOutcome::Aborted);
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
        let facts = usage.0.lock().expect("usage capture lock").clone();
        assert_eq!(
            facts.as_slice(),
            &[unknown_usage(ticket, "acme", "dialogue-1")]
        );

        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let transport = CountingTransport(std::sync::Arc::clone(&calls));
        let abort = DispatchAbort::default();
        let attempts = AbortingAttempts(abort.clone());
        let authorized_input = authorized();
        let ticket = authorized_input.ticket;
        let outcome = dispatch_authorized(
            authorized_input,
            prompt("hello").await,
            &mut DiscardSink,
            Some(&abort),
            &creds(),
            &consent,
            &attempts,
            &usage,
            &transport,
        )
        .await
        .expect("an abort is a domain outcome");
        assert_eq!(outcome, InferenceDispatchOutcome::Aborted);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        let facts = usage.0.lock().expect("usage capture lock").clone();
        assert_eq!(
            facts.as_slice(),
            &[unknown_usage(ticket, "acme", "dialogue-1")]
        );
    }

    #[tokio::test]
    async fn abort_accounting_failure_is_a_technical_error_not_a_clean_abort() {
        let consent = FixedConsent(Some(record(1)));
        let usage = FailingUsage(Mutex::new(0));
        let transport = DroppingTransport::default();
        let abort = DispatchAbort::default();
        let started = std::sync::Arc::clone(&transport.started);
        let dropped = std::sync::Arc::clone(&transport.dropped);
        let mut sink = DiscardSink;

        let cred_store = creds();
        let mut dispatch = Box::pin(dispatch_authorized(
            authorized(),
            prompt("hello").await,
            &mut sink,
            Some(&abort),
            &cred_store,
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
        abort.abort();
        let result = dispatch.await;
        assert!(matches!(
            result,
            Err(InferenceTechnicalError::StorageUnavailable { .. })
        ));
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(*usage.0.lock().expect("usage call lock"), 1);
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
    async fn admission_resolves_each_closed_world_consumer() {
        let (credential_store, credential) = provisioned();

        let consent = FixedConsent(Some(record_for(CapabilityKind::Learning)));
        let refs = FixedRefs(vec![credential.clone()]);
        let prepared =
            prepare_learning_admission(&consent, &refs, &credential_store, Vec::new()).await;
        let PreparedAdmission::Ready(request) = prepared.expect("preparation answers") else {
            panic!("a complete setup must prepare a learning admission");
        };
        let mut tracker = ene_permission::EvaluationTracker::new();
        let Admission::Admitted(authorized) = request.authorize(&mut tracker) else {
            panic!("the learning candidate is inside the closed world");
        };
        assert_eq!(authorized.consent_premise().0, "consent-1");

        let consent = FixedConsent(Some(record_for(CapabilityKind::Dialogue)));
        let refs = FixedRefs(vec![credential.clone()]);
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
        assert_eq!(authorized.consent_premise().0, "consent-1");
    }

    #[tokio::test]
    async fn admission_declines_missing_or_mismatched_setup() {
        let (credential_store, credential) = provisioned();
        let dialogue_consent = FixedConsent(Some(record_for(CapabilityKind::Dialogue)));
        let learning_consent = FixedConsent(Some(record_for(CapabilityKind::Learning)));
        let refs = FixedRefs(vec![credential.clone()]);
        let prepared =
            prepare_learning_admission(&dialogue_consent, &refs, &credential_store, Vec::new())
                .await;
        assert_eq!(
            prepared,
            Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete))
        );

        let premise = TaskAgentAttemptPremise {
            delegation: RawId::new(),
            task: RawId::new(),
            task_revision: RevisionInner::from_u64(1),
            data_use: vec![RawId::new()],
        };
        let prepared =
            prepare_task_agent_admission(&learning_consent, &refs, &credential_store, premise)
                .await;
        assert_eq!(
            prepared,
            Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete))
        );

        let prepared = prepare_learning_admission(
            &FixedConsent(None),
            &FixedRefs(Vec::new()),
            &credential_store,
            Vec::new(),
        )
        .await;
        assert_eq!(
            prepared,
            Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete))
        );

        let empty_store = MemoryCredentialStore::new();
        let prepared = prepare_learning_admission(
            &learning_consent,
            &FixedRefs(vec![credential]),
            &empty_store,
            Vec::new(),
        )
        .await;
        assert_eq!(
            prepared,
            Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete))
        );
    }

    #[tokio::test]
    async fn learning_preparation_never_returns_inference_errors_as_outcomes() {
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
