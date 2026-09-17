//! Inference dispatch contracts: admission, tickets, routes, outcomes, and
//! usage facts.
//!
//! This crate binds one authorized use to one provider call and owns the
//! whole admission-to-accounting order behind [`InferenceExecutor`]:
//! [`prepare_dialogue_admission`], [`prepare_learning_admission`], and
//! [`prepare_task_agent_admission`] resolve the consent and credential
//! premise for their consumer, [`AdmissionRequest::authorize`] runs the
//! single-use [`ene_permission::check_live_authorization`] /
//! [`ene_permission::EvaluationTracker::consume`] decision, and
//! [`dispatch_authorized`] claims the attempt, calls the provider
//! transport, re-checks adoption, and records usage. The provider request
//! is built inside `dispatch_authorized` from an [`AuthorizedInference`],
//! so no caller assembles permission or credential premises by hand. The
//! input cap is checked before the durable attempt claim; callers arrive
//! with an already-admitted use.
//!
//! A Task Agent attempt carries an opaque [`TaskAgentAttemptPremise`]; the
//! claim verifies it against the delegation row and the current Task in the
//! same transaction as the consent and credential-set compare, and compares
//! the premise's `data_use` source correlation against the canonical current
//! erasure-condition store in that same transaction. A covered source is a
//! [`NotSentReason::DataUseHeld`] refusal with zero provider bytes. The
//! inference crate never imports Task-owned or preservation-owned types; the
//! mapping happens in the Host composition root.
//!
//! Body text is redacted from [`core::fmt::Debug`]: [`ProviderRequest`]
//! hides `input`, and [`InferenceResultArrival`] hides `output_text`.
//! Usage token counts are [`Option`]s with [`UsageSource::Unknown`], never
//! zero, when the provider reports nothing. Cost is derived from those counts
//! and the immutable pricing snapshot bound to the ticket at admission
//! ([`cost::project_cost`]); a missing rate or unknown counts settle as
//! [`cost::UsageCostFact::Unknown`], never a zero amount.
//!
//! The attempt claim is also the cap-admission linearization point
//! (`usage-cost-cap` §9): when a current provider or system cap applies, the
//! same short transaction reads the cap revisions, sums the window's
//! `Reserved + CommittedReported + CommittedUnknown` consumption, and inserts
//! the conservative upper-bound reservation only if every applicable cap
//! would still hold. A held or indeterminate cap decision creates no attempt
//! and no reservation, and the provider receives zero bytes. Settlement
//! (`record_usage`) commits reported usage to `CommittedReported` (counting
//! the actual cost and releasing the unused reservation) and every uncertain
//! outcome to `CommittedUnknown`, which keeps the reserved upper bound
//! counted; a `Released` reservation is never inferred from a transport error
//! string.

pub mod cost;
pub mod pricing;
pub mod provider;

use std::future::Future;
use std::pin::Pin;

use cost::UsageEstimate;
use ene_credential::{
    CredentialRef, CredentialRefRepository, CredentialSetRevision, CredentialStore, ScrubbedText,
};
use ene_permission::{
    CapabilityKind, CheckLiveAuthorizationQuery, ConsentRecord, ConsentRepository, ConsentRevision,
    ConsumerKind, DenyCode, EvaluationTracker, InferenceUseCandidate, LiveAuthorizationDecision,
    PermissionEvaluationId, PurposeKind, UsageCapRef, UsageReservationRef, UsageReservationState,
    check_live_authorization,
};
use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
use pricing::{PricingCatalog, PricingResolution, PricingSnapshot, PricingSnapshotRef};
use thiserror::Error;

/// Maximum accepted input length in Unicode scalar values.
pub const MAX_INPUT_CHARS: usize = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InferenceTicketId(pub RawId);

/// Task Agent correlation of one inference attempt, as opaque values.
///
/// The inference domain never imports Task-owned types: the task side (or
/// the Host composition root) maps `DelegationId` / `TaskRef` into this
/// premise, and the claim carries it through to the durable attempt row.
///
/// `data_use` is the canonical source correlation of the logical input the
/// attempt sends: the adopted purpose entry's source and every adopted
/// instruction entry's source, in logical-input order. The values are opaque
/// [`RawId`]s — never bodies or hashes — and the order and duplicates are
/// preserved because each entry keeps its own correlation. The claim compares
/// every source against the current erasure conditions inside its
/// transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAgentAttemptPremise {
    /// Delegation correspondence this turn runs under.
    pub delegation: RawId,
    /// Relied-on Task identity.
    pub task: RawId,
    /// Relied-on Task revision; always travels with the identity pair.
    pub task_revision: RevisionInner,
    /// Canonical source correlation of the logical input, in input order.
    /// A Task Agent attempt always names at least its purpose source; the
    /// claim refuses an empty correlation instead of treating it as no use.
    pub data_use: Vec<RawId>,
}

