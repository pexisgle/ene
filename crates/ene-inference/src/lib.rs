//! Inference dispatch contracts: tickets, routes, outcomes, and usage facts.
//!
//! This crate binds one authorized use to one provider call. It reuses
//! [`ene_permission::InferenceUseCandidate`] from `ene-permission` (never
//! redefined here). The caller mints the single-use
//! [`ene_permission::PermissionEvaluationId`] via
//! [`ene_permission::check_live_authorization`] and burns it through
//! [`ene_permission::EvaluationTracker::consume`] under its own short lock
//! before calling [`send`]; this crate keeps no tracker state.
//!
//! The [`ResolvedRoute`] is a premise supplied by core: core loads consent
//! and the credential ref, so there is no `resolve()` here. Likewise
//! [`UsageRepository`] is only the persistence boundary; [`send`] does not
//! record usage itself, core does after dispatch.
//!
//! Body text is redacted from [`core::fmt::Debug`]: [`RequestInferenceCommand`]
//! hides `input_text`, and [`InferenceResultArrival`] hides `output_text`.
//! Usage token counts are [`Option`]s with [`UsageSource::Unknown`], never
//! zero, when the provider reports nothing.

pub mod provider;

use std::future::Future;
use std::pin::Pin;

use ene_credential::{
    CredentialRef, CredentialRefRepository, CredentialStore, credential_availability,
};
use ene_permission::{
    CapabilityKind, CheckLiveAuthorizationQuery, ConsentRecord, ConsentRepository, ConsentRevision,
    ConsumerKind, DenyCode, EvaluationTracker, InferenceUseCandidate, LiveAuthorizationDecision,
    PermissionEvaluationId, PurposeKind, check_live_authorization,
};
use ene_primitive::RawId;
use thiserror::Error;

/// Maximum accepted input length in Unicode scalar values.
pub const MAX_INPUT_CHARS: usize = 8_000;

/// Opaque identity of one inference ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InferenceTicketId(pub RawId);

/// Provider route and consent premise for one inference use.
///
/// Supplied by core, which loads the current consent record and credential
/// ref. There is no `resolve()` in this crate: route construction is core's
/// job, route checking (against the candidate and the consent premise flag)
/// is [`send`]'s job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoute {
    /// Provider name; must exactly equal the candidate's `provider_ref`.
    pub provider: String,
    /// Model name; must exactly equal the candidate's `model`.
    pub model: String,
    /// Credential the provider call will be billed against.
    pub credential: CredentialRef,
    /// Consent premise core acted on, as `(consent id, revision)`.
    pub consent: (String, ConsentRevision),
}

/// Command dispatching one authorized inference use.
#[derive(Clone, PartialEq, Eq)]
pub struct RequestInferenceCommand {
    /// Ticket identifying this use end to end.
    pub ticket: InferenceTicketId,
    /// The authorized candidate; its fingerprint must match the evaluation id.
    pub candidate: InferenceUseCandidate,
    /// Single-use authorization minted for this candidate.
    pub authorization: PermissionEvaluationId,
    /// Route premise supplied by core.
    pub route: ResolvedRoute,
    /// Round body text; [`core::fmt::Debug`] redacts this.
    pub input_text: String,
}

impl core::fmt::Debug for RequestInferenceCommand {
    /// Renders every field except `input_text`, which is body text.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RequestInferenceCommand")
            .field("ticket", &self.ticket)
            .field("candidate", &self.candidate)
            .field("authorization", &self.authorization)
            .field("route", &self.route)
            .field("input_text", &"<redacted>")
            .finish()
    }
}

/// Outcome of an inference dispatch attempt.
///
/// State and data travel in the same variant: a completion always carries
/// its arrival, and a refusal never does. Unrepresentable pairings such as
/// "completed without arrival" cannot be constructed, so callers never
/// re-check arrival presence after matching the outcome. Technical
/// failures stay outside in the surrounding `Result`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchResult {
    /// The provider call completed; carries the full arrival.
    Completed(InferenceResultArrival),
    /// Nothing was sent; the reason names the failing gate.
    NotSent(NotSentReason),
}

