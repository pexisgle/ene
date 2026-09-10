//! One-to-one text round trip: intake, reply, stream, ack, timeline.
//!
//! `HostHandle::submit_text` runs the full accepted-input pipeline: durable
//! idempotent replay, presence attach, intake evaluation, owner append, live
//! authorization, inference dispatch, reply append with undelivered
//! registration, usage recording, and the response stream.
//! `HostHandle::confirm_presentation` applies presentation observations, and
//! `HostHandle::answer_history` restores the filtered timeline.
//!
//! `Stage 2` wire reason vocabulary for
//! [`NeedsRevalidation`](ene_api::v1::round::RoundIntakeOutcomeWire::NeedsRevalidation)
//! outcomes: `"missing-generation-view"`, `"unknown-companion"`,
//! `"stopped-companion"` (all straight from
//! [`ene_presentation::RevalidationReason`]), plus `"setup-incomplete"` and
//! `"consent-stale"` for the authorization gates and `"not-in-allowlist"` as a
//! defensive closed-world denial. `"unknown-reason"` is defensive only:
//! [`ene_presentation::check_intake`] never emits its source variant.
//!
//! Infallible-frame mapping used here (no `Result`: [`HostHandle::handle_frame`]
//! answers every frame):
//!
//! - Store failures before acceptance become
//!   [`HeldForTransition`](ene_api::v1::round::RoundIntakeOutcomeWire::HeldForTransition):
//!   no work started, so a later retry is safe.
//! - A reused command key with a different [`RequestFingerprint`] becomes the
//!   typed [`Reject`](ene_api::v1::payload::WirePayload::Reject)
//!   (`ConflictingCommand`), judged by one fingerprint comparison shared
//!   with the store's in-transaction pre-check: declined without side
//!   effects, never an intake outcome, never a retry signal.
//! - A stale or held owner append becomes the matching outcome frame. Its
//!   projection entry stays mapped but unpublished: no open-round record was
//!   made and no ack carried it, so later intakes surface the round as stale
//!   rather than rebinding anything onto it.
//! - Permission denial becomes `NeedsRevalidation` with the setup/consent
//!   reason above: the Client recovers by running the setup flow, then retries
//!   with a fresh local id.
//! - Any failure after acceptance (inference not sent, transport error, reply
//!   append lost) becomes the accept ack plus a stream closed as
//!   [`Interrupted`](ene_api::v1::round::StreamClose::Interrupted). Usage
//!   accounting follows certainty, never adoption: never-sent calls record
//!   no fact, uncertain attempts record
//!   [`Unknown`](ene_inference::UsageSource::Unknown) counts, and reported
//!   counts are kept even when the reply cannot be adopted. A usage-record
//!   failure after a durable reply keeps the `Completed` close: the reply
//!   happened, and the usage gap is the documented `Stage 2` follow-up
//!   (retry queue), not a reason to misreport the stream.
//! - Presentation observations and unresolvable confirmation rounds produce no
//!   reply: confirmation is an observation, never a report of completion.

use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::RevalidationReasonWire;
use ene_api::v1::refs::{CommandWireId, RoundWireId, StreamWireId};
use ene_api::v1::reject::RejectKind;
use ene_api::v1::round::{
    ConfirmPresentationWire, HistoryItem, HistoryRequest, HistoryRole as HistoryRoleWire,
    HistoryView, PresentationStatus, RoundIntakeOutcomeWire, StreamClose, SubmitTextInput,
    TextStreamClose, TextStreamFrameWire, TextStreamOpen,
};
use ene_companion::{
    AppendHistoryCommand, CommandId, CompanionId, CompanionLifecycle, CompanionRepository,
    HistoryAppendOutcome, HistoryRepository, HistoryRole, PresentationMark, ReportStatus,
    RequestFingerprint, RoundIntentMark, UndeliveredRepository,
};
use ene_credential::{CredentialRef, CredentialRefRepository, credential_availability};
use ene_inference::{
    AttemptBeginOutcome, DispatchResult, InferenceAttempt, InferenceAttemptRepository,
    InferenceTicketId, ProviderTransport, RequestInferenceCommand, ResolvedRoute, UsageFact,
    UsageSource, send,
};
use ene_permission::{
    CapabilityKind, CheckLiveAuthorizationQuery, ConsentRepository, ConsentRevision, ConsumerKind,
    DenyCode, InferenceUseCandidate, LiveAuthorizationDecision, PurposeKind,
    check_live_authorization,
};
use ene_plugin_ipc::WireFrame;
use ene_presence::{
    ClientId, ConfirmTransitionOutcome, LiveReachabilityRef, MoveDecision, PresenceAttribution,
    PresenceCheckRef, PresenceGeneration, PresenceRepository, PresenceState, ThinMoveReason,
};
use ene_presentation::{
    ClientInputRef, CompanionAvailability, IntakePremise, OpenRound, RevalidationReason, RoundId,
    RoundIntakeOutcome, RoundIntent, SubmitClientInputCandidate, check_intake,
};
use ene_primitive::{RawId, WallClockWithTz};
use ene_store::Store;

use crate::serve::{
    HostHandle, LiveInput, device_client, outgoing_frame, reject_frame, unpaired_close,
};
use crate::setup::default_credential;

/// Maximum stream chunk size in Unicode scalar values.
///
/// Chunking walks `char` boundaries, so a chunk never splits a code point;
/// multi-byte text stays intact at the cost of byte-uneven frames.
pub const CHUNK_CHARS: usize = 200;

