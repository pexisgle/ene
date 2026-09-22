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
//! outcomes: `intake_reason` maps [`ene_presentation::RevalidationReason`]
//! to `"missing-generation-view"`, `"unknown-companion"`,
//! `"stopped-companion"`, `"missing-command-id"`, `"input-over-limit"`, and
//! the defensive `"unknown-reason"`; `admission_reason` maps the admission
//! declines to `"setup-incomplete"`, `"consent-stale"`,
//! `"not-in-allowlist"`, and `"evaluation-consumed"`. `"unknown-reason"` is
//! defensive only: [`ene_presentation::check_intake`] never emits its source
//! variant.
//!
//! Infallible-frame mapping used here (no `Result`: [`HostHandle::handle_frame`]
//! answers every frame):
//!
//! - Store failures before acceptance become
//!   [`HeldForTransition`](ene_api::v1::round::RoundIntakeOutcomeWire::HeldForTransition):
//!   no work started, so a later retry is safe.
//! - A reused command key with a different [`RequestFingerprint`] becomes the
//!   typed
//!   [`CommandReplayReject`](ene_api::v1::payload::WirePayload::CommandReplayReject)
//!   (`CommandIdConflict`), judged by the companion's fingerprint comparison
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

use ene_api::v1::command::CommandReplayRejectWire;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{CommandWireId, RoundWireId, StreamWireId};
use ene_api::v1::refs::{ConnectionWireId, RevalidationReasonWire};
use ene_api::v1::round::{
    ConfirmPresentationWire, HISTORY_LIMIT_MAX, HistoryItem, HistoryRequest, HistoryResponse,
    HistoryRole as HistoryRoleWire, PresentationStatus, RoundIntakeOutcomeWire, StreamClose,
    SubmitTextInput, TextStreamClose, TextStreamFrameWire, TextStreamOpen,
};
use ene_companion::dialogue::{
    AcceptedDialogueInput, DialogueBegin, DialogueOutcome, ReplayClassification,
    assemble_dialogue_input, begin_turn_committed, classify_replay, finish_turn, pin_experience,
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
    outgoing_fact, outgoing_frame, stale_reject, unpaired_close,
};

/// An absent command id yields [`None`] (no replay key); the caller declines
/// the submit with `missing-command-id`, because idempotency keys are
/// mandatory. Transport retry reuses the same command ID with a fresh message
/// ID within one sender incarnation; the store answers replays with the
/// original acceptance instead of re-appending.
fn command_id_for(envelope: &ene_api::v1::envelope::WireEnvelope) -> Option<CommandId> {
    let CommandWireId(id) = envelope.correlation.command_id?;
    Some(CommandId(ene_primitive::RawId::from_uuid(id)))
}