/// Why an inference use was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotSentReason {
    /// The evaluation id was unknown, already consumed, or bound to a
    /// different fingerprint.
    ///
    /// Produced by admission, never by dispatch.
    EvaluationConsumed,
    /// No complete setup premise: no consent is recorded, the consent
    /// references an unregistered credential, or the bearer is missing.
    ///
    /// Produced by admission, never by dispatch.
    SetupIncomplete,
    /// The stored consent or credential-set premise moved away from the one
    /// the use was admitted under before the attempt claim; a duplicate claim
    /// of the same ticket also answers this. A consent that moves during the
    /// provider wait is not this refusal: the response is then
    /// [`InferenceDispatchOutcome::Completed`] with `adopted: false`.
    ConsentStale,
    /// The live-authorization allowlist refused the use.
    ///
    /// Produced by admission, never by dispatch.
    NotInAllowlist,
    /// The input exceeds [`MAX_INPUT_CHARS`].
    OverLimit,
    /// The Task Agent attempt's delegation/task premise no longer holds
    /// before the claim: the relied revision moved, or the delegation row is
    /// gone. Produced by dispatch only, distinct from [`Self::ConsentStale`].
    TaskPremiseStale,
    /// The canonical current erasure-condition store covers at least one
    /// source in the attempt's `data_use`. A data-use hold is a domain
    /// refusal, not task/consent staleness and not a storage error: the
    /// provider receives zero bytes and the attempt is not claimed.
    DataUseHeld,
    /// At least one current cap applies to the route and the window's
    /// consumption plus this request's safe upper bound would exceed it. The
    /// provider receives zero bytes and neither an attempt nor a reservation
    /// is created. A cap refusal is a domain outcome, never consent
    /// staleness, a data-use hold, or a storage failure.
    UsageCapReached,
    /// At least one current cap applies to the route, but no finite safe
    /// upper bound can be constructed for this request (no reviewed rate, no
    /// provider estimate, or a currency the cap cannot be compared in), so
    /// the cap cannot be proven satisfied. The provider receives zero bytes:
    /// an unprovable bound is never treated as zero or released.
    UsageCapIndeterminate,
}