/// Splits response text into [`CHUNK_CHARS`]-sized stream deltas.
///
/// Char-boundary safe by construction (iteration is over `chars`). Always
/// returns at least one chunk: empty text yields one empty delta so every
/// stream carries a final frame (a frame without `is_final` never completes
/// anything, and an empty stream would leave completion ambiguous).
#[must_use]
pub fn chunk_text(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0_usize;
    for next in text.chars() {
        if count == CHUNK_CHARS {
            chunks.push(std::mem::take(&mut current));
            count = 0;
        }
        current.push(next);
        count += 1;
    }
    chunks.push(current);
    chunks
}

/// Maps the envelope's client-minted command ID to the durable idempotency
/// identity, if it parses as a UUID.
///
/// Garbage maps to [`None`] (no replay key) rather than rejection: a
/// malformed key only degrades that sender's own idempotency, and every
/// well-formed client mints fresh UUIDs. Transport retry reuses the same
/// command ID with a fresh message ID within one sender incarnation; the
/// store answers replays with the original acceptance instead of
/// re-appending.
fn command_id_for(envelope: &ene_api::v1::envelope::WireEnvelope) -> Option<CommandId> {
    let CommandWireId(id) = envelope.correlation.command_id?;
    Some(CommandId(ene_primitive::RawId::from_uuid(id)))
}

/// How one send attempt resolved, for usage-accounting purposes only.
///
/// Result adoption and usage accounting stay separate: the provider may
/// have spent tokens even when the Host cannot adopt the reply, and a
/// never-attempted call spends nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendOutcomeClass<'a> {
    /// The attempt may have run (transport error, including timeouts):
    /// certainty is unknown, never zero.
    AttemptUncertain,
    /// The call definitely never ran (pre-send refusal): no provider use,
    /// so no fact is recorded at all.
    NeverSent,
    /// The provider completed and reported: the known fact is recorded
    /// against its ticket whether or not the reply is adopted.
    Reported(&'a UsageFact),
}

/// Decides the usage fact for one finished send attempt, if any.
///
/// `None` records nothing (definitely no provider use); `Some` records the
/// fact — known counts when reported, unknown counts when the attempt is
/// uncertain. Never zero-as-unknown: unknown counts travel as [`None`].
fn usage_for_disposition(
    ticket: InferenceTicketId,
    provider: &str,
    model: &str,
    outcome: SendOutcomeClass<'_>,
) -> Option<UsageFact> {
    match outcome {
        SendOutcomeClass::AttemptUncertain => Some(UsageFact {
            ticket,
            provider: provider.to_string(),
            model: model.to_string(),
            input_tokens: None,
            output_tokens: None,
            source: UsageSource::Unknown,
        }),
        SendOutcomeClass::NeverSent => None,
        SendOutcomeClass::Reported(usage) => Some(usage.clone()),
    }
}

/// Canonical client round premise of one [`SubmitTextInput`] send.
///
/// One rule covers every carrier. The payload names the premise (IPC §21
/// maps the intake candidate's round from `SubmitTextInput.round`); the
/// envelope `round_view` is the mirror the Client relied on (IPC §5:
/// comparison material, not a claim) and must agree with the payload
/// premise once it is populated. A disagreement means the frame carries
/// two different round premises: neither side is adopted — the caller
/// answers stale with current values, and the Client re-syncs.
///
/// A force-new request is the design's round-less new-round request
/// (IPC §13.1: `round = None`, `round_view = None`), so it names no
/// premise at all. A force-new frame carrying a premise in either carrier
/// is self-contradictory and is rejected here, never silently
/// reinterpreted as the flag or joined on the hint.
enum RoundPremise {
    /// A round-less request: join-or-mint.
    Auto,
    /// Join the round this Client-supplied reference names.
    Existing(String),
}

fn canonical_round_premise(
    submit: &SubmitTextInput,
    round_view: Option<&RoundWireId>,
) -> Option<RoundPremise> {
    match (&submit.round, round_view) {
        (None, None) => Some(RoundPremise::Auto),
        (Some(round), None) if !submit.fresh => Some(RoundPremise::Existing(round.0.clone())),
        (Some(round), Some(view)) if !submit.fresh && view == round => {
            Some(RoundPremise::Existing(round.0.clone()))
        }
        _ => None,
    }
}

fn deny_reason(code: DenyCode) -> &'static str {
    match code {
        DenyCode::SetupIncomplete => "setup-incomplete",
        DenyCode::ConsentStale | DenyCode::Superseded => "consent-stale",
        DenyCode::NotInAllowlist => "not-in-allowlist",
    }
}

/// Maps an intake revalidation reason to the `Stage 2` wire vocabulary.
fn intake_reason(reason: &RevalidationReason) -> &'static str {
    match reason {
        RevalidationReason::MissingGenerationView => "missing-generation-view",
        RevalidationReason::UnknownCompanion => "unknown-companion",
        RevalidationReason::StoppedCompanion => "stopped-companion",
        RevalidationReason::MissingCommandId => "missing-command-id",
        RevalidationReason::UnknownReasonTag => "unknown-reason",
    }
}

/// Builds an accept ack for a Host-issued round.
fn accept_frame(frame: &WireFrame, live: &LiveInput, round: &RoundWireId) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound {
            round: round.clone(),
        }),
    )
}

/// Builds a stream opening for a round at a presence generation.
fn open_frame(
    frame: &WireFrame,
    live: &LiveInput,
    stream: &StreamWireId,
    round: &RoundWireId,
    generation: u64,
) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::TextStreamOpen(TextStreamOpen {
            stream: *stream,
            round: round.clone(),
            generation,
        }),
    )
}