/// Why an inference use was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotSentReason {
    /// Host-side authorization pre-check rejected the use.
    ///
    /// Produced before dispatch, never by [`send`].
    AuthRejected,
    /// Route and candidate disagree, or the route consent premise failed.
    ConsentMismatch,
    /// The credential is unavailable.
    ///
    /// Produced by Host-side pre-checks, never by [`send`].
    CredentialUnavailable,
    /// The evaluation id was unknown, already consumed, or bound to a
    /// different fingerprint.
    ///
    /// Produced by Host-side pre-checks (the consume step before dispatch),
    /// never by [`send`].
    EvaluationConsumed,
    /// No complete setup premise: no consent is recorded, or the consent
    /// references an unavailable credential.
    ///
    /// Produced by admission, never by [`send`].
    SetupIncomplete,
    /// The stored consent moved away from the premise the use was admitted
    /// under, before the attempt claim or before adoption.
    ConsentStale,
    /// The live-authorization allowlist refused the use.
    ///
    /// Produced by admission, never by [`send`].
    NotInAllowlist,
    /// `input_text` exceeds [`MAX_INPUT_CHARS`].
    OverLimit,
}

/// Reference to a completed inference result.
///
/// Kept for boundary projections that name a completion without carrying
/// its body; [`send`] itself returns the arrival inline via
/// [`DispatchResult::Completed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InferenceResultRef {
    /// Ticket the result answers.
    pub ticket: InferenceTicketId,
}

/// Arrival of one inference result with its usage fact.
#[derive(Clone, PartialEq, Eq)]
pub struct InferenceResultArrival {
    /// Ticket the result answers.
    pub ticket: InferenceTicketId,
    /// Provider output text; [`core::fmt::Debug`] redacts this.
    pub output_text: String,
    /// Token accounting for the call.
    pub usage: UsageFact,
}

impl core::fmt::Debug for InferenceResultArrival {
    /// Renders ticket and usage; `output_text` is body text and redacted.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InferenceResultArrival")
            .field("ticket", &self.ticket)
            .field("output_text", &"<redacted>")
            .field("usage", &self.usage)
            .finish()
    }
}

/// Provenance of a usage fact's token counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageSource {
    /// Counts came from the provider response.
    Reported,
    /// Counts were estimated Host-side.
    Estimated,
    /// No counts are known; token fields must be [`None`], never zero.
    Unknown,
}

/// Token accounting for one ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageFact {
    /// Ticket the accounting belongs to.
    pub ticket: InferenceTicketId,
    /// Provider that served the call.
    pub provider: String,
    /// Model that served the call.
    pub model: String,
    /// Input tokens, or [`None`] when unknown (never zero-as-unknown).
    pub input_tokens: Option<u64>,
    /// Output tokens, or [`None`] when unknown (never zero-as-unknown).
    pub output_tokens: Option<u64>,
    /// Provenance of the counts.
    pub source: UsageSource,
}

/// Certainty of an inference attempt, for Host-side bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InferenceCertainty {
    /// The provider call completed.
    Completed,
    /// The provider call failed.
    ProviderFailed,
    /// The call may have run but the response was lost.
    ResponseLost,
}

/// Technical failures of inference dispatch.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InferenceTechnicalError {
    /// The provider transport failed.
    #[error("provider transport failed: {0}")]
    ProviderTransportFailed(String),
    /// The timeout-bound HTTP client could not be built. Reported instead
    /// of falling back to an unbounded default, so the timeout invariant
    /// can never silently disappear.
    #[error("http client build failed")]
    HttpClientBuildFailed,
    /// The provider may have run the call but the response was lost.
    #[error("provider response lost")]
    ResponseLost,
    /// Inference persistence was unreachable or rejected the operation.
    #[error("inference storage unavailable: {reason}")]
    StorageUnavailable {
        /// Backend-supplied cause, without body text or secrets.
        reason: String,
    },
}

/// Provider-bound request for one completion.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderRequest {
    /// Model to complete with.
    pub model: String,
    /// Input text; [`core::fmt::Debug`] redacts this.
    pub input: String,
}

impl core::fmt::Debug for ProviderRequest {
    /// Renders the model; `input` is body text and redacted.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ProviderRequest")
            .field("model", &self.model)
            .field("input", &"<redacted>")
            .finish()
    }
}

/// Provider-bound response for one completion.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderResponse {
    /// Output text; [`core::fmt::Debug`] redacts this.
    pub text: String,
    /// Reported token counts, if the provider supplied any.
    pub usage: Option<RawUsage>,
}

impl core::fmt::Debug for ProviderResponse {
    /// Renders usage presence; `text` is body text and redacted.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ProviderResponse")
            .field("text", &"<redacted>")
            .field("usage", &self.usage)
            .finish()
    }
}

/// Raw token counts reported by a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RawUsage {
    /// Reported input tokens.
    pub input_tokens: u64,
    /// Reported output tokens.
    pub output_tokens: u64,
}