#[derive(Clone, PartialEq, Eq)]
pub struct InferenceResultArrival {
    pub ticket: InferenceTicketId,
    /// Provider output text; [`core::fmt::Debug`] redacts this.
    pub output_text: String,
    /// Token accounting for the call.
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
    /// No counts are known; token fields must be [`None`], never zero.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageFact {
    pub ticket: InferenceTicketId,
    pub provider: String,
    pub model: String,
    /// Input tokens, or [`None`] when unknown (never zero-as-unknown).
    pub input_tokens: Option<u64>,
    /// Cached input is a subset of input. All three counts are present for
    /// Reported, or all absent for Unknown; missing cache detail is not zero.
    pub cached_input_tokens: Option<u64>,
    /// Output tokens, or [`None`] when unknown (never zero-as-unknown).
    pub output_tokens: Option<u64>,
    pub source: UsageSource,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InferenceTechnicalError {
    #[error("provider transport failed: {0}")]
    ProviderTransportFailed(String),
    /// The delta consumer stopped the stream: a stale presentation premise
    /// or a failed delivery ended the provider read. Usage follows the
    /// attempt as usual, but the reply is never adopted.
    #[error("stream aborted: {reason}")]
    StreamAborted {
        /// Short cause class, never body text.
        reason: String,
    },
    /// The timeout-bound HTTP client could not be built. Reported instead
    /// of falling back to an unbounded default, so the timeout invariant
    /// can never silently disappear.
    #[error("http client build failed")]
    HttpClientBuildFailed,
    /// The provider may have run the call but the response was lost.
    #[error("provider response lost")]
    ResponseLost,
    /// The reviewed first-party pricing catalog could not be constructed (a
    /// first-party data defect, never a provider or user failure). Detected
    /// before the attempt claim, so no provider byte is sent.
    #[error("pricing catalog unavailable")]
    PricingCatalogUnavailable,
    /// A durable cost fact cannot be projected: the stored usage and pricing
    /// rows disagree, or the amount is not representable. Distinct from
    /// storage unavailability and never resolved to a zero or guessed cost.
    #[error("usage cost projection failed: {reason}")]
    CostProjectionFailed {
        /// Bounded cause class, without body text or secrets.
        reason: String,
    },
    #[error("inference storage unavailable: {reason}")]
    StorageUnavailable {
        /// Backend-supplied cause, without body text or secrets.
        reason: String,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProviderRequest {
    pub model: String,
    /// Authorized credential the provider call bills. The transport resolves
    /// its bearer per request, so a consent reassignment applies immediately.
    pub credential: CredentialRef,
    /// Input text; [`core::fmt::Debug`] redacts this.
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
    /// Output text; [`core::fmt::Debug`] redacts this.
    pub text: String,
    /// Reported token counts, if the provider supplied any.
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
    /// Included in input tokens, not an additional count.
    pub cached_input_tokens: u64,
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
    /// response carries the full text for adoption and History. The default
    /// fallback is explicit: a transport without incremental support
    /// completes synchronously and pushes the whole text once at the end,
    /// honoring an abort the same way, so callers always observe at most one
    /// delta and never a fabricated early one.
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

/// Flow-control answer from one delta push: the provider loop continues on
/// [`DeltaFlow::Continue`] and stops reading on [`DeltaFlow::Abort`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaFlow {
    Continue,
    /// Stop the provider read with this short cause class (never body text).
    /// The transport reports it through
    /// [`InferenceTechnicalError::StreamAborted`].
    Abort(&'static str),
}

/// Async receiver of provider deltas, threaded from dispatch to transport.
///
/// A boxed-future method (rather than native `async fn`) keeps the trait
/// object-safe, matching [`ProviderTransport`]: dispatch holds
/// `&mut (dyn DeltaSink + Send)` and the transport awaits each push, so a
/// slow consumer backpressures the provider read instead of queueing
/// unboundedly, and a stale consumer aborts it before further deltas are
/// shown or adopted.
pub trait DeltaSink: Send {
    fn push_delta<'a>(
        &'a mut self,
        delta: &'a str,
    ) -> Pin<Box<dyn Future<Output = DeltaFlow> + Send + 'a>>;
}

/// Delta receiver that keeps nothing: for inference whose output is never
/// presented (Learning formation), where adoption is the only outcome.
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
    /// [`UsageReservationState::CommittedReported`] with the actual cost
    /// derived from the bound pricing snapshot (releasing the unused
    /// reservation), and an unknown fact commits it as
    /// [`UsageReservationState::CommittedUnknown`], which keeps the reserved
    /// upper bound counted against every cap. The first settlement wins; a
    /// duplicate never revises a terminal reservation.
    async fn record_usage(&self, fact: UsageFact) -> Result<(), InferenceTechnicalError>;

    /// Projects the durable cost fact of a settled ticket.
    ///
    /// `Ok(None)` means no token settlement exists for the ticket yet (the
    /// attempt may still be in flight); it is not a zero-cost fact. A settled
    /// fact whose stored rows disagree or whose amount does not fit the money
    /// representation is a technical error, never a guessed or truncated
    /// cost. This read performs no pricing refresh and rewrites nothing.
    async fn load_usage_cost(
        &self,
        ticket: InferenceTicketId,
    ) -> Result<Option<UsageCostRecord>, InferenceTechnicalError>;

    /// Loads the ticket's usage reservation, if the admission created one.
    ///
    /// `Ok(None)` means no reservation exists: either the claim predates the
    /// cap regime or no cap applied to the route, so there is no reserved
    /// amount. A malformed or internally inconsistent row is a technical
    /// error. This read settles nothing.
    async fn load_usage_reservation(
        &self,
        ticket: InferenceTicketId,
    ) -> Result<Option<UsageReservation>, InferenceTechnicalError>;

    /// Settles every non-terminal usage reservation as
    /// [`UsageReservationState::CommittedUnknown`] (`usage-cost-cap` §15).
    ///
    /// This is the Host-startup re-evaluation of reservations orphaned by a
    /// crash: external consumption cannot be denied, so the reserved upper
    /// bound stays counted and the ticket settles an Unknown token usage
    /// fact; a crash never releases a reservation and never zeroes usage.
    /// Terminal reservations are not re-counted or re-inserted. Returns the
    /// number of reservations settled by this call; the operation is
    /// idempotent.
    async fn reconcile_orphaned_usage_reservations(&self) -> Result<u64, InferenceTechnicalError>;
}

/// One durable usage reservation as read back for correlation.
///
/// The reference is the identity the cap-admission boundary minted; `ticket`
/// correlates it to the claimed attempt. `committed` is present exactly for
/// [`UsageReservationState::CommittedReported`], where it carries the actual
/// cost the reservation settled at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageReservation {
    /// Durable reservation identity.
    pub reference: UsageReservationRef,
    /// Claimed attempt this reservation was opened for.
    pub ticket: InferenceTicketId,
    /// Provider route the upper bound was computed for.
    pub provider: String,
    /// Model route the upper bound was computed for.
    pub model: String,
    /// Immutable pricing snapshot the upper bound was computed under.
    pub pricing: PricingSnapshotRef,
    /// Conservative cap amount the admission reserved.
    pub upper_bound: cost::Money,
    /// Current lifecycle state.
    pub state: UsageReservationState,
    /// Actual committed amount, exactly for `CommittedReported`.
    pub committed: Option<cost::Money>,
    /// Instant the reservation was opened; its cap window is derived from it.
    pub opened_at: WallClockWithTz,
}

/// One settled ticket's durable token and cost facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageCostRecord {
    /// Token usage exactly as settled (Reported counts or Unknown).
    pub usage: UsageFact,
    /// Cost projected from the pricing snapshot bound at the attempt claim.
    pub cost: cost::UsageCostFact,
}