/// Builds a stream close record.
fn close_frame(
    frame: &WireFrame,
    live: &LiveInput,
    stream: &StreamWireId,
    status: StreamClose,
) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::TextStreamClose(TextStreamClose {
            stream: *stream,
            status,
        }),
    )
}

/// Builds the accept-plus-interrupted sequence for a post-accept failure.
fn interrupted_frames(
    frame: &WireFrame,
    live: &LiveInput,
    round: &RoundWireId,
    generation: u64,
) -> Vec<WireFrame> {
    let stream = StreamWireId(RawId::new().as_uuid());
    vec![
        accept_frame(frame, live, round),
        open_frame(frame, live, &stream, round, generation),
        close_frame(frame, live, &stream, StreamClose::Interrupted),
    ]
}

/// Builds a stale-round outcome frame with explicit current values.
fn stale_frame_with(
    frame: &WireFrame,
    live: &LiveInput,
    current_round: Option<RoundWireId>,
    current_generation: u64,
) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound {
            current_round,
            current_generation,
        }),
    )
}

/// Builds a held-for-transition outcome frame.
fn held_frame(frame: &WireFrame, live: &LiveInput) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::HeldForTransition),
    )
}

/// Builds a needs-revalidation outcome frame with a fixed wire reason.
fn revalidate_frame(frame: &WireFrame, live: &LiveInput, reason: &str) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::NeedsRevalidation {
            reason: RevalidationReasonWire(reason.to_string()),
        }),
    )
}

/// Outcome of one presence attach compare-and-commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachOutcome {
    /// The compare won and the confirm committed: carries the fresh fact.
    ///
    /// Only this arm lets the caller proceed with the new generation; the
    /// generation comes from the committed fact, never from an assumption
    /// that the attach succeeded.
    Attached(PresenceAttribution),
    /// The compare lost (or the store failed): the caller reloads and
    /// reports the resulting state honestly instead of proceeding.
    Raced,
}

impl HostHandle {
    /// Returns the open round wire ref for a client/companion pair, if any.
    pub(crate) fn open_wire_for(
        &self,
        client_ref: &str,
        companion_key: &str,
    ) -> Option<RoundWireId> {
        let open = self.open_round_for(client_ref, companion_key)?;
        let wire = self.wire_for_round(&open.round)?;
        Some(RoundWireId(wire))
    }

    /// Builds a stale-round outcome frame from the current attribution.
    pub(crate) fn stale_frame(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        client_ref: &str,
        companion_key: &str,
        generation: u64,
    ) -> WireFrame {
        let current_round = self.open_wire_for(client_ref, companion_key);
        stale_frame_with(frame, live, current_round, generation)
    }

    /// Records an unknown-usage fact for a ticket whose provider attempt has
    /// uncertain outcome.
    ///
    /// Only for attempts that may have run (transport errors, including
    /// timeouts where the call may have executed): a ticket that was
    /// definitely never sent records NO fact (see [`usage_for_disposition`]),
    /// and a completed attempt records its reported counts even when the
    /// reply cannot be adopted. Best-effort post-accept bookkeeping: when
    /// the store itself rejects the record, the stream close already
    /// returned stays authoritative and the usage gap becomes the documented
    /// `Stage 2` follow-up.
    pub(crate) async fn record_unknown_usage(
        &self,
        ticket: InferenceTicketId,
        provider: &str,
        model: &str,
    ) {
        self.record_usage_decision(usage_for_disposition(
            ticket,
            provider,
            model,
            SendOutcomeClass::AttemptUncertain,
        ))
        .await;
    }

    /// Records a decided usage fact, if any.
    ///
    /// [`None`] (definitely no provider use) stores nothing; [`Some`]
    /// stores the fact best-effort, with the already-returned stream close
    /// staying authoritative on store failure.
    pub(crate) async fn record_usage_decision(&self, decided: Option<UsageFact>) {
        use ene_inference::UsageRepository;
        let Some(fact) = decided else {
            return;
        };
        if self.store.record_usage(fact).await.is_err() {
            // The stream close is authoritative; usage persistence retries
            // belong to later milestone work, not to this frame.
        }
    }

    /// Attaches presence for the paired device when none is active..
    ///
    /// Best-effort by design and called only from the submit path: the caller
    /// pins the `NoActive` view it just read (`expected_generation` must equal
    /// the current generation, and the compare additionally pins the
    /// `NoActive`/unowned shape), and only the compare winner proceeds — the
    /// returned [`AttachOutcome::Attached`] fact carries the fresh generation
    /// the winner proceeds with. A lost compare race, a denied or held
    /// outcome, or a store failure all answer
    /// [`AttachOutcome::Raced`]: the caller reloads and reports the resulting
    /// state honestly instead of proceeding. The [`ClientId`] comes from the
    /// deterministic device mapping, so a re-attaching device re-derives the
    /// same id. No reply is produced here; the caller re-reads attribution
    /// only on the raced path.
    pub(crate) async fn attach_presence(
        &self,
        device_wire: &str,
        connection_live: bool,
        expected_generation: PresenceGeneration,
    ) -> AttachOutcome {
        let client = device_client(device_wire);
        let Ok(companion) = self.store.ensure_running_companion().await else {
            return AttachOutcome::Raced;
        };
        attach_from(
            &self.store,
            companion.as_raw(),
            PresenceCheckRef {
                expected_generation,
                expected_state: PresenceState::NoActive,
                expected_active: None,
            },
            client,
            connection_live,
        )
        .await
    }