enum RoundPremise {
    Auto,
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

/// Builds the typed command-id-conflict reject for `command`: the reused id
/// travels structured, never inside an untyped detail string (IPC §24).
fn command_conflict_frame(frame: &WireFrame, live: &LiveInput, command: &CommandId) -> WireFrame {
    outgoing_frame(
        frame,
        live,
        WirePayload::CommandReplayReject(CommandReplayRejectWire::CommandIdConflict {
            command_id: CommandWireId(command.0.as_uuid()),
        }),
    )
}

fn admission_reason(reason: NotSentReason) -> &'static str {
    match reason {
        NotSentReason::SetupIncomplete => "setup-incomplete",
        NotSentReason::ConsentStale => "consent-stale",
        NotSentReason::NotInAllowlist => "not-in-allowlist",
        NotSentReason::EvaluationConsumed => "evaluation-consumed",
        NotSentReason::OverLimit => "unknown-reason",
        NotSentReason::TaskPremiseStale => "unknown-reason",
        NotSentReason::DataUseHeld => "unknown-reason",
        NotSentReason::UsageCapReached => "unknown-reason",
        NotSentReason::UsageCapIndeterminate => "unknown-reason",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachOutcome {
    Attached(PresenceAttribution),
    Raced,
    Superseded,
}

impl HostHandle {
    pub(crate) fn stale_frame(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        companion_key: &str,
        generation: u64,
    ) -> WireFrame {
        let current_round = self
            .open_round_for(&live.connection_id, companion_key)
            .and_then(|open| self.wire_for_round(&open.round))
            .map(RoundWireId);
        stale_frame_with(frame, live, current_round, generation)
    }

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
    /// replay, presence attach, presentation intake, dialogue prompt
    /// assembly, then the companion-owned turn
    /// (`ene_companion::dialogue::begin_turn_committed`/`finish_turn`) with its
    /// inference boundary. The assembled prompt's canonical read-set rides
    /// the admission as the attempt's `data_use`, so the claim gate and the
    /// deletion admission see the exact provenance the provider input was
    /// built from. Admission precedes the append so a declined input leaves
    /// neither history rows nor transient round claims behind; the round
    /// projection is minted atomically with its map entry (one domain round,
    /// one wire), and a racy duplicate that lands on
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
    /// (`CommandIdConflict`), never an intake outcome. Provider deltas may
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
        let Some(round_premise) =
            canonical_round_premise(submit, frame.envelope.observed.round_view.as_ref())
        else {
            return emit_end(
                sink,
                self.stale_frame(frame, live, &companion_key, attribution.generation.as_u64()),
            );
        };
        let scrubber = CredentialScrubber {
            refs: &self.store,
            store: &self.cred_store,
        };
        let Ok(scrubbed) = scrubber.scrub(&submit.body.text).await else {
            return emit_end(sink, held_frame(frame, live));
        };
        let credential_set = scrubbed.credential_set();
        let text = scrubbed.into_text();
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
        match classify_replay(&self.store, companion, &command, incoming_fingerprint).await {
            ReplayClassification::Replay { round_wire, .. } => {
                return emit_end(
                    sink,
                    self.replay_frame(frame, live, round_wire, attribution.generation.as_u64()),
                );
            }
            ReplayClassification::Conflict => {
                return emit_end(sink, command_conflict_frame(frame, live, &command));
            }
            ReplayClassification::Held => {
                return emit_end(sink, held_frame(frame, live));
            }
            ReplayClassification::None => {}
        }
        if !ene_companion::dialogue::dialogue_input_fits(&text) {
            return emit_end(
                sink,
                revalidate_frame(
                    frame,
                    live,
                    intake_reason(&RevalidationReason::InputOverLimit),
                ),
            );
        }
        let mut attached_generation: Option<PresenceGeneration> = None;
        if matches!(
            attribution.state,
            PresenceState::NoActive | PresenceState::RecoveryWait
        ) {
            let Some(viewed) = frame.envelope.observed.presence_generation_view else {
                return emit_end(
                    sink,
                    revalidate_frame(
                        frame,
                        live,
                        intake_reason(&RevalidationReason::MissingGenerationView),
                    ),
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
                    let fact = outgoing_fact(
                        frame,
                        live,
                        WirePayload::PresenceAttribution(attribution_to_wire(self, &attribution)),
                    );
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
                    match current.state {
                        PresenceState::InTransition | PresenceState::RecoveryWait => {
                            return emit_end(sink, held_frame(frame, live));
                        }
                        // A companion stopped under this submit's feet is a
                        // revalidation premise, not an expired round: the
                        // Client recovers by re-running the setup flow.
                        PresenceState::Stopped => {
                            return emit_end(
                                sink,
                                revalidate_frame(
                                    frame,
                                    live,
                                    intake_reason(&RevalidationReason::StoppedCompanion),
                                ),
                            );
                        }
                        _ => {}
                    };
                    return emit_end(
                        sink,
                        self.stale_frame(frame, live, &companion_key, current.generation.as_u64()),
                    );
                }
                AttachOutcome::Superseded => {
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
            RoundIntakeOutcome::StaleRound { current_round, .. } => {
                let current_round = current_round
                    .and_then(|round| self.wire_for_round(&round))
                    .map(RoundWireId);
                return emit_end(
                    sink,
                    stale_frame_with(frame, live, current_round, attribution.generation.as_u64()),
                );
            }
            RoundIntakeOutcome::HeldForTransition => {
                return emit_end(sink, held_frame(frame, live));
            }
            RoundIntakeOutcome::NeedsRevalidation { reason } => {
                return emit_end(sink, revalidate_frame(frame, live, intake_reason(&reason)));
            }
        };
        let round_wire = self.round_wire_or_mint(&accepted);
        let generation_number = attribution.generation.as_u64();
        let executor = HostInference {
            store: &self.store,
            cred_store: &self.cred_store,
            tracker: &self.tracker,
            transport,
        };
        let Ok(prompt) =
            assemble_dialogue_input(companion, &text, &self.store, &self.store, &scrubber).await
        else {
            return emit_end(sink, held_frame(frame, live));
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
        let authorized = match executor.admit_dialogue(prompt.data_use().to_vec()).await {
            Ok(Admission::Admitted(authorized)) => *authorized,
            Ok(Admission::Declined(reason)) => {
                return emit_end(
                    sink,
                    revalidate_frame(frame, live, admission_reason(reason)),
                );
            }
            Err(_) => return emit_end(sink, held_frame(frame, live)),
        };
        let inference_claim = authorized.ticket().0;
        // The consent premise the admission was granted under. The stream
        // baseline must be this admitted premise, not a fresh read taken after
        // the append: a consent move committing in that window would otherwise
        // become the baseline, presenting post-move deltas as current while
        // the reply append compares against the admitted premise and refuses.
        let admitted_consent = {
            let (id, rev) = authorized.consent_premise();
            (id.to_owned(), rev)
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
                    prompt,
                    authorized,
                    |owner| store.append_message_sync(owner),
                    |companion, command| store.lookup_command_sync(companion, command),
                )
            })
            .await;
        let begin = match committed {
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
                let fence_epoch = self.transient_fence.epoch();
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
                // committed under the admission consent, and any move or
                // unreadable read since means the stream can no longer be
                // proven to run under the admitted premise. Abort before
                // baselining on the changed value.
                let consent_current = match self.store.load_current(CapabilityKind::Dialogue).await
                {
                    Ok(Some(record)) => (record.id, record.rev.as_u64()) == admitted_consent,
                    _ => false,
                };
                if !consent_current {
                    if opened {
                        self.with_current_connection(live, || {
                            emit_end(
                                sink,
                                close_frame(frame, live, &stream, StreamClose::Interrupted),
                            );
                        });
                    }
                    return;
                }
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
                    consent: admitted_consent,
                    credential_set,
                    tx: stream_tx.clone(),
                    seq: 0,
                    opened,
                    fence_epoch,
                    inference_claim,
                };
                let task_control =
                    crate::task_control::HostTaskControl::new(self, companion, live.connection_id);
                let outcome = {
                    let is_current = || {
                        self.transient_fence.epoch() == fence_epoch
                            && matches!(
                                self.store.inference_claim_held_sync(inference_claim),
                                Ok(false)
                            )
                            && self
                                .open_round_for(&live.connection_id, &companion_key)
                                .is_none_or(|open| open.round == accepted)
                    };
                    finish_turn(
                        turn,
                        &self.store,
                        &executor,
                        &scrubber,
                        &task_control,
                        &mut gate,
                        &is_current,
                    )
                    .await
                };
                match outcome {
                    DialogueOutcome::Completed { input, .. } => {
                        {
                            let _pin = self.host_transient_arrival.acquire_pin().await;
                            if let Some(experience) = pin_experience(&input, &self.store).await {
                                #[cfg(any(test, feature = "test-support"))]
                                self.store
                                    .pause_learning_pin_queue_if_armed_for_tests()
                                    .await;
                                self.queue_learning_formation(experience).await;
                            }
                        }
                        gate.finish().await;
                    }
                    DialogueOutcome::Interrupted => {
                        gate.interrupt().await;
                    }
                }
            }
            DialogueBegin::Replayed { round_wire, .. } => {
                emit_end(
                    sink,
                    self.replay_frame(frame, live, round_wire, generation_number),
                );
            }
            DialogueBegin::StaleExpected { current } => {
                emit_end(sink, stale_frame_with(frame, live, None, current.as_u64()));
            }
            DialogueBegin::StaleConsent => {
                emit_end(
                    sink,
                    revalidate_frame(frame, live, admission_reason(NotSentReason::ConsentStale)),
                );
            }
            DialogueBegin::StaleCredentialSet => {
                emit_end(sink, held_frame(frame, live));
            }
            DialogueBegin::Conflict => {
                emit_end(sink, command_conflict_frame(frame, live, &command));
            }
            DialogueBegin::Held => emit_end(sink, held_frame(frame, live)),
            DialogueBegin::HeldForErasure => emit_end(sink, held_frame(frame, live)),
            DialogueBegin::HeldByLifecycle(_) => {
                emit_end(
                    sink,
                    revalidate_frame(
                        frame,
                        live,
                        intake_reason(&RevalidationReason::StoppedCompanion),
                    ),
                );
            }
        }
    }