/// One claimed inference attempt: the ticket plus the consent premise and
/// route it may run under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceAttempt {
    pub ticket: InferenceTicketId,
    /// Consumer the attempt was admitted for. Persisted so usage attribution
    /// can distinguish a Task Agent turn from companion dialogue/learning
    /// even when the capability (consent slot) is shared.
    pub consumer: ConsumerKind,
    /// Capability the attempt was admitted under. The claim reads exactly
    /// this capability's consent row, so a dialogue attempt can never be
    /// validated against learning consent or vice versa.
    pub capability: CapabilityKind,
    /// Purpose binding this use; persisted with the same discipline as
    /// `consumer`.
    pub purpose: PurposeKind,
    /// Consent premise the attempt relies on, as an `(id, rev)` pair that
    /// travels together (never a bare revision), so exhaustion stays visible
    /// at the boundary.
    pub expected_consent: (String, ConsentRevision),
    /// Credential-set premise the prompt was scrubbed under. The claim
    /// compares it against the durable set in the same transaction, so a
    /// prompt that predates a credential registration is never sent.
    pub expected_credential_set: CredentialSetRevision,
    pub provider: String,
    pub model: String,
    /// Task Agent correlation, present exactly for a Task Agent attempt. The
    /// claim verifies it against the delegation row and the current Task in
    /// the same transaction.
    pub task_agent: Option<TaskAgentAttemptPremise>,
    /// Reviewed pricing snapshot resolved for this route immediately before
    /// the claim (`usage-cost-cap` §9), or `None` when the first-party
    /// catalog has no reviewed rate for the route. The claim publishes the
    /// snapshot durably and binds its reference to the attempt, so the cost
    /// fact of this ticket can never be repriced by a later catalog revision.
    /// `None` settles the cost as Unknown, never as zero.
    pub pricing: Option<PricingSnapshot>,
    /// Conservative safe upper bound of this request's billable token usage,
    /// resolved by the provider adapter before the claim (`usage-cost-cap`
    /// §8), or `None` when the adapter cannot construct a finite bound.
    /// [`Self::pricing`] and this estimate together are what the claim turns
    /// into a cap reservation; when a current cap applies and either is
    /// absent, the claim refuses with [`AttemptBeginOutcome::CapIndeterminate`]
    /// rather than sending without a provable bound.
    pub usage_estimate: Option<UsageEstimate>,
}

