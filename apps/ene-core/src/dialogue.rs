//! One-to-one text round trip: intake, reply, stream, ack, timeline.
//!
//! `HostHandle::submit_text` mediates one accepted input: durable idempotent
//! replay, presence attach, presentation intake, then the companion-owned
//! turn (`ene_companion::dialogue`), whose inference boundary
//! (`ene_inference::InferenceExecutor`) owns admission, the attempt claim,
//! the provider call, adoption, and usage accounting. The Host maps the
//! resulting domain outcome to frames and records the open round between the
//! owner append and dispatch. `HostHandle::confirm_presentation` applies
//! presentation observations, and `HostHandle::answer_history` restores the
//! filtered timeline.
//!
//! `Stage 2` wire reason vocabulary for
//! [`NeedsRevalidation`](ene_api::v1::round::RoundIntakeOutcomeWire::NeedsRevalidation)
//! outcomes: `"missing-generation-view"`, `"unknown-companion"`,
//! `"stopped-companion"` (all straight from
//! [`ene_presentation::RevalidationReason`]), plus `"setup-incomplete"` and
//! `"consent-stale"` for the admission gates and `"not-in-allowlist"` as a
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
//!   (`ConflictingCommand`), judged by the companion's fingerprint comparison
//!   shared with the store's in-transaction pre-check: declined without side
//!   effects, never an intake outcome, never a retry signal.
//! - A stale or held owner append becomes the matching outcome frame. Its
//!   projection entry stays mapped but unpublished: no open-round record was
//!   made and no ack carried it, so later intakes surface the round as stale
//!   rather than rebinding anything onto it.
//! - An admission decline becomes `NeedsRevalidation` with the setup/consent
//!   reason above: the Client recovers by running the setup flow, then retries
//!   with a fresh local id.
//! - Any failure after acceptance (inference not sent, transport error, reply
//!   append lost) becomes the accept ack plus a stream closed as
//!   [`Interrupted`](ene_api::v1::round::StreamClose::Interrupted). Usage
//!   accounting follows certainty, never adoption (owned by
//!   `ene-inference`): never-sent calls record no fact, uncertain attempts
//!   record [`Unknown`](ene_inference::UsageSource::Unknown) counts, and
//!   reported counts are kept even when the reply cannot be adopted. A
//!   usage-record failure after a durable reply keeps the `Completed` close:
//!   the reply happened, and the usage gap is the documented `Stage 2`
//!   follow-up (retry queue), not a reason to misreport the stream.
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
use ene_companion::dialogue::{
    AcceptedDialogueInput, DialogueBegin, DialogueOutcome, ReplayClassification, begin_turn,
    classify_replay, finish_turn,
};
use ene_companion::{
    CommandId, CompanionId, CompanionLifecycle, CompanionRepository, HistoryRepository,
    HistoryRole, PresentationMark, ReportStatus, RequestFingerprint, RoundIntentMark,
    UndeliveredRepository,
};
use ene_credential::{
    CredentialRefRepository, CredentialSetRepository as _, CredentialStore, REDACTED_CREDENTIAL,
    ScrubbedText,
};
use ene_inference::{
    Admission, AuthorizedInference, InferenceDispatchOutcome, InferenceExecutor,
    InferenceTechnicalError, NotSentReason, PreparedAdmission, ProviderTransport,
};
use ene_learning::{ExperienceCandidate, SecretScrubError, SecretScrubber as _};
use ene_permission::EvaluationTracker;
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
use tokio::sync::Mutex as AsyncMutex;

use crate::serve::{
    CredStore, HostHandle, LiveInput, device_client, outgoing_frame, reject_frame, unpaired_close,
};

/// Maximum stream chunk size in Unicode scalar values.
///
/// Chunks never split a code point, at the cost of byte-uneven frames.
pub const CHUNK_CHARS: usize = 200;

