//! One-to-one text round trip: intake, reply, stream, ack, timeline.
//!
//! `HostHandle::submit_text` runs the full accepted-input pipeline: idempotent
//! replay, intake evaluation, owner append, live authorization, inference
//! dispatch, reply append with undelivered registration, usage recording, and
//! the response stream. `HostHandle::confirm_presentation` applies
//! presentation observations, and `HostHandle::answer_history` restores the
//! filtered timeline.
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
use ene_api::v1::refs::{RoundWireId, StreamWireId};
use ene_api::v1::round::{
    ConfirmPresentationWire, HistoryItem, HistoryRequest, HistoryRole as HistoryRoleWire,
    HistoryView, PresentationStatus, RoundIntakeOutcomeWire, StreamClose, SubmitTextInput,
    TextStreamClose, TextStreamFrameWire, TextStreamOpen,
};
use ene_companion::{
    AppendHistoryCommand, CompanionLifecycle, CompanionRepository, HistoryAppendOutcome,
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
use ene_presence::{LiveReachabilityRef, PresenceGeneration, PresenceRepository};
use ene_presentation::{
    ClientInputRef, CompanionAvailability, IntakePremise, OpenRound, RevalidationReason,
    RoundIntakeOutcome, SubmitClientInputCandidate, check_intake,
};
use ene_primitive::{RawId, WallClockWithTz};

use crate::serve::{HostHandle, LiveInput, outgoing_frame};

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
fn accept_frame(frame: &WireFrame, round: &RoundWireId) -> WireFrame {
    outgoing_frame(
        frame,
        "RoundIntakeOutcome",
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound {
            round: round.clone(),
        }),
    )
}

/// Builds a stream opening for a round at a presence generation.
fn open_frame(
    frame: &WireFrame,
    stream: &StreamWireId,
    round: &RoundWireId,
    generation: u64,
) -> WireFrame {
    outgoing_frame(
        frame,
        "TextStreamOpen",
        WirePayload::TextStreamOpen(TextStreamOpen {
            stream: *stream,
            round: round.clone(),
            generation,
        }),
    )
}

/// Builds a stream close record.
fn close_frame(frame: &WireFrame, stream: &StreamWireId, status: StreamClose) -> WireFrame {
    outgoing_frame(
        frame,
        "TextStreamClose",
        WirePayload::TextStreamClose(TextStreamClose {
            stream: *stream,
            status,
        }),
    )
}

/// Builds the accept-plus-interrupted sequence for a post-accept failure.
fn interrupted_frames(frame: &WireFrame, round: &RoundWireId, generation: u64) -> Vec<WireFrame> {
    let stream = StreamWireId(RawId::new().as_uuid());
    vec![
        accept_frame(frame, round),
        open_frame(frame, &stream, round, generation),
        close_frame(frame, &stream, StreamClose::Interrupted),
    ]
}

/// Builds a stale-round outcome frame with explicit current values.
fn stale_frame_with(
    frame: &WireFrame,
    current_round: Option<RoundWireId>,
    current_generation: u64,
) -> WireFrame {
    outgoing_frame(
        frame,
        "RoundIntakeOutcome",
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound {
            current_round,
            current_generation,
        }),
    )
}

/// Builds a held-for-transition outcome frame.
fn held_frame(frame: &WireFrame) -> WireFrame {
    outgoing_frame(
        frame,
        "RoundIntakeOutcome",
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::HeldForTransition),
    )
}

/// Builds a needs-revalidation outcome frame with a fixed wire reason.
fn revalidate_frame(frame: &WireFrame, reason: &str) -> WireFrame {
    outgoing_frame(
        frame,
        "RoundIntakeOutcome",
        WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::NeedsRevalidation {
            reason: RevalidationReasonWire(reason.to_string()),
        }),
    )
}

impl HostHandle {
    /// Returns the open round wire ref for a client/companion pair, if any.
    pub(crate) fn open_wire_for(
        &self,
        client_ref: &str,
        companion_key: &str,
    ) -> Option<RoundWireId> {
        let open = self
            .open_rounds
            .get(&(client_ref.to_string(), companion_key.to_string()))?;
        self.rounds
            .iter()
            .find(|(_, round)| **round == open.round)
            .map(|(wire, _)| RoundWireId(wire.clone()))
    }