/// One claimed attempt as read back for attribution and restart.
///
/// This is the durable correlation a delayed result starts from:
/// `ticket -> consumer/purpose/delegation -> relied TaskRef -> delegator`.
/// Reading it never replays or re-claims the attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceAttemptRecord {
    pub ticket: InferenceTicketId,
    pub consumer: ConsumerKind,
    pub capability: CapabilityKind,
    pub purpose: PurposeKind,
    pub provider: String,
    pub model: String,
    /// Task Agent correlation, present iff the consumer is
    /// [`ConsumerKind::TaskAgent`]; a record that disagrees is never composed.
    pub task_agent: Option<TaskAgentAttemptPremise>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AttemptBeginOutcome {
    /// The attempt is claimed under the expected consent: the caller may
    /// issue provider I/O outside any lock. A later consent move cannot
    /// un-start it; result adoption still decides separately. When a current
    /// cap applied to the route, the reservation is durable before this
    /// answer: the cap can never be oversubscribed by a concurrent claim.
    Started,
    /// The expected consent or credential-set premise no longer holds (or was
    /// never recorded), or this ticket was already claimed: the caller must
    /// NOT issue provider I/O for this ticket.
    Stale,
    /// The Task Agent delegation/task premise no longer holds: the caller
    /// must NOT issue provider I/O for this ticket. Distinct from
    /// [`Self::Stale`], which is about consent or credential currency.
    TaskPremiseStale,
    /// At least one canonical source of the attempt's `data_use` is covered
    /// by a current erasure condition: the caller must NOT issue provider I/O
    /// for this ticket, and the attempt is not recorded. Distinct from both
    /// staleness outcomes and never a storage error.
    DataUseHeld,
    /// A current cap's window consumption plus this request's safe upper
    /// bound would exceed the limit: the caller must NOT issue provider I/O
    /// for this ticket, and neither the attempt nor a reservation is
    /// recorded. The carried reference names the first violated cap in the
    /// deterministic evaluation order (system before provider, daily before
    /// monthly).
    HeldByCap(UsageCapRef),
    /// At least one current cap applies to the route, but the claim cannot
    /// construct a finite safe upper bound under it (no reviewed rate, no
    /// provider estimate, a cap currency the request cannot be compared in,
    /// or an unrepresentable sum): the caller must NOT issue provider I/O.
    /// The cap is never treated as satisfied and the bound is never assumed
    /// zero.
    CapIndeterminate,
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

    /// Loads the claimed attempt for one ticket, if any.
    ///
    /// `None` means no attempt was claimed for this ticket. Malformed or
    /// internally inconsistent rows (unknown consumer/purpose names, a
    /// partial Task Agent correlation group, or a Task Agent consumer with
    /// no correlation) are technical errors and are never composed into a
    /// record. This read never re-claims or replays the provider call.
    async fn load_inference_attempt(
        &self,
        ticket: InferenceTicketId,
    ) -> Result<Option<InferenceAttemptRecord>, InferenceTechnicalError>;
}

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
    credential: CredentialRef,
    task_agent: Option<TaskAgentAttemptPremise>,
}

impl AdmissionRequest {
    /// Runs the live authorization and consumes its id on success.
    #[must_use]
    pub fn authorize(self, tracker: &mut EvaluationTracker) -> Admission {
        // Setup is complete by construction: preparation only yields a
        // request after resolving the registry ref and verifying that the
        // store holds its bearer.
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
                        authorization,
                        task_agent: self.task_agent,
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
    prepare_admission(
        consent,
        credential_refs,
        credential_store,
        ConsumerKind::CompanionDialogue,
        CapabilityKind::Dialogue,
        PurposeKind::DialogueResponse,
        None,
    )
    .await
}

/// Resolves the consent and credential premises for one learning-formation
/// admission.
///
/// Learning is a distinct consumer and purpose: it shares the current
/// provider assignment, but never presents itself as dialogue, so consent
/// accounting and the closed-world allowlist can tell the two uses apart.
///
/// The returned request still needs [`AdmissionRequest::authorize`]; this
/// function performs no authorization and holds no lock.
pub async fn prepare_learning_admission(
    consent: &impl ConsentRepository,
    credential_refs: &impl CredentialRefRepository,
    credential_store: &impl CredentialStore,
) -> Result<PreparedAdmission, InferenceTechnicalError> {
    prepare_admission(
        consent,
        credential_refs,
        credential_store,
        ConsumerKind::CompanionLearning,
        CapabilityKind::Learning,
        PurposeKind::MemoryFormation,
        None,
    )
    .await
}

/// Resolves the consent and credential premises for one Task Agent turn.
///
/// The Task Agent inherits the delegating companion's current assignment:
/// it uses the `Dialogue` capability consent (the delegator's reasoning
/// route) under the dedicated [`ConsumerKind::TaskAgent`] /
/// [`PurposeKind::TaskAgentTurn`] triple, so a Task Agent use can never be
/// admitted as dialogue or learning. `task_agent` is the opaque
/// delegation/relied-revision premise the claim verifies inside its
/// transaction.
///
/// The returned request still needs [`AdmissionRequest::authorize`]; this
/// function performs no authorization and holds no lock.
pub async fn prepare_task_agent_admission(
    consent: &impl ConsentRepository,
    credential_refs: &impl CredentialRefRepository,
    credential_store: &impl CredentialStore,
    task_agent: TaskAgentAttemptPremise,
) -> Result<PreparedAdmission, InferenceTechnicalError> {
    prepare_admission(
        consent,
        credential_refs,
        credential_store,
        ConsumerKind::TaskAgent,
        CapabilityKind::Dialogue,
        PurposeKind::TaskAgentTurn,
        Some(task_agent),
    )
    .await
}

/// Shared preparation for one consumer's admission.
///
/// An absent consent, a consent whose credential ref is not registered, or a
/// ref whose bearer is missing answers [`PreparedAdmission::Declined`]
/// immediately: there is no partial setup to authorize.
async fn prepare_admission(
    consent: &impl ConsentRepository,
    credential_refs: &impl CredentialRefRepository,
    credential_store: &impl CredentialStore,
    consumer: ConsumerKind,
    capability: CapabilityKind,
    purpose: PurposeKind,
    task_agent: Option<TaskAgentAttemptPremise>,
) -> Result<PreparedAdmission, InferenceTechnicalError> {
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
    task_agent: Option<TaskAgentAttemptPremise>,
}

impl AuthorizedInference {
    /// Consent premise as `(id, rev)`.
    #[must_use]
    pub fn consent_premise(&self) -> (&str, u64) {
        (&self.consent.0, self.consent.1.as_u64())
    }