    /// Reloads the consent record and checks it still names the authorized
    /// route. Both the pre-send and the adoption gate funnel through here:
    /// short compare-before-commit reads around the long provider await, so
    /// a mid-flight consent move cannot silently ride on a stale check.
    /// Store failures fail closed (`false`).
    async fn consent_matches(&self, id: &str, rev: ConsentRevision) -> bool {
        let Ok(Some(current)) = self.store.load_current().await else {
            return false;
        };
        current.id == id && current.rev == rev
    }

    /// Runs the submit pipeline for one [`SubmitTextInput`] frame.
    ///
    /// Order: companion mapping, mandatory command key, durable idempotent
    /// replay, presence attach, intake evaluation, setup/consent/credential
    /// admission (live authorization included), owner append, transient
    /// round recording, inference dispatch, reply append with undelivered
    /// registration, usage recording, then the response stream. Admission
    /// precedes the append so a declined input leaves neither history rows
    /// nor transient round claims behind; the round projection is minted
    /// atomically with its map entry (one domain round, one wire), and a
    /// racy duplicate that lands on
    /// [`HistoryAppendOutcome::AlreadyCommittedAs`] answers the original
    /// accept without re-running inference. The inbound companion ref
    /// resolves through [`HostHandle::resolve_companion`]: presence facts
    /// issue the projection the Client echoes back. The canonical round
    /// premise comes from the input `round`, a populated envelope
    /// `round_view` must agree with it, and a force-new request carries no
    /// premise at all — a contradictory frame (disagreement, or a premise
    /// under `fresh`) answers stale with current values instead of
    /// adopting either side; a present-but-unresolvable round is stale,
    /// never rebound.
    ///
    /// Presence attach runs only when the loaded attribution is `NoActive`,
    /// and only on the envelope's observed generation premise: a missing
    /// `presence_generation_view` answers `NeedsRevalidation` with
    /// `"missing-generation-view"` (no attach is attempted), a view that does
    /// not equal the current `NoActive` generation answers `StaleRound` with
    /// the current values, and only then does the compare-and-commit run with
    /// the observed `(NoActive, generation)` expectation. Only the compare
    /// winner proceeds, with the fresh generation from the committed fact
    /// (never an assumed one); a lost race reloads and answers
    /// `StaleRound`/`HeldForTransition` as appropriate.
    ///
    /// Idempotency is durable over the envelope `command_id`: a parseable
    /// command id first looks up the history row through
    /// [`lookup_command`](HistoryRepository::lookup_command). A hit is
    /// judged by the same [`RequestFingerprint`] the store compares
    /// in-transaction — role, body, language, sender incarnation, and the
    /// canonical round intent, never the Host-decided round or its
    /// projection — so an exact retry replays the stored accept ack
    /// verbatim without re-appending or re-streaming anything, including
    /// after a restart. A hit with a different request (or one whose
    /// fingerprint cannot be reconstructed) answers a typed wire rejection
    /// (`ConflictingCommand`), never an intake outcome. Stream outcome
    /// replay is explicitly out of scope: only the accept ack replays. An
    /// unparsable or missing command id carries no replay key: it is
    /// declined before any state changes. Response text is never presented
    /// unless its reply append committed.
    pub(crate) async fn submit_text(
        &self,
        frame: &WireFrame,
        submit: &SubmitTextInput,
        live: &LiveInput,
        transport: &impl ProviderTransport,
    ) -> Vec<WireFrame> {
        let Some(device_wire) = live.paired_device.clone() else {
            return vec![unpaired_close(frame, live)];
        };
        let client = device_client(&device_wire);
        // The inbound companion ref resolves through the handle mapping —
        // never assumed, never derived. An unknown ref (including a
        // projection rotated by a restart) revalidates so the Client
        // relearns the current projection from presence and converges.
        let companion = match self.resolve_companion(&submit.companion.0).await {
            Err(_) => return vec![held_frame(frame, live)],
            Ok(None) => {
                return vec![revalidate_frame(
                    frame,
                    live,
                    intake_reason(&RevalidationReason::UnknownCompanion),
                )];
            }
            Ok(Some(companion)) => companion,
        };
        let Ok(Some(mut attribution)) = self.store.load_attribution(companion.as_raw()).await
        else {
            return vec![held_frame(frame, live)];
        };
        let companion_key = companion.as_raw().as_uuid().to_string();
        // Idempotency keys are mandatory: a command without one cannot be
        // replayed safely, so it is declined before any state changes
        // (before attach, before maps, before appends).
        let Some(command) = command_id_for(&frame.envelope) else {
            return vec![revalidate_frame(
                frame,
                live,
                intake_reason(&RevalidationReason::MissingCommandId),
            )];
        };
        // Canonical round premise first: the two carriers must agree before
        // anything else is judged, so a contradictory frame is declined
        // stale with current values instead of adopting one side.
        let Some(round_premise) =
            canonical_round_premise(submit, frame.envelope.observed.round_view.as_ref())
        else {
            return vec![self.stale_frame(
                frame,
                live,
                &live.client_ref,
                &companion_key,
                attribution.generation.as_u64(),
            )];
        };
        // The request fingerprint is the immutable client semantics: role,
        // body, language, sending incarnation, and the canonical round
        // intent. `fresh` requests the design's round-less new-round shape,
        // so a force-new frame carries no premise (the gate above declined
        // one); otherwise the premise decides.
        let round_intent = if submit.fresh {
            RoundIntentMark::New
        } else {
            match &round_premise {
                RoundPremise::Auto => RoundIntentMark::Auto,
                RoundPremise::Existing(reference) => RoundIntentMark::Existing(reference.clone()),
            }
        };
        let incoming_fingerprint = RequestFingerprint {
            role: HistoryRole::Owner,
            text: submit.body.text.clone(),
            lang: submit.body.lang.0.clone(),
            incarnation: Some((
                frame.envelope.sender.incarnation_id.counter,
                frame.envelope.sender.incarnation_id.random,
            )),
            round_intent: round_intent.clone(),
        };
        // Durable replay precedes presence attach: an exact retry answers
        // from the stored marker without advancing presence generation or
        // touching any other state, while a conflicting reuse rejects just
        // as early. One comparison, one judge: the same
        // [`RequestFingerprint`] value the store reconstructs
        // in-transaction; a stored row without one proves nothing and is
        // declined the same way (fail-closed).
        match self.store.lookup_command(companion, &command).await {
            Err(_) => return vec![held_frame(frame, live)],
            Ok(Some(found)) => {
                let replays = found
                    .request_fingerprint()
                    .is_some_and(|stored| stored == incoming_fingerprint);
                if !replays {
                    return vec![reject_frame(
                        frame,
                        live,
                        RejectKind::ConflictingCommand,
                        format!(
                            "command {} reused with a different request",
                            command.0.as_uuid().as_hyphenated()
                        ),
                    )];
                }
                return self
                    .replay_accept(
                        frame,
                        live,
                        companion,
                        &command,
                        attribution.generation.as_u64(),
                    )
                    .await;
            }
            Ok(None) => {}
        }
        // The winner's intake premise below carries the fresh generation from
        // the committed fact. Any other path carries the envelope view
        // untouched: intake reports a missing or mismatched view honestly.
        let mut attached_generation: Option<PresenceGeneration> = None;
        if attribution.state == PresenceState::NoActive {
            let Some(viewed) = frame.envelope.observed.presence_generation_view else {
                return vec![revalidate_frame(frame, live, "missing-generation-view")];
            };
            if viewed != attribution.generation.as_u64() {
                return vec![self.stale_frame(
                    frame,
                    live,
                    &live.client_ref,
                    &companion_key,
                    attribution.generation.as_u64(),
                )];
            }
            match self
                .attach_presence(&device_wire, live.connection_live, attribution.generation)
                .await
            {
                AttachOutcome::Attached(fresh) => {
                    attached_generation = Some(fresh.generation);
                    attribution = fresh;
                }
                AttachOutcome::Raced => {
                    let Ok(Some(current)) = self.store.load_attribution(companion.as_raw()).await
                    else {
                        return vec![held_frame(frame, live)];
                    };
                    if matches!(
                        current.state,
                        PresenceState::InTransition | PresenceState::RecoveryWait
                    ) {
                        return vec![held_frame(frame, live)];
                    }
                    return vec![self.stale_frame(
                        frame,
                        live,
                        &live.client_ref,
                        &companion_key,
                        current.generation.as_u64(),
                    )];
                }
            }
        }
        let requested = match &round_premise {
            RoundPremise::Auto => None,
            RoundPremise::Existing(reference) => match self.round_for(reference) {
                Some(round) => Some(round),
                None => {
                    return vec![self.stale_frame(
                        frame,
                        live,
                        &live.client_ref,
                        &companion_key,
                        attribution.generation.as_u64(),
                    )];
                }
            },
        };
        // One meaning per value, matching the fingerprint's round intent:
        // a force-new request mints and never joins (the CLI rejects
        // combining `--new` with `--round`, and the premise gate above
        // already declined a premise-carrying force-new frame); a resolved
        // premise joins that round, and no premise joins-or-mints.
        let intent = if submit.fresh {
            RoundIntent::New
        } else {
            requested.map_or(RoundIntent::Auto, RoundIntent::Existing)
        };
        let Ok(lifecycle) = self.store.load_lifecycle(companion).await else {
            return vec![held_frame(frame, live)];
        };
        let premise = IntakePremise {
            candidate: SubmitClientInputCandidate {
                companion: companion.as_raw(),
                client,
                claimed_generation: attached_generation.or_else(|| {
                    frame
                        .envelope
                        .observed
                        .presence_generation_view
                        .map(PresenceGeneration::from_u64)
                }),
                round: intent,
                input_ref: ClientInputRef {
                    text: submit.body.text.clone(),
                    lang: submit.body.lang.0.clone(),
                },
                local_id: submit.local_id.0.clone(),
            },
            attribution,
            companion: match lifecycle {
                Some(CompanionLifecycle::Running) => CompanionAvailability::Running,
                Some(_) => CompanionAvailability::Stopped,
                None => CompanionAvailability::Unknown,
            },
            live: LiveReachabilityRef {
                client,
                connection_live: live.connection_live,
            },
            open_round: self.open_round_for(&live.client_ref, &companion_key),
        };
        let accepted = match check_intake(premise) {
            RoundIntakeOutcome::AcceptedForRound { round } => round,
            RoundIntakeOutcome::StaleRound { .. } => {
                return vec![self.stale_frame(
                    frame,
                    live,
                    &live.client_ref,
                    &companion_key,
                    attribution.generation.as_u64(),
                )];
            }
            RoundIntakeOutcome::HeldForTransition => return vec![held_frame(frame, live)],
            RoundIntakeOutcome::NeedsRevalidation { reason } => {
                return vec![revalidate_frame(frame, live, intake_reason(&reason))];
            }
        };
        let Ok(stored_consent) = self.store.load_current().await else {
            return vec![held_frame(frame, live)];
        };
        let Some(consent) = stored_consent else {
            return vec![revalidate_frame(frame, live, "setup-incomplete")];
        };
        let Ok(known_refs) = self.store.list_refs().await else {
            return vec![held_frame(frame, live)];
        };
        let credential = match known_refs
            .iter()
            .find(|known| known.id() == consent.credential_id)
            .cloned()
        {
            Some(known) => known,
            None => CredentialRef::new(consent.provider.clone(), "main")
                .unwrap_or_else(|_| default_credential()),
        };
        let repo_known = known_refs
            .iter()
            .any(|known| known.id() == consent.credential_id);
        let setup_complete =
            credential_availability(&credential, repo_known, &self.cred_store).present;
        let candidate = InferenceUseCandidate {
            consumer: ConsumerKind::CompanionDialogue,
            capability: CapabilityKind::Dialogue,
            provider_ref: consent.provider.clone(),
            model: consent.model.clone(),
            purpose: PurposeKind::DialogueResponse,
        };
        let query = CheckLiveAuthorizationQuery {
            candidate: candidate.clone(),
            expected_consent: Some((consent.id.clone(), consent.rev)),
            setup_complete,
        };
        let authorization = {
            let mut tracker = self.tracker.lock().await;
            let decision = check_live_authorization(&query, Some(&consent), &mut tracker);
            match decision {
                LiveAuthorizationDecision::AllowForThisUse(authorization) => {
                    if tracker.consume(&authorization, &candidate.fingerprint()) {
                        Some(authorization)
                    } else {
                        None
                    }
                }
                LiveAuthorizationDecision::Deny(reason) => {
                    return vec![revalidate_frame(frame, live, deny_reason(reason.code))];
                }
                LiveAuthorizationDecision::NeedsRevalidation(_) => {
                    return vec![revalidate_frame(frame, live, "consent-stale")];
                }
            }
        };
        let Some(authorization) = authorization else {
            return vec![revalidate_frame(frame, live, "evaluation-consumed")];
        };
        // One projection per round: the atomic get-or-create below reuses
        // an already-mapped round's wire, so every message in the round —
        // and every replay — names the same projection; only an unmapped
        // (freshly minted) round mints. The mint may land before the
        // durable append below decides, but the projection is never
        // published on a failed append (no ack or stream carries it), and
        // an unpublished, unguessable mapping entry is not authority:
        // acceptance still comes only from intake plus the durable commit.
        // The map itself stays per-process, dropped by a restart.
        let round_wire = self.round_wire_or_mint(&accepted);
        let sender_incarnation = (
            frame.envelope.sender.incarnation_id.counter,
            frame.envelope.sender.incarnation_id.random,
        );
        let generation_number = attribution.generation.as_u64();
        let owner_cmd = AppendHistoryCommand {
            companion,
            round: accepted.as_raw(),
            role: HistoryRole::Owner,
            text: submit.body.text.clone(),
            lang: submit.body.lang.0.clone(),
            at: WallClockWithTz::now(),
            expected_generation: attribution.generation,
            expected_consent: Some((consent.id.clone(), consent.rev.as_u64())),
            local_id: Some(submit.local_id.0.clone()).filter(|key| !key.is_empty()),
            command_id: Some(command),
            round_wire: Some(round_wire.0.clone()),
            round_intent: Some(round_intent),
            incarnation: Some(sender_incarnation),
        };
        match self.store.append_message(owner_cmd).await {
            Ok(HistoryAppendOutcome::CommittedAs { .. }) => {}
            Ok(HistoryAppendOutcome::AlreadyCommittedAs { .. }) => {
                // Lost the race with a concurrent same-command submit after
                // the lookup above: resolve durably so the ack survives
                // restarts like any other replay.
                return self
                    .replay_accept(frame, live, companion, &command, generation_number)
                    .await;
            }
            Ok(HistoryAppendOutcome::StaleExpected { current }) => {
                return vec![stale_frame_with(frame, live, None, current.as_u64())];
            }
            Ok(HistoryAppendOutcome::StaleConsent) => {
                return vec![revalidate_frame(frame, live, "consent-stale")];
            }
            Ok(HistoryAppendOutcome::CommandConflict) => {
                // The key already owns a different request than the stored
                // fingerprint covers — the store decided this
                // in-transaction from the same [`RequestFingerprint`] the
                // early replay compares. Same-send retries never reach
                // here: they replay as `AlreadyCommittedAs` from the stored
                // accept, even across round drift. Declined without side
                // effects, never rebound, never an intake outcome, never a
                // retry signal.
                return vec![reject_frame(
                    frame,
                    live,
                    RejectKind::ConflictingCommand,
                    format!(
                        "command {} reused with a different request",
                        command.0.as_uuid().as_hyphenated()
                    ),
                )];
            }
            Ok(HistoryAppendOutcome::HeldByLifecycle { .. }) => {
                return vec![revalidate_frame(frame, live, "stopped-companion")];
            }
            Err(_) => return vec![held_frame(frame, live)],
        }
        self.record_open_round(
            &live.client_ref,
            &companion_key,
            OpenRound {
                companion: companion.as_raw(),
                client,
                round: accepted,
                generation: attribution.generation,
            },
        );
        let ticket = InferenceTicketId(RawId::new());
        let route = ResolvedRoute {
            provider: consent.provider.clone(),
            model: consent.model.clone(),
            credential: credential.clone(),
            consent: (consent.id.clone(), consent.rev),
        };
        let command = RequestInferenceCommand {
            ticket,
            candidate,
            authorization,
            route,
            input_text: submit.body.text.clone(),
        };
        if !self.consent_matches(&consent.id, consent.rev).await {
            // Never attempted: no provider use, so no usage fact (cf.
            // `usage_for_disposition`).
            return interrupted_frames(frame, live, &round_wire, generation_number);
        }
        // Linearization point: claim this ticket's attempt under the
        // expected consent in one short transaction, then issue provider
        // I/O outside any lock. A mutation that committed first fails the
        // claim stale — no byte leaves; a mutation that commits after only
        // affects result adoption (handled below), never the fact that the
        // attempt started under a verified premise.
        match self
            .store
            .begin_inference_attempt(InferenceAttempt {
                ticket,
                expected_consent: (consent.id.clone(), consent.rev.as_u64()),
                provider: consent.provider.clone(),
                model: consent.model.clone(),
            })
            .await
        {
            Ok(AttemptBeginOutcome::Started) => {}
            Ok(AttemptBeginOutcome::Stale) | Err(_) => {
                // The provider never ran (fail-closed on store failure
                // too): no usage fact, same as never sent.
                return interrupted_frames(frame, live, &round_wire, generation_number);
            }
        }
        // The `true` verdict is the claim above: `send` keeps its own gate
        // as defense in depth, but the durable determination already bound
        // this ticket to its consent premise.
        let send_outcome = send(command, true, transport).await;
        let Ok(dispatch) = send_outcome else {
            self.record_unknown_usage(ticket, &consent.provider, &consent.model)
                .await;
            return interrupted_frames(frame, live, &round_wire, generation_number);
        };
        let DispatchResult::Completed(arrival) = dispatch else {
            // Definitely never sent: the decision table records no fact.
            self.record_usage_decision(usage_for_disposition(
                ticket,
                &consent.provider,
                &consent.model,
                SendOutcomeClass::NeverSent,
            ))
            .await;
            return interrupted_frames(frame, live, &round_wire, generation_number);
        };
        if !self.consent_matches(&consent.id, consent.rev).await {
            // The provider ran and reported: adoption failed, accounting did
            // not. The known fact stays against its ticket even though the
            // reply is not adopted.
            self.record_usage_decision(usage_for_disposition(
                ticket,
                &consent.provider,
                &consent.model,
                SendOutcomeClass::Reported(&arrival.usage),
            ))
            .await;
            return interrupted_frames(frame, live, &round_wire, generation_number);
        }
        let reply_cmd = AppendHistoryCommand {
            companion,
            round: accepted.as_raw(),
            role: HistoryRole::Companion,
            text: arrival.output_text.clone(),
            lang: submit.body.lang.0.clone(),
            at: WallClockWithTz::now(),
            expected_generation: attribution.generation,
            expected_consent: Some((consent.id.clone(), consent.rev.as_u64())),
            local_id: None,
            command_id: None,
            // Same round, same projection: the reply belongs to the accepted
            // round. No command key and no round intent: the reply is
            // Host-produced, never a client command, so it carries no
            // replay key at all.
            round_wire: Some(round_wire.0.clone()),
            round_intent: None,
            incarnation: None,
        };
        if !matches!(
            self.store
                .append_reply_with_undelivered(reply_cmd, true)
                .await,
            Ok((HistoryAppendOutcome::CommittedAs { .. }, _))
        ) {
            // The provider ran and reported but the reply could not be
            // adopted: accounting still records the known fact against
            // its ticket.
            self.record_usage_decision(usage_for_disposition(
                ticket,
                &consent.provider,
                &consent.model,
                SendOutcomeClass::Reported(&arrival.usage),
            ))
            .await;
            return interrupted_frames(frame, live, &round_wire, generation_number);
        }
        // Adopted reply: the reported fact records best-effort; a store
        // failure keeps the `Completed` close (the reply happened).
        self.record_usage_decision(usage_for_disposition(
            ticket,
            &consent.provider,
            &consent.model,
            SendOutcomeClass::Reported(&arrival.usage),
        ))
        .await;
        let stream = StreamWireId(RawId::new().as_uuid());
        let mut responses = vec![
            accept_frame(frame, live, &round_wire),
            open_frame(frame, live, &stream, &round_wire, generation_number),
        ];
        for (position, delta) in chunk_text(&arrival.output_text).iter().enumerate() {
            responses.push(outgoing_frame(
                frame,
                live,
                WirePayload::TextStreamFrame(TextStreamFrameWire {
                    stream,
                    seq: position as u64,
                    delta: delta.clone(),
                    is_final: false,
                }),
            ));
        }
        if let Some(last) = responses.last_mut()
            && let WirePayload::TextStreamFrame(closing) = &mut last.payload
        {
            closing.is_final = true;
        }
        responses.push(close_frame(frame, live, &stream, StreamClose::Completed));
        responses
    }