/// Always returns at least one chunk: empty text yields one empty delta so
/// every stream carries a final frame (a frame without `is_final` never
/// completes anything, and an empty stream would leave completion ambiguous).
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

/// Canonical client round premise of one [`SubmitTextInput`] send.
///
/// The payload names the premise (`SubmitTextInput.round`); the envelope
/// `round_view` is the mirror the Client relied on (IPC §5: comparison
/// material, not a claim) and must agree with it once populated. A
/// disagreement means two different round premises: neither side is adopted
/// — the caller answers stale with current values and the Client re-syncs.
///
/// A force-new request is the design's round-less new-round request
/// (IPC §13.1: `round = None`, `round_view = None`), so it names no premise
/// at all. A force-new frame carrying a premise in either carrier is
/// self-contradictory and is rejected here, never silently reinterpreted as
/// the flag or joined on the hint.
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

fn intake_reason(reason: &RevalidationReason) -> &'static str {
    match reason {
        RevalidationReason::MissingGenerationView => "missing-generation-view",
        RevalidationReason::UnknownCompanion => "unknown-companion",
        RevalidationReason::StoppedCompanion => "stopped-companion",
        RevalidationReason::MissingCommandId => "missing-command-id",
        RevalidationReason::UnknownReasonTag => "unknown-reason",
    }
}

fn accept_frame(frame: &WireFrame, live: &LiveInput, round: &RoundWireId) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound {
            round: round.clone(),
        }),
    )
}

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

fn held_frame(frame: &WireFrame, live: &LiveInput) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::HeldForTransition),
    )
}

fn revalidate_frame(frame: &WireFrame, live: &LiveInput, reason: &str) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::NeedsRevalidation {
            reason: RevalidationReasonWire(reason.to_string()),
        }),
    )
}

fn command_conflict_detail(command: &CommandId) -> String {
    format!(
        "command {} reused with a different request",
        command.0.as_uuid().as_hyphenated()
    )
}

/// Admission never produces the route-mismatch or over-limit reasons; they
/// map defensively rather than claiming a setup failure.
fn admission_reason(reason: NotSentReason) -> &'static str {
    match reason {
        NotSentReason::SetupIncomplete => "setup-incomplete",
        NotSentReason::ConsentStale | NotSentReason::ConsentMismatch => "consent-stale",
        NotSentReason::NotInAllowlist => "not-in-allowlist",
        NotSentReason::EvaluationConsumed => "evaluation-consumed",
        NotSentReason::OverLimit => "unknown-reason",
    }
}