    /// Task Agent correlation carried through the claim, when present.
    #[must_use]
    pub fn task_agent_premise(&self) -> Option<&TaskAgentAttemptPremise> {
        self.task_agent.as_ref()
    }
}

/// Cooperative, best-effort abort signal for one in-flight dispatch.
///
/// The token is local and non-durable: it survives only in this process, and
/// signalling it proves nothing about a provider request or an external
/// effect. It exists so a caller can stop a claimed dispatch without losing
/// the attempt's accounting — dispatch observes it inside the post-claim
/// provider wait, drops the provider future best-effort, records the
/// uncertain usage fact, and only then reports
/// [`InferenceDispatchOutcome::Aborted`]. Dropping the dispatch future from
/// outside the owner boundary is never a substitute: it would skip that
/// accounting.
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
    /// Raises the signal and wakes a dispatch that is waiting.
    ///
    /// `notify_one` (not `notify_waiters`) closes the check-then-wait race:
    /// the permit is stored when the dispatch is between its flag check and
    /// its first `notified` poll, so the wakeup is never lost.
    pub fn abort(&self) {
        self.inner
            .aborted
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.notify.notify_one();
    }

    /// Whether the signal is already raised.
    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.inner.aborted.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Waits until the signal is raised; returns immediately when it already is.
    pub async fn aborted(&self) {
        while !self.is_aborted() {
            self.inner.notify.notified().await;
        }
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
        arrival: InferenceResultArrival,
        /// Whether the post-await consent check still admitted the reply.
        adopted: bool,
    },
    /// No provider I/O was performed by this dispatch and no usage fact was
    /// recorded. A duplicate dispatch of an already-claimed ticket answers
    /// this without sending again.
    NotSent(NotSentReason),
    /// The local abort signal stopped this dispatch. A dispatch that was
    /// already claimed records the uncertain usage fact before answering; if
    /// that fact cannot be recorded, the dispatch fails as a storage
    /// technical error instead of claiming a clean abort. One refused before
    /// the claim records no attempt and no usage fact. This never claims
    /// that a provider request or an external effect stopped.
    Aborted,
}