    /// Applies one presentation observation with no reply.
    ///
    /// Confirmation is an observation, never a report of completion: matching
    /// pending undelivered entries for the round move to presented (or to
    /// presentation-unknown for any non-presented status, including wire
    /// `Failed`), and nothing is answered. An unresolvable round, a missing
    /// companion, a pending-list failure, or a per-row mark failure all end
    /// silently; the durable report state stays authoritative either way.
    pub(crate) async fn confirm_presentation(
        &self,
        _frame: &WireFrame,
        confirm: &ConfirmPresentationWire,
    ) -> Vec<WireFrame> {
        let Some(round) = self.round_for(&confirm.round.0) else {
            return Vec::new();
        };
        let mark = PresentationMark {
            round: round.as_raw(),
            presented: matches!(confirm.status, PresentationStatus::Presented),
        };
        let Ok(companion) = self.store.ensure_running_companion().await else {
            return Vec::new();
        };
        let Ok(pending) = self.store.list_pending(companion).await else {
            return Vec::new();
        };
        for entry in pending {
            if entry.round == mark.round
                && entry.status == ReportStatus::Pending
                && self
                    .store
                    .compare_and_mark_reported(entry.id, ReportStatus::Pending, mark)
                    .await
                    .is_err()
            {
                // One stale or unavailable row never blocks the remaining
                // observations; the store stays authoritative.
            }
        }
        Vec::new()
    }

