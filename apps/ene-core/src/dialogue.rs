use ene_api::codec::WireFrame;
use ene_api::v1::command::CommandReplayRejectWire;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{CommandWireId, RoundWireId, StreamWireId};
use ene_api::v1::refs::{ConnectionWireId, RevalidationReasonWire};
use ene_api::v1::round::{
    ConfirmPresentationWire, HISTORY_LIMIT_MAX, HistoryItem, HistoryRequest, HistoryResponse,
    HistoryRole as HistoryRoleWire, PresentationStatus, RoundIntakeOutcomeWire, RoundTarget,
    StreamClose, SubmitTextInput, TextStreamClose, TextStreamFrameWire, TextStreamOpen,
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
    CredStore, HostHandle, LiveInput, attribution_to_wire, device_client, emit_control, emit_end,
    outgoing_fact, outgoing_frame, stale_reject, unpaired_close,
};

fn command_id_for(envelope: &ene_api::v1::envelope::WireEnvelope) -> Option<CommandId> {
    let CommandWireId(id) = envelope.correlation.command_id?;
    Some(CommandId(ene_primitive::RawId::from_uuid(id)))
}

fn canonical_round_intent(
    submit: &SubmitTextInput,
    round_view: Option<&RoundWireId>,
) -> Option<RoundIntentMark> {
    match &submit.target {
        RoundTarget::New => round_view.is_none().then_some(RoundIntentMark::New),
        RoundTarget::Existing(round) => {
            (round_view == Some(round)).then_some(RoundIntentMark::Existing(round.0.clone()))
        }
    }
}