fn completed_frames(
    frame: &WireFrame,
    live: &LiveInput,
    round: &RoundWireId,
    generation: u64,
    text: &str,
) -> Vec<WireFrame> {
    let stream = StreamWireId(RawId::new().as_uuid());
    let mut responses = vec![
        accept_frame(frame, live, round),
        open_frame(frame, live, &stream, round, generation),
    ];
    for (position, delta) in chunk_text(text).iter().enumerate() {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachOutcome {
    /// Only this arm lets the caller proceed, with the generation from the
    /// committed fact — never an assumption that the attach succeeded.
    Attached(PresenceAttribution),
    /// The compare lost or the store failed: the caller reloads instead of
    /// proceeding.
    Raced,
}

impl HostHandle {
    pub(crate) fn open_wire_for(
        &self,
        client_ref: &str,
        companion_key: &str,
    ) -> Option<RoundWireId> {
        let open = self.open_round_for(client_ref, companion_key)?;
        let wire = self.wire_for_round(&open.round)?;
        Some(RoundWireId(wire))
    }

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

    /// Attaches presence for the paired device when none is active.
    ///
    /// Called only from the submit path with the `NoActive` generation it
    /// just read; only an [`AttachOutcome::Attached`] fact carries the fresh
    /// generation the caller may proceed with, and [`AttachOutcome::Raced`]
    /// means the caller reloads rather than proceeding. The [`ClientId`] comes
    /// from the deterministic device mapping, so a re-attaching device
    /// re-derives the same id. No reply is produced here.
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

    /// Mediates one [`SubmitTextInput`] frame into the companion turn.
    ///
    /// Order: companion mapping, mandatory command key, durable idempotent
    /// replay, presence attach, presentation intake, then the companion-owned
    /// turn (`ene_companion::dialogue::begin_turn`/`finish_turn`) with its
    /// inference boundary. Admission precedes the append so a declined input
    /// leaves neither history rows nor transient round claims behind; the
    /// round projection is minted atomically with its map entry (one domain
    /// round, one wire), and a racy duplicate that lands on
    /// [`HistoryAppendOutcome::AlreadyCommittedAs`] answers the original
    /// accept without re-running inference. The canonical round premise comes
    /// from the input `round`, a populated envelope `round_view` must agree
    /// with it, and a force-new request carries no premise at all — a
    /// contradictory frame answers stale with current values instead of
    /// adopting either side; a present-but-unresolvable round is stale, never
    /// rebound.
    ///
    /// Presence attach runs only when the loaded attribution is `NoActive`,
    /// and only on the envelope's observed generation premise: a missing
    /// `presence_generation_view` revalidates, a view that does not equal the
    /// current `NoActive` generation answers stale with the current values,
    /// and only then does the compare-and-commit run.
    ///
    /// Idempotency is durable over the envelope `command_id`, looked up
    /// through [`lookup_command`](HistoryRepository::lookup_command) and
    /// judged by the same [`RequestFingerprint`] the store compares
    /// in-transaction (role, body, language, sender incarnation, and the
    /// canonical round intent — never the Host-decided round or its
    /// projection): an exact retry replays the stored accept ack verbatim
    /// without re-appending or re-streaming anything, including after a
    /// restart, while a different request answers a typed wire rejection
    /// (`ConflictingCommand`), never an intake outcome. Response text is never
    /// presented unless its reply append committed. Stream outcome replay is
    /// explicitly out of scope: only the accept ack replays.
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
        // The companion ref resolves through the handle mapping, never
        // assumed or derived: an unknown ref (a projection rotated by a
        // restart) revalidates so the Client relearns from presence.
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
        // Registered credentials never reach durable History or a model
        // prompt: the owner input is redacted before any durable decision
        // (the fingerprint included), so a retry redacts the same raw text to
        // the same canonical form. An unprovable secret boundary holds the
        // send without side effects instead of storing or sending raw text.
        let scrubber = CredentialScrubber {
            refs: &self.store,
            store: &self.cred_store,
        };
        let Ok(scrubbed) = scrubber.scrub(&submit.body.text).await else {
            return vec![held_frame(frame, live)];
        };
        let credential_set = scrubbed.credential_set;
        let text = scrubbed.text;
        // The request fingerprint is the immutable client semantics: role,
        // body, language, sending incarnation, and the canonical round
        // intent. Force-new carries no premise (the gate above declined
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
            text: text.clone(),
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
        // as early. The companion owns the fingerprint judge; core only
        // maps its verdict to frames.
        match classify_replay(&self.store, companion, &command, incoming_fingerprint).await {
            ReplayClassification::Replay { round, round_wire } => {
                return self.replay_frames(
                    frame,
                    live,
                    round,
                    round_wire,
                    attribution.generation.as_u64(),
                );
            }
            ReplayClassification::Conflict => {
                return vec![reject_frame(
                    frame,
                    live,
                    RejectKind::ConflictingCommand,
                    command_conflict_detail(&command),
                )];
            }
            ReplayClassification::Held => return vec![held_frame(frame, live)],
            ReplayClassification::None => {}
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
        // force-new mints and never joins; a resolved premise joins that
        // round; no premise joins-or-mints.
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
                    text: text.clone(),
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
        // The companion owns the accepted-turn order: admission precedes the
        // durable append, the Host records the open round only after that
        // append commits, and dispatch plus reply integration follow.
        let round_wire = self.round_wire_or_mint(&accepted);
        let generation_number = attribution.generation.as_u64();
        let executor = HostInference {
            store: &self.store,
            cred_store: &self.cred_store,
            tracker: &self.tracker,
            transport,
        };
        let input = AcceptedDialogueInput {
            companion,
            round: accepted.as_raw(),
            client,
            generation: attribution.generation,
            text,
            credential_set,
            lang: submit.body.lang.0.clone(),
            local_id: Some(submit.local_id.0.clone()).filter(|key| !key.is_empty()),
            command,
            round_wire: round_wire.0.clone(),
            round_intent,
            incarnation: Some((
                frame.envelope.sender.incarnation_id.counter,
                frame.envelope.sender.incarnation_id.random,
            )),
        };
        match begin_turn(input, &self.store, &executor).await {
            DialogueBegin::Ready(turn) => {
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
                match finish_turn(turn, &self.store, &executor, &scrubber).await {
                    DialogueOutcome::Completed { text, experience } => {
                        // The durable reply is the client-visible completion:
                        // the formation pass is queued and runs after the
                        // response is handed off, never before it (design
                        // H-1: response completion and all Learning updates
                        // are not one condition). The queue item is the
                        // premise pinned at completion, never a later re-read.
                        if let Some(experience) = experience {
                            self.queue_learning_formation(*experience);
                        }
                        completed_frames(frame, live, &round_wire, generation_number, &text)
                    }
                    DialogueOutcome::Interrupted => {
                        interrupted_frames(frame, live, &round_wire, generation_number)
                    }
                }
            }
            DialogueBegin::Replayed { round, round_wire } => {
                self.replay_frames(frame, live, round, round_wire, generation_number)
            }
            DialogueBegin::StaleExpected { current } => {
                vec![stale_frame_with(frame, live, None, current.as_u64())]
            }
            DialogueBegin::StaleConsent => vec![revalidate_frame(frame, live, "consent-stale")],
            DialogueBegin::StaleCredentialSet => {
                // The input may carry a newly registered value; hold so the
                // Client retries and the Host re-scrubs under the new set.
                vec![held_frame(frame, live)]
            }
            DialogueBegin::Conflict => vec![reject_frame(
                frame,
                live,
                RejectKind::ConflictingCommand,
                command_conflict_detail(&command),
            )],
            DialogueBegin::Held => vec![held_frame(frame, live)],
            DialogueBegin::HeldByLifecycle(_) => {
                vec![revalidate_frame(frame, live, "stopped-companion")]
            }
            DialogueBegin::Declined(reason) => {
                vec![revalidate_frame(frame, live, admission_reason(reason))]
            }
        }
    }

    /// Applies one presentation observation with no reply.
    ///
    /// Confirmation is an observation, never a report of completion: matching
    /// pending undelivered entries for the round move to presented (or to
    /// presentation-unknown for any non-presented status, including wire
    /// `Failed`). Failures end silently; the durable report state stays
    /// authoritative either way.
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
        // The same companion mapping as submits: an unknown ref answers an
        // empty view, the documented `Stage 2` gap for this path.
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

    fn wire_for_round_value(&self, round: RawId) -> Option<String> {
        self.wire_for_round(&RoundId::from_raw(round))
    }

    /// The stored round wire travels verbatim, so a retry after a restart
    /// replays instead of going stale on the dropped transient map.
    /// Pre-opaque rows (no stored wire) fall back to the transient map; only
    /// when both miss does the intake answer stale with the current
    /// generation, and the Client recovers missed items through history.
    fn replay_frames(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        round: RawId,
        stored_wire: Option<String>,
        generation: u64,
    ) -> Vec<WireFrame> {
        let wire = stored_wire.or_else(|| self.wire_for_round_value(round));
        match wire {
            Some(wire) => vec![accept_frame(frame, live, &RoundWireId(wire))],
            None => vec![stale_frame_with(frame, live, None, generation)],
        }
    }

    /// Queues the Experience premise pinned at one completed reply.
    ///
    /// Best-effort by design: the stream outcome was already decided by the
    /// durable reply append, so a formation decline or failure never rewrites
    /// it. Each item carries its own source range, transcript, and
    /// Client / round / continuity correspondence, so the worker judges
    /// exactly that Experience; it never reads a later History window and
    /// silently folds newer turns into an older pass.
    fn queue_learning_formation(&self, experience: ExperienceCandidate) {
        lock_learning_queue(&self.learning_queue).push_back(experience);
    }

    /// Whether a queued formation pass is waiting.
    pub(crate) fn has_pending_learning(&self) -> bool {
        !lock_learning_queue(&self.learning_queue).is_empty()
    }

    /// The queued Experience premises, in completion order.
    ///
    /// Test-only: lets a regression prove the queue carries the pinned source
    /// boundary and correspondence, not just a companion id.
    #[cfg(test)]
    pub(crate) fn pending_learning_premises(&self) -> Vec<ExperienceCandidate> {
        lock_learning_queue(&self.learning_queue)
            .iter()
            .cloned()
            .collect()
    }

    /// Drains queued Learning formation passes, one pinned premise at a time.
    ///
    /// The queue is in-memory and best-effort: a crash before the drain loses
    /// only the pending derived updates, exactly as a crash during the
    /// previous synchronous pass did. No pass is durable, so a restart never
    /// replays an old one and cannot duplicate a formation. The worker lock
    /// serializes passes; the repository's compare-before-commit additionally
    /// keeps a genuine overlap from overwriting newer recognition. Stopped
    /// companions are skipped because stopping must not start new internal
    /// activity. A pass failure drops its item, so there is no retry storm.
    pub(crate) async fn run_pending_learning<T: ProviderTransport>(&self, transport: &T) {
        let _serialized = self.learning_worker.lock().await;
        loop {
            let next = {
                let mut queue = lock_learning_queue(&self.learning_queue);
                queue.pop_front()
            };
            let Some(experience) = next else {
                break;
            };
            let companion = CompanionId::from_raw(experience.companion);
            match self.store.load_lifecycle(companion).await {
                Ok(Some(CompanionLifecycle::Running)) => {}
                _ => continue,
            }
            let executor = HostInference {
                store: &self.store,
                cred_store: &self.cred_store,
                tracker: &self.tracker,
                transport,
            };
            let scrubber = CredentialScrubber {
                refs: &self.store,
                store: &self.cred_store,
            };
            drop(
                ene_companion::dialogue::propose_experience(
                    experience,
                    &self.store,
                    &executor,
                    &scrubber,
                )
                .await,
            );
        }
    }
}

fn lock_learning_queue(
    queue: &std::sync::Mutex<std::collections::VecDeque<ExperienceCandidate>>,
) -> std::sync::MutexGuard<'_, std::collections::VecDeque<ExperienceCandidate>> {
    match queue.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Redacts registered credential values from text on its way to a model
/// prompt or durable content.
///
/// Uses the existing credential boundary: bearer values are borrowed inside
/// `with_bearer` and only redacted copies escape. The credential store pins
/// its values for the whole Host run, so the revision read here names exactly
/// the values being applied; an explicit approval or the startup sweep
/// advances that revision with its own durable sweep. Every value is applied
/// longest-first so a shorter registered value cannot split an occurrence of
/// a longer one. An unreadable registry or bearer fails closed: absence
/// cannot be proven, so the caller must not use the original text.
struct CredentialScrubber<'a> {
    refs: &'a Store,
    store: &'a CredStore,
}

impl ene_learning::SecretScrubber for CredentialScrubber<'_> {
    async fn scrub(&self, text: &str) -> Result<ScrubbedText, SecretScrubError> {
        let credential_set = self
            .refs
            .current_set_revision()
            .await
            .map_err(|_| SecretScrubError::RegistryUnavailable)?;
        let refs = self
            .refs
            .list_refs()
            .await
            .map_err(|_| SecretScrubError::RegistryUnavailable)?;
        let mut known: Vec<(usize, ene_credential::CredentialRef)> = Vec::with_capacity(refs.len());
        for credential in refs {
            let length = self
                .store
                .with_bearer(&credential, |bearer| bearer.len())
                .map_err(|_| SecretScrubError::SecretUnavailable)?;
            if length == 0 {
                // An empty value matches every position; treating it as
                // unprovable keeps the raw text out of prompts and storage.
                return Err(SecretScrubError::SecretUnavailable);
            }
            known.push((length, credential));
        }
        known.sort_by_key(|(length, _)| std::cmp::Reverse(*length));
        let mut scrubbed = text.to_owned();
        for (_, credential) in known {
            let replaced = self.store.with_bearer(&credential, |bearer| {
                scrubbed.replace(bearer, REDACTED_CREDENTIAL)
            });
            let Ok(next) = replaced else {
                // A registered credential exists but its bearer cannot be
                // read, so absence of the value cannot be proven. Fail closed
                // rather than risk putting the raw text in a prompt or a
                // durable Learning row.
                return Err(SecretScrubError::SecretUnavailable);
            };
            scrubbed = next;
        }
        Ok(ScrubbedText {
            text: scrubbed,
            credential_set,
        })
    }
}