/// Transport boundary for provider completions.
///
/// The method returns a boxed `Send` future (rather than native `async fn`)
/// so connection tasks can spawn it on the multi-threaded runtime.
/// Implementations keep their internals unchanged and box at the boundary.
pub trait ProviderTransport: Send + Sync {
    /// Runs one completion with no retries or side effects beyond the call.
    fn complete(
        &self,
        req: ProviderRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse, InferenceTechnicalError>> + Send + '_>>;
}

/// Persistence boundary for usage facts.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait UsageRepository: Send + Sync {
    /// Records one usage fact.
    async fn record_usage(&self, fact: UsageFact) -> Result<(), InferenceTechnicalError>;
}

/// One claimed inference attempt: the ticket plus the consent premise and
/// route it may run under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceAttempt {
    /// Ticket the attempt would run under.
    pub ticket: InferenceTicketId,
    /// Consent premise the attempt relies on, as an `(id, rev)` pair that
    /// travels together (never a bare revision), so exhaustion stays visible
    /// at the boundary.
    pub expected_consent: (String, ConsentRevision),
    /// Provider the attempt would bill.
    pub provider: String,
    /// Model the attempt would run.
    pub model: String,
}

/// Outcome of claiming an inference attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttemptBeginOutcome {
    /// The attempt is claimed under the expected consent: the caller may
    /// issue provider I/O outside any lock. A later consent move cannot
    /// un-start it; result adoption still decides separately.
    Started,
    /// The expected consent no longer holds (or was never recorded): the
    /// caller must NOT issue provider I/O for this ticket.
    Stale,
}

/// Linearization point for starting provider I/O.
///
/// Claiming an attempt and mutating consent share one serialization
/// domain (short `Immediate` transactions, never held across I/O): either
/// the claim commits first and the attempt runs under a known-good
/// premise, or the mutation commits first and the claim fails stale
/// before any byte leaves. Re-claiming one ticket never sends twice: a
/// duplicate claim answers [`AttemptBeginOutcome::Stale`].
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait InferenceAttemptRepository: Send + Sync {
    /// Claims `attempt` iff the expected consent still holds.
    async fn begin_inference_attempt(
        &self,
        attempt: InferenceAttempt,
    ) -> Result<AttemptBeginOutcome, InferenceTechnicalError>;
}

/// Dispatches one authorized inference use.
///
/// Caller protocol: the Host first runs
/// [`ene_permission::check_live_authorization`], then consumes the minted id
/// through [`ene_permission::EvaluationTracker::consume`] under its own short
/// lock, and only then calls [`send`]. This function performs no
/// authorization bookkeeping itself: a call reaching it has already burned
/// its single-use id, so the same authorization value presented twice sends
/// twice.
///
/// Gates, in order:
///
/// 1. Route provider/model must exactly equal the candidate's; otherwise
///    [`NotSentReason::ConsentMismatch`].
/// 2. `route_consent_match` (core's verdict that the route consent premise
///    matches stored consent) must hold; otherwise [`NotSentReason::ConsentMismatch`].
/// 3. `input_text` longer than [`MAX_INPUT_CHARS`] yields
///    [`NotSentReason::OverLimit`].
/// 4. Otherwise the transport runs. Transport errors propagate as
///    [`Err`]; a response with no usage maps to [`None`] counts with
///    [`UsageSource::Unknown`], never zero.
///
/// [`NotSentReason::AuthRejected`], [`NotSentReason::CredentialUnavailable`],
/// and [`NotSentReason::EvaluationConsumed`] are Host-side pre-check
/// outcomes and are never produced here.
pub async fn send(
    cmd: RequestInferenceCommand,
    route_consent_match: bool,
    transport: &impl ProviderTransport,
) -> Result<DispatchResult, InferenceTechnicalError> {
    if cmd.route.provider != cmd.candidate.provider_ref || cmd.route.model != cmd.candidate.model {
        return Ok(DispatchResult::NotSent(NotSentReason::ConsentMismatch));
    }
    if !route_consent_match {
        return Ok(DispatchResult::NotSent(NotSentReason::ConsentMismatch));
    }
    if cmd.input_text.chars().count() > MAX_INPUT_CHARS {
        return Ok(DispatchResult::NotSent(NotSentReason::OverLimit));
    }
    let response = transport
        .complete(ProviderRequest {
            model: cmd.route.model.clone(),
            input: cmd.input_text.clone(),
        })
        .await?;
    let usage = match response.usage {
        Some(raw) => UsageFact {
            ticket: cmd.ticket,
            provider: cmd.route.provider.clone(),
            model: cmd.route.model.clone(),
            input_tokens: Some(raw.input_tokens),
            output_tokens: Some(raw.output_tokens),
            source: UsageSource::Reported,
        },
        None => UsageFact {
            ticket: cmd.ticket,
            provider: cmd.route.provider.clone(),
            model: cmd.route.model.clone(),
            input_tokens: None,
            output_tokens: None,
            source: UsageSource::Unknown,
        },
    };
    let arrival = InferenceResultArrival {
        ticket: cmd.ticket,
        output_text: response.text,
        usage,
    };
    Ok(DispatchResult::Completed(arrival))
}