    pub(crate) async fn confirm_presentation(
        &self,
        live: &LiveInput,
        confirm: &ConfirmPresentationWire,
    ) -> Vec<WireFrame> {
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
        let _gate = self.presentation_gate().await;
        let store = self.store.clone();
        let _applied = self
            .with_current_connection_blocking(live, move || {
                for entry in page.entries {
                    if entry.round == Some(mark.round) {
                        drop(store.compare_and_mark_reported_sync(entry.id, entry.status, mark));
                    }
                }
            })
            .await;
        Vec::new()
    }

    pub(crate) async fn answer_history(
        &self,
        frame: &WireFrame,
        request: &HistoryRequest,
        live: &LiveInput,
    ) -> Vec<WireFrame> {
        let coverage = self.current_coverage().await;
        let response = match self.read_history(request).await {
            HistoryResponse::Items(items) => {
                let had_body = items.iter().any(|item| !item.text.is_empty());
                let items: Vec<_> = items
                    .into_iter()
                    .filter(|item| !coverage.covers(&item.text))
                    .collect();
                let serves_body = items.iter().any(|item| !item.text.is_empty());
                if had_body && serves_body && !self.note_client_body_delivery(live).await {
                    HistoryResponse::Unavailable
                } else {
                    let fresh = self.current_coverage().await;
                    HistoryResponse::Items(
                        items
                            .into_iter()
                            .filter(|item| !fresh.covers(&item.text))
                            .collect(),
                    )
                }
            }
            other => other,
        };
        vec![outgoing_frame(
            frame,
            live,
            WirePayload::HistoryResponse(response),
        )]
    }

