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
//! - A stale or held owner append becomes the matching outcome frame; the
//!   minted round is left recorded but belongs to the old generation, so later
//!   intakes surface it as stale rather than rebinding it.
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
/// The payload names the premise (IPC §21 maps the intake candidate's round
/// from `SubmitTextInput.round`); the envelope `round_view` is the mirror
/// the Client relied on (IPC §5: comparison material, not a claim) and must
/// agree with the payload premise once it is populated. A disagreement
/// means the frame carries two different round premises: neither side is
/// adopted — the caller answers stale with current values, and the Client
/// re-syncs.
///
/// `fresh` does not change the premise: an explicit new-round force ignores
/// every round hint per the payload contract, so there is nothing to agree
/// on, and the caller builds [`RoundIntentMark::New`] from the flag alone.
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
        (Some(round), None) => Some(RoundPremise::Existing(round.0.clone())),
        (Some(round), Some(view)) if view == round => Some(RoundPremise::Existing(round.0.clone())),
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
        "RoundIntakeOutcome",
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
        "TextStreamOpen",
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
        "TextStreamClose",
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
        "RoundIntakeOutcome",
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
        "RoundIntakeOutcome",
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::HeldForTransition),
    )
}

/// Builds a needs-revalidation outcome frame with a fixed wire reason.
fn revalidate_frame(frame: &WireFrame, live: &LiveInput, reason: &str) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        "RoundIntakeOutcome",
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
    /// premise comes from the input `round`, and a populated envelope
    /// `round_view` must agree with it — a disagreement answers stale with
    /// current values instead of adopting either side; a present-but-
    /// unresolvable round is stale, never rebound.
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
        // intent. `fresh` forces a new round and ignores every hint per the
        // payload contract; otherwise the premise decides.
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
        // an explicit new-round force beats any premise (the CLI already
        // rejects combining them, so both set means a hand-built frame);
        // otherwise a resolved premise joins that round and no premise
        // joins-or-mints.
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
            companion: CompanionAvailability {
                known: true,
                running: lifecycle == Some(CompanionLifecycle::Running),
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
            .find(|known| known.id == consent.credential_id)
            .cloned()
        {
            Some(known) => known,
            None => CredentialRef {
                id: consent.credential_id.clone(),
                provider: consent.provider.clone(),
                label: String::from("main"),
            },
        };
        let repo_known = known_refs
            .iter()
            .any(|known| known.id == consent.credential_id);
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
        // attempt started under a verified premise. This replaces the old
        // check-then-send gap with a serialized determination.
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
                "TextStreamFrame",
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
            "HistoryView",
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
        "HistoryView",
        WirePayload::HistoryView(HistoryView { items: Vec::new() }),
    )
}

#[cfg(test)]
mod tests {
    use super::{AttachOutcome, CHUNK_CHARS, chunk_text};
    use crate::serve::{CredStore, HostHandle, LiveInput, device_client};
    use crate::test_support::{live_input, memory_handle_with, remove_data_dir};
    use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
    use ene_api::v1::management::{
        IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome,
        ManagementViewRequest, RationaleOrigin,
    };
    use ene_api::v1::payload::WirePayload;
    use ene_api::v1::refs::{
        BaseViewMark, ClientIncarnationId, ClientLocalId, CommandWireId, CompanionWireRef,
        ConnectionWireId, ManagementTargetWire, RoundWireId, TextLangWire, WireMessageType,
    };
    use ene_api::v1::reject::RejectKind;
    use ene_api::v1::round::{
        ConfirmPresentationWire, PresentationStatus, RoundIntakeOutcomeWire, StreamClose,
        SubmitTextInput, TextBodyWire,
    };
    use ene_credential::{CredentialRef, MemoryCredentialStore};
    use ene_inference::fake::{FakeFailure, FakeProviderTransport};
    use ene_presentation::RoundId;
    use ene_primitive::RawId;

    fn sender() -> WireSender {
        WireSender {
            device_id: None,
            incarnation_id: ClientIncarnationId {
                counter: 3,
                random: 4,
            },
            connection_id: None,
        }
    }

    /// Stamps a hand-built frame with the connection binding its `live`
    /// premises carry, so direct-handle frames pass the gate the same way a
    /// connection-table-built frame would.
    fn stamped(
        mut frame: ene_plugin_ipc::WireFrame,
        connection: ConnectionWireId,
    ) -> ene_plugin_ipc::WireFrame {
        frame.envelope.sender.connection_id = Some(connection);
        frame
    }