/// One admission decision: an authorized use or a refusal.
///
/// Admission resolves consent and credential premises, then runs the
/// single-use live authorization. A decline happens before any history
/// append or provider call, so it leaves no side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Authorized; carries the already-consumed single-use authorization.
    Admitted(Box<AuthorizedInference>),
    /// Declined without side effects; carries the failing gate.
    Declined(NotSentReason),
}

/// Admission premises with the single-use decision still pending.
///
/// The caller runs [`AdmissionRequest::authorize`] under its own short
/// tracker lock, so no lock is held across the premise loads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionRequest {
    candidate: InferenceUseCandidate,
    consent: ConsentRecord,
    setup_complete: bool,
    credential: CredentialRef,
}

impl AdmissionRequest {
    /// Runs the live authorization and consumes its id on success.
    #[must_use]
    pub fn authorize(self, tracker: &mut EvaluationTracker) -> Admission {
        let query = CheckLiveAuthorizationQuery {
            candidate: self.candidate.clone(),
            expected_consent: Some((self.consent.id.clone(), self.consent.rev)),
            setup_complete: self.setup_complete,
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
                        authorization,
                    }))
                } else {
                    Admission::Declined(NotSentReason::EvaluationConsumed)
                }
            }
            LiveAuthorizationDecision::Deny(reason) => {
                Admission::Declined(not_sent_for_deny(reason.code))
            }
            LiveAuthorizationDecision::NeedsRevalidation(_) => {
                Admission::Declined(NotSentReason::ConsentStale)
            }
        }
    }
}

/// Premises prepared for one dialogue-purpose admission decision.
///
/// Repository failures stay in the surrounding `Err`; an absent or
/// incomplete setup answers [`PreparedAdmission::Declined`] without a
/// tracker decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedAdmission {
    /// Ready for the single-use decision under the caller's short lock.
    Ready(Box<AdmissionRequest>),
    /// Declined without touching the tracker.
    Declined(NotSentReason),
}

/// Resolves the consent and credential premises for one dialogue admission.
///
/// The returned request still needs [`AdmissionRequest::authorize`]; this
/// function performs no authorization and holds no lock.
pub async fn prepare_dialogue_admission(
    consent: &impl ConsentRepository,
    credential_refs: &impl CredentialRefRepository,
    credential_store: &impl CredentialStore,
) -> Result<PreparedAdmission, InferenceTechnicalError> {
    let record =
        consent
            .load_current()
            .await
            .map_err(|_| InferenceTechnicalError::StorageUnavailable {
                reason: String::from("load consent"),
            })?;
    let Some(record) = record else {
        return Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete));
    };
    let known_refs = credential_refs.list_refs().await.map_err(|_| {
        InferenceTechnicalError::StorageUnavailable {
            reason: String::from("list credential refs"),
        }
    })?;
    let credential = match known_refs
        .iter()
        .find(|known| known.id() == record.credential_id)
        .cloned()
    {
        Some(known) => known,
        None => match CredentialRef::new(record.provider.clone(), "main") {
            Ok(synthesized) => synthesized,
            Err(_) => {
                return Ok(PreparedAdmission::Declined(NotSentReason::SetupIncomplete));
            }
        },
    };
    let repo_known = known_refs
        .iter()
        .any(|known| known.id() == record.credential_id);
    let setup_complete = credential_availability(&credential, repo_known, credential_store).present;
    let candidate = InferenceUseCandidate {
        consumer: ConsumerKind::CompanionDialogue,
        capability: CapabilityKind::Dialogue,
        provider_ref: record.provider.clone(),
        model: record.model.clone(),
        purpose: PurposeKind::DialogueResponse,
    };
    Ok(PreparedAdmission::Ready(Box::new(AdmissionRequest {
        candidate,
        consent: record,
        setup_complete,
        credential,
    })))
}

/// The resolved, authorized premise of one not-yet-attempted inference use.
///
/// Opaque outside this crate behind accessors: a caller sequences the
/// durable append between admission and dispatch without learning
/// credential material or permission internals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedInference {
    ticket: InferenceTicketId,
    consent: (String, ConsentRevision),
    provider: String,
    model: String,
    credential: CredentialRef,
    candidate: InferenceUseCandidate,
    authorization: PermissionEvaluationId,
}

