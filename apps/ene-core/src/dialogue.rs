//! One-to-one text round trip: intake, reply, stream, ack, timeline.
//!
//! `HostHandle::submit_text` mediates one accepted input: durable idempotent
//! replay, presence attach, presentation intake, then the companion-owned
//! turn (`ene_companion::dialogue`), whose inference boundary
//! (`ene_inference::InferenceExecutor`) owns admission, the attempt claim,
//! the provider call, adoption, and usage accounting. The Host maps the
//! resulting domain outcome to frames and records the open round between the
//! owner append and dispatch. A companion reply carrying a
//! `[task-control]` directive is interpreted by the companion and executed
//! through `HostTaskControl` against the existing Task
//! owner boundaries before the reply is stored; the stored reply is the
//! owner-derived text, never the directive. `HostHandle::confirm_presentation`
//! applies presentation observations, and `HostHandle::answer_history`
//! restores the filtered timeline.
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
use ene_api::v1::refs::{CommandWireId, RoundWireId, StreamWireId};
use ene_api::v1::refs::{ConnectionWireId, RevalidationReasonWire};
use ene_api::v1::reject::RejectKind;
use ene_api::v1::round::{
    ConfirmPresentationWire, HistoryItem, HistoryRequest, HistoryResponse,
    HistoryRole as HistoryRoleWire, PresentationStatus, RoundIntakeOutcomeWire, StreamClose,
    SubmitTextInput, TextStreamClose, TextStreamFrameWire, TextStreamOpen,
};
use ene_companion::dialogue::{
    AcceptedDialogueInput, DialogueBegin, DialogueOutcome, ReplayClassification,
    begin_turn_committed, classify_replay, finish_turn,
};
use ene_companion::{
    CommandId, CompanionId, CompanionLifecycle, CompanionRepository, HistoryRepository,
    HistoryRole, PresentationMark, RequestFingerprint, RoundIntentMark, UNDELIVERED_PAGE_MAX,
    UndeliveredRepository,
};
use ene_credential::{
    CredentialScrubber, CredentialSetRepository as _, CredentialSetRevision, ScrubbedText,
};
use ene_inference::{
    Admission, AuthorizedInference, DeltaFlow, DeltaSink, InferenceDispatchOutcome,
    InferenceExecutor, InferenceTechnicalError, NotSentReason, PreparedAdmission,
    ProviderTransport, TaskAgentAttemptPremise,
};
use ene_learning::{ExperienceCandidate, SecretScrubber as _};
use ene_permission::{CapabilityKind, ConsentRepository as _, EvaluationTracker};
use ene_plugin_ipc::WireFrame;
use ene_presence::{
    ConfirmTransitionOutcome, LiveReachabilityRef, MoveDecision, PresenceAttribution,
    PresenceCheckRef, PresenceGeneration, PresenceRepository as _, PresenceState, ThinMoveReason,
};
use ene_presentation::{
    ClientInputRef, CompanionAvailability, IntakePremise, OpenRound, RevalidationReason, RoundId,
    RoundIntakeOutcome, RoundIntent, SubmitClientInputCandidate, check_intake,
};
use ene_primitive::{RawId, WallClockWithTz};
use ene_store::Store;
use tokio::sync::Mutex as AsyncMutex;

use crate::serve::{
    CredStore, FrameSink, HostHandle, LiveInput, attribution_to_wire, device_client, emit_end,
    outgoing_fact, outgoing_frame, reject_frame, stale_reject, unpaired_close,
};

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
        RevalidationReason::InputOverLimit => "input-over-limit",
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