    async fn read_history(&self, request: &HistoryRequest) -> HistoryResponse {
        // An over-limit read is refused before any store work: the bound
        // rides the storage query, never a full scan truncated afterward.
        if request.limit > HISTORY_LIMIT_MAX {
            return HistoryResponse::InvalidRequest;
        }
        // The same companion mapping as submits: an unknown ref means the
        // Client's projection rotated, and it recovers by re-reading
        // presence, never by treating the timeline as empty.
        let companion = match self.resolve_companion(&request.companion.0).await {
            Err(_) => return HistoryResponse::Unavailable,
            Ok(None) => return HistoryResponse::StaleCompanion,
            Ok(Some(companion)) => companion,
        };
        let since = match request.since.as_deref() {
            None => None,
            Some(bound) => match WallClockWithTz::parse_rfc3339(bound) {
                Ok(parsed) => Some(parsed),
                Err(_) => return HistoryResponse::InvalidRequest,
            },
        };
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
            Ok(items) => {
                let mut mapped = Vec::with_capacity(items.len());
                for item in &items {
                    // The current writers always persist the wire; a row
                    // without one is unreadable, never a license to publish
                    // the domain id as a wire ref.
                    let Some(wire) = item.round_wire.clone() else {
                        return HistoryResponse::Unavailable;
                    };
                    mapped.push(HistoryItem {
                        round: RoundWireId(wire),
                        role: match item.role {
                            HistoryRole::Owner => HistoryRoleWire::Owner,
                            HistoryRole::Companion => HistoryRoleWire::Companion,
                        },
                        text: item.text.clone(),
                        at: item.at.to_rfc3339(),
                    });
                }
                HistoryResponse::Items(mapped)
            }
            Err(_) => HistoryResponse::Unavailable,
        }
    }

    /// The stored round wire travels verbatim, so a retry after a restart
    /// replays instead of going stale on the dropped transient map; a missing
    /// wire is stale, and the Client recovers missed items through history.
    fn replay_frame(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        stored_wire: Option<String>,
        generation: u64,
    ) -> WireFrame {
        match stored_wire {
            Some(wire) => accept_frame(frame, live, &RoundWireId(wire)),
            None => stale_frame_with(frame, live, None, generation),
        }
    }

    pub(crate) async fn queue_learning_formation(&self, experience: ExperienceCandidate) {
        let _gate = self.host_transient_arrival.lock().await;
        crate::lock_unpoison(&self.learning_queue).push_back(experience);
        self.host_transient_arrival.note_queued_arrival();
        #[cfg(any(test, feature = "test-support"))]
        self.store
            .pause_host_transient_arrival_publish_if_armed_for_tests()
            .await;
        crate::transient_erasure::publish_owed_learning_arrivals(
            &self.store,
            &self.host_transient_arrival,
            &self.learning_queue,
        )
        .await;
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
    /// activity. A pass failure drops its item, so there is no retry storm,
    /// and a pass the erasure gate refused is reported as held, never as an
    /// empty pass: its origin is old, so it is dropped rather than re-queued
    /// while the durable association holds it until erasure clears.
    ///
    /// Taking a candidate off the pending queue parks it in the worker-owned
    /// `taken` slot until a body-free formation identity is published. HostTransient
    /// can no longer drop that transcript as a queue entry; it also cannot
    /// report Verified while the slot still carries a covered body. After the
    /// identity commits, deletion correspondence outlives the slot, so a
    /// deletion that completes before the Learning claim still refuses the
    /// stale origin at the provider gate.
    pub(crate) async fn run_pending_learning<T: ProviderTransport>(&self, transport: &T) {
        let _serialized = self.learning_worker.lock().await;
        loop {
            let next = {
                let mut queue = crate::lock_unpoison(&self.learning_queue);
                queue.take_pending()
            };
            let Some(experience) = next else {
                break;
            };
            #[cfg(any(test, feature = "test-support"))]
            self.store.pause_learning_take_if_armed_for_tests().await;
            let formation = match self
                .store
                .begin_learning_formation(experience.companion, experience.sources.clone())
                .await
            {
                Ok(formation) => formation,
                Err(_) => {
                    crate::lock_unpoison(&self.learning_queue).clear_taken();
                    continue;
                }
            };
            crate::lock_unpoison(&self.learning_queue).clear_taken();
            #[cfg(any(test, feature = "test-support"))]
            self.store
                .pause_learning_formation_if_armed_for_tests()
                .await;
            let refuse = self
                .store
                .learning_formation_must_refuse(formation)
                .await
                .unwrap_or(true);
            if refuse {
                drop(self.store.settle_learning_formation(formation).await);
                continue;
            }
            let companion = CompanionId::from_raw(experience.companion);
            match self.store.load_lifecycle(companion).await {
                Ok(Some(CompanionLifecycle::Running)) => {}
                _ => {
                    drop(self.store.settle_learning_formation(formation).await);
                    continue;
                }
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
            // A HeldForErasure decision changes nothing here: the origin is
            // settled below and never re-queued or re-claimed under a fresh
            // identity, and the correspondence row keeps it held until erasure
            // clears it.
            drop(
                ene_companion::dialogue::propose_experience(
                    experience,
                    &self.store,
                    &executor,
                    &scrubber,
                )
                .await,
            );
            drop(self.store.settle_learning_formation(formation).await);
        }
    }
}

pub(crate) struct HostInference<'a, T> {
    store: &'a Store,
    cred_store: &'a CredStore,
    tracker: &'a AsyncMutex<EvaluationTracker>,
    transport: &'a T,
}