impl AuthorizedInference {
    /// Ticket this use runs under.
    #[must_use]
    pub fn ticket(&self) -> InferenceTicketId {
        self.ticket
    }

    /// Consent premise the use was authorized under, as `(id, rev)`.
    #[must_use]
    pub fn consent_premise(&self) -> (&str, u64) {
        (&self.consent.0, self.consent.1.as_u64())
    }
}

/// Outcome of one dispatched (attempted) inference use.
///
/// Usage accounting is already decided and recorded when this returns: a
/// never-attempted use records no fact, an uncertain attempt records
/// [`UsageSource::Unknown`], and a completed call records its reported
/// counts even when `adopted` is false.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceDispatchOutcome {
    /// The provider call completed; carries the arrival and whether
    /// adoption consent still held after the await.
    Completed {
        /// Full result of the call.
        arrival: InferenceResultArrival,
        /// Whether the post-await consent check still admitted the reply.
        adopted: bool,
    },
    /// Definitely never sent; no usage fact was recorded.
    NotSent(NotSentReason),
}

/// The owner boundary for one dialogue inference call.
///
/// `ene-inference` owns permission, credential, attempt, provider, and
/// usage ordering behind this boundary; the caller (companion dialogue)
/// only sequences its own durable append between admission and dispatch.
/// Implementations are wired by the Host composition root.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait InferenceExecutor: Send + Sync {
    /// Resolves and authorizes one dialogue-purpose use without sending.
    async fn admit_dialogue(&self) -> Result<Admission, InferenceTechnicalError>;

    /// Claims the attempt, calls the provider, records usage, and reports
    /// whether adoption consent survived the await.
    async fn dispatch(
        &self,
        authorized: AuthorizedInference,
        input_text: String,
    ) -> Result<InferenceDispatchOutcome, InferenceTechnicalError>;
}

/// Dispatches one authorized use: consent re-check, attempt claim, provider
/// call, adoption re-check, and usage recording.
///
/// A technical provider failure records an unknown-usage fact before
/// propagating: the attempt may have run. A never-sent outcome records no
/// fact. A completed call records its reported counts whether or not the
/// reply is adopted.
pub async fn dispatch_authorized(
    authorized: AuthorizedInference,
    input_text: String,
    consent: &impl ConsentRepository,
    attempts: &impl InferenceAttemptRepository,
    usage: &impl UsageRepository,
    transport: &impl ProviderTransport,
) -> Result<InferenceDispatchOutcome, InferenceTechnicalError> {
    let ticket = authorized.ticket;
    let (consent_id, consent_rev) = (authorized.consent.0.clone(), authorized.consent.1);
    let (provider, model) = (authorized.provider.clone(), authorized.model.clone());
    if !consent_matches(consent, &consent_id, consent_rev).await {
        // Never attempted: no provider use, so no usage fact.
        return Ok(InferenceDispatchOutcome::NotSent(
            NotSentReason::ConsentStale,
        ));
    }
    let command = RequestInferenceCommand {
        ticket,
        candidate: authorized.candidate,
        authorization: authorized.authorization,
        route: ResolvedRoute {
            provider: provider.clone(),
            model: model.clone(),
            credential: authorized.credential,
            consent: (consent_id.clone(), consent_rev),
        },
        input_text,
    };
    match attempts
        .begin_inference_attempt(InferenceAttempt {
            ticket,
            expected_consent: (consent_id.clone(), consent_rev.as_u64()),
            provider: provider.clone(),
            model: model.clone(),
        })
        .await
    {
        Ok(AttemptBeginOutcome::Started) => {}
        // Fail closed on a stale claim or a store failure: the provider
        // never ran, so no byte leaves and no usage fact is recorded.
        Ok(AttemptBeginOutcome::Stale) | Err(_) => {
            return Ok(InferenceDispatchOutcome::NotSent(
                NotSentReason::ConsentStale,
            ));
        }
    }
    // The attempt claim is the determination `send` relies on; the route
    // and candidate were built from the same consent premise.
    match send(command, true, transport).await {
        Err(error) => {
            record_usage_decision(usage, Some(unknown_usage(ticket, &provider, &model))).await;
            Err(error)
        }
        Ok(DispatchResult::NotSent(reason)) => Ok(InferenceDispatchOutcome::NotSent(reason)),
        Ok(DispatchResult::Completed(arrival)) => {
            let adopted = consent_matches(consent, &consent_id, consent_rev).await;
            record_usage_decision(usage, Some(arrival.usage.clone())).await;
            Ok(InferenceDispatchOutcome::Completed { arrival, adopted })
        }
    }
}

