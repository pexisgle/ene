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

#![cfg_attr(
    test,
    allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        reason = "test fixtures may unwrap values whose failure would be a fixture bug"
    )
)]

use ene_credential::CredentialRef;
use ene_permission::{ConsentRevision, InferenceUseCandidate, PermissionEvaluationId};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InferenceUseOutcome {
    /// The provider call completed; the arrival carries the result ref.
    SentAndCompleted(InferenceResultRef),
    /// Nothing was sent; the reason names the failing gate.
    NotSent(NotSentReason),
    /// Core capability gating refused the use before dispatch.
    ///
    /// Produced by Host-side pre-checks, never by [`send`].
    InsufficientCapability,
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
    /// `input_text` exceeds [`MAX_INPUT_CHARS`].
    OverLimit,
}

/// Reference to a completed inference result.
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
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the transport impl"
)]
pub trait ProviderTransport: Send + Sync {
    /// Runs one completion with no retries or side effects beyond the call.
    async fn complete(
        &self,
        req: ProviderRequest,
    ) -> Result<ProviderResponse, InferenceTechnicalError>;
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
    /// travels together (never a bare revision).
    pub expected_consent: (String, u64),
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
/// [`NotSentReason::EvaluationConsumed`], and
/// [`InferenceUseOutcome::InsufficientCapability`] are Host-side pre-check
/// outcomes and are never produced here.
pub async fn send(
    cmd: RequestInferenceCommand,
    route_consent_match: bool,
    transport: &impl ProviderTransport,
) -> Result<(InferenceUseOutcome, Option<InferenceResultArrival>), InferenceTechnicalError> {
    if cmd.route.provider != cmd.candidate.provider_ref || cmd.route.model != cmd.candidate.model {
        return Ok((
            InferenceUseOutcome::NotSent(NotSentReason::ConsentMismatch),
            None,
        ));
    }
    if !route_consent_match {
        return Ok((
            InferenceUseOutcome::NotSent(NotSentReason::ConsentMismatch),
            None,
        ));
    }
    if cmd.input_text.chars().count() > MAX_INPUT_CHARS {
        return Ok((InferenceUseOutcome::NotSent(NotSentReason::OverLimit), None));
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
    Ok((
        InferenceUseOutcome::SentAndCompleted(InferenceResultRef { ticket: cmd.ticket }),
        Some(arrival),
    ))
}

/// In-memory provider transport for tests and core integration tests.
///
/// Performs zero I/O: it replays configured text and usage, or a configured
/// failure. Provider adapters and retry policies are a behaviors-stage
/// concern, not here.
pub mod fake {
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
        #[expect(
            clippy::unused_async_trait_impl,
            reason = "zero-I/O replay has nothing to await; async is required by the trait"
        )]
        async fn complete(
            &self,
            _req: ProviderRequest,
        ) -> Result<ProviderResponse, InferenceTechnicalError> {
            if let Some(fail) = &self.fail {
                return Err(match fail {
                    FakeFailure::Transport(reason) => {
                        InferenceTechnicalError::ProviderTransportFailed(reason.clone())
                    }
                    FakeFailure::ResponseLost => InferenceTechnicalError::ResponseLost,
                });
            }
            Ok(ProviderResponse {
                text: self.text.clone(),
                usage: self.usage,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeFailure, FakeProviderTransport};
    use super::{
        InferenceResultArrival, InferenceTicketId, InferenceUseOutcome, NotSentReason, RawUsage,
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
        let (outcome, arrival) = send(cmd, true, &transport)
            .await
            .expect("inference dispatch answers an outcome");
        assert!(matches!(outcome, InferenceUseOutcome::SentAndCompleted(_)));
        let arrival = arrival.expect("a completed send carries an arrival");
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
        let (outcome, arrival) = send(cmd, true, &transport)
            .await
            .expect("inference dispatch answers an outcome");
        assert!(matches!(outcome, InferenceUseOutcome::SentAndCompleted(_)));
        let arrival = arrival.expect("a completed send carries an arrival");
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
        let (outcome, arrival) = send(cmd, true, &transport)
            .await
            .expect("inference dispatch answers an outcome");
        assert_eq!(
            outcome,
            InferenceUseOutcome::NotSent(NotSentReason::ConsentMismatch)
        );
        assert_eq!(arrival, None);
    }

    #[tokio::test]
    async fn failed_consent_premise_is_not_sent() {
        let cmd = command("hello");
        let transport = FakeProviderTransport::new("hi there".to_owned(), None);
        let (outcome, arrival) = send(cmd, false, &transport)
            .await
            .expect("inference dispatch answers an outcome");
        assert_eq!(
            outcome,
            InferenceUseOutcome::NotSent(NotSentReason::ConsentMismatch)
        );
        assert_eq!(arrival, None);
    }

    #[tokio::test]
    async fn over_limit_input_is_not_sent() {
        let big: String = "x".repeat(super::MAX_INPUT_CHARS + 1);
        let cmd = command(&big);
        let transport = FakeProviderTransport::new("hi there".to_owned(), None);
        let (outcome, arrival) = send(cmd, true, &transport)
            .await
            .expect("inference dispatch answers an outcome");
        assert_eq!(
            outcome,
            InferenceUseOutcome::NotSent(NotSentReason::OverLimit)
        );
        assert_eq!(arrival, None);
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
