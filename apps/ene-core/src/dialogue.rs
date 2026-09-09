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
//!   [`Interrupted`](ene_api::v1::round::StreamClose::Interrupted). Usage is
//!   still recorded with [`Unknown`](ene_inference::UsageSource::Unknown)
//!   counts on the inference-failure paths. A usage-record failure after a
//!   durable reply keeps the `Completed` close: the reply happened, and the
//!   usage gap is the documented `Stage 2` follow-up (retry queue), not a
//!   reason to misreport the stream.
//! - Presentation observations and unresolvable confirmation rounds produce no
//!   reply: confirmation is an observation, never a report of completion.

use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::RevalidationReasonWire;
use ene_api::v1::refs::{CommandWireId, RoundWireId, StreamWireId};
use ene_api::v1::round::{
    ConfirmPresentationWire, HistoryItem, HistoryRequest, HistoryRole as HistoryRoleWire,
    HistoryView, PresentationStatus, RoundIntakeOutcomeWire, StreamClose, SubmitTextInput,
    TextStreamClose, TextStreamFrameWire, TextStreamOpen,
};
use ene_companion::{
    AppendHistoryCommand, CommandId, CompanionLifecycle, CompanionRepository, HistoryAppendOutcome,
    HistoryRepository, HistoryRole, PresentationMark, ReportStatus, UndeliveredRepository,
};
use ene_credential::{CredentialRef, CredentialRefRepository, credential_availability};
use ene_inference::{
    InferenceTicketId, InferenceUseOutcome, ProviderTransport, RequestInferenceCommand,
    ResolvedRoute, UsageSource, send,
};
use ene_permission::{
    CapabilityKind, CheckLiveAuthorizationQuery, ConsentRepository, ConsumerKind, DenyCode,
    InferenceUseCandidate, LiveAuthorizationDecision, PurposeKind, check_live_authorization,
};
use ene_plugin_ipc::WireFrame;
use ene_presence::{
    ClientId, LiveReachabilityRef, MoveDecision, PresenceAttribution, PresenceCheckRef,
    PresenceGeneration, PresenceRepository, PresenceState, ThinMoveReason,
};
use ene_presentation::{
    ClientInputRef, CompanionAvailability, IntakePremise, OpenRound, RevalidationReason, RoundId,
    RoundIntakeOutcome, SubmitClientInputCandidate, check_intake,
};
use ene_primitive::{RawId, WallClockWithTz};
use ene_store::Store;

use crate::serve::{HostHandle, LiveInput, device_client, outgoing_frame, unpaired_close};

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
/// command ID with a fresh message ID; the store answers replays with the
/// original acceptance instead of re-appending.
fn command_id_for(envelope: &ene_api::v1::envelope::WireEnvelope) -> Option<CommandId> {
    let CommandWireId(id) = envelope.correlation.command_id?;
    Some(CommandId(ene_primitive::RawId::from_uuid(id)))
}