/// The owner boundary for one inference call.
///
/// `ene-inference` owns permission, credential, attempt, provider, and
/// usage ordering behind this boundary; the caller (companion dialogue,
/// learning formation, ...) only sequences its own durable work between
/// admission and dispatch. Implementations are wired by the Host composition
/// root.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait InferenceExecutor: Send + Sync {
    /// Resolves and authorizes one dialogue-purpose use without sending.
    async fn admit_dialogue(&self) -> Result<Admission, InferenceTechnicalError>;

    /// Resolves and authorizes one learning-formation use without sending.
    ///
    /// A distinct consumer/purpose from dialogue: the result is judged by
    /// Learning, never presented as a dialogue response.
    async fn admit_learning(&self) -> Result<Admission, InferenceTechnicalError>;

    /// Resolves and authorizes one Task Agent turn without sending.
    ///
    /// The dedicated entry point fixes the inherited triple
    /// `(TaskAgent, Dialogue, TaskAgentTurn)`: a Task Agent turn never goes
    /// through [`Self::admit_dialogue`] / [`Self::admit_learning`]. The
    /// `task_agent` premise rides the attempt claim and the durable
    /// correlation.
    async fn admit_task_agent(
        &self,
        task_agent: TaskAgentAttemptPremise,
    ) -> Result<Admission, InferenceTechnicalError>;

    /// Claims the attempt, calls the provider, records usage, and reports
    /// whether adoption consent survived the await.
    ///
    /// `sink` receives provider deltas in order while the call runs; the
    /// returned arrival carries the full text for adoption, so display and
    /// durable adoption stay separate facts.
    ///
    /// `abort` is the caller's local best-effort stop signal, when one
    /// exists. A raised signal refuses before the claim (no attempt, no
    /// provider bytes); a signal that fires during the provider wait drops
    /// the provider future and records the uncertain usage fact before
    /// answering [`InferenceDispatchOutcome::Aborted`], so a claimed
    /// dispatch never loses its accounting.
    async fn dispatch(
        &self,
        authorized: AuthorizedInference,
        prompt: ScrubbedText,
        sink: &mut (dyn DeltaSink + Send),
        abort: Option<&DispatchAbort>,
    ) -> Result<InferenceDispatchOutcome, InferenceTechnicalError>;
}

/// Dispatches one authorized use: input validation, pricing resolution,
/// safe usage estimate, attempt claim with cap admission, provider call,
/// adoption re-check, and usage recording.
///
/// The input cap is checked here, before the durable attempt claim: an
/// over-limit request is a never-sent refusal and must not leave an attempt
/// row behind. The reviewed pricing snapshot for the route is resolved
/// immediately before the claim and travels with the attempt, so the ticket's
/// cost fact is bound to the rate the call ran under and a later catalog
/// revision cannot reprice it; an unreviewed route claims without a rate and
/// settles its cost as Unknown. The provider adapter's safe usage upper bound
/// rides the same claim: when a current cap applies, the claim reserves the
/// bound and compares every applicable cap inside its transaction, so a
/// request that would exceed a cap is refused with zero provider bytes and no
/// attempt row. An applicable cap without a finite bound refuses
/// [`NotSentReason::UsageCapIndeterminate`]; neither case is folded into
/// consent staleness or a storage failure. The prompt's credential-set
/// premise is compared in the same claim transaction, so a prompt scrubbed
/// before a credential became registered is never sent. From the successful
/// claim onward, every path either records usage (uncertain or reported) or
/// reports stale before any provider I/O. A technical provider failure
/// records an unknown-usage fact before propagating: the attempt may have
/// run. A completed call records its reported counts whether or not the reply
/// is adopted; an adoption read failure still records the reported counts
/// before propagating the storage error. A reported fact requires all three
/// counts with cached input a subset of input; any missing, malformed, or
/// inconsistent provider usage settles as [`UsageSource::Unknown`] with all
/// counts absent, never zero. The settlement is the first complete fact for
/// the ticket and cannot be revised by a later duplicate.
///
/// `abort` is the caller's local best-effort stop signal, when one exists.
/// A signal already raised before the claim refuses without claiming
/// anything; a signal that fires during the provider wait drops the provider
/// future best-effort and records the unknown-usage fact for the claimed
/// attempt before answering [`InferenceDispatchOutcome::Aborted`] — a usage
/// write failure on that path is a storage technical error, so a claimed
/// abort can never masquerade as a clean stop with lost accounting. The
/// accounting is never skipped by the abort: only the provider I/O is.
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
    // The pricing snapshot is resolved right before the claim (usage-cost-cap
    // §9): the rate this call runs under is fixed for the ticket and a later
    // catalog revision only affects calls admitted after it. An unpriced
    // route claims without a snapshot; its cost fact settles Unknown instead
    // of guessing a rate, and a catalog defect refuses before any claim.
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
    // The estimate is resolved before the claim from the exact request the
    // adapter will send, and it travels with the attempt: the claim converts
    // it into the reservation upper bound under the admission pricing
    // snapshot. An adapter with no finite bound answers `None`; uncapped
    // routes still send, cap-enabled routes refuse typedly.
    let usage_estimate = transport.usage_estimate(&request);
    // The claim is the linearization point: it reads, compares, and inserts
    // in one short transaction, so a stale consent, a stale credential-set
    // premise, a moved Task Agent delegation/task premise, or an exceeded
    // usage cap fails here before any byte leaves. A store failure is
    // infrastructure, never a refusal.
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
                // The select drops the provider future best-effort; the
                // accounting below is not best-effort. The attempt is
                // claimed, so the call may have run: the unknown-usage fact
                // must be durable before Aborted can promise it, so a
                // storage failure here is a technical error, never a clean
                // abort that silently lost the fact.
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
            // The attempt is claimed, so the call may have run: record the
            // uncertain usage before propagating the technical failure.
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
    // Accounting follows the attempt, so the reported fact is recorded
    // before the adoption read: an adoption read failure must not discard
    // what the provider already spent.
    usage.record_usage(arrival.usage.clone()).await?;
    let adopted = consent_matches(consent, capability, &consent_id, consent_rev).await?;
    Ok(InferenceDispatchOutcome::Completed { arrival, adopted })
}