/// Admission never produces the over-limit, task-premise-stale, or data-use
/// hold reasons, which belong to dispatch; they map defensively rather than
/// claiming a setup failure.
fn admission_reason(reason: NotSentReason) -> &'static str {
    match reason {
        NotSentReason::SetupIncomplete => "setup-incomplete",
        NotSentReason::ConsentStale => "consent-stale",
        NotSentReason::NotInAllowlist => "not-in-allowlist",
        NotSentReason::EvaluationConsumed => "evaluation-consumed",
        NotSentReason::OverLimit => "unknown-reason",
        NotSentReason::TaskPremiseStale => "unknown-reason",
        NotSentReason::DataUseHeld => "unknown-reason",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachOutcome {
    /// Only this arm lets the caller proceed, with the generation from the
    /// committed fact — never an assumption that the attach succeeded.
    Attached(PresenceAttribution),
    /// The compare lost or the store failed: the caller reloads instead of
    /// proceeding.
    Raced,
    /// The connection was superseded before the ownership section: nothing
    /// was attached and the caller answers the typed stale rejection.
    Superseded,
}

impl HostHandle {
    pub(crate) fn open_wire_for(
        &self,
        connection: &ConnectionWireId,
        companion_key: &str,
    ) -> Option<RoundWireId> {
        let open = self.open_round_for(connection, companion_key)?;
        let wire = self.wire_for_round(&open.round)?;
        Some(RoundWireId(wire))
    }

    pub(crate) fn stale_frame(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        companion_key: &str,
        generation: u64,
    ) -> WireFrame {
        let current_round = self.open_wire_for(&live.connection_id, companion_key);
        stale_frame_with(frame, live, current_round, generation)
    }

    /// Attaches presence for the paired device when none is active, or
    /// restores it for a summoning client while recovery waits.
    ///
    /// Called only from the submit path with the generation it just read,
    /// under `NoActive` or `RecoveryWait`: only an [`AttachOutcome::Attached`]
    /// fact carries the fresh generation the caller may proceed with, and
    /// [`AttachOutcome::Raced`] means the caller reloads rather than
    /// proceeding. The [`ClientId`] comes from the deterministic device
    /// mapping, so a re-attaching device re-derives the same id. A summon on
    /// the current generation wins for any client and cancels the recovery
    /// intent (S5-14); a stale premise loses at the store compare, so the
    /// original client arriving late never auto-restores over a decided
    /// present. No reply is produced here.
    ///
    /// The compare/begin and confirm run synchronously inside the
    /// connection-ownership section (CCT §10.4), so a same-device replacement
    /// that wins the section leaves presence untouched and answers
    /// [`AttachOutcome::Superseded`]: the stale submit never attaches.
    pub(crate) async fn attach_presence(
        &self,
        live: &LiveInput,
        device_wire: &str,
        connection_live: bool,
        expected_state: PresenceState,
        expected_generation: PresenceGeneration,
    ) -> AttachOutcome {
        let client = device_client(device_wire);
        let companion = match self.store.ensure_running_companion().await {
            Ok(companion) => companion,
            Err(_) => return AttachOutcome::Raced,
        };
        let store = self.store.clone();
        let expected = PresenceCheckRef {
            expected_generation,
            expected_state,
            expected_active: None,
        };
        let attached = self
            .with_current_connection_blocking(live, move || {
                let Ok(MoveDecision::TransitioningToNew { generation }) = store
                    .compare_and_begin_transition_sync(
                        companion.as_raw(),
                        expected,
                        Some(client),
                        ThinMoveReason::InitialAttach,
                    )
                else {
                    return AttachOutcome::Raced;
                };
                let premise = LiveReachabilityRef {
                    client,
                    connection_live,
                };
                match store.confirm_transition_sync(companion.as_raw(), generation, premise) {
                    Ok(ConfirmTransitionOutcome::Confirmed(fact)) => AttachOutcome::Attached(fact),
                    Ok(ConfirmTransitionOutcome::RejectedAsStalePresence { .. }) | Err(_) => {
                        AttachOutcome::Raced
                    }
                }
            })
            .await;
        match attached {
            Some(outcome) => outcome,
            None => AttachOutcome::Superseded,
        }
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
    /// Presence attach runs when the loaded attribution is `NoActive` or
    /// `RecoveryWait`, and only on the envelope's observed generation
    /// premise: a missing `presence_generation_view` revalidates, a view
    /// that does not equal the current generation answers stale with the
    /// current values, and only then does the compare-and-commit run.
    ///
    /// A committed attach publishes the resulting attribution fact to this
    /// connection (IPC §12.2): the fact is unsolicited — it names no
    /// `reply_to`, so a Client awaiting this submit's answer absorbs it
    /// instead of mistaking it for one — and it precedes the
    /// auto-presented absence summary as well as this submit's own accept,
    /// open, and stream frames. The order is load-bearing: the summary's
    /// receipt carries the fresh generation and the Client's first ACK for
    /// it echoes the generation it observed, so a summary delivered ahead
    /// of its fact could only be answered `StalePresentation`.
    ///
    /// Idempotency is durable over the envelope `command_id`, looked up
    /// through [`lookup_command`](HistoryRepository::lookup_command) and
    /// judged by the same [`RequestFingerprint`] the store compares
    /// in-transaction (role, body, language, sender incarnation, and the
    /// canonical round intent — never the Host-decided round or its
    /// projection): an exact retry replays the stored accept ack verbatim
    /// without re-appending or re-streaming anything, including after a
    /// restart, while a different request answers a typed wire rejection
    /// (`ConflictingCommand`), never an intake outcome. Provider deltas may
    /// be presented before durable reply adoption while the presentation
    /// premise remains current. Durable completion/replay is reported only
    /// after the final reply append succeeds. Stream outcome replay is
    /// explicitly out of scope: only the accept ack replays.
    pub(crate) async fn submit_text(
        &self,
        frame: &WireFrame,
        submit: &SubmitTextInput,
        live: &LiveInput,
        transport: &impl ProviderTransport,
        sink: &mut dyn FrameSink,
        stream_tx: &tokio::sync::mpsc::Sender<WireFrame>,
    ) {
        let Some(device_wire) = live.paired_device.clone() else {
            return emit_end(sink, unpaired_close(frame, live));
        };
        let client = device_client(&device_wire);
        // The companion ref resolves through the handle mapping, never
        // assumed or derived: an unknown ref (a projection rotated by a
        // restart) revalidates so the Client relearns from presence.
        let companion = match self.resolve_companion(&submit.companion.0).await {
            Err(_) => {
                return emit_end(sink, held_frame(frame, live));
            }
            Ok(None) => {
                return emit_end(
                    sink,
                    revalidate_frame(
                        frame,
                        live,
                        intake_reason(&RevalidationReason::UnknownCompanion),
                    ),
                );
            }
            Ok(Some(companion)) => companion,
        };
        let Ok(Some(mut attribution)) = self.store.load_attribution(companion.as_raw()).await
        else {
            return emit_end(sink, held_frame(frame, live));
        };
        let companion_key = companion.as_raw().as_uuid().to_string();
        // Idempotency keys are mandatory: a command without one cannot be
        // replayed safely, so it is declined before any state changes
        // (before attach, before maps, before appends).
        let Some(command) = command_id_for(&frame.envelope) else {
            return emit_end(
                sink,
                revalidate_frame(
                    frame,
                    live,
                    intake_reason(&RevalidationReason::MissingCommandId),
                ),
            );
        };
        // Canonical round premise first: the two carriers must agree before
        // anything else is judged, so a contradictory frame is declined
        // stale with current values instead of adopting one side.
        let Some(round_premise) =
            canonical_round_premise(submit, frame.envelope.observed.round_view.as_ref())
        else {
            return emit_end(
                sink,
                self.stale_frame(frame, live, &companion_key, attribution.generation.as_u64()),
            );
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
            return emit_end(sink, held_frame(frame, live));
        };
        let credential_set = scrubbed.credential_set();
        let text = scrubbed.into_text();
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
                for response in self.replay_frames(
                    frame,
                    live,
                    round,
                    round_wire,
                    attribution.generation.as_u64(),
                ) {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
                return;
            }
            ReplayClassification::Conflict => {
                return emit_end(
                    sink,
                    reject_frame(
                        frame,
                        live,
                        RejectKind::ConflictingCommand,
                        command_conflict_detail(&command),
                    ),
                );
            }
            ReplayClassification::Held => {
                return emit_end(sink, held_frame(frame, live));
            }
            ReplayClassification::None => {}
        }
        // The scrubbed current input is secured before any optional
        // background. If it cannot fit the final request budget even alone,
        // reducing background cannot help: decline before acceptance (no
        // append, no presence move) with the explicit reason instead of
        // storing an unsendable turn and closing an interrupted stream.
        if !ene_companion::dialogue::dialogue_input_fits(&text) {
            return emit_end(sink, revalidate_frame(frame, live, "input-over-limit"));
        }
        // The winner's intake premise below carries the fresh generation from
        // the committed fact. Any other path carries the envelope view
        // untouched: intake reports a missing or mismatched view honestly.
        let mut attached_generation: Option<PresenceGeneration> = None;
        if matches!(
            attribution.state,
            PresenceState::NoActive | PresenceState::RecoveryWait
        ) {
            let Some(viewed) = frame.envelope.observed.presence_generation_view else {
                return emit_end(
                    sink,
                    revalidate_frame(frame, live, "missing-generation-view"),
                );
            };
            if viewed != attribution.generation.as_u64() {
                return emit_end(
                    sink,
                    self.stale_frame(frame, live, &companion_key, attribution.generation.as_u64()),
                );
            }
            match self
                .attach_presence(
                    live,
                    &device_wire,
                    live.connection_live,
                    attribution.state,
                    attribution.generation,
                )
                .await
            {
                AttachOutcome::Attached(fresh) => {
                    attached_generation = Some(fresh.generation);
                    attribution = fresh;
                    // Presence transition distribution (IPC §12.1 M-6,
                    // V-3): the compare-and-commit above made this
                    // companion's attribution authoritative at a new
                    // generation, so the resulting fact goes to this
                    // subscriber before anything that depends on it.
                    // Unsolicited by construction (outgoing_fact sets no
                    // reply_to), so the Client absorbs it while its request
                    // is in flight and never reads it as the answer to this
                    // submit; the accept/open/stream frames below keep their
                    // own correlation.
                    //
                    // Ordering is the invariant, not decoration: the
                    // auto-presented summary that follows carries the fresh
                    // generation on its receipt, and the first ACK for it
                    // echoes the generation the Client observed. Ahead of the
                    // fact that echo is the stale pre-summon view, and the
                    // ACK is refused as StalePresentation, leaving the backlog
                    // it carried unpresented.
                    let fact = outgoing_fact(
                        frame,
                        live,
                        WirePayload::PresenceAttribution(attribution_to_wire(self, &attribution)),
                    );
                    // Summon auto-present: this submit just established
                    // formal presence, so the absence backlog presents
                    // without an Owner query. Best-effort and bounded: a
                    // full buffer drops the push (the explicit request
                    // path re-presents), and the new turn's own reply
                    // still streams normally afterwards. An undelivered
                    // fact skips the push: an ACK for a summary whose fact
                    // never arrived could only be refused stale, so that
                    // recovery belongs to the explicit request path.
                    if matches!(
                        self.with_current_connection(live, || sink.emit(fact)),
                        Some(Ok(()))
                    ) {
                        for summary in self
                            .auto_present_for(frame, live, companion, &attribution)
                            .await
                        {
                            if !matches!(
                                self.with_current_connection(live, || sink.emit(summary)),
                                Some(Ok(()))
                            ) {
                                break;
                            }
                        }
                    }
                }
                AttachOutcome::Raced => {
                    let Ok(Some(current)) = self.store.load_attribution(companion.as_raw()).await
                    else {
                        return emit_end(sink, held_frame(frame, live));
                    };
                    if matches!(
                        current.state,
                        PresenceState::InTransition | PresenceState::RecoveryWait
                    ) {
                        return emit_end(sink, held_frame(frame, live));
                    }
                    {
                        return emit_end(
                            sink,
                            self.stale_frame(
                                frame,
                                live,
                                &companion_key,
                                current.generation.as_u64(),
                            ),
                        );
                    };
                }
                AttachOutcome::Superseded => {
                    // A same-device replacement won the ownership section:
                    // this submit attaches nothing and stops before any
                    // owner append, round, or provider dispatch.
                    return emit_end(
                        sink,
                        stale_reject(frame, live, "presence attach on a superseded connection"),
                    );
                }
            }
        }
        let requested = match &round_premise {
            RoundPremise::Auto => None,
            RoundPremise::Existing(reference) => match self.round_for(reference) {
                Some(round) => Some(round),
                None => {
                    return emit_end(
                        sink,
                        self.stale_frame(
                            frame,
                            live,
                            &companion_key,
                            attribution.generation.as_u64(),
                        ),
                    );
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
            return emit_end(sink, held_frame(frame, live));
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
            open_round: self.open_round_for(&live.connection_id, &companion_key),
        };
        let accepted = match check_intake(premise) {
            RoundIntakeOutcome::AcceptedForRound { round } => round,
            RoundIntakeOutcome::StaleRound { .. } => {
                return emit_end(
                    sink,
                    self.stale_frame(frame, live, &companion_key, attribution.generation.as_u64()),
                );
            }
            RoundIntakeOutcome::HeldForTransition => {
                return emit_end(sink, held_frame(frame, live));
            }
            RoundIntakeOutcome::NeedsRevalidation { reason } => {
                return emit_end(sink, revalidate_frame(frame, live, intake_reason(&reason)));
            }
        };
        // The companion owns the accepted-turn order: admission precedes the
        // durable append, the Host records the open round only after that
        // append commits, and dispatch plus reply integration follow.
        //
        // Admission (permission, credential, consent) runs outside the
        // ownership section; the durable Owner append is the Client-dependent
        // acceptance commit and runs inside it (CCT §10.4). A connection
        // superseded while admission ran commits no row, opens no round,
        // dispatches no provider call, and streams nothing.
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
        let authorized = match executor.admit_dialogue().await {
            Ok(Admission::Admitted(authorized)) => *authorized,
            Ok(Admission::Declined(reason)) => {
                return emit_end(
                    sink,
                    revalidate_frame(frame, live, admission_reason(reason)),
                );
            }
            Err(_) => return emit_end(sink, held_frame(frame, live)),
        };
        // Test-only race gate: pause after admission and before the guarded
        // acceptance section, so a test can supersede the connection in
        // between and pin that nothing commits.
        #[cfg(test)]
        if let Some(gate) = self.submit_accept_gate() {
            gate.pause().await;
        }
        let store = self.store.clone();
        let commit_input = input.clone();
        let committed = self
            .with_current_connection_blocking(live, move || {
                begin_turn_committed(
                    commit_input,
                    authorized,
                    |owner| store.append_message_sync(owner),
                    |companion, command| store.lookup_command_sync(companion, command),
                )
            })
            .await;
        let begin = match committed {
            // Superseded before the acceptance section: no Owner row, no
            // open round, no provider dispatch, no stream, and no accept ack
            // that could name a round.
            None => {
                return emit_end(
                    sink,
                    stale_reject(frame, live, "input on a superseded connection"),
                );
            }
            Some(begin) => begin,
        };
        match begin {
            DialogueBegin::Ready(turn) => {
                // The connection-bound open round is installed under the
                // ownership section: a replacement that wins this section
                // leaves the durable Owner row and the reply's durable
                // adoption to continue (the reply registers as undelivered
                // for the new connection), while this connection opens no
                // round and its stream aborts before any further publication.
                #[cfg(test)]
                {
                    let gate = crate::lock_unpoison(&self.submit_open_gate).clone();
                    if let Some(gate) = gate {
                        gate.pause().await;
                    }
                }
                let installed = self.record_open_round(
                    live,
                    &live.client_ref,
                    &companion_key,
                    OpenRound {
                        companion: companion.as_raw(),
                        client,
                        round: accepted,
                        generation: attribution.generation,
                    },
                );
                #[cfg(test)]
                {
                    let gate = crate::lock_unpoison(&self.submit_publish_gate).clone();
                    if let Some(gate) = gate {
                        gate.pause().await;
                    }
                }
                let stream = StreamWireId(RawId::new().as_uuid());
                // Installation is not publication authority: replacement can
                // win between them. Queue both control frames in one short
                // ownership section, never socket I/O or an await. Already
                // accepted work continues without a wire stream if stale.
                let opened = if installed {
                    match self.with_current_connection(live, || {
                        sink.emit(accept_frame(frame, live, &round_wire))?;
                        sink.emit(open_frame(
                            frame,
                            live,
                            &stream,
                            &round_wire,
                            generation_number,
                        ))
                    }) {
                        Some(Ok(())) => true,
                        Some(Err(_)) => return,
                        None => false,
                    }
                } else {
                    false
                };
                // Baselines the gate on the current record: the owner append
                // committed under the admission consent, and any move since
                // fails the attempt claim before the first delta. An
                // unreadable record fails closed to an interrupted stream,
                // like any post-acceptance failure.
                let Ok(Some(consent)) = self.store.load_current(CapabilityKind::Dialogue).await
                else {
                    if opened {
                        self.with_current_connection(live, || {
                            emit_end(
                                sink,
                                close_frame(frame, live, &stream, StreamClose::Interrupted),
                            );
                        });
                    }
                    return;
                };
                let consent = (consent.id, consent.rev.as_u64());
                let mut gate = StreamGate {
                    handle: self,
                    frame,
                    live,
                    connection: live.connection_id,
                    companion_key: companion_key.clone(),
                    companion,
                    stream,
                    round: accepted,
                    generation: attribution.generation,
                    consent,
                    credential_set,
                    tx: stream_tx.clone(),
                    seq: 0,
                    opened,
                };
                let task_control =
                    crate::task_control::HostTaskControl::new(self, companion, live.connection_id);
                let outcome = {
                    // The open round is Host-owned transient state the
                    // companion must never read directly: hand finish_turn
                    // a sync predicate instead. It runs after provider
                    // completion as an early refusal, sparing a doomed
                    // append attempt; durable adoption authority stays in
                    // the store transaction, which compares the turn's
                    // Owner message premise atomically.
                    let is_current = || {
                        self.open_round_for(&live.connection_id, &companion_key)
                            .is_none_or(|open| open.round == accepted)
                    };
                    finish_turn(
                        turn,
                        &self.store,
                        &executor,
                        &self.store,
                        &scrubber,
                        &task_control,
                        &mut gate,
                        &is_current,
                    )
                    .await
                };
                match outcome {
                    DialogueOutcome::Completed { experience, .. } => {
                        // The durable reply is the client-visible completion:
                        // the formation pass is queued and runs after the
                        // response is handed off, never before it (design
                        // H-1: response completion and all Learning updates
                        // are not one condition). The queue item is the
                        // premise pinned at completion, never a later re-read.
                        if let Some(experience) = experience {
                            self.queue_learning_formation(*experience);
                        }
                        gate.finish().await;
                    }
                    DialogueOutcome::Interrupted => {
                        gate.interrupt().await;
                    }
                }
            }
            DialogueBegin::Replayed { round, round_wire } => {
                for response in
                    self.replay_frames(frame, live, round, round_wire, generation_number)
                {
                    if sink.emit(response).is_err() {
                        break;
                    }
                }
            }
            DialogueBegin::StaleExpected { current } => {
                emit_end(sink, stale_frame_with(frame, live, None, current.as_u64()));
            }
            DialogueBegin::StaleConsent => {
                emit_end(sink, revalidate_frame(frame, live, "consent-stale"));
            }
            DialogueBegin::StaleCredentialSet => {
                // The input may carry a newly registered value; hold so the
                // Client retries and the Host re-scrubs under the new set.
                emit_end(sink, held_frame(frame, live));
            }
            DialogueBegin::Conflict => emit_end(
                sink,
                reject_frame(
                    frame,
                    live,
                    RejectKind::ConflictingCommand,
                    command_conflict_detail(&command),
                ),
            ),
            DialogueBegin::Held => emit_end(sink, held_frame(frame, live)),
            DialogueBegin::HeldByLifecycle(_) => {
                emit_end(sink, revalidate_frame(frame, live, "stopped-companion"));
            }
            DialogueBegin::Declined(reason) => {
                emit_end(
                    sink,
                    revalidate_frame(frame, live, admission_reason(reason)),
                );
            }
        }
    }

    /// Applies one presentation observation with no reply.
    ///
    /// Confirmation is an observation, never a report of completion: matching
    /// unpresented entries for the round move to presented on a presented
    /// status, and a non-presented status is a presentation start against
    /// `Pending` (the row stays re-presentable) or a current not-presented
    /// receipt against `PresentationUnknown` (the row returns to `Pending`).
    /// Failures end silently; the durable report state stays authoritative
    /// either way.
    ///
    /// Linearization (CCT §10.4): the round resolve, companion resolve, and
    /// the bounded `list_unpresented` page read run as prepare, then the
    /// durable compare-and-mark for every matching row runs inside one
    /// connection-ownership section with the currentness re-check. The
    /// prepared statuses are CAS premises only: a replacement that wins the
    /// table commits nothing (zero durable mutation for the stale
    /// connection), and one that loses cannot interleave a supersession
    /// between the check and any row's commit. The transition lock is taken
    /// only across the short guarded section, never during the async
    /// prepare.
    pub(crate) async fn confirm_presentation(
        &self,
        _frame: &WireFrame,
        live: &LiveInput,
        confirm: &ConfirmPresentationWire,
    ) -> Vec<WireFrame> {
        // Confirmation moves durable presentation status for rows of one
        // round: a connection superseded, replaced, or closed before this
        // observation is applied changes nothing (CCT §10.4).
        if self.with_current_connection(live, || ()).is_none() {
            return Vec::new();
        }
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
        // The bounded first page is enough for this observation path; the
        // full reconnect backlog subscription belongs to the presentation
        // slice, which re-pages with a cursor. This read is prepare only:
        // the statuses it returns are CAS premises, never authority.
        let Ok(page) = self
            .store
            .list_unpresented(companion, None, UNDELIVERED_PAGE_MAX)
            .await
        else {
            return Vec::new();
        };
        #[cfg(test)]
        if let Some(gate) = self.confirm_commit_gate() {
            gate.pause().await;
        }
        // Serialize commits, not preparation, with receipt transitions.
        // Prepared statuses are CAS premises, never ownership authority.
        let _gate = self.presentation_gate().await;
        let store = self.store.clone();
        let _applied = self
            .with_current_connection_blocking(live, move || {
                for entry in page.entries {
                    if entry.round == Some(mark.round) {
                        // An unavailable or stale row does not block the rest.
                        drop(store.compare_and_mark_reported_sync(entry.id, entry.status, mark));
                    }
                }
            })
            .await;
        Vec::new()
    }

    /// Items map oldest-first with Host-filtered display facts only, never
    /// undelivered reporting. A successful read may be empty; empty is
    /// distinct from `Unavailable` (the read failed), `InvalidRequest`
    /// (malformed `since`), and `StaleCompanion` (the projection rotated).
    /// Failure payloads stay operation-level and never echo a body, secret,
    /// or backend error.
    pub(crate) async fn answer_history(
        &self,
        frame: &WireFrame,
        request: &HistoryRequest,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let response = self.read_history(request).await;
        vec![outgoing_frame(
            frame,
            live,
            WirePayload::HistoryResponse(response),
        )]
    }

    async fn read_history(&self, request: &HistoryRequest) -> HistoryResponse {
        // The same companion mapping as submits: an unknown ref means the
        // Client's projection rotated, and it recovers by re-reading
        // presence, never by treating the timeline as empty.
        let companion = match self.resolve_companion(&request.companion.0).await {
            Err(_) => return HistoryResponse::Unavailable,
            Ok(None) => return HistoryResponse::StaleCompanion,
            Ok(Some(companion)) => companion,
        };
        // A malformed bound is never silently widened to "no bound".
        let since = match request.since.as_deref() {
            None => None,
            Some(bound) => match WallClockWithTz::parse_rfc3339(bound) {
                Ok(parsed) => Some(parsed),
                Err(_) => return HistoryResponse::InvalidRequest,
            },
        };
        // A round-scoped request resolves the stored projection durably, so
        // an old round stays addressable after a restart dropped the
        // transient wire map. A well-formed projection with no stored items
        // is an empty result; only an unreadable store is `Unavailable`.
        let round = match &request.round {
            None => None,
            Some(wire) => match self.store.round_for_stored_wire(companion, &wire.0).await {
                Ok(Some(round)) => Some(round),
                Ok(None) => return HistoryResponse::Items(Vec::new()),
                Err(_) => return HistoryResponse::Unavailable,
            },
        };
        match self
            .store
            .load_timeline(companion, since, round, request.limit)
            .await
        {
            Ok(items) => HistoryResponse::Items(
                items
                    .iter()
                    .map(|item| HistoryItem {
                        // The stored projection travels verbatim so views
                        // agree with accept acks, including after a restart.
                        // Pre-opaque rows fall back to the transient map,
                        // then to the legacy domain rendering (continuity
                        // for pre-release rows only).
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
                    .collect(),
            ),
            Err(_) => HistoryResponse::Unavailable,
        }
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
    /// it. Each item carries its own source range and transcript, so the
    /// worker judges exactly that Experience; it never reads a later History
    /// window and silently folds newer turns into an older pass.
    fn queue_learning_formation(&self, experience: ExperienceCandidate) {
        crate::lock_unpoison(&self.learning_queue).push_back(experience);
    }

    /// Whether a queued formation pass is waiting.
    pub(crate) fn has_pending_learning(&self) -> bool {
        !crate::lock_unpoison(&self.learning_queue).is_empty()
    }

    /// The queued Experience premises, in completion order.
    ///
    /// Test-only: lets a regression prove the queue carries the pinned source
    /// boundary and transcript, not just a companion id.
    #[cfg(test)]
    pub(crate) fn pending_learning_premises(&self) -> Vec<ExperienceCandidate> {
        crate::lock_unpoison(&self.learning_queue)
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
                let mut queue = crate::lock_unpoison(&self.learning_queue);
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

/// Pure wiring: every admission, attempt, provider, adoption, and usage
/// decision lives in `ene-inference`; this adapter only hands it the concrete
/// repositories and takes the short tracker lock for the single-use
/// authorization.
pub(crate) struct HostInference<'a, T> {
    store: &'a Store,
    cred_store: &'a CredStore,
    tracker: &'a AsyncMutex<EvaluationTracker>,
    transport: &'a T,
}

impl<'a, T: ProviderTransport + Send + Sync> HostInference<'a, T> {
    /// Builds the Host inference boundary from its concrete repositories.
    ///
    /// Composition only: every admission, attempt, provider, adoption, and
    /// usage decision lives in `ene-inference`.
    pub(crate) fn new(
        store: &'a Store,
        cred_store: &'a CredStore,
        tracker: &'a AsyncMutex<EvaluationTracker>,
        transport: &'a T,
    ) -> Self {
        HostInference {
            store,
            cred_store,
            tracker,
            transport,
        }
    }
}

impl<T: ProviderTransport + Send + Sync> HostInference<'_, T> {
    /// Runs one prepare step and turns it into an admission under the
    /// single-use tracker lock.
    async fn admit(
        &self,
        prepared: impl std::future::Future<Output = Result<PreparedAdmission, InferenceTechnicalError>>,
    ) -> Result<Admission, InferenceTechnicalError> {
        match prepared.await? {
            PreparedAdmission::Declined(reason) => Ok(Admission::Declined(reason)),
            PreparedAdmission::Ready(request) => {
                let mut tracker = self.tracker.lock().await;
                Ok(request.authorize(&mut tracker))
            }
        }
    }
}

impl<T: ProviderTransport + Send + Sync> InferenceExecutor for HostInference<'_, T> {
    async fn admit_dialogue(&self) -> Result<Admission, InferenceTechnicalError> {
        self.admit(ene_inference::prepare_dialogue_admission(
            self.store,
            self.store,
            self.cred_store,
        ))
        .await
    }

    async fn admit_learning(&self) -> Result<Admission, InferenceTechnicalError> {
        self.admit(ene_inference::prepare_learning_admission(
            self.store,
            self.store,
            self.cred_store,
        ))
        .await
    }

    async fn admit_task_agent(
        &self,
        task_agent: TaskAgentAttemptPremise,
    ) -> Result<Admission, InferenceTechnicalError> {
        self.admit(ene_inference::prepare_task_agent_admission(
            self.store,
            self.store,
            self.cred_store,
            task_agent,
        ))
        .await
    }

    async fn dispatch(
        &self,
        authorized: AuthorizedInference,
        prompt: ScrubbedText,
        sink: &mut (dyn DeltaSink + Send),
        abort: Option<&ene_inference::DispatchAbort>,
    ) -> Result<InferenceDispatchOutcome, InferenceTechnicalError> {
        ene_inference::dispatch_authorized(
            authorized,
            prompt,
            sink,
            abort,
            self.store,
            self.store,
            self.store,
            self.transport,
        )
        .await
    }
}

/// Presentation gate for one open text stream: each delta is shown only
/// while its premise is still current.
///
/// Baselines are captured at stream open, after the owner append committed,
/// so they equal the admitted premises: the presented round, the presence
/// generation, the dialogue consent `(id, rev)`, and the credential-set
/// revision. Every delta re-reads those premises before it is shown. A newer
/// submit replacing the open round, a presence move, a consent move, a
/// lifecycle stop, or a credential registration aborts the stream first, so
/// no delta produced after invalidation is presented as current. Deltas shown
/// before the change stay as historical partial presentation; the aborted
/// provider read never completes, so the stale reply is never adopted.
/// Delivery itself backpressures through the bounded channel: a slow client
/// paces the provider instead of queueing unboundedly.
struct StreamGate<'a> {
    handle: &'a HostHandle,
    frame: &'a WireFrame,
    live: &'a LiveInput,
    /// The connection that opened this stream; every publication re-checks
    /// that it is still the device's current authenticated connection.
    connection: ConnectionWireId,
    companion_key: String,
    companion: CompanionId,
    stream: StreamWireId,
    round: RoundId,
    generation: PresenceGeneration,
    consent: (String, u64),
    credential_set: CredentialSetRevision,
    tx: tokio::sync::mpsc::Sender<WireFrame>,
    seq: u64,
    /// No wire close (or delta) is legal unless Open was queued.
    opened: bool,
}

impl StreamGate<'_> {
    /// Whether the connection that opened this stream is still current.
    ///
    /// Reads the connection table, never the stale `LiveInput` snapshot: a
    /// same-device replacement keeps `client_ref` and presence generation
    /// identical, so only the table can tell that this stream's connection
    /// was superseded (IPC §9.3 replacement). Synchronous by design: the
    /// publication path calls it immediately before `permit.send`, with no
    /// await in between.
    fn connection_current(&self) -> bool {
        self.live
            .authority
            .is_current_authenticated(&self.live.connection_id)
    }

    /// Re-reads every presentation premise; any move — or any unreadable
    /// premise — stops the stream. Failing closed keeps an unprovable
    /// premise from presenting as current.
    async fn current(&self) -> bool {
        // The connection itself must still be the current authenticated one:
        // a replacement invalidates this stream even though the device,
        // client id, generation, and round key look unchanged.
        if !self.connection_current() {
            return false;
        }
        // A newer submit replaced this stream's round: the owner's
        // attention moved on, so further deltas are not current.
        let open = self
            .handle
            .open_round_for(&self.connection, &self.companion_key);
        if open.is_none_or(|retained| retained.round != self.round) {
            return false;
        }
        let Ok(Some(attribution)) = self
            .handle
            .store
            .load_attribution(self.companion.as_raw())
            .await
        else {
            return false;
        };
        if attribution.generation != self.generation {
            return false;
        }
        let consent = match self
            .handle
            .store
            .load_current(CapabilityKind::Dialogue)
            .await
        {
            Ok(Some(record)) => (record.id, record.rev.as_u64()),
            _ => return false,
        };
        if consent != self.consent {
            return false;
        }
        let Ok(lifecycle) = self.handle.store.load_lifecycle(self.companion).await else {
            return false;
        };
        if !matches!(lifecycle, Some(CompanionLifecycle::Running)) {
            return false;
        }
        let Ok(set) = self.handle.store.current_set_revision().await else {
            return false;
        };
        set == self.credential_set
    }

    fn delta_frame(&self, delta: &str, is_final: bool) -> WireFrame {
        outgoing_frame(
            self.frame,
            self.live,
            WirePayload::TextStreamFrame(TextStreamFrameWire {
                stream: self.stream,
                seq: self.seq,
                delta: delta.to_owned(),
                is_final,
            }),
        )
    }

    /// Closes the displayed delta sequence as completed: the final empty
    /// frame carries the next sequence number, and the close states
    /// completion separately. A gone client ends the send; the reply is
    /// already durable, so the close is best-effort either way.
    ///
    /// A connection that is no longer current — replaced while the reply
    /// committed — never hears `Completed`: the durable reply stands, but
    /// this stream was interrupted by the replacement, and the reply reaches
    /// the new connection through its own presentation subscription.
    async fn finish(&mut self) {
        if !self.connection_current() || !self.current().await {
            self.interrupt().await;
            return;
        }
        if !self.publish_current(self.delta_frame("", true)).await {
            self.interrupt().await;
            return;
        }
        if !self
            .publish_current(close_frame(
                self.frame,
                self.live,
                &self.stream,
                StreamClose::Completed,
            ))
            .await
        {
            self.interrupt().await;
        }
    }

    /// Queues one close path frame if the stream opened on the wire and the
    /// connection is still current. An unopened stream sends nothing: a
    /// stream the Client never saw opens no close-only lifecycle now.
    async fn publish_current(&mut self, frame: WireFrame) -> bool {
        if !self.opened {
            return false;
        }
        let Ok(permit) = self.tx.reserve().await else {
            return false;
        };
        self.handle
            .with_current_connection(self.live, || permit.send(frame))
            .is_some()
    }

    /// Closes the stream interrupted after a stale or failed run: displayed
    /// deltas stay, and no reply is adopted.
    async fn interrupt(&mut self) {
        if !self.opened {
            return;
        }
        if self
            .tx
            .send(close_frame(
                self.frame,
                self.live,
                &self.stream,
                StreamClose::Interrupted,
            ))
            .await
            .is_err()
        {
            // Same gone-client close as above; nothing durable is at stake.
        }
    }
}

impl DeltaSink for StreamGate<'_> {
    fn push_delta<'a>(
        &'a mut self,
        delta: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = DeltaFlow> + Send + 'a>> {
        Box::pin(async move {
            // Fast path: never reserve capacity for an already-stale
            // stream, and never hold a permit across the premise reads.
            if !self.opened || !self.current().await {
                return DeltaFlow::Abort("the presentation premise went stale");
            }
            let frame = self.delta_frame(delta, false);
            let permit = match self.tx.reserve().await {
                Ok(permit) => permit,
                Err(_) => return DeltaFlow::Abort("the client connection is gone"),
            };
            // Re-check after the capacity wait: the premise may have gone
            // stale while parked, and a stale delta must never publish.
            // `permit.send` is synchronous, so no await sits between this
            // check and the publication.
            if !self.current().await {
                drop(permit);
                return DeltaFlow::Abort("the presentation premise went stale");
            }
            // Final connection currentness check, synchronous and after the
            // last await: a same-device replacement during the premise reads
            // must not let this stream publish one more delta (IPC §9.3).
            if self
                .handle
                .with_current_connection(self.live, || permit.send(frame))
                .is_none()
            {
                return DeltaFlow::Abort("the connection was replaced");
            }
            self.seq += 1;
            DeltaFlow::Continue
        })
    }
}

#[cfg(test)]
mod tests;
