use std::collections::VecDeque;

use ene_api::codec::WireFrame;
use ene_api::v1::deletion::{ClientTempClass, DeletionDemand};
use ene_api::v1::handshake::AuthResult;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::presence::PresenceAttributionWire;
use ene_api::v1::refs::{ConnectionWireId, RoundWireId, WireMessageId};
use ene_api::v1::round::RoundIntakeOutcomeWire;

use super::frames::auth_rejected_guidance;

pub const DEFERRED_CAP: usize = 32;

pub const PENDING_ERASURE_CAP: usize = 32;

#[derive(Default)]
pub struct SessionState {
    generation: Option<u64>,
    companion: Option<String>,
    open_round: Option<RoundWireId>,
    deferred: VecDeque<WireFrame>,
    defer_erasure: bool,
    pending_erasure: VecDeque<DeletionDemand>,
}

impl core::fmt::Debug for SessionState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SessionState")
            .field("generation", &self.generation)
            .field("companion", &self.companion)
            .field("open_round", &self.open_round)
            .field("deferred_len", &self.deferred.len())
            .field("defer_erasure", &self.defer_erasure)
            .field("pending_erasure_len", &self.pending_erasure.len())
            .finish()
    }
}

impl SessionState {
    pub fn generation(&self) -> Option<u64> {
        self.generation
    }

    pub fn observe_presence(&mut self, fact: &PresenceAttributionWire) {
        self.generation = Some(fact.generation);
        self.companion = Some(fact.companion.0.clone());
    }

    pub fn companion_ref(&self) -> String {
        self.companion
            .clone()
            .unwrap_or_else(|| String::from(crate::DEFAULT_COMPANION_REF))
    }

    pub fn note_stale_generation(&mut self, current: u64) {
        self.generation = Some(current);
    }

    pub fn open_round(&self) -> Option<&RoundWireId> {
        self.open_round.as_ref()
    }

    pub fn observe_intake(&mut self, payload: &WirePayload) {
        let WirePayload::RoundIntakeOutcome(outcome) = payload else {
            return;
        };
        match outcome {
            RoundIntakeOutcomeWire::AcceptedForRound { round } => {
                self.open_round = Some(round.clone());
            }
            RoundIntakeOutcomeWire::StaleRound { .. } => self.open_round = None,
            RoundIntakeOutcomeWire::HeldForTransition
            | RoundIntakeOutcomeWire::NeedsRevalidation { .. } => {}
        }
    }

    #[must_use]
    pub fn round_target(&self) -> ene_api::v1::round::RoundTarget {
        match &self.open_round {
            Some(round) => ene_api::v1::round::RoundTarget::Existing(round.clone()),
            None => ene_api::v1::round::RoundTarget::New,
        }
    }

    pub fn push_deferred(&mut self, frame: WireFrame) {
        if self.deferred.len() >= DEFERRED_CAP {
            let _ = self.deferred.pop_front();
        }
        self.deferred.push_back(frame);
    }

    pub fn clear_deferred_frames(&mut self) {
        self.deferred.clear();
    }

    pub fn wipe_transient(&mut self) -> Vec<ClientTempClass> {
        self.clear_deferred_frames();
        vec![
            ClientTempClass::PresentationBuffer,
            ClientTempClass::InputDraft,
        ]
    }

    pub fn set_defer_erasure(&mut self, defer: bool) {
        self.defer_erasure = defer;
    }

    #[must_use]
    pub fn defer_erasure(&self) -> bool {
        self.defer_erasure
    }

    pub fn push_pending_erasure(&mut self, demand: DeletionDemand) {
        if self.pending_erasure.len() >= PENDING_ERASURE_CAP {
            let _ = self.pending_erasure.pop_front();
        }
        self.pending_erasure.push_back(demand);
    }

    pub fn take_pending_erasure(&mut self) -> Option<DeletionDemand> {
        self.pending_erasure.pop_front()
    }

    pub fn take_undelivered(&mut self) -> Vec<WireFrame> {
        let mut summaries = Vec::new();
        let mut rest = VecDeque::with_capacity(self.deferred.len());
        for frame in self.deferred.drain(..) {
            if matches!(frame.payload, WirePayload::UndeliveredResponse(_)) {
                summaries.push(frame);
            } else {
                rest.push_back(frame);
            }
        }
        self.deferred = rest;
        summaries
    }
}

pub fn stale_generation_of(answer: &WirePayload) -> Option<u64> {
    if let WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::StaleRound {
        current_generation,
        ..
    }) = answer
    {
        Some(*current_generation)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FrameDecision {
    AbsorbPresence(PresenceAttributionWire),
    AbsorbBodyHint,
    Answer(WirePayload),
    Defer,
}

#[must_use]
pub fn decide_frame(own_message_id: WireMessageId, frame: &WireFrame) -> FrameDecision {
    match &frame.payload {
        WirePayload::PresenceAttribution(fact) => FrameDecision::AbsorbPresence(fact.clone()),
        WirePayload::BodyStateHint(_) => FrameDecision::AbsorbBodyHint,
        _ if frame.envelope.correlation.reply_to == Some(own_message_id) => {
            FrameDecision::Answer(frame.payload.clone())
        }
        _ => FrameDecision::Defer,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthDecision {
    Accepted { connection_id: ConnectionWireId },
    Guidance { message: String },
    Unexpected { message: String },
}

#[must_use]
pub fn decide_auth(payload: &WirePayload) -> AuthDecision {
    match payload {
        WirePayload::AuthResult(result) => match result {
            AuthResult::Accepted { connection_id } => AuthDecision::Accepted {
                connection_id: *connection_id,
            },
            AuthResult::Rejected { reason } => AuthDecision::Guidance {
                message: auth_rejected_guidance(reason),
            },
        },
        unexpected => AuthDecision::Unexpected {
            message: format!(
                "unexpected {} during authentication; expected AuthResult",
                unexpected.message_type()
            ),
        },
    }
}