    /// Answers one [`HistoryRequest`] with the filtered timeline.
    ///
    /// Items map oldest-first with Host-filtered display facts only, never
    /// undelivered reporting. An unparseable `since` bound is ignored (display
    /// filtering only, never currentness evidence); a store failure answers an
    /// empty view, which is the documented `Stage 2` gap (there is no error
    /// DTO on this path, and leaving the request unanswered would be worse).
    pub(crate) async fn answer_history(
        &self,
        frame: &WireFrame,
        request: &HistoryRequest,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        // The timeline resolves through the same companion mapping as
        // submits: an unknown ref (or an unreadable store) answers an empty
        // view, which is the documented `Stage 2` gap for this path.
        let Ok(Some(companion)) = self.resolve_companion(&request.companion.0).await else {
            return vec![empty_history(frame, live)];
        };
        let since = request
            .since
            .as_deref()
            .and_then(|bound| WallClockWithTz::parse_rfc3339(bound).ok());
        let items = self
            .store
            .load_timeline(companion, since, request.limit)
            .await
            .unwrap_or_default();
        let view_items = items
            .iter()
            .map(|item| HistoryItem {
                // The stored projection travels verbatim so views agree with
                // accept acks, including after a restart. Pre-opaque rows
                // fall back to the transient map, then to the legacy domain
                // rendering (continuity for pre-release rows only).
                round: RoundWireId(
                    item.round_wire
                        .clone()
                        .or_else(|| self.wire_for_round_value(item.round))
                        .unwrap_or_else(|| item.round.as_uuid().to_string()),
                ),
                role: match item.role {
                    HistoryRole::Owner => HistoryRoleWire::Owner,
                    HistoryRole::Companion => HistoryRoleWire::Companion,
                },
                text: item.text.clone(),
                at: item.at.to_rfc3339(),
            })
            .collect();
        vec![outgoing_frame(
            frame,
            live,
            WirePayload::HistoryView(HistoryView { items: view_items }),
        )]
    }