fn intake_reason(reason: &RevalidationReason) -> &'static str {
    match reason {
        RevalidationReason::MissingGenerationView => "missing-generation-view",
        RevalidationReason::UnknownCompanion => "unknown-companion",
        RevalidationReason::StoppedCompanion => "stopped-companion",
        RevalidationReason::MissingCommandId => "missing-command-id",
        RevalidationReason::InputOverLimit => "input-over-limit",
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
        NotSentReason::CredentialRotated => "unknown-reason",
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
        companion: CompanionId,
    ) -> AttachOutcome {
        let client = device_client(device_wire);
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

    pub(crate) async fn submit_text(
        &self,
        frame: &WireFrame,
        submit: &SubmitTextInput,
        live: &LiveInput,
        transport: &impl ProviderTransport,
        sink: &tokio::sync::mpsc::Sender<WireFrame>,
        abort: &ene_inference::DispatchAbort,
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
        let Some(round_intent) =
            canonical_round_intent(submit, frame.envelope.observed.round_view.as_ref())
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
                    companion,
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
                        self.with_current_connection(live, || emit_control(sink, fact)),
                        Some(Ok(()))
                    ) {
                        for summary in self
                            .auto_present_for(frame, live, companion, &attribution)
                            .await
                        {
                            if !matches!(
                                self.with_current_connection(live, || emit_control(sink, summary)),
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
        let requested = match &round_intent {
            RoundIntentMark::Existing(reference) => match self.round_for(reference) {
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
            RoundIntentMark::Auto | RoundIntentMark::New => None,
        };
        let intent = match (&round_intent, requested) {
            (RoundIntentMark::New, _) => RoundIntent::New,
            (RoundIntentMark::Existing(_), Some(round)) => RoundIntent::Existing(round),
            _ => RoundIntent::Auto,
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
        let admitted_consent = {
            let (id, rev) = authorized.consent_premise();
            (id.to_owned(), rev)
        };
        let store = self.store.clone();
        let committed = self
            .with_current_connection_blocking(live, move || {
                begin_turn_committed(
                    input,
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
                #[cfg(any(test, feature = "test-support"))]
                {
                    let publish_gate = crate::lock_unpoison(&self.submit_publish_gate).clone();
                    if let Some(gate) = publish_gate {
                        gate.pause().await;
                    }
                }
                let stream = StreamWireId(RawId::new().as_uuid());
                let fence_epoch = self.transient_fence.epoch();
                let opened = if installed {
                    match self.with_current_connection(live, || {
                        emit_control(sink, accept_frame(frame, live, &round_wire))?;
                        emit_control(
                            sink,
                            open_frame(frame, live, &stream, &round_wire, generation_number),
                        )
                    }) {
                        Some(Ok(())) => true,
                        Some(Err(_)) => return,
                        None => false,
                    }
                } else {
                    false
                };
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
                    tx: sink.clone(),
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
                        Some(abort),
                    )
                    .await
                };
                match outcome {
                    DialogueOutcome::Completed { input, .. } => {
                        {
                            let _pin = self.host_transient_arrival.acquire_pin().await;
                            if let Some(experience) = pin_experience(&input, &self.store).await {
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
        let response = match self.read_history(request).await {
            HistoryResponse::Items(items) => {
                let coverage = self.current_coverage().await;
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
        if request.limit > HISTORY_LIMIT_MAX {
            return HistoryResponse::InvalidRequest;
        }
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
        crate::transient_erasure::publish_owed_learning_arrivals(
            &self.store,
            &self.host_transient_arrival,
            &self.learning_queue,
        )
        .await;
    }

    pub(crate) fn has_pending_learning(&self) -> bool {
        !crate::lock_unpoison(&self.learning_queue).is_empty()
    }

    pub(crate) async fn run_pending_learning<T: ProviderTransport>(
        &self,
        transport: &T,
        abort: &ene_inference::DispatchAbort,
        admission_stop: &mut tokio::sync::watch::Receiver<bool>,
    ) {
        let _serialized = self.learning_worker.lock().await;
        loop {
            if abort.is_aborted() || *admission_stop.borrow() {
                // Host shutdown stops new learning admission; the queued work
                // stays unclaimed instead of racing the stop.
                break;
            }
            let next = {
                let mut queue = crate::lock_unpoison(&self.learning_queue);
                queue.take_pending()
            };
            let Some(experience) = next else {
                break;
            };
            if abort.is_aborted() || *admission_stop.borrow() {
                crate::lock_unpoison(&self.learning_queue).clear_taken();
                break;
            }
            if abort.is_aborted() || *admission_stop.borrow() {
                crate::lock_unpoison(&self.learning_queue).clear_taken();
                break;
            }
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
            drop(
                ene_companion::dialogue::propose_experience(
                    experience,
                    &self.store,
                    &executor,
                    &scrubber,
                    Some(abort),
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
            self.cred_store,
            self.store,
            self.store,
            self.store,
            self.transport,
        )
        .await
    }

    async fn dispatch_with_claim_scope(
        &self,
        authorized: AuthorizedInference,
        prompt: ScrubbedText,
        sink: &mut (dyn DeltaSink + Send),
        abort: Option<&ene_inference::DispatchAbort>,
        acquire_claim_scope: Box<dyn FnOnce() -> ene_inference::InferenceClaimFuture + Send>,
    ) -> Result<InferenceDispatchOutcome, InferenceTechnicalError> {
        ene_inference::dispatch_authorized_with_claim_scope(
            authorized,
            prompt,
            sink,
            abort,
            acquire_claim_scope,
            self.cred_store,
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

    async fn durable_current(&self) -> bool {
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
            if !self.opened || !self.connection_current() {
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
            if !self.durable_current().await {
                drop(permit);
                return DeltaFlow::Abort("the presentation premise went stale");
            }
            if !self.connection_current() {
                drop(permit);
                return DeltaFlow::Continue;
            }
            if !self.handle.note_client_body_delivery(self.live).await {
                drop(permit);
                return DeltaFlow::Abort("the delivery evidence could not be committed");
            }
            if !self.durable_current().await {
                drop(permit);
                return DeltaFlow::Abort("the presentation premise went stale");
            }
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
mod tests {
    use ene_api::v1::refs::{ClientLocalId, CompanionWireRef, RoundWireId, TextLangWire};
    use ene_api::v1::round::{RoundTarget, SubmitTextInput, TextBodyWire};
    use ene_companion::RoundIntentMark;

    use super::canonical_round_intent;

    fn submit(target: RoundTarget) -> SubmitTextInput {
        SubmitTextInput {
            companion: CompanionWireRef(String::from("companion-1")),
            target,
            local_id: ClientLocalId(String::from("local-1")),
            body: TextBodyWire {
                text: String::from("hello"),
                lang: TextLangWire(String::from("en")),
            },
        }
    }

    fn round(name: &str) -> RoundWireId {
        RoundWireId(String::from(name))
    }

    #[test]
    fn round_intake_resolves_only_target_and_observed_views_that_agree() {
        let cases = [
            (
                "New without an observed round",
                RoundTarget::New,
                None,
                Some(RoundIntentMark::New),
            ),
            (
                "Existing with its observed round",
                RoundTarget::Existing(round("r1")),
                Some(round("r1")),
                Some(RoundIntentMark::Existing(String::from("r1"))),
            ),
            (
                "New with a contradicted observed round",
                RoundTarget::New,
                Some(round("r1")),
                None,
            ),
            (
                "Existing without an observed round",
                RoundTarget::Existing(round("r1")),
                None,
                None,
            ),
            (
                "Existing with a different observed round",
                RoundTarget::Existing(round("r1")),
                Some(round("r2")),
                None,
            ),
        ];

        for (case, target, observed, expected) in cases {
            assert_eq!(
                canonical_round_intent(&submit(target), observed.as_ref()),
                expected,
                "{case}"
            );
        }
    }
}