    fn submit_frame(
        companion: &str,
        generation: Option<u64>,
        round: Option<RoundWireId>,
        local_id: &str,
        text: &str,
        connection: ConnectionWireId,
    ) -> ene_plugin_ipc::WireFrame {
        let mut envelope = new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("SubmitTextInput")),
        );
        envelope.observed.presence_generation_view = generation;
        envelope.observed.round_view = round.clone();
        // Every submit mints a fresh command id: transport retry reuses the
        // id (the replay test resends the same frame), while distinct sends
        // stay distinct durable commands.
        envelope.correlation.command_id = Some(CommandWireId(RawId::new().as_uuid()));
        let frame = ene_plugin_ipc::WireFrame {
            envelope,
            payload: WirePayload::SubmitTextInput(SubmitTextInput {
                companion: CompanionWireRef(companion.to_string()),
                round,
                fresh: false,
                local_id: ClientLocalId(local_id.to_string()),
                body: TextBodyWire {
                    text: text.to_string(),
                    lang: TextLangWire(String::from("en")),
                },
            }),
        };
        stamped(frame, connection)
    }

    fn intent_frame(
        kind: ManagementIntentKind,
        target: &str,
        base: &str,
        connection: ConnectionWireId,
    ) -> ene_plugin_ipc::WireFrame {
        intent_frame_with_id(
            kind,
            target,
            base,
            connection,
            CommandWireId(RawId::new().as_uuid()),
        )
    }

    /// Builds an intent frame with a caller-chosen idempotency key, so
    /// replay tests can resend the same logical intent byte-for-byte.
    fn intent_frame_with_id(
        kind: ManagementIntentKind,
        target: &str,
        base: &str,
        connection: ConnectionWireId,
        intent_id: CommandWireId,
    ) -> ene_plugin_ipc::WireFrame {
        let frame = ene_plugin_ipc::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("ManagementIntent")),
            ),
            payload: WirePayload::ManagementIntent(ManagementIntent {
                intent_id,
                kind,
                target: ManagementTargetWire(target.to_string()),
                base_view: BaseViewMark(base.to_string()),
                rationale: IntentRationaleWire {
                    origin: RationaleOrigin::ManagementSurface,
                    quote: None,
                },
            }),
        };
        stamped(frame, connection)
    }

    fn history_frame(companion: &str, connection: ConnectionWireId) -> ene_plugin_ipc::WireFrame {
        let frame = ene_plugin_ipc::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("HistoryRequest")),
            ),
            payload: WirePayload::HistoryRequest(ene_api::v1::round::HistoryRequest {
                companion: CompanionWireRef(companion.to_string()),
                since: None,
                limit: 100,
            }),
        };
        stamped(frame, connection)
    }

    fn confirm_frame(
        round: &RoundWireId,
        connection: ConnectionWireId,
    ) -> ene_plugin_ipc::WireFrame {
        let frame = ene_plugin_ipc::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("ConfirmPresentation")),
            ),
            payload: WirePayload::ConfirmPresentation(ConfirmPresentationWire {
                round: round.clone(),
                stream: None,
                status: PresentationStatus::Presented,
                detail: None,
            }),
        };
        stamped(frame, connection)
    }

    fn view_request_frame(connection: ConnectionWireId) -> ene_plugin_ipc::WireFrame {
        let frame = ene_plugin_ipc::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("ManagementViewRequest")),
            ),
            payload: WirePayload::ManagementViewRequest(ManagementViewRequest {
                sections: Vec::new(),
            }),
        };
        stamped(frame, connection)
    }

    fn ok_transport() -> FakeProviderTransport {
        FakeProviderTransport::new(String::from("hi there"), None)
    }

    /// Result-based handle setup for the round regression tests: a failed
    /// open or a failed setup is a test failure, never a silent pass.
    async fn round_test_handle(
        tag: &str,
        live: &LiveInput,
        transport: &FakeProviderTransport,
    ) -> Result<(HostHandle, std::path::PathBuf), String> {
        let Some((handle, dir)) = setup_handle(tag).await else {
            return Err(String::from("handle open failed"));
        };
        if !register_assign_complete(&handle, live, transport).await {
            remove_data_dir(&dir);
            return Err(String::from("setup must complete"));
        }
        Ok((handle, dir))
    }

    /// Extracts the accepted round from the first answer, failing the test
    /// on anything else.
    fn accepted_round(responses: &[ene_plugin_ipc::WireFrame]) -> Result<RoundWireId, String> {
        let Some(first) = responses.first() else {
            return Err(String::from("the submit must answer"));
        };
        match &first.payload {
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round }) => {
                Ok(round.clone())
            }
            other => Err(format!("the submit must accept, got {other:?}")),
        }
    }

    /// Extracts a wire rejection kind from the first answer.
    fn reject_kind(responses: &[ene_plugin_ipc::WireFrame]) -> Result<RejectKind, String> {
        let Some(first) = responses.first() else {
            return Err(String::from("the submit must answer"));
        };
        match &first.payload {
            WirePayload::Reject(notice) => Ok(notice.kind),
            other => Err(format!("expected a typed reject, got {other:?}")),
        }
    }

    /// Counts durable history rows through the repository contract.
    async fn timeline_count(handle: &HostHandle) -> Result<usize, String> {
        use ene_companion::CompanionRepository as _;
        use ene_companion::HistoryRepository as _;

        let companion = handle
            .store
            .ensure_running_companion()
            .await
            .map_err(|error| format!("the companion must resolve: {error:?}"))?;
        let timeline = handle
            .store
            .load_timeline(companion, None, 100)
            .await
            .map_err(|error| format!("the timeline must load: {error:?}"))?;
        Ok(timeline.len())
    }

    /// Loads the current attribution through the repository contract.
    async fn current_generation(handle: &HostHandle) -> Result<u64, String> {
        use ene_companion::CompanionRepository as _;
        use ene_presence::PresenceRepository as _;

        let companion = handle
            .store
            .ensure_running_companion()
            .await
            .map_err(|error| format!("the companion must resolve: {error:?}"))?;
        let attribution = handle
            .store
            .load_attribution(companion.as_raw())
            .await
            .map_err(|error| format!("attribution must load: {error:?}"))?;
        let Some(current) = attribution else {
            return Err(String::from("attribution must load"));
        };
        Ok(current.generation.as_u64())
    }
    async fn setup_handle(tag: &str) -> Option<(HostHandle, std::path::PathBuf)> {
        memory_handle_with(tag, |store| {
            store.insert(
                CredentialRef {
                    id: String::from("openai:main"),
                    provider: String::from("openai"),
                    label: String::from("main"),
                },
                "test-bearer",
            );
        })
        .await
    }

    async fn register_assign_complete(
        handle: &HostHandle,
        live: &LiveInput,
        transport: &FakeProviderTransport,
    ) -> bool {
        let registered = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ConfigureCredentialIntent,
                    "credential:openai:main",
                    "consent-none",
                    live.connection_id,
                ),
                live.clone(),
                transport,
            )
            .await;
        if registered.len() != 1 {
            return false;
        }
        if !matches!(handle.approve_credential("openai", "main").await, Ok(true)) {
            return false;
        }
        let assigned = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:openai:dialogue-1:openai:main",
                    "consent-none",
                    live.connection_id,
                ),
                live.clone(),
                transport,
            )
            .await;
        if assigned.len() != 1 {
            return false;
        }
        let completed = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "setup:complete",
                    "consent-rev-1",
                    live.connection_id,
                ),
                live.clone(),
                transport,
            )
            .await;
        let Some(done) = completed.first() else {
            return false;
        };
        matches!(
            &done.payload,
            WirePayload::ManagementOutcome(ManagementOutcome::AppliedAsOneTime)
        )
    }

    #[test]
    fn empty_text_yields_one_empty_chunk() {
        assert_eq!(chunk_text(""), vec![String::new()]);
    }

    #[test]
    fn chunk_boundaries_hold_at_200_chars() {
        let full: String = "a".repeat(CHUNK_CHARS);
        assert_eq!(chunk_text(&full), vec![full]);
        let over: String = "a".repeat(CHUNK_CHARS + 1);
        let chunks = chunk_text(&over);
        assert_eq!(chunks.len(), 2, "one char over fills two chunks");
        let Some(first) = chunks.first() else {
            return;
        };
        assert_eq!(first.chars().count(), CHUNK_CHARS);
        let Some(second) = chunks.get(1) else {
            return;
        };
        assert_eq!(second, "a");
    }

    #[test]
    fn multibyte_text_never_splits_a_code_point() {
        let emoji: String = "😀".repeat(250);
        let chunks = chunk_text(&emoji);
        assert_eq!(chunks.len(), 2, "250 emoji fill two 200-char chunks");
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.chars().count() <= CHUNK_CHARS),
            "every chunk respects the bound"
        );
        assert_eq!(chunks.concat(), emoji, "reassembly preserves the text");
    }

    #[tokio::test]
    async fn submit_without_setup_needs_revalidation() {
        use ene_companion::CompanionRepository as _;
        use ene_companion::HistoryRepository as _;
        use ene_presence::PresenceRepository as _;

        let Some((handle, dir)) = memory_handle_with("dlg-nosetup", |_| {}).await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        let denied = handle
            .handle_frame(
                submit_frame(
                    handle.companion_wire(),
                    Some(0),
                    None,
                    "local-1",
                    "hello",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert_eq!(denied.len(), 1, "denial answers once");
        let Some(only) = denied.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &only.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::NeedsRevalidation {
                    reason
                }) if reason.0 == "setup-incomplete"
            ),
            "missing consent denies as setup-incomplete, got {:?}",
            only.payload
        );
        let companion = handle.store.ensure_running_companion().await;
        assert!(
            companion.is_ok(),
            "the companion must resolve, got {companion:?}"
        );
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let attribution = handle.store.load_attribution(companion.as_raw()).await;
        assert!(
            matches!(attribution, Ok(Some(_))),
            "attribution must load, got {attribution:?}"
        );
        let Ok(Some(current)) = attribution else {
            remove_data_dir(&dir);
            return;
        };
        assert_eq!(
            current.state,
            ene_presence::PresenceState::Present,
            "the winning attach commits before the consent gate runs"
        );
        assert_eq!(
            current.generation.as_u64(),
            1,
            "one attach moves generation zero to one"
        );
        assert_eq!(
            current.active_client,
            Some(device_client("client-a")),
            "the attach names the submitting device"
        );
        let timeline = handle.store.load_timeline(companion, None, 50).await;
        assert!(
            matches!(&timeline, Ok(items) if items.is_empty()),
            "a declined input must leave no history row, got {timeline:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn attach_without_generation_view_needs_revalidation() {
        use ene_companion::CompanionRepository as _;
        use ene_presence::PresenceRepository as _;

        let Some((handle, dir)) = setup_handle("dlg-noview").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-never-attached");
        let answers = handle
            .handle_frame(
                submit_frame(
                    handle.companion_wire(),
                    None,
                    None,
                    "local-1",
                    "hello",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert_eq!(answers.len(), 1, "a viewless submit answers once");
        let Some(only) = answers.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &only.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::NeedsRevalidation {
                    reason
                }) if reason.0 == "missing-generation-view"
            ),
            "a missing generation view revalidates, got {:?}",
            only.payload
        );
        let companion = handle.store.ensure_running_companion().await;
        assert!(
            companion.is_ok(),
            "the companion must resolve, got {companion:?}"
        );
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let attribution = handle.store.load_attribution(companion.as_raw()).await;
        assert!(
            matches!(attribution, Ok(Some(_))),
            "attribution must load, got {attribution:?}"
        );
        let Ok(Some(current)) = attribution else {
            remove_data_dir(&dir);
            return;
        };
        assert_eq!(
            current.state,
            ene_presence::PresenceState::NoActive,
            "a viewless submit attempts no attach"
        );
        assert_eq!(
            current.generation.as_u64(),
            0,
            "a viewless submit moves no generation"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn attach_with_stale_view_reports_current_values() {
        use ene_companion::CompanionRepository as _;
        use ene_presence::PresenceRepository as _;

        let Some((handle, dir)) = setup_handle("dlg-staleview").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-never-attached");
        let answers = handle
            .handle_frame(
                submit_frame(
                    handle.companion_wire(),
                    Some(7),
                    None,
                    "local-1",
                    "hello",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert_eq!(answers.len(), 1, "a stale-view submit answers once");
        let Some(only) = answers.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &only.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound {
                    current_round: None,
                    current_generation: 0,
                })
            ),
            "a stale view is rejected with the current values, got {:?}",
            only.payload
        );
        let companion = handle.store.ensure_running_companion().await;
        assert!(
            companion.is_ok(),
            "the companion must resolve, got {companion:?}"
        );
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let attribution = handle.store.load_attribution(companion.as_raw()).await;
        let Ok(Some(current)) = attribution else {
            remove_data_dir(&dir);
            return;
        };
        assert_eq!(
            current.state,
            ene_presence::PresenceState::NoActive,
            "a stale-view submit attempts no attach"
        );
        assert_eq!(
            current.generation.as_u64(),
            0,
            "a stale-view submit moves no generation"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn attach_compare_loser_reports_raced() {
        use ene_companion::CompanionRepository as _;
        use ene_presence::PresenceRepository as _;

        let Some((handle, dir)) = setup_handle("dlg-race").await else {
            return;
        };
        let companion = handle.store.ensure_running_companion().await;
        assert!(
            companion.is_ok(),
            "the companion must resolve, got {companion:?}"
        );
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let initial = handle.store.load_attribution(companion.as_raw()).await;
        assert!(
            matches!(initial, Ok(Some(_))),
            "attribution must load, got {initial:?}"
        );
        let Ok(Some(seen)) = initial else {
            remove_data_dir(&dir);
            return;
        };
        assert_eq!(
            seen.state,
            ene_presence::PresenceState::NoActive,
            "the race starts from no active client"
        );
        assert_eq!(
            seen.generation.as_u64(),
            0,
            "the race starts from generation zero"
        );
        let first = handle
            .attach_presence("client-race", true, seen.generation)
            .await;
        assert!(
            matches!(
                first,
                AttachOutcome::Attached(fresh)
                if fresh.generation.as_u64() == 1
                    && fresh.state == ene_presence::PresenceState::Present
            ),
            "the first compare with the observed premise wins generation one, got {first:?}"
        );
        let second = handle
            .attach_presence("client-race", true, seen.generation)
            .await;
        assert!(
            matches!(second, AttachOutcome::Raced),
            "the second compare with the same observed premise loses, got {second:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn full_dialogue_round_streams_and_restores() {
        use ene_companion::CompanionRepository as _;
        use ene_presence::PresenceRepository as _;

        let Some((handle, dir)) = setup_handle("dlg-full").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        assert!(
            register_assign_complete(&handle, &live, &transport).await,
            "setup must complete"
        );
        let shown = handle
            .handle_frame(
                view_request_frame(live.connection_id),
                live.clone(),
                &transport,
            )
            .await;
        assert_eq!(shown.len(), 1, "a view request answers once");
        let frame = submit_frame(
            handle.companion_wire(),
            Some(0),
            None,
            "local-1",
            "hello",
            live.connection_id,
        );
        let responses = handle
            .handle_frame(frame.clone(), live.clone(), &transport)
            .await;
        assert_eq!(
            responses.len(),
            4,
            "the winning attach accepts with open, one frame, close, got {responses:?}"
        );
        for response in &responses {
            assert_eq!(
                response.envelope.sender.connection_id,
                Some(live.connection_id),
                "every response echoes the connection"
            );
        }
        let Some(accepted) = responses.first() else {
            remove_data_dir(&dir);
            return;
        };
        let WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { round }) =
            &accepted.payload
        else {
            remove_data_dir(&dir);
            return;
        };
        let round = round.clone();
        let Some(opened) = responses.get(1) else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &opened.payload,
                WirePayload::TextStreamOpen(open) if open.generation == 1
            ),
            "the stream opens at the freshly attached generation, got {:?}",
            opened.payload
        );
        let Some(stream_frame) = responses.get(2) else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &stream_frame.payload,
                WirePayload::TextStreamFrame(frame) if frame.is_final && frame.seq == 0
            ),
            "the single frame is final at seq zero"
        );
        let Some(closed) = responses.get(3) else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &closed.payload,
                WirePayload::TextStreamClose(close) if close.status == StreamClose::Completed
            ),
            "the stream completes"
        );
        let companion = handle.store.ensure_running_companion().await;
        assert!(
            companion.is_ok(),
            "the companion must resolve, got {companion:?}"
        );
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let attribution = handle.store.load_attribution(companion.as_raw()).await;
        let Ok(Some(current)) = attribution else {
            remove_data_dir(&dir);
            return;
        };
        assert_eq!(
            current.state,
            ene_presence::PresenceState::Present,
            "the first submit attaches the paired device"
        );
        assert_eq!(
            current.generation.as_u64(),
            1,
            "one attach moves generation zero to one"
        );
        assert_eq!(
            current.active_client,
            Some(device_client("client-a")),
            "the attach names the submitting device"
        );
        let confirmed = handle
            .handle_frame(
                confirm_frame(&round, live.connection_id),
                live.clone(),
                &transport,
            )
            .await;
        assert!(
            confirmed.is_empty(),
            "a presentation observation answers nothing"
        );
        let restored = handle
            .handle_frame(
                history_frame(handle.companion_wire(), live.connection_id),
                live.clone(),
                &transport,
            )
            .await;
        assert_eq!(restored.len(), 1, "history answers once");
        let Some(view) = restored.first() else {
            remove_data_dir(&dir);
            return;
        };
        let WirePayload::HistoryView(view) = &view.payload else {
            remove_data_dir(&dir);
            return;
        };
        assert_eq!(view.items.len(), 2, "owner input plus reply restore");
        let replayed = handle.handle_frame(frame, live.clone(), &transport).await;
        assert_eq!(replayed.len(), 1, "a command replay answers once");
        let Some(replay) = replayed.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &replay.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound {
                    round: replayed,
                }) if *replayed == round
            ),
            "the replay re-acks the original round without re-executing, got {:?}",
            replay.payload
        );
        let again = handle
            .handle_frame(
                history_frame(handle.companion_wire(), live.connection_id),
                live.clone(),
                &transport,
            )
            .await;
        let Some(second) = again.first() else {
            remove_data_dir(&dir);
            return;
        };
        let WirePayload::HistoryView(second) = &second.payload else {
            remove_data_dir(&dir);
            return;
        };
        assert_eq!(second.items.len(), 2, "the replay appends nothing durable");
        remove_data_dir(&dir);
    }

    /// A `fresh` send mints a new round even when an open round would
    /// match: force-new is its own round intent.
    #[tokio::test]
    async fn fresh_send_mints_despite_matching_open_round() -> Result<(), String> {
        let transport = ok_transport();
        let live = live_input("client-a");
        let (handle, dir) = round_test_handle("dlg-fresh", &live, &transport).await?;
        let first = submit_frame(
            handle.companion_wire(),
            Some(0),
            None,
            "local-1",
            "hello",
            live.connection_id,
        );
        let first_round =
            accepted_round(&handle.handle_frame(first, live.clone(), &transport).await)?;
        // The second send observes the current generation so only the
        // round intent differs from a plain continuation.
        let generation = current_generation(&handle).await?;
        let mut second = submit_frame(
            handle.companion_wire(),
            Some(generation),
            None,
            "local-2",
            "again",
            live.connection_id,
        );
        let WirePayload::SubmitTextInput(ref mut input) = second.payload else {
            remove_data_dir(&dir);
            return Err(String::from("the send frame must carry its input"));
        };
        input.fresh = true;
        let second_round =
            accepted_round(&handle.handle_frame(second, live.clone(), &transport).await)?;
        assert_ne!(
            second_round, first_round,
            "a fresh send must mint instead of joining, got {second_round:?}"
        );
        remove_data_dir(&dir);
        Ok(())
    }

    /// Repeated sends into the same open round reuse its one wire
    /// projection: 1 domain round, 1 wire, across every message.
    #[tokio::test]
    async fn same_round_reuses_one_wire_projection() -> Result<(), String> {
        let transport = ok_transport();
        let live = live_input("client-a");
        let (handle, dir) = round_test_handle("dlg-one-wire", &live, &transport).await?;
        let first = submit_frame(
            handle.companion_wire(),
            Some(0),
            None,
            "local-1",
            "hello",
            live.connection_id,
        );
        let first_round =
            accepted_round(&handle.handle_frame(first, live.clone(), &transport).await)?;
        // The second send observes the current generation so it joins the
        // open round instead of re-attaching.
        let generation = current_generation(&handle).await?;
        let second = submit_frame(
            handle.companion_wire(),
            Some(generation),
            None,
            "local-2",
            "again",
            live.connection_id,
        );
        let second_round =
            accepted_round(&handle.handle_frame(second, live.clone(), &transport).await)?;
        assert_eq!(
            second_round, first_round,
            "a joined round must reuse its one projection, got {second_round:?}"
        );
        remove_data_dir(&dir);
        Ok(())
    }

    /// An exact retry — same command id, same body, same language, same
    /// incarnation, same round premise, same `fresh` flag — replays the
    /// original accept verbatim even after the surrounding round state
    /// moved on under a different command. Nothing is re-appended and no
    /// provider attempt runs: a re-execution against a failing transport
    /// would answer an interrupted stream, the replay answers one frame.
    #[tokio::test]
    async fn retry_after_round_advance_replays_the_stored_accept() -> Result<(), String> {
        let transport = ok_transport();
        let live = live_input("client-a");
        let (handle, dir) = round_test_handle("dlg-retry-drift", &live, &transport).await?;
        let first = submit_frame(
            handle.companion_wire(),
            Some(0),
            None,
            "local-1",
            "hello",
            live.connection_id,
        );
        let first_round = accepted_round(
            &handle
                .handle_frame(first.clone(), live.clone(), &transport)
                .await,
        )?;
        // The open round moves on under a different command: the drift is
        // Host state only, never part of the retried request.
        let generation = current_generation(&handle).await?;
        let mut drift = submit_frame(
            handle.companion_wire(),
            Some(generation),
            None,
            "local-2",
            "hello",
            live.connection_id,
        );
        if let WirePayload::SubmitTextInput(ref mut input) = drift.payload {
            input.fresh = true;
        }
        let drift_round =
            accepted_round(&handle.handle_frame(drift, live.clone(), &transport).await)?;
        assert_ne!(
            drift_round, first_round,
            "the drift send must mint a new round"
        );
        let before = timeline_count(&handle).await?;
        // The exact retry resends the original frame byte-for-byte (same
        // command id, same payload, same incarnation and premises).
        let failing = FakeProviderTransport::failing(FakeFailure::Transport(String::from(
            "must never be reached",
        )));
        let responses = handle.handle_frame(first, live.clone(), &failing).await;
        assert_eq!(
            responses.len(),
            1,
            "the replay answers the accept ack only, with no stream: {responses:?}"
        );
        assert_eq!(
            accepted_round(&responses)?,
            first_round,
            "the retry replays the original accept, not the drifted round"
        );
        assert_eq!(
            timeline_count(&handle).await?,
            before,
            "the retry appends nothing durable"
        );
        remove_data_dir(&dir);
        Ok(())
    }

    /// Same command id and same request content, but the client round
    /// intent flips from join-or-mint to an explicit join: a different
    /// request, answered with the typed wire rejection and no side effects.
    #[tokio::test]
    async fn retry_with_changed_round_intent_conflicts() -> Result<(), String> {
        let transport = ok_transport();
        let live = live_input("client-a");
        let (handle, dir) = round_test_handle("dlg-retry-intent", &live, &transport).await?;
        let first = submit_frame(
            handle.companion_wire(),
            Some(0),
            None,
            "local-1",
            "hello",
            live.connection_id,
        );
        let first_round = accepted_round(
            &handle
                .handle_frame(first.clone(), live.clone(), &transport)
                .await,
        )?;
        // Same command id, same body/lang/incarnation, but the premise now
        // names the round explicitly (Auto -> Existing): a different
        // request. The companion projection stays the one this handle
        // issued, so only the round intent differs.
        let mut joined = first.clone();
        if let WirePayload::SubmitTextInput(ref mut input) = joined.payload {
            input.round = Some(first_round.clone());
            joined.envelope.observed.round_view = Some(first_round.clone());
        }
        let declined = handle.handle_frame(joined, live.clone(), &transport).await;
        assert_eq!(
            reject_kind(&declined)?,
            RejectKind::ConflictingCommand,
            "a changed round intent must conflict, got {declined:?}"
        );
        assert_eq!(
            timeline_count(&handle).await?,
            2,
            "the conflicting retry appends nothing durable"
        );
        remove_data_dir(&dir);
        Ok(())
    }

    /// Same command id and request content, but `fresh` flips
    /// join-or-mint into force-new: a different request semantics, so the
    /// key conflicts instead of replaying the stored accept.
    #[tokio::test]
    async fn retry_with_forced_fresh_conflicts() -> Result<(), String> {
        let transport = ok_transport();
        let live = live_input("client-a");
        let (handle, dir) = round_test_handle("dlg-retry-fresh", &live, &transport).await?;
        let first = submit_frame(
            handle.companion_wire(),
            Some(0),
            None,
            "local-1",
            "hello",
            live.connection_id,
        );
        let original_round = accepted_round(
            &handle
                .handle_frame(first.clone(), live.clone(), &transport)
                .await,
        )?;
        let mut forced = first.clone();
        if let WirePayload::SubmitTextInput(ref mut input) = forced.payload {
            input.fresh = true;
        }
        let declined = handle.handle_frame(forced, live.clone(), &transport).await;
        assert_eq!(
            reject_kind(&declined)?,
            RejectKind::ConflictingCommand,
            "a fresh flip on the same key must conflict"
        );
        // The stored accept stays authoritative: the exact retry still
        // replays after the conflicting attempt changed nothing.
        let replayed = handle.handle_frame(first, live.clone(), &transport).await;
        assert_eq!(
            accepted_round(&replayed)?,
            original_round,
            "the untouched retry still replays the original accept"
        );
        assert_eq!(
            timeline_count(&handle).await?,
            2,
            "conflict and replay append nothing durable"
        );
        remove_data_dir(&dir);
        Ok(())
    }

    /// Concurrent sends that join the same open round all use its one wire
    /// projection: the atomic get-or-create never mints a second one.
    #[tokio::test]
    async fn concurrent_joins_of_one_round_share_one_wire() -> Result<(), String> {
        let transport = ok_transport();
        let live = live_input("client-a");
        let (handle, dir) = round_test_handle("dlg-concurrent", &live, &transport).await?;
        let original = submit_frame(
            handle.companion_wire(),
            Some(0),
            None,
            "local-1",
            "hello",
            live.connection_id,
        );
        let open_wire = accepted_round(
            &handle
                .handle_frame(original, live.clone(), &transport)
                .await,
        )?;
        let generation = current_generation(&handle).await?;
        let first = submit_frame(
            handle.companion_wire(),
            Some(generation),
            None,
            "local-2",
            "one",
            live.connection_id,
        );
        let second = submit_frame(
            handle.companion_wire(),
            Some(generation),
            None,
            "local-3",
            "two",
            live.connection_id,
        );
        let (left, right) = tokio::join!(
            handle.handle_frame(first, live.clone(), &transport),
            handle.handle_frame(second, live.clone(), &transport)
        );
        let left_wire = accepted_round(&left)?;
        let right_wire = accepted_round(&right)?;
        assert_eq!(
            left_wire, right_wire,
            "concurrent joins of one round must share its one wire"
        );
        assert_eq!(left_wire, open_wire, "the open round keeps its projection");
        remove_data_dir(&dir);
        Ok(())
    }

    /// The get-or-create itself is atomic: truly parallel requests for one
    /// round all receive the same wire, never two mints.
    #[tokio::test]
    async fn concurrent_projection_requests_mint_one_wire() -> Result<(), String> {
        let Some((handle, dir)) = setup_handle("dlg-mint-race").await else {
            return Err(String::from("handle open must succeed"));
        };
        let round = RoundId::from_raw(RawId::new());
        let wires: Vec<RoundWireId> = std::thread::scope(|scope| {
            let joins: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| handle.round_wire_or_mint(&round)))
                .collect();
            joins
                .into_iter()
                .map(|join| join.join().map_err(|_| String::from("mint task panicked")))
                .collect::<Vec<Result<RoundWireId, String>>>()
        })
        .into_iter()
        .collect::<Result<Vec<_>, String>>()?;
        let mut distinct: Vec<String> = wires.iter().map(|wire| wire.0.clone()).collect();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            1,
            "one round must mint exactly one wire, got {distinct:?}"
        );
        remove_data_dir(&dir);
        Ok(())
    }

    /// After a Host restart, the exact retry replays the original accept
    /// from the durable row: the stored wire projection travels verbatim,
    /// nothing re-executes, nothing re-appends.
    #[tokio::test]
    async fn replay_after_restart_replays_from_durable_wire() -> Result<(), String> {
        let Some((handle, dir)) = setup_handle("dlg-restart").await else {
            return Err(String::from("handle open must succeed"));
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        if !register_assign_complete(&handle, &live, &transport).await {
            remove_data_dir(&dir);
            return Err(String::from("setup must complete"));
        }
        let frame = submit_frame(
            handle.companion_wire(),
            Some(0),
            None,
            "local-9",
            "hello",
            live.connection_id,
        );
        let accepted = handle
            .handle_frame(frame.clone(), live.clone(), &transport)
            .await;
        let accepted_wire = accepted_round(&accepted)?.0;
        drop(handle);
        let fresh = MemoryCredentialStore::new();
        fresh.insert(
            CredentialRef {
                id: String::from("openai:main"),
                provider: String::from("openai"),
                label: String::from("main"),
            },
            "test-bearer",
        );
        let reopened = HostHandle::open_with_cred_store(&dir, CredStore::Memory(fresh))
            .await
            .map_err(|error| format!("the store must reopen: {error:?}"))?;
        let relive = live_input("client-a");
        let mut resent = frame;
        resent.envelope.sender.connection_id = Some(relive.connection_id);
        // A restarted handle rotates its companion projection: a real Client
        // relearns it from the reconnect presence fact before retrying. The
        // retried command (id, content, incarnation) is unchanged, so the
        // replay path still answers the original accept.
        if let WirePayload::SubmitTextInput(ref mut input) = resent.payload {
            input.companion = CompanionWireRef(reopened.companion_wire().to_string());
        }
        let replayed = reopened
            .handle_frame(resent, relive.clone(), &transport)
            .await;
        assert_eq!(
            replayed.len(),
            1,
            "the restart replay answers once, got {replayed:?}"
        );
        assert_eq!(
            accepted_round(&replayed)?.0,
            accepted_wire,
            "the restart replay answers the original accept from durable state"
        );
        let restored = reopened
            .handle_frame(
                history_frame(reopened.companion_wire(), relive.connection_id),
                relive.clone(),
                &transport,
            )
            .await;
        let Some(view_frame) = restored.first() else {
            remove_data_dir(&dir);
            return Err(String::from("history must answer"));
        };
        let WirePayload::HistoryView(view) = &view_frame.payload else {
            remove_data_dir(&dir);
            return Err(String::from("history must answer with a view"));
        };
        assert_eq!(
            view.items.len(),
            2,
            "the restart replay appends nothing durable"
        );
        remove_data_dir(&dir);
        Ok(())
    }

    #[tokio::test]
    async fn disconnect_clears_an_attached_device() {
        use ene_companion::CompanionRepository as _;
        use ene_presence::PresenceRepository as _;

        let Some((handle, dir)) = setup_handle("dlg-disc").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        assert!(
            register_assign_complete(&handle, &live, &transport).await,
            "setup must complete"
        );
        let accepted = handle
            .handle_frame(
                submit_frame(
                    handle.companion_wire(),
                    Some(0),
                    None,
                    "local-1",
                    "hello",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert!(
            accepted.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
            )),
            "the send attaches and accepts, got {accepted:?}"
        );
        handle.note_disconnect("client-a").await;
        let companion = handle.store.ensure_running_companion().await;
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let attribution = handle.store.load_attribution(companion.as_raw()).await;
        let Ok(Some(current)) = attribution else {
            remove_data_dir(&dir);
            return;
        };
        assert_eq!(
            current.state,
            ene_presence::PresenceState::NoActive,
            "socket close falls back to no active client"
        );
        assert_eq!(
            current.active_client, None,
            "socket close clears the active client"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn replay_after_disconnect_neither_stales_nor_reattaches() {
        use ene_companion::CompanionRepository as _;
        use ene_presence::PresenceRepository as _;

        let Some((handle, dir)) = setup_handle("dlg-replay-disc").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        assert!(
            register_assign_complete(&handle, &live, &transport).await,
            "setup must complete"
        );
        let frame = submit_frame(
            handle.companion_wire(),
            Some(0),
            None,
            "local-1",
            "hello",
            live.connection_id,
        );
        let accepted = handle
            .handle_frame(frame.clone(), live.clone(), &transport)
            .await;
        assert!(
            accepted.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
            )),
            "the send attaches and accepts, got {accepted:?}"
        );
        handle.note_disconnect("client-a").await;
        // The durable replay check precedes presence attach: the same
        // command replays its original accept even though the device is
        // NoActive again, and presence stays untouched (no re-attach, no
        // generation advance for a send that changes nothing).
        let replayed = handle.handle_frame(frame, live.clone(), &transport).await;
        assert!(
            replayed.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
            )),
            "the post-disconnect retry must replay, not stale, got {replayed:?}"
        );
        let companion = handle.store.ensure_running_companion().await;
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let attribution = handle.store.load_attribution(companion.as_raw()).await;
        assert!(
            matches!(&attribution, Ok(Some(current)) if current.state == ene_presence::PresenceState::NoActive && current.active_client.is_none()),
            "the replay must not re-attach presence, got {attribution:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn provider_failure_interrupts_after_accept() {
        let Some((handle, dir)) = setup_handle("dlg-fail").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        assert!(
            register_assign_complete(&handle, &live, &transport).await,
            "setup must complete"
        );
        let accepted = handle
            .handle_frame(
                submit_frame(
                    handle.companion_wire(),
                    Some(0),
                    None,
                    "local-1",
                    "probe",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert!(
            accepted.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
            )),
            "the first send attaches and accepts, got {accepted:?}"
        );
        let failing = FakeProviderTransport::failing(FakeFailure::Transport(String::from("down")));
        let responses = handle
            .handle_frame(
                submit_frame(
                    handle.companion_wire(),
                    Some(1),
                    None,
                    "local-9",
                    "hello",
                    live.connection_id,
                ),
                live.clone(),
                &failing,
            )
            .await;
        assert_eq!(responses.len(), 3, "accept plus an interrupted stream");
        let Some(closed) = responses.get(2) else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &closed.payload,
                WirePayload::TextStreamClose(close) if close.status == StreamClose::Interrupted
            ),
            "a send failure interrupts the stream"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn consent_replay_is_idempotent_and_moves_report_staleness() {
        let Some((handle, dir)) = setup_handle("dlg-cas").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        let registered = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ConfigureCredentialIntent,
                    "credential:openai:main",
                    "consent-none",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert_eq!(registered.len(), 1, "register answers once");
        assert!(
            handle.approve_credential("openai", "main").await.is_ok(),
            "approval must succeed"
        );
        let assigned = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:openai:dialogue-1:openai:main",
                    "consent-none",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(stored) = assigned.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &stored.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView { .. })
            ),
            "the first assign commits"
        );
        let replayed = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:openai:dialogue-1:openai:main",
                    "consent-none",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(same) = replayed.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &same.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView { current })
                if current.0 == "consent-rev-1"
            ),
            "the identical replay on its stale base reports staleness (not silent success), got {:?}",
            same.payload
        );
        let converged = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:openai:dialogue-1:openai:main",
                    "consent-rev-1",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(same) = converged.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &same.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView {
                    revision
                }) if revision.0 == "1"
            ),
            "repeating the identical assign on a fresh base is a no-op at the same revision, got {:?}",
            same.payload
        );
        let moved = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:openai:dialogue-2:openai:main",
                    "consent-none",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(stale) = moved.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &stale.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView { current })
                if current.0 == "consent-rev-1"
            ),
            "a changed assign on a stale base reports the rebuilt current mark"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn assign_intent_replay_returns_the_stored_success() {
        let Some((handle, dir)) = setup_handle("dlg-intentreplay").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        let registered = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ConfigureCredentialIntent,
                    "credential:openai:main",
                    "consent-none",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert_eq!(registered.len(), 1, "register answers once");
        assert!(
            handle.approve_credential("openai", "main").await.is_ok(),
            "approval must succeed"
        );
        let intent_id = CommandWireId(RawId::new().as_uuid());
        let assign_x = |base: &str| {
            intent_frame_with_id(
                ManagementIntentKind::ManageRuleConsentCap,
                "consent:openai:dialogue-1:openai:main",
                base,
                live.connection_id,
                intent_id,
            )
        };
        let assigned = handle
            .handle_frame(assign_x("consent-none"), live.clone(), &transport)
            .await;
        assert!(
            matches!(
                &assigned.first().map(|first| &first.payload),
                Some(WirePayload::ManagementOutcome(
                    ManagementOutcome::StoredAsRuleView { .. }
                ))
            ),
            "the first assign commits, got {assigned:?}"
        );
        // Exact retry (same id, same bytes; the base is stale now): the
        // durable replay answers the stored success at the current revision
        // instead of reporting staleness.
        let replayed = handle
            .handle_frame(assign_x("consent-none"), live.clone(), &transport)
            .await;
        let Some(same) = replayed.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &same.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView {
                    revision
                }) if revision.0 == "1"
            ),
            "the exact retry must replay success at rev 1, got {:?}",
            same.payload
        );
        // The route moves on under a different intent; retrying X still
        // answers its own prior outcome verbatim (§6.2: never re-executed).
        // The rev-1 mark no longer names current state, so the caller's
        // NEXT intent built on it reports stale and converges — replay
        // stays honest by returning history, not by recomputing the present.
        let moved = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:openai:dialogue-2:openai:main",
                    "consent-rev-1",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert!(
            matches!(
                &moved.first().map(|first| &first.payload),
                Some(WirePayload::ManagementOutcome(
                    ManagementOutcome::StoredAsRuleView { .. }
                ))
            ),
            "the route move commits, got {moved:?}"
        );
        let stale_retry = handle
            .handle_frame(assign_x("consent-none"), live.clone(), &transport)
            .await;
        assert!(
            matches!(
                &stale_retry.first().map(|first| &first.payload),
                Some(WirePayload::ManagementOutcome(ManagementOutcome::StoredAsRuleView {
                    revision
                })) if revision.0 == "1"
            ),
            "a replay after the route moved still answers its prior outcome, got {stale_retry:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn assign_intent_conflict_clarifies_without_side_effects() {
        use ene_permission::ConsentRepository as _;

        let Some((handle, dir)) = setup_handle("dlg-intentconflict").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        assert!(
            register_assign_complete(&handle, &live, &transport).await,
            "setup must complete"
        );
        let intent_id = CommandWireId(RawId::new().as_uuid());
        let assigned = handle
            .handle_frame(
                intent_frame_with_id(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:openai:dialogue-1:openai:main",
                    "consent-rev-1",
                    live.connection_id,
                    intent_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert!(
            matches!(
                &assigned.first().map(|first| &first.payload),
                Some(WirePayload::ManagementOutcome(
                    ManagementOutcome::StoredAsRuleView { .. }
                ))
            ),
            "the first assign commits, got {assigned:?}"
        );
        // Same intent id, different content: declined without adopting the
        // new meaning, and the stored route is untouched.
        let conflicted = handle
            .handle_frame(
                intent_frame_with_id(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:openai:dialogue-2:openai:main",
                    "consent-rev-1",
                    live.connection_id,
                    intent_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(only) = conflicted.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &only.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::NeedsClarification)
            ),
            "a reused id with new content must clarify, got {:?}",
            only.payload
        );
        let current = handle.store.load_current().await;
        assert!(
            matches!(&current, Ok(Some(record)) if record.model == "dialogue-1"),
            "the conflict must not move consent, got {current:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn malformed_target_reuse_conflicts_without_side_effects() {
        use ene_permission::ConsentRepository as _;

        let Some((handle, dir)) = setup_handle("dlg-malformed-reuse").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        assert!(
            register_assign_complete(&handle, &live, &transport).await,
            "setup must complete"
        );
        let intent_id = CommandWireId(RawId::new().as_uuid());
        // Malformed target first: clarifies AND claims the id, so the row
        // exists for what follows.
        let malformed = handle
            .handle_frame(
                intent_frame_with_id(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:bogus",
                    "consent-rev-1",
                    live.connection_id,
                    intent_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert!(
            matches!(
                &malformed.first().map(|first| &first.payload),
                Some(WirePayload::ManagementOutcome(
                    ManagementOutcome::NeedsClarification
                ))
            ),
            "a malformed target must clarify, got {malformed:?}"
        );
        // Same id with a now-valid target: must conflict, never proceed to
        // assign — the prior row owns this id.
        let reused = handle
            .handle_frame(
                intent_frame_with_id(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:openai:dialogue-9:openai:main",
                    "consent-rev-1",
                    live.connection_id,
                    intent_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(only) = reused.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &only.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::NeedsClarification)
            ),
            "reusing a clarified id with new content must clarify, got {:?}",
            only.payload
        );
        let current = handle.store.load_current().await;
        assert!(
            matches!(&current, Ok(Some(record)) if record.model == "dialogue-1"),
            "the conflict must not move consent, got {current:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn complete_replay_returns_the_stored_snapshot() {
        let Some((handle, dir)) = setup_handle("dlg-completeray").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        assert!(
            register_assign_complete(&handle, &live, &transport).await,
            "setup must complete"
        );
        let intent_id = CommandWireId(RawId::new().as_uuid());
        let complete = intent_frame_with_id(
            ManagementIntentKind::ManageRuleConsentCap,
            "setup:complete",
            "consent-rev-1",
            live.connection_id,
            intent_id,
        );
        let applied = handle
            .handle_frame(complete.clone(), live.clone(), &transport)
            .await;
        assert!(
            matches!(
                &applied.first().map(|first| &first.payload),
                Some(WirePayload::ManagementOutcome(
                    ManagementOutcome::AppliedAsOneTime
                ))
            ),
            "the first completion applies, got {applied:?}"
        );
        let replayed = handle
            .handle_frame(complete, live.clone(), &transport)
            .await;
        assert!(
            matches!(
                &replayed.first().map(|first| &first.payload),
                Some(WirePayload::ManagementOutcome(
                    ManagementOutcome::AppliedAsOneTime
                ))
            ),
            "the exact retry must replay applied, got {replayed:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn complete_stale_replay_returns_its_own_mark() {
        let Some((handle, dir)) = setup_handle("dlg-completestale").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        assert!(
            register_assign_complete(&handle, &live, &transport).await,
            "setup must complete"
        );
        let intent_id = CommandWireId(RawId::new().as_uuid());
        let stale = intent_frame_with_id(
            ManagementIntentKind::ManageRuleConsentCap,
            "setup:complete",
            "consent-none",
            live.connection_id,
            intent_id,
        );
        let first = handle
            .handle_frame(stale.clone(), live.clone(), &transport)
            .await;
        assert!(
            matches!(
                &first.first().map(|first| &first.payload),
                Some(WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView {
                    current
                })) if current.0 == "consent-rev-1"
            ),
            "the stale completion reports rev 1, got {first:?}"
        );
        // Move the route on under a different id, then retry the stale id:
        // the snapshot still names rev 1 (history, not present).
        let moved = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:openai:dialogue-2:openai:main",
                    "consent-rev-1",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        assert!(
            matches!(
                &moved.first().map(|first| &first.payload),
                Some(WirePayload::ManagementOutcome(
                    ManagementOutcome::StoredAsRuleView { .. }
                ))
            ),
            "the route move commits, got {moved:?}"
        );
        let replayed = handle.handle_frame(stale, live.clone(), &transport).await;
        assert!(
            matches!(
                &replayed.first().map(|first| &first.payload),
                Some(WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView {
                    current
                })) if current.0 == "consent-rev-1"
            ),
            "the stale retry must replay its own mark, got {replayed:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn assign_stale_replay_returns_its_own_mark() {
        let Some((handle, dir)) = setup_handle("dlg-assignstale").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        let intent_id = CommandWireId(RawId::new().as_uuid());
        // No consent yet: a rev-9 base is stale on its face.
        let stale = intent_frame_with_id(
            ManagementIntentKind::ManageRuleConsentCap,
            "consent:openai:dialogue-1:openai:main",
            "consent-rev-9",
            live.connection_id,
            intent_id,
        );
        for attempt in 0..2 {
            let answered = handle
                .handle_frame(stale.clone(), live.clone(), &transport)
                .await;
            assert!(
                matches!(
                    &answered.first().map(|first| &first.payload),
                    Some(WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView {
                        current
                    })) if current.0 == "consent-none"
                ),
                "attempt {attempt} must report the empty mark, got {answered:?}"
            );
        }
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn submit_without_command_id_is_declined_without_side_effects() {
        use ene_companion::{CompanionRepository as _, HistoryRepository as _};

        let Some((handle, dir)) = setup_handle("dlg-nocmd").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        assert!(
            register_assign_complete(&handle, &live, &transport).await,
            "setup must complete"
        );
        let mut frame = submit_frame(
            handle.companion_wire(),
            Some(0),
            None,
            "local-1",
            "hello",
            live.connection_id,
        );
        frame.envelope.correlation.command_id = None;
        let declined = handle.handle_frame(frame, live.clone(), &transport).await;
        let Some(only) = declined.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &only.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::NeedsRevalidation {
                    reason
                }) if reason.0 == "missing-command-id"
            ),
            "a keyless command must decline explicitly, got {:?}",
            only.payload
        );
        let companion = handle.store.ensure_running_companion().await;
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let timeline = handle.store.load_timeline(companion, None, 50).await;
        assert!(
            matches!(&timeline, Ok(items) if items.is_empty()),
            "a declined keyless input must leave no history row, got {timeline:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn submit_with_reused_command_and_new_text_is_declined() {
        use ene_companion::{CompanionRepository as _, HistoryRepository as _};

        let Some((handle, dir)) = setup_handle("dlg-mismatch").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        assert!(
            register_assign_complete(&handle, &live, &transport).await,
            "setup must complete"
        );
        let frame = submit_frame(
            handle.companion_wire(),
            Some(0),
            None,
            "local-1",
            "hello",
            live.connection_id,
        );
        let first = handle
            .handle_frame(frame.clone(), live.clone(), &transport)
            .await;
        assert!(
            first.first().is_some_and(|answer| matches!(
                &answer.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
            )),
            "the first send must accept"
        );
        let mut forged = frame;
        if let WirePayload::SubmitTextInput(ref mut input) = forged.payload {
            input.body.text = String::from("different words, same command");
        }
        let declined = handle.handle_frame(forged, live.clone(), &transport).await;
        let Some(only) = declined.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &only.payload,
                WirePayload::Reject(notice) if notice.kind == RejectKind::ConflictingCommand
            ),
            "a reused key with new content must decline, got {:?}",
            only.payload
        );
        let companion = handle.store.ensure_running_companion().await;
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let timeline = handle.store.load_timeline(companion, None, 50).await;
        assert!(
            matches!(&timeline, Ok(items) if items.len() == 2),
            "the declined forgery must append nothing (owner plus reply only), got {timeline:?}"
        );
        remove_data_dir(&dir);
    }

    /// Transport that revokes consent mid-flight: it bumps the stored
    /// consent revision inside `complete` (before delegating to the inner
    /// fake), so the adoption gate after the await sees a moved record.
    /// Models a real revocation landing during a slow provider call.
    struct RevokingTransport {
        db: std::path::PathBuf,
        inner: FakeProviderTransport,
    }

    impl ene_inference::ProviderTransport for RevokingTransport {
        fn complete(
            &self,
            req: ene_inference::ProviderRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            ene_inference::ProviderResponse,
                            ene_inference::InferenceTechnicalError,
                        >,
                    > + Send
                    + '_,
            >,
        > {
            let db = self.db.clone();
            let inner = self.inner.clone();
            Box::pin(async move {
                use ene_permission::ConsentRepository as _;
                if let Ok(store) = ene_store::Store::open(&db).await
                    && let Ok(Some(current)) = store.load_current().await
                {
                    use ene_permission::{ConsentRepository as _, ConsentRevision};
                    let bumped = ene_permission::ConsentRecord {
                        id: current.id.clone(),
                        rev: ConsentRevision::from_u64(current.rev.as_u64() + 1),
                        provider: current.provider.clone(),
                        model: current.model.clone(),
                        credential_id: current.credential_id.clone(),
                    };
                    let _bumped = store
                        .compare_and_save(Some((current.id, current.rev)), bumped)
                        .await;
                }
                inner.complete(req).await
            })
        }
    }

    #[tokio::test]
    async fn submit_with_unknown_companion_needs_revalidation() {
        use ene_companion::{CompanionRepository as _, HistoryRepository as _};

        let Some((handle, dir)) = setup_handle("dlg-unknowncomp").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        assert!(
            register_assign_complete(&handle, &live, &transport).await,
            "setup must complete"
        );
        let declined = handle
            .handle_frame(
                submit_frame(
                    "not-the-issued-projection",
                    Some(0),
                    None,
                    "local-1",
                    "hello",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(only) = declined.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &only.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::NeedsRevalidation {
                    reason
                }) if reason.0 == "unknown-companion"
            ),
            "an unresolvable companion ref must revalidate, got {:?}",
            only.payload
        );
        let companion = handle.store.ensure_running_companion().await;
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let timeline = handle.store.load_timeline(companion, None, 50).await;
        assert!(
            matches!(&timeline, Ok(items) if items.is_empty()),
            "the unknown-companion send must append nothing, got {timeline:?}"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn consent_move_mid_flight_interrupts_adoption() {
        use ene_companion::{CompanionRepository as _, HistoryRepository as _};

        let Some((handle, dir)) = setup_handle("dlg-midflight").await else {
            return;
        };
        let live = live_input("client-a");
        let fake = ok_transport();
        assert!(
            register_assign_complete(&handle, &live, &fake).await,
            "setup must complete"
        );
        let transport = RevokingTransport {
            db: dir.join("app.db"),
            inner: fake,
        };
        let frame = submit_frame(
            handle.companion_wire(),
            Some(0),
            None,
            "local-1",
            "hello",
            live.connection_id,
        );
        let responses = handle.handle_frame(frame, live.clone(), &transport).await;
        let Some(last) = responses.last() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &last.payload,
                WirePayload::TextStreamClose(close)
                    if close.status == StreamClose::Interrupted
            ),
            "a mid-flight consent move must interrupt, got {:?}",
            last.payload
        );
        let companion = handle.store.ensure_running_companion().await;
        let Ok(companion) = companion else {
            remove_data_dir(&dir);
            return;
        };
        let timeline = handle.store.load_timeline(companion, None, 50).await;
        assert!(
            matches!(&timeline, Ok(items) if items.len() == 1),
            "only the owner row commits on interrupted adoption, got {timeline:?}"
        );
        remove_data_dir(&dir);
    }

    #[test]
    fn usage_certainty_never_invents_or_discards_facts() {
        use super::{SendOutcomeClass, usage_for_disposition};
        use ene_inference::{InferenceTicketId, UsageFact, UsageSource};

        let ticket = InferenceTicketId(RawId::new());
        assert_eq!(
            usage_for_disposition(ticket, "openai", "dialogue-1", SendOutcomeClass::NeverSent),
            None,
            "a never-sent call spends nothing, so no fact is recorded"
        );
        let uncertain = usage_for_disposition(
            ticket,
            "openai",
            "dialogue-1",
            SendOutcomeClass::AttemptUncertain,
        );
        assert_eq!(
            uncertain,
            Some(UsageFact {
                ticket,
                provider: String::from("openai"),
                model: String::from("dialogue-1"),
                input_tokens: None,
                output_tokens: None,
                source: UsageSource::Unknown,
            }),
            "an uncertain attempt records unknown counts, never zero"
        );
        let reported = UsageFact {
            ticket,
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            input_tokens: Some(7),
            output_tokens: Some(9),
            source: UsageSource::Reported,
        };
        assert_eq!(
            usage_for_disposition(
                ticket,
                "openai",
                "dialogue-1",
                SendOutcomeClass::Reported(&reported)
            ),
            Some(reported),
            "reported counts survive even when the caller cannot adopt the reply"
        );
    }

    #[tokio::test]
    async fn register_holds_until_host_local_approval() {
        let Some((handle, dir)) = memory_handle_with("dlg-credgate", |_| {}).await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        let pending = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ConfigureCredentialIntent,
                    "credential:openai:main",
                    "consent-none",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(first) = pending.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &first.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::HeldByOperation)
            ),
            "an unapproved registration holds, got {:?}",
            first.payload
        );
        assert!(
            handle.approve_credential("openai", "main").await.is_ok(),
            "host-local approval must succeed"
        );
        let usable = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ConfigureCredentialIntent,
                    "credential:openai:main",
                    "consent-none",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(second) = usable.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &second.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::AppliedAsOneTime)
            ),
            "re-request after approval applies, got {:?}",
            second.payload
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn setup_edge_cases_clarify_or_hold() {
        let Some((handle, dir)) = memory_handle_with("dlg-edge", |_| {}).await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        let malformed = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ConfigureCredentialIntent,
                    "credential:lonely",
                    "consent-none",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(first) = malformed.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &first.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::NeedsClarification)
            ),
            "a malformed register target clarifies"
        );
        let stale_base = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:openai:dialogue-1:openai:main",
                    "consent-rev-99",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(second) = stale_base.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &second.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView { .. })
            ),
            "a moved base view reports staleness"
        );
        let foreign = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::StopCompanion,
                    "companion-1",
                    "consent-none",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(third) = foreign.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &third.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::NeedsClarification)
            ),
            "a non-setup kind clarifies"
        );
        let incomplete = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "setup:complete",
                    "consent-none",
                    live.connection_id,
                ),
                live.clone(),
                &transport,
            )
            .await;
        let Some(fourth) = incomplete.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &fourth.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::NeedsClarification)
            ),
            "an incomplete premise cannot complete setup"
        );
        remove_data_dir(&dir);
    }
}