    /// Resolves a domain round back to its wire string for the replay path.
    ///
    /// Thin wrapper over the shared map so the replay lookup reads one
    /// vocabulary: [`None`] means the process no longer maps the round and
    /// the caller answers stale.
    fn wire_for_round_value(&self, round: RawId) -> Option<String> {
        self.wire_for_round(&RoundId::from_raw(round))
    }

    /// Answers the original accept ack for a replayed command from durable
    /// state: the stored round wire travels verbatim, so a retry after a
    /// restart replays instead of going stale on the dropped transient map.
    /// Pre-opaque rows (no stored wire) fall back to the transient map; only
    /// when both miss does the intake answer stale with the current
    /// generation, and the Client recovers missed items through history.
    async fn replay_accept(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        companion: CompanionId,
        command: &CommandId,
        generation: u64,
    ) -> Vec<WireFrame> {
        let stored = self
            .store
            .lookup_command(companion, command)
            .await
            .ok()
            .flatten();
        let wire = stored
            .as_ref()
            .and_then(|row| row.round_wire.clone())
            .or_else(|| {
                stored
                    .as_ref()
                    .and_then(|row| self.wire_for_round_value(row.round))
            });
        let Some(wire) = wire else {
            return vec![stale_frame_with(frame, live, None, generation)];
        };
        vec![accept_frame(frame, live, &RoundWireId(wire))]
    }
}