/// Loads the current consent and checks it still names exactly `id` at `rev`.
///
/// A storage failure is [`InferenceTechnicalError::StorageUnavailable`],
/// never a silent `false`: the caller must distinguish "moved" from
/// "unreadable".
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

/// Unknown-counts fact for an attempt that may have run.
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

/// In-memory provider transport for tests and core integration tests.
///
/// Performs zero I/O: it replays configured text and usage, or a configured
/// failure. Real provider adapters and retry policies live elsewhere.
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
        /// Safe upper bound this fake advertises; `None` mirrors an adapter
        /// that cannot construct a finite bound.
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

        /// Advertises a finite safe upper bound for every request, so a
        /// cap-enabled dispatch can reserve under it.
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
        NotSentReason, PermissionEvaluationId, ProviderRequest, ProviderResponse,
        ProviderTransport, RawUsage, TaskAgentAttemptPremise, UsageCostRecord, UsageEstimate,
        UsageFact, UsageRepository, UsageReservation, UsageSource, dispatch_authorized,
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

    /// An attempt repository whose claim always answers stale, modelling a
    /// credential-set move between scrub and send.
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

    /// An attempt repository whose claim cannot prove a finite safe upper
    /// bound under a current cap.
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

    /// An attempt repository that keeps the claimed attempts for correlation
    /// assertions.
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
        async fn load_usage_cost(
            &self,
            _ticket: InferenceTicketId,
        ) -> Result<Option<UsageCostRecord>, InferenceTechnicalError> {
            Err(InferenceTechnicalError::StorageUnavailable {
                reason: String::from("usage store down"),
            })
        }

        #[expect(
            clippy::unused_async_trait_impl,
            reason = "in-test fake; async matches the repository contract"
        )]
        async fn load_usage_reservation(
            &self,
            _ticket: InferenceTicketId,
        ) -> Result<Option<UsageReservation>, InferenceTechnicalError> {
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
        ) -> Result<u64, InferenceTechnicalError> {
            Err(InferenceTechnicalError::StorageUnavailable {
                reason: String::from("usage store down"),
            })
        }
    }

    /// Transport that counts calls without performing I/O.
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

    /// Transport whose call reports that it started and then waits forever;
    /// dropping the call flips `dropped`, proving the abort ended the
    /// provider future instead of waiting for it.
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
            authorization: PermissionEvaluationId(RawId::new()),
            task_agent: None,
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
            authorization: PermissionEvaluationId(RawId::new()),
            task_agent: Some(premise),
        }
    }

    /// The same authorized premise for an explicit route, so dispatch pricing
    /// is exercised for reviewed and unreviewed provider/model pairs.
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
        // Only the adoption read remains: a different revision there models
        // the consent move landing during the provider await.
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
        let prepared = prepare_learning_admission(&consent, &refs, &credential_store).await;
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
        let prepared = prepare_learning_admission(&consent, &refs, &credential_store).await;
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
        assert_eq!(authorized.task_agent_premise(), Some(&premise));
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
        let prepared = prepare_dialogue_admission(&consent, &refs, &credential_store).await;
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
            prepare_learning_admission(&FixedConsent(None), &refs, &credential_store).await;
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
        let result = prepare_learning_admission(&failing, &refs, &credential_store).await;
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