/// Loads the current consent and checks it still names exactly `id` at `rev`.
async fn consent_matches(
    consent: &impl ConsentRepository,
    id: &str,
    revision: ConsentRevision,
) -> bool {
    let Ok(Some(current)) = consent.load_current().await else {
        return false;
    };
    current.id == id && current.rev == revision
}

/// Unknown-counts fact for an attempt that may have run.
fn unknown_usage(ticket: InferenceTicketId, provider: &str, model: &str) -> UsageFact {
    UsageFact {
        ticket,
        provider: provider.to_string(),
        model: model.to_string(),
        input_tokens: None,
        output_tokens: None,
        source: UsageSource::Unknown,
    }
}

/// Records one decided usage fact best-effort.
///
/// The caller's stream outcome is authoritative; a usage persistence
/// failure is the documented later-stage retry gap, never a reason to
/// rewrite what already happened.
async fn record_usage_decision(usage: &impl UsageRepository, fact: Option<UsageFact>) {
    let Some(fact) = fact else {
        return;
    };
    if usage.record_usage(fact).await.is_err() {
        // Best-effort: the stream close stays authoritative.
    }
}

/// Maps one permission deny code to its inference refusal reason.
fn not_sent_for_deny(code: DenyCode) -> NotSentReason {
    match code {
        DenyCode::SetupIncomplete => NotSentReason::SetupIncomplete,
        DenyCode::ConsentStale | DenyCode::Superseded => NotSentReason::ConsentStale,
        DenyCode::NotInAllowlist => NotSentReason::NotInAllowlist,
    }
}

/// In-memory provider transport for tests and core integration tests.
///
/// Performs zero I/O: it replays configured text and usage, or a configured
/// failure. Provider adapters and retry policies are a behaviors-stage
/// concern, not here.
pub mod fake {
    use std::future::Future;
    use std::pin::Pin;

    use super::RawUsage;
    use super::{InferenceTechnicalError, ProviderRequest, ProviderResponse, ProviderTransport};

    /// Failure mode of a [`FakeProviderTransport`].
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum FakeFailure {
        /// Fail like a broken transport with the given reason.
        Transport(String),
        /// Fail like a lost response.
        ResponseLost,
    }