/// Runs the attach compare-and-commit plus confirm for one device client.
///
/// Best-effort: the compare pins the caller's observed `(NoActive,
/// generation)` expectation, so a concurrent move wins by failing this compare
/// instead of overwriting. A lost compare race, a denied or held outcome, a
/// failed confirm, or a store failure all answer [`AttachOutcome::Raced`];
/// only a committed confirm answers [`AttachOutcome::Attached`] with the
/// fresh fact. An unconfirmed transition reads back as `InTransition`, so the
/// caller's reload reports held, which is honest.
async fn attach_from(
    store: &Store,
    companion: RawId,
    expected: PresenceCheckRef,
    client: ClientId,
    connection_live: bool,
) -> AttachOutcome {
    let Ok(MoveDecision::TransitioningToNew { generation }) = store
        .compare_and_begin_transition(
            companion,
            expected,
            Some(client),
            ThinMoveReason::InitialAttach,
        )
        .await
    else {
        return AttachOutcome::Raced;
    };
    let premise = LiveReachabilityRef {
        client,
        connection_live,
    };
    match store
        .confirm_transition(companion, generation, premise)
        .await
    {
        Ok(ConfirmTransitionOutcome::Confirmed(fact)) => AttachOutcome::Attached(fact),
        Ok(ConfirmTransitionOutcome::RejectedAsStalePresence { .. }) | Err(_) => {
            AttachOutcome::Raced
        }
    }
}

/// Builds an empty timeline answer for the companion-ensure failure path.
fn empty_history(frame: &WireFrame, live: &LiveInput) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::HistoryView(HistoryView { items: Vec::new() }),
    )
}

#[cfg(test)]
mod tests;