impl<'a, T: ProviderTransport + Send + Sync> HostInference<'a, T> {
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
    async fn admit_dialogue(
        &self,
        data_use: Vec<ene_primitive::RawId>,
    ) -> Result<Admission, InferenceTechnicalError> {
        self.admit(ene_inference::prepare_dialogue_admission(
            self.store,
            self.store,
            self.cred_store,
            data_use,
        ))
        .await
    }

    async fn admit_learning(
        &self,
        data_use: Vec<ene_primitive::RawId>,
    ) -> Result<Admission, InferenceTechnicalError> {
        self.admit(ene_inference::prepare_learning_admission(
            self.store,
            self.store,
            self.cred_store,
            data_use,
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

struct StreamGate<'a> {
    handle: &'a HostHandle,
    frame: &'a WireFrame,
    live: &'a LiveInput,
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
    opened: bool,
    fence_epoch: u64,
    inference_claim: RawId,
}

impl StreamGate<'_> {
    fn connection_current(&self) -> bool {
        self.live
            .authority
            .is_current_authenticated(&self.live.connection_id)
    }

    /// Re-reads the durable presentation premises only: the round,
    /// attribution, consent, lifecycle, credential set, erasure fence, and
    /// inference claim. A replaced connection does not fail here, so a
    /// refusal to publish on the wire stays distinct from a moved premise.
    async fn durable_current(&self) -> bool {
        // A Targeted Deletion invalidated Host transient payloads since this
        // stream opened: the remaining deltas can no longer prove they are
        // uncovered, so they fail closed instead of publishing.
        if self.handle.transient_fence_epoch() != self.fence_epoch {
            return false;
        }
        if !matches!(
            self.handle
                .store
                .inference_claim_held(self.inference_claim)
                .await,
            Ok(false)
        ) {
            return false;
        }
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

    async fn finish(&mut self) {
        if !self.connection_current() || !self.durable_current().await {
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
            // Fast path: read the premise before reserving, so an
            // already-stale stream never reserves capacity. The permit is
            // then deliberately held across the post-reserve re-checks
            // (dropped on any refusal) so the bounded channel paces the
            // provider while still refusing a stale delta.
            if !self.opened || !self.connection_current() {
                // CCT §9.3: a refused install or a replacement before
                // publication opens no wire stream and sends no close, but
                // the accepted turn's dispatch/adoption contract still runs.
                // Only a moved durable premise aborts.
                if !self.durable_current().await {
                    return DeltaFlow::Abort("the presentation premise went stale");
                }
                return DeltaFlow::Continue;
            }
            if !self.durable_current().await {
                return DeltaFlow::Abort("the presentation premise went stale");
            }
            let frame = self.delta_frame(delta, false);
            let permit = match self.tx.reserve().await {
                Ok(permit) => permit,
                Err(_) => return DeltaFlow::Abort("the client connection is gone"),
            };
            // Re-check after the capacity wait: the premise may have gone
            // stale while parked, and a stale delta must never publish. A
            // replacement during the wait only suppresses the wire copy.
            if !self.durable_current().await {
                drop(permit);
                return DeltaFlow::Abort("the presentation premise went stale");
            }
            if !self.connection_current() {
                drop(permit);
                return DeltaFlow::Continue;
            }
            // Write-ahead delivery evidence: the delta body may only leave the
            // Host after this incarnation's durable evidence row is committed,
            // so a crash between the send and the record cannot lose the copy
            // (lifecycle §8.1). A failed commit aborts the provider read
            // instead of creating an unaccountable copy; the turn is not
            // adopted, so no partial copy is published.
            if !self.handle.note_client_body_delivery(self.live).await {
                drop(permit);
                return DeltaFlow::Abort("the delivery evidence could not be committed");
            }
            // Final premise check after the durable write: the write awaited,
            // so a condition, fence, or connection that moved meanwhile must
            // still stop this delta before it is published. The evidence row,
            // when written, is conservative and re-derived by a later demand.
            if !self.durable_current().await {
                drop(permit);
                return DeltaFlow::Abort("the presentation premise went stale");
            }
            // Final connection currentness check, synchronous and after the
            // last await: a same-device replacement during the premise reads
            // must not let this stream publish one more delta (IPC §9.3).
            // The accepted turn still completes and is adopted, so it is
            // delivered through its presentation subscription instead.
            if !self.connection_current() {
                drop(permit);
                return DeltaFlow::Continue;
            }
            if self
                .handle
                .with_current_connection(self.live, || permit.send(frame))
                .is_none()
            {
                return DeltaFlow::Continue;
            }
            self.seq += 1;
            DeltaFlow::Continue
        })
    }
}

#[cfg(test)]
mod tests;