/// The compare pins the caller's observed `(NoActive, generation)`
/// expectation, so a concurrent move wins by failing this compare instead of
/// overwriting. Only a committed confirm answers [`AttachOutcome::Attached`]
/// with the fresh fact; a lost race, denied or held outcome, failed confirm,
/// or store failure answers [`AttachOutcome::Raced`]. An unconfirmed
/// transition reads back as `InTransition`, so the caller's reload reports
/// held, which is honest.
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

/// Pure wiring: every admission, attempt, provider, adoption, and usage
/// decision lives in `ene-inference`; this adapter only hands it the concrete
/// repositories and takes the short tracker lock for the single-use
/// authorization.
struct HostInference<'a, T> {
    store: &'a Store,
    cred_store: &'a CredStore,
    tracker: &'a AsyncMutex<EvaluationTracker>,
    transport: &'a T,
}

impl<T: ProviderTransport + Send + Sync> InferenceExecutor for HostInference<'_, T> {
    async fn admit_dialogue(&self) -> Result<Admission, InferenceTechnicalError> {
        match ene_inference::prepare_dialogue_admission(self.store, self.store, self.cred_store)
            .await?
        {
            PreparedAdmission::Declined(reason) => Ok(Admission::Declined(reason)),
            PreparedAdmission::Ready(request) => {
                let mut tracker = self.tracker.lock().await;
                Ok(request.authorize(&mut tracker))
            }
        }
    }

    async fn admit_learning(&self) -> Result<Admission, InferenceTechnicalError> {
        match ene_inference::prepare_learning_admission(self.store, self.store, self.cred_store)
            .await?
        {
            PreparedAdmission::Declined(reason) => Ok(Admission::Declined(reason)),
            PreparedAdmission::Ready(request) => {
                let mut tracker = self.tracker.lock().await;
                Ok(request.authorize(&mut tracker))
            }
        }
    }

    async fn dispatch(
        &self,
        authorized: AuthorizedInference,
        prompt: ScrubbedText,
    ) -> Result<InferenceDispatchOutcome, InferenceTechnicalError> {
        ene_inference::dispatch_authorized(
            authorized,
            prompt,
            self.store,
            self.store,
            self.store,
            self.transport,
        )
        .await
    }
}

fn empty_history(frame: &WireFrame, live: &LiveInput) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::HistoryView(HistoryView { items: Vec::new() }),
    )
}

#[cfg(test)]
mod tests;