    /// Zero-I/O transport replaying fixed text, usage, or failure.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct FakeProviderTransport {
        /// Text returned on success.
        pub text: String,
        /// Usage returned on success, or [`None`] for unknown usage.
        pub usage: Option<RawUsage>,
        /// Failure to return instead of succeeding.
        pub fail: Option<FakeFailure>,
    }

    impl FakeProviderTransport {
        /// Replays a successful completion with the given text and usage.
        #[must_use]
        pub fn new(text: String, usage: Option<RawUsage>) -> Self {
            Self {
                text,
                usage,
                fail: None,
            }
        }

        /// Always fails with the given mode.
        #[must_use]
        pub fn failing(fail: FakeFailure) -> Self {
            Self {
                text: String::new(),
                usage: None,
                fail: Some(fail),
            }
        }
    }

    impl ProviderTransport for FakeProviderTransport {
        /// Replays the configured response or failure without I/O.
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
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeFailure, FakeProviderTransport};
    use super::{
        DispatchResult, InferenceResultArrival, InferenceTicketId, NotSentReason, RawUsage,
        RequestInferenceCommand, ResolvedRoute, UsageSource, send,
    };
    use ene_credential::CredentialRef;
    use ene_permission::{
        CapabilityKind, ConsentRevision, ConsumerKind, InferenceUseCandidate,
        PermissionEvaluationId, PurposeKind,
    };
    use ene_primitive::RawId;

    fn candidate() -> InferenceUseCandidate {
        InferenceUseCandidate {
            consumer: ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            provider_ref: "acme".to_owned(),
            model: "dialogue-1".to_owned(),
            purpose: PurposeKind::DialogueResponse,
        }
    }

    fn route() -> ResolvedRoute {
        ResolvedRoute {
            provider: "acme".to_owned(),
            model: "dialogue-1".to_owned(),
            credential: CredentialRef::new("acme", "main").expect("valid test fixture"),
            consent: ("consent-1".to_owned(), ConsentRevision::from_u64(3)),
        }
    }

    fn command(input_text: &str) -> RequestInferenceCommand {
        RequestInferenceCommand {
            ticket: InferenceTicketId(RawId::new()),
            candidate: candidate(),
            authorization: PermissionEvaluationId(RawId::new()),
            route: route(),
            input_text: input_text.to_owned(),
        }
    }

    #[tokio::test]
    async fn success_maps_reported_usage() {
        let cmd = command("hello");
        let ticket = cmd.ticket;
        let transport = FakeProviderTransport::new(
            "hi there".to_owned(),
            Some(RawUsage {
                input_tokens: 4,
                output_tokens: 2,
            }),
        );
        let result = send(cmd, true, &transport)
            .await
            .expect("inference dispatch answers an outcome");
        let DispatchResult::Completed(arrival) = result else {
            panic!("a successful provider call completes");
        };
        assert_eq!(arrival.ticket, ticket);
        assert_eq!(arrival.output_text, "hi there");
        assert_eq!(arrival.usage.input_tokens, Some(4));
        assert_eq!(arrival.usage.output_tokens, Some(2));
        assert_eq!(arrival.usage.source, UsageSource::Reported);
    }

    #[tokio::test]
    async fn missing_usage_maps_to_unknown_not_zero() {
        let cmd = command("hello");
        let transport = FakeProviderTransport::new("hi there".to_owned(), None);
        let result = send(cmd, true, &transport)
            .await
            .expect("inference dispatch answers an outcome");
        let DispatchResult::Completed(arrival) = result else {
            panic!("a successful provider call completes");
        };
        assert_eq!(arrival.usage.input_tokens, None);
        assert_eq!(arrival.usage.output_tokens, None);
        assert_eq!(arrival.usage.source, UsageSource::Unknown);
    }

    #[tokio::test]
    async fn route_mismatch_is_not_sent() {
        let cmd = RequestInferenceCommand {
            ticket: InferenceTicketId(RawId::new()),
            candidate: candidate(),
            authorization: PermissionEvaluationId(RawId::new()),
            route: ResolvedRoute {
                model: "other-model".to_owned(),
                ..route()
            },
            input_text: "hello".to_owned(),
        };
        let transport = FakeProviderTransport::new("hi there".to_owned(), None);
        let result = send(cmd, true, &transport)
            .await
            .expect("inference dispatch answers an outcome");
        assert_eq!(
            result,
            DispatchResult::NotSent(NotSentReason::ConsentMismatch)
        );
    }

    #[tokio::test]
    async fn failed_consent_premise_is_not_sent() {
        let cmd = command("hello");
        let transport = FakeProviderTransport::new("hi there".to_owned(), None);
        let result = send(cmd, false, &transport)
            .await
            .expect("inference dispatch answers an outcome");
        assert_eq!(
            result,
            DispatchResult::NotSent(NotSentReason::ConsentMismatch)
        );
    }

    #[tokio::test]
    async fn over_limit_input_is_not_sent() {
        let big: String = "x".repeat(super::MAX_INPUT_CHARS + 1);
        let cmd = command(&big);
        let transport = FakeProviderTransport::new("hi there".to_owned(), None);
        let result = send(cmd, true, &transport)
            .await
            .expect("inference dispatch answers an outcome");
        assert_eq!(result, DispatchResult::NotSent(NotSentReason::OverLimit));
    }

    #[tokio::test]
    async fn transport_failure_is_a_technical_error() {
        let cmd = command("hello");
        let transport = FakeProviderTransport::failing(FakeFailure::Transport("down".to_owned()));
        let result = send(cmd, true, &transport).await;
        assert!(matches!(
            result,
            Err(super::InferenceTechnicalError::ProviderTransportFailed(_))
        ));
    }

    #[tokio::test]
    async fn lost_response_is_a_technical_error() {
        let cmd = command("hello");
        let transport = FakeProviderTransport::failing(FakeFailure::ResponseLost);
        let result = send(cmd, true, &transport).await;
        assert!(matches!(
            result,
            Err(super::InferenceTechnicalError::ResponseLost)
        ));
    }

    #[test]
    fn debug_redacts_body_text() {
        let cmd = command("harbor-sunset-body-probe");
        let rendered = format!("{cmd:?}");
        assert!(!rendered.contains("harbor-sunset-body-probe"));
        let arrival = InferenceResultArrival {
            ticket: cmd.ticket,
            output_text: "harbor-sunset-output-probe".to_owned(),
            usage: super::UsageFact {
                ticket: cmd.ticket,
                provider: "acme".to_owned(),
                model: "dialogue-1".to_owned(),
                input_tokens: None,
                output_tokens: None,
                source: UsageSource::Unknown,
            },
        };
        let rendered_arrival = format!("{arrival:?}");
        assert!(!rendered_arrival.contains("harbor-sunset-output-probe"));
    }
}