/// Maps a closed-world denial gate to the `Stage 2` wire reason vocabulary.
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

    /// Records an unknown-usage fact for a ticket whose inference never completed.
    ///
    /// Best-effort post-accept bookkeeping: when the store itself rejects the
    /// record, the stream close already returned stays authoritative and the
    /// usage gap becomes the documented `Stage 2` follow-up.
    pub(crate) async fn record_unknown_usage(
        &self,
        ticket: InferenceTicketId,
        provider: &str,
        model: &str,
    ) {
        use ene_inference::UsageFact;
        use ene_inference::UsageRepository;
        let fact = UsageFact {
            ticket,
            provider: provider.to_string(),
            model: model.to_string(),
            input_tokens: None,
            output_tokens: None,
            source: UsageSource::Unknown,
        };
        if self.store.record_usage(fact).await.is_err() {
            // The stream close is authoritative; usage persistence retries
            // belong to later milestone work, not to this frame.
        }
    }

    /// Attaches presence for the paired device when none is active.
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

    /// Runs the submit pipeline for one [`SubmitTextInput`] frame.
    ///
    /// Order: durable idempotent replay, presence attach, intake evaluation,
    /// setup/consent/credential admission (live authorization included),
    /// owner append, transient round recording, inference dispatch, reply
    /// append with undelivered registration, usage recording, then the
    /// response stream. Admission precedes the append so a declined input
    /// leaves neither history rows nor transient round claims behind; maps
    /// are recorded only after the append commits, and a racy duplicate
    /// that lands on [`HistoryAppendOutcome::AlreadyCommittedAs`] answers
    /// the original accept without re-running inference. The wire companion
    /// ref is echo-only: this Host serves a single companion resolved
    /// through [`ensure_running_companion`](CompanionRepository::ensure_running_companion),
    /// because no response in `Stage 2` ever issues a companion ref for the
    /// Client to echo back. The round premise prefers the envelope
    /// `round_view` (the comparison-material carrier) and falls back to the
    /// input `round`; a present-but-unresolvable round is stale, never
    /// rebound.
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
    /// [`lookup_command`](HistoryRepository::lookup_command), and a hit
    /// returns the original accept ack without re-appending or re-streaming
    /// anything. The found round maps back to its wire ref through the
    /// per-process rounds map; when the map no longer knows it (notably after
    /// a restart) the intake answers stale instead of guessing, and the
    /// Client recovers missed stream items through
    /// [`HostHandle::answer_history`]. Stream outcome replay is explicitly out
    /// of scope: only the accept ack replays. An unparsable or missing
    /// command id carries no replay key: it skips the lookup and appends with
    /// `command_id` [`None`], while `local_id` is still stored as metadata.
    /// Response text is never presented unless its reply append committed.
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
        let Ok(companion) = self.store.ensure_running_companion().await else {
            return vec![held_frame(frame, live)];
        };
        let Ok(Some(mut attribution)) = self.store.load_attribution(companion.as_raw()).await
        else {
            return vec![held_frame(frame, live)];
        };
        let companion_key = companion.as_raw().as_uuid().to_string();
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
        if let Some(command) = command_id_for(&frame.envelope) {
            match self.store.lookup_command(companion, &command).await {
                Err(_) => return vec![held_frame(frame, live)],
                Ok(Some(found)) => {
                    let Some(wire) = self.wire_for_round_value(found.round) else {
                        return vec![stale_frame_with(
                            frame,
                            live,
                            None,
                            attribution.generation.as_u64(),
                        )];
                    };
                    return vec![accept_frame(frame, live, &RoundWireId(wire))];
                }
                Ok(None) => {}
            }
        }
        let round_hint = frame
            .envelope
            .observed
            .round_view
            .clone()
            .or_else(|| submit.round.clone());
        let requested = match round_hint {
            None => None,
            Some(hint) => match self.round_for(&hint.0) {
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
                round: requested,
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
        let round_wire = RoundWireId(accepted.as_raw().as_uuid().to_string());
        self.record_round(&round_wire.0, accepted);
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
        let generation_number = attribution.generation.as_u64();
        let owner_cmd = AppendHistoryCommand {
            companion,
            round: accepted.as_raw(),
            role: HistoryRole::Owner,
            text: submit.body.text.clone(),
            lang: submit.body.lang.0.clone(),
            at: WallClockWithTz::now(),
            expected_generation: attribution.generation,
            local_id: Some(submit.local_id.0.clone()).filter(|key| !key.is_empty()),
            command_id: command_id_for(&frame.envelope),
        };
        match self.store.append_message(owner_cmd).await {
            Ok(HistoryAppendOutcome::CommittedAs { .. }) => {}
            Ok(HistoryAppendOutcome::AlreadyCommittedAs { round: prior, .. }) => {
                let Some(wire) = self.wire_for_round_value(prior) else {
                    return vec![stale_frame_with(frame, live, None, generation_number)];
                };
                return vec![accept_frame(frame, live, &RoundWireId(wire))];
            }
            Ok(HistoryAppendOutcome::StaleExpected { current }) => {
                return vec![stale_frame_with(frame, live, None, current.as_u64())];
            }
            Ok(HistoryAppendOutcome::HeldByLifecycle { .. }) => {
                return vec![revalidate_frame(frame, live, "stopped-companion")];
            }
            Err(_) => return vec![held_frame(frame, live)],
        }
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
        let send_outcome = send(command, true, transport).await;
        let Ok((outcome, arrival)) = send_outcome else {
            self.record_unknown_usage(ticket, &consent.provider, &consent.model)
                .await;
            return interrupted_frames(frame, live, &round_wire, generation_number);
        };
        let InferenceUseOutcome::SentAndCompleted(_) = outcome else {
            self.record_unknown_usage(ticket, &consent.provider, &consent.model)
                .await;
            return interrupted_frames(frame, live, &round_wire, generation_number);
        };
        let Some(arrival) = arrival else {
            self.record_unknown_usage(ticket, &consent.provider, &consent.model)
                .await;
            return interrupted_frames(frame, live, &round_wire, generation_number);
        };
        let reply_cmd = AppendHistoryCommand {
            companion,
            round: accepted.as_raw(),
            role: HistoryRole::Companion,
            text: arrival.output_text.clone(),
            lang: submit.body.lang.0.clone(),
            at: WallClockWithTz::now(),
            expected_generation: attribution.generation,
            local_id: None,
            command_id: None,
        };
        match self
            .store
            .append_reply_with_undelivered(reply_cmd, true)
            .await
        {
            Ok((HistoryAppendOutcome::CommittedAs { .. }, _)) => {}
            Ok(_) | Err(_) => {
                return interrupted_frames(frame, live, &round_wire, generation_number);
            }
        }
        {
            use ene_inference::UsageRepository;
            if self.store.record_usage(arrival.usage).await.is_err() {
                // The reply is durable and will stream; the usage gap is the
                // documented follow-up, never a reason to misreport completion.
            }
        }
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
        let Ok(companion) = self.store.ensure_running_companion().await else {
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
                round: RoundWireId(item.round.as_uuid().to_string()),
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
        Ok(confirmed) => AttachOutcome::Attached(confirmed),
        Err(_) => AttachOutcome::Raced,
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
    use ene_api::v1::round::{
        ConfirmPresentationWire, PresentationStatus, RoundIntakeOutcomeWire, StreamClose,
        SubmitTextInput, TextBodyWire,
    };
    use ene_credential::{CredentialRef, MemoryCredentialStore};
    use ene_inference::fake::{FakeFailure, FakeProviderTransport};
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
        envelope.observed.round_view = round;
        // Every submit mints a fresh command id: transport retry reuses the
        // id (the replay test resends the same frame), while distinct sends
        // stay distinct durable commands.
        envelope.correlation.command_id = Some(CommandWireId(RawId::new().as_uuid()));
        let frame = ene_plugin_ipc::WireFrame {
            envelope,
            payload: WirePayload::SubmitTextInput(SubmitTextInput {
                companion: CompanionWireRef(String::from("companion-echo")),
                round: None,
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
        let frame = ene_plugin_ipc::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("ManagementIntent")),
            ),
            payload: WirePayload::ManagementIntent(ManagementIntent {
                intent_id: CommandWireId(RawId::new().as_uuid()),
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

    fn history_frame(connection: ConnectionWireId) -> ene_plugin_ipc::WireFrame {
        let frame = ene_plugin_ipc::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("HistoryRequest")),
            ),
            payload: WirePayload::HistoryRequest(ene_api::v1::round::HistoryRequest {
                companion: CompanionWireRef(String::from("companion-echo")),
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
                submit_frame(Some(0), None, "local-1", "hello", live.connection_id),
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
                submit_frame(None, None, "local-1", "hello", live.connection_id),
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
                submit_frame(Some(7), None, "local-1", "hello", live.connection_id),
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
        let frame = submit_frame(Some(0), None, "local-1", "hello", live.connection_id);
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
            .handle_frame(history_frame(live.connection_id), live.clone(), &transport)
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
            .handle_frame(history_frame(live.connection_id), live.clone(), &transport)
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

    #[tokio::test]
    async fn replay_after_restart_is_stale_without_remap() {
        let Some((handle, dir)) = setup_handle("dlg-restart").await else {
            return;
        };
        let transport = ok_transport();
        let live = live_input("client-a");
        assert!(
            register_assign_complete(&handle, &live, &transport).await,
            "setup must complete"
        );
        let frame = submit_frame(Some(0), None, "local-9", "hello", live.connection_id);
        let accepted = handle
            .handle_frame(frame.clone(), live.clone(), &transport)
            .await;
        assert!(
            accepted.first().is_some_and(|first| matches!(
                &first.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
            )),
            "the first send attaches and accepts, got {accepted:?}"
        );
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
        let reopened = HostHandle::open_with_cred_store(&dir, CredStore::Memory(fresh)).await;
        assert!(
            reopened.is_ok(),
            "the store must reopen over the same data directory"
        );
        let Ok(reopened) = reopened else {
            remove_data_dir(&dir);
            return;
        };
        let relive = live_input("client-a");
        let mut resent = frame;
        resent.envelope.sender.connection_id = Some(relive.connection_id);
        let replayed = reopened
            .handle_frame(resent, relive.clone(), &transport)
            .await;
        assert_eq!(replayed.len(), 1, "the restart replay answers once");
        let Some(replay) = replayed.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &replay.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound {
                    current_round: None,
                    current_generation: 1,
                })
            ),
            "an unmapped durable round replays as stale with the durable generation, got {:?}",
            replay.payload
        );
        let restored = reopened
            .handle_frame(
                history_frame(relive.connection_id),
                relive.clone(),
                &transport,
            )
            .await;
        let Some(view) = restored.first() else {
            remove_data_dir(&dir);
            return;
        };
        let WirePayload::HistoryView(view) = &view.payload else {
            remove_data_dir(&dir);
            return;
        };
        assert_eq!(
            view.items.len(),
            2,
            "the restart replay appends nothing durable"
        );
        remove_data_dir(&dir);
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
                submit_frame(Some(0), None, "local-1", "hello", live.connection_id),
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
                submit_frame(Some(0), None, "local-1", "probe", live.connection_id),
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
                submit_frame(Some(1), None, "local-9", "hello", live.connection_id),
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
    async fn consent_replayed_base_reports_staleness() {
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
        let Some(stale) = replayed.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &stale.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::StaleBaseView { current })
                if current.0 == "consent-rev-1"
            ),
            "the replayed base reports the rebuilt current mark"
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