    /// Builds a stale-round outcome frame from the current attribution.
    pub(crate) fn stale_frame(
        &self,
        frame: &WireFrame,
        client_ref: &str,
        companion_key: &str,
        generation: u64,
    ) -> WireFrame {
        let current_round = self.open_wire_for(client_ref, companion_key);
        stale_frame_with(frame, current_round, generation)
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

    /// Runs the submit pipeline for one [`SubmitTextInput`] frame.
    ///
    /// Order: idempotent replay, intake evaluation, owner append, consent and
    /// credential load, live authorization, inference dispatch, reply append
    /// with undelivered registration, usage recording, then the response
    /// stream. The wire companion ref is echo-only: this Host serves a single
    /// companion resolved through
    /// [`ensure_running_companion`](CompanionRepository::ensure_running_companion),
    /// because no response in `Stage 2` ever issues a companion ref for the
    /// Client to echo back. The round premise prefers the envelope
    /// `round_view` (the comparison-material carrier) and falls back to the
    /// input `round`; a present-but-unresolvable round is stale, never
    /// rebound. A `local_id` replay returns the accept ack without
    /// re-executing anything (at-most-once effect per idempotency key;
    /// response recovery after a lost reply is via [`HostHandle::answer_history`]).
    /// Response text is never presented unless its reply append committed.
    pub(crate) async fn submit_text(
        &mut self,
        frame: &WireFrame,
        submit: &SubmitTextInput,
        live: &LiveInput,
        transport: &impl ProviderTransport,
    ) -> Vec<WireFrame> {
        let client = self.client_for(&live.client_ref);
        let Ok(companion) = self.store.ensure_running_companion().await else {
            return vec![held_frame(frame)];
        };
        let Ok(Some(attribution)) = self.store.load_attribution(companion.as_raw()).await else {
            return vec![held_frame(frame)];
        };
        let companion_key = companion.as_raw().as_uuid().to_string();
        let seen_key = (live.client_ref.clone(), submit.local_id.0.clone());
        if self.seen_local_ids.contains(&seen_key)
            && let Some(replay_round) = self.open_wire_for(&live.client_ref, &companion_key)
        {
            return vec![accept_frame(frame, &replay_round)];
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
                        &live.client_ref,
                        &companion_key,
                        attribution.generation.as_u64(),
                    )];
                }
            },
        };
        let Ok(lifecycle) = self.store.load_lifecycle(companion).await else {
            return vec![held_frame(frame)];
        };
        let premise = IntakePremise {
            candidate: SubmitClientInputCandidate {
                companion: companion.as_raw(),
                client,
                claimed_generation: frame
                    .envelope
                    .observed
                    .presence_generation_view
                    .map(PresenceGeneration::from_u64),
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
            open_round: self
                .open_rounds
                .get(&(live.client_ref.clone(), companion_key.clone()))
                .copied(),
        };
        let accepted = match check_intake(premise) {
            RoundIntakeOutcome::AcceptedForRound { round } => round,
            RoundIntakeOutcome::StaleRound { .. } => {
                return vec![self.stale_frame(
                    frame,
                    &live.client_ref,
                    &companion_key,
                    attribution.generation.as_u64(),
                )];
            }
            RoundIntakeOutcome::HeldForTransition => return vec![held_frame(frame)],
            RoundIntakeOutcome::NeedsRevalidation { reason } => {
                return vec![revalidate_frame(frame, intake_reason(&reason))];
            }
        };
        let round_wire = RoundWireId(accepted.as_raw().as_uuid().to_string());
        self.record_round(&round_wire.0, accepted);
        self.open_rounds.insert(
            (live.client_ref.clone(), companion_key.clone()),
            OpenRound {
                companion: companion.as_raw(),
                client,
                round: accepted,
                generation: attribution.generation,
            },
        );
        self.seen_local_ids.insert(seen_key);
        let generation_number = attribution.generation.as_u64();
        let owner_cmd = AppendHistoryCommand {
            companion,
            round: accepted.as_raw(),
            role: HistoryRole::Owner,
            text: submit.body.text.clone(),
            lang: submit.body.lang.0.clone(),
            at: WallClockWithTz::now(),
            expected_generation: attribution.generation,
        };
        match self.store.append_message(owner_cmd).await {
            Ok(HistoryAppendOutcome::CommittedAs { .. }) => {}
            Ok(HistoryAppendOutcome::StaleExpected { current }) => {
                return vec![stale_frame_with(frame, None, current.as_u64())];
            }
            Ok(HistoryAppendOutcome::HeldByLifecycle { .. }) => {
                return vec![revalidate_frame(frame, "stopped-companion")];
            }
            Err(_) => return vec![held_frame(frame)],
        }
        let Ok(stored_consent) = self.store.load_current().await else {
            return vec![held_frame(frame)];
        };
        let Some(consent) = stored_consent else {
            return vec![revalidate_frame(frame, "setup-incomplete")];
        };
        let Ok(known_refs) = self.store.list_refs().await else {
            return vec![held_frame(frame)];
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
        let authorization =
            match check_live_authorization(&query, Some(&consent), &mut self.tracker) {
                LiveAuthorizationDecision::AllowForThisUse(authorization) => authorization,
                LiveAuthorizationDecision::Deny(reason) => {
                    return vec![revalidate_frame(frame, deny_reason(reason.code))];
                }
                LiveAuthorizationDecision::NeedsRevalidation(_) => {
                    return vec![revalidate_frame(frame, "consent-stale")];
                }
            };
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
        let Ok((outcome, arrival)) = send(command, true, &mut self.tracker, transport).await else {
            self.record_unknown_usage(ticket, &consent.provider, &consent.model)
                .await;
            return interrupted_frames(frame, &round_wire, generation_number);
        };
        let InferenceUseOutcome::SentAndCompleted(_) = outcome else {
            self.record_unknown_usage(ticket, &consent.provider, &consent.model)
                .await;
            return interrupted_frames(frame, &round_wire, generation_number);
        };
        let Some(arrival) = arrival else {
            self.record_unknown_usage(ticket, &consent.provider, &consent.model)
                .await;
            return interrupted_frames(frame, &round_wire, generation_number);
        };
        let reply_cmd = AppendHistoryCommand {
            companion,
            round: accepted.as_raw(),
            role: HistoryRole::Companion,
            text: arrival.output_text.clone(),
            lang: submit.body.lang.0.clone(),
            at: WallClockWithTz::now(),
            expected_generation: attribution.generation,
        };
        match self
            .store
            .append_reply_with_undelivered(reply_cmd, true)
            .await
        {
            Ok((HistoryAppendOutcome::CommittedAs { .. }, _)) => {}
            Ok(_) | Err(_) => {
                return interrupted_frames(frame, &round_wire, generation_number);
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
            accept_frame(frame, &round_wire),
            open_frame(frame, &stream, &round_wire, generation_number),
        ];
        for (position, delta) in chunk_text(&arrival.output_text).iter().enumerate() {
            responses.push(outgoing_frame(
                frame,
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
        responses.push(close_frame(frame, &stream, StreamClose::Completed));
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
        &mut self,
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
        &mut self,
        frame: &WireFrame,
        request: &HistoryRequest,
    ) -> Vec<WireFrame> {
        let Ok(companion) = self.store.ensure_running_companion().await else {
            return vec![empty_history(frame)];
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
            "HistoryView",
            WirePayload::HistoryView(HistoryView { items: view_items }),
        )]
    }
}

/// Builds an empty timeline answer for the companion-ensure failure path.
fn empty_history(frame: &WireFrame) -> WireFrame {
    outgoing_frame(
        frame,
        "HistoryView",
        WirePayload::HistoryView(HistoryView { items: Vec::new() }),
    )
}

#[cfg(test)]
mod tests {
    use super::{CHUNK_CHARS, chunk_text};
    use crate::test_support::{live_input, memory_handle_with, remove_data_dir};
    use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
    use ene_api::v1::handshake::CapabilityAdvertise;
    use ene_api::v1::management::{
        IntentRationaleWire, ManagementIntent, ManagementIntentKind, ManagementOutcome,
        ManagementViewRequest, RationaleOrigin,
    };
    use ene_api::v1::payload::WirePayload;
    use ene_api::v1::refs::{
        BaseViewMark, ClientIncarnationId, ClientLocalId, CommandWireId, CompanionWireRef,
        ManagementTargetWire, RoundWireId, TextLangWire, WireMessageType,
    };
    use ene_api::v1::round::{
        ConfirmPresentationWire, PresentationStatus, RoundIntakeOutcomeWire, StreamClose,
        SubmitTextInput, TextBodyWire,
    };
    use ene_credential::CredentialRef;
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

    fn submit_frame(
        generation: Option<u64>,
        round: Option<RoundWireId>,
        local_id: &str,
        text: &str,
    ) -> ene_plugin_ipc::WireFrame {
        let mut envelope = new_outgoing_envelope(
            ProtocolVersion::V1,
            sender(),
            WireMessageType(String::from("SubmitTextInput")),
        );
        envelope.observed.presence_generation_view = generation;
        envelope.observed.round_view = round;
        ene_plugin_ipc::WireFrame {
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
        }
    }

    fn advertise_frame() -> ene_plugin_ipc::WireFrame {
        ene_plugin_ipc::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("CapabilityAdvertise")),
            ),
            payload: WirePayload::CapabilityAdvertise(CapabilityAdvertise {
                supported_protocol: vec![ProtocolVersion::V1],
                features: Vec::new(),
                platform: String::from("test"),
            }),
        }
    }

    fn intent_frame(
        kind: ManagementIntentKind,
        target: &str,
        base: &str,
    ) -> ene_plugin_ipc::WireFrame {
        ene_plugin_ipc::WireFrame {
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
        }
    }

    fn history_frame() -> ene_plugin_ipc::WireFrame {
        ene_plugin_ipc::WireFrame {
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
        }
    }

    fn confirm_frame(round: &RoundWireId) -> ene_plugin_ipc::WireFrame {
        ene_plugin_ipc::WireFrame {
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
        }
    }

    fn view_request_frame() -> ene_plugin_ipc::WireFrame {
        ene_plugin_ipc::WireFrame {
            envelope: new_outgoing_envelope(
                ProtocolVersion::V1,
                sender(),
                WireMessageType(String::from("ManagementViewRequest")),
            ),
            payload: WirePayload::ManagementViewRequest(ManagementViewRequest {
                sections: Vec::new(),
            }),
        }
    }

    fn ok_transport() -> FakeProviderTransport {
        FakeProviderTransport::new(String::from("hi there"), None)
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

    async fn setup_handle(tag: &str) -> Option<(crate::serve::HostHandle, std::path::PathBuf)> {
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

    #[tokio::test]
    async fn submit_without_setup_needs_revalidation() {
        let Some((mut handle, dir)) = memory_handle_with("dlg-nosetup", |_| {}).await else {
            return;
        };
        let transport = ok_transport();
        let advertised = handle
            .handle_frame(advertise_frame(), live_input("client-a"), &transport)
            .await;
        assert_eq!(advertised.len(), 1, "advertise answers once");
        let probe = handle
            .handle_frame(
                submit_frame(Some(0), None, "local-1", "hello"),
                live_input("client-a"),
                &transport,
            )
            .await;
        assert_eq!(probe.len(), 1, "a wrong generation answers once");
        let Some(first) = probe.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &first.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound { .. })
            ),
            "generation zero is stale after the attach"
        );
        let WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound {
            current_generation,
            ..
        }) = &first.payload
        else {
            remove_data_dir(&dir);
            return;
        };
        let generation = *current_generation;
        let denied = handle
            .handle_frame(
                submit_frame(Some(generation), None, "local-2", "hello"),
                live_input("client-a"),
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
            "missing consent denies as setup-incomplete"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn submit_for_an_unattached_client_is_stale() {
        let Some((mut handle, dir)) = setup_handle("dlg-stale").await else {
            return;
        };
        let transport = ok_transport();
        let responses = handle
            .handle_frame(
                submit_frame(Some(0), None, "local-1", "hello"),
                live_input("client-never-attached"),
                &transport,
            )
            .await;
        assert_eq!(responses.len(), 1, "stale answers once");
        let Some(first) = responses.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &first.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound { .. })
            ),
            "an inactive client is stale"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn full_dialogue_round_streams_and_restores() {
        let Some((mut handle, dir)) = setup_handle("dlg-full").await else {
            return;
        };
        let transport = ok_transport();
        let registered = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ConfigureCredentialIntent,
                    "credential:openai:main",
                    "consent-none",
                ),
                live_input("client-a"),
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
                ),
                live_input("client-a"),
                &transport,
            )
            .await;
        assert_eq!(assigned.len(), 1, "assign answers once");
        let completed = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "setup:complete",
                    "consent-rev-1",
                ),
                live_input("client-a"),
                &transport,
            )
            .await;
        assert_eq!(completed.len(), 1, "complete answers once");
        let Some(done) = completed.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &done.payload,
                WirePayload::ManagementOutcome(ManagementOutcome::AppliedAsOneTime)
            ),
            "a complete premise applies"
        );
        let shown = handle
            .handle_frame(view_request_frame(), live_input("client-a"), &transport)
            .await;
        assert_eq!(shown.len(), 1, "a view request answers once");
        let advertised = handle
            .handle_frame(advertise_frame(), live_input("client-a"), &transport)
            .await;
        assert_eq!(advertised.len(), 1, "advertise answers once");
        let probe = handle
            .handle_frame(
                submit_frame(Some(0), None, "local-1", "hello"),
                live_input("client-a"),
                &transport,
            )
            .await;
        let Some(stale) = probe.first() else {
            remove_data_dir(&dir);
            return;
        };
        let WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound {
            current_generation,
            ..
        }) = &stale.payload
        else {
            remove_data_dir(&dir);
            return;
        };
        let generation = *current_generation;
        let frame = submit_frame(Some(generation), None, "local-1", "hello");
        let responses = handle
            .handle_frame(frame.clone(), live_input("client-a"), &transport)
            .await;
        assert_eq!(responses.len(), 4, "accept streams open, one frame, close");
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
        let confirmed = handle
            .handle_frame(confirm_frame(&round), live_input("client-a"), &transport)
            .await;
        assert!(
            confirmed.is_empty(),
            "a presentation observation answers nothing"
        );
        let restored = handle
            .handle_frame(history_frame(), live_input("client-a"), &transport)
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
        let replayed = handle
            .handle_frame(frame, live_input("client-a"), &transport)
            .await;
        assert_eq!(replayed.len(), 1, "a local id replay answers once");
        let Some(replay) = replayed.first() else {
            remove_data_dir(&dir);
            return;
        };
        assert!(
            matches!(
                &replay.payload,
                WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
            ),
            "the replay re-acks without re-executing"
        );
        remove_data_dir(&dir);
    }

    #[tokio::test]
    async fn provider_failure_interrupts_after_accept() {
        let Some((mut handle, dir)) = setup_handle("dlg-fail").await else {
            return;
        };
        let transport = ok_transport();
        let _ = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ConfigureCredentialIntent,
                    "credential:openai:main",
                    "consent-none",
                ),
                live_input("client-a"),
                &transport,
            )
            .await;
        let _ = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ManageRuleConsentCap,
                    "consent:openai:dialogue-1:openai:main",
                    "consent-none",
                ),
                live_input("client-a"),
                &transport,
            )
            .await;
        let _ = handle
            .handle_frame(advertise_frame(), live_input("client-a"), &transport)
            .await;
        let probe = handle
            .handle_frame(
                submit_frame(Some(0), None, "local-1", "probe"),
                live_input("client-a"),
                &transport,
            )
            .await;
        let Some(stale) = probe.first() else {
            remove_data_dir(&dir);
            return;
        };
        let WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound {
            current_generation,
            ..
        }) = &stale.payload
        else {
            remove_data_dir(&dir);
            return;
        };
        let generation = *current_generation;
        let failing = FakeProviderTransport::failing(FakeFailure::Transport(String::from("down")));
        let responses = handle
            .handle_frame(
                submit_frame(Some(generation), None, "local-9", "hello"),
                live_input("client-a"),
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
    async fn setup_edge_cases_clarify_or_hold() {
        let Some((mut handle, dir)) = memory_handle_with("dlg-edge", |_| {}).await else {
            return;
        };
        let transport = ok_transport();
        let malformed = handle
            .handle_frame(
                intent_frame(
                    ManagementIntentKind::ConfigureCredentialIntent,
                    "credential:lonely",
                    "consent-none",
                ),
                live_input("client-a"),
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
                ),
                live_input("client-a"),
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
                ),
                live_input("client-a"),
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
                ),
                live_input("client-a"),
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