#[cfg(test)]
mod dispatch_tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::fake::{FakeFailure, FakeProviderTransport};
    use super::{
        AttemptBeginOutcome, AuthorizedInference, InferenceAttempt, InferenceAttemptRepository,
        InferenceDispatchOutcome, InferenceTechnicalError, InferenceTicketId, MAX_INPUT_CHARS,
        NotSentReason, PermissionEvaluationId, RawUsage, UsageFact, UsageRepository, UsageSource,
        dispatch_authorized,
    };
    use ene_credential::CredentialRef;
    use ene_permission::{
        CapabilityKind, ConsentCommitOutcome, ConsentRecord, ConsentRepository, ConsentRevision,
        ConsumerKind, InferenceUseCandidate, PermissionTechnicalError, PurposeKind,
    };
    use ene_primitive::RawId;

    fn record(revision: u64) -> ConsentRecord {
        ConsentRecord {
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
        async fn load_current(&self) -> Result<Option<ConsentRecord>, PermissionTechnicalError> {
            Ok(self.0.clone())
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn compare_and_save(
            &self,
            _expected: Option<(String, ConsentRevision)>,
            _record: ConsentRecord,
        ) -> Result<ConsentCommitOutcome, PermissionTechnicalError> {
            Err(PermissionTechnicalError::StorageUnavailable {
                reason: String::from("read-only test consent"),
            })
        }
    }

    /// Returns the first record once, then the second: models a consent move
    /// landing during the provider await.
    struct SwitchingConsent {
        first: ConsentRecord,
        second: ConsentRecord,
        reads: AtomicUsize,
    }

    impl ConsentRepository for SwitchingConsent {
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn load_current(&self) -> Result<Option<ConsentRecord>, PermissionTechnicalError> {
            let read = self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(Some(if read == 0 {
                self.first.clone()
            } else {
                self.second.clone()
            }))
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn compare_and_save(
            &self,
            _expected: Option<(String, ConsentRevision)>,
            _record: ConsentRecord,
        ) -> Result<ConsentCommitOutcome, PermissionTechnicalError> {
            Err(PermissionTechnicalError::StorageUnavailable {
                reason: String::from("read-only test consent"),
            })
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
            authorization: PermissionEvaluationId(RawId::new()),
        }
    }

    #[tokio::test]
    async fn transport_failure_records_unknown_counts() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let transport = FakeProviderTransport::failing(FakeFailure::Transport("down".to_owned()));
        let result = dispatch_authorized(
            authorized(),
            String::from("hello"),
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
        assert_eq!(facts[0].output_tokens, None);
    }

    #[tokio::test]
    async fn never_sent_records_no_fact() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = FixedConsent(Some(record(1)));
        let transport = FakeProviderTransport::new(String::from("hi"), None);
        let result = dispatch_authorized(
            authorized(),
            "x".repeat(MAX_INPUT_CHARS + 1),
            &consent,
            &StartedAttempts,
            &usage,
            &transport,
        )
        .await;
        assert_eq!(
            result.expect("dispatch answers an outcome"),
            InferenceDispatchOutcome::NotSent(NotSentReason::OverLimit)
        );
        assert!(
            usage.0.lock().expect("usage capture lock").is_empty(),
            "a definitely-never-sent call spends nothing"
        );
    }

    #[tokio::test]
    async fn completed_records_reported_counts_even_when_adoption_moves() {
        let usage = CapturedUsage(Mutex::new(Vec::new()));
        let consent = SwitchingConsent {
            first: record(1),
            second: record(2),
            reads: AtomicUsize::new(0),
        };
        let transport = FakeProviderTransport::new(
            String::from("hi there"),
            Some(RawUsage {
                input_tokens: 4,
                output_tokens: 2,
            }),
        );
        let outcome = dispatch_authorized(
            authorized(),
            String::from("hello"),
            &consent,
            &StartedAttempts,
            &usage,
            &transport,
        )
        .await
        .expect("dispatch answers an outcome");
        let InferenceDispatchOutcome::Completed { adopted, .. } = outcome else {
            panic!("a provider success completes");
        };
        assert!(!adopted, "the moved consent refuses adoption");
        let facts = usage.0.lock().expect("usage capture lock");
        assert_eq!(facts.len(), 1, "the reported fact is kept");
        assert_eq!(facts[0].source, UsageSource::Reported);
        assert_eq!(facts[0].input_tokens, Some(4));
        assert_eq!(facts[0].output_tokens, Some(2));
    }
}
