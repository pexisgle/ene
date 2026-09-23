use std::collections::VecDeque;

use ene_api::v1::deletion::{ClientTempClass, DeletionDemand};
use ene_api::v1::handshake::{AuthResult, PairingProvisionSecret};
use ene_api::v1::payload::{BodyStateHint, WirePayload};
use ene_api::v1::presence::{PresenceAttributionWire, PresenceStateWire};
use ene_api::v1::refs::{ConnectionWireId, WireMessageId};
use ene_api::v1::round::RoundIntakeOutcomeWire;
use ene_plugin_ipc::WireFrame;

use super::frames::auth_rejected_guidance;

pub const DEFERRED_CAP: usize = 32;

#[derive(Clone, PartialEq, Default)]
pub struct SessionState {
    generation: Option<u64>,
    presence: Option<PresenceStateWire>,
    companion: Option<String>,
    connection_id: Option<ConnectionWireId>,
    pairing_secret: Option<PairingProvisionSecret>,
    deferred: VecDeque<WireFrame>,
    defer_erasure: bool,
    pending_erasure: VecDeque<DeletionDemand>,
}

impl core::fmt::Debug for SessionState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SessionState")
            .field("generation", &self.generation)
            .field("presence", &self.presence)
            .field("companion", &self.companion)
            .field("connection_id", &self.connection_id)
            .field(
                "pairing_secret",
                &self.pairing_secret.as_ref().map(|_| "[redacted]"),
            )
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

    #[must_use]
    pub fn presence_state(&self) -> Option<PresenceStateWire> {
        self.presence
    }

    pub fn set_connection(&mut self, connection_id: ConnectionWireId) {
        self.connection_id = Some(connection_id);
    }

    pub fn pairing_secret(&self) -> Option<&str> {
        self.pairing_secret
            .as_ref()
            .map(PairingProvisionSecret::expose_secret)
    }

    pub fn set_pairing_secret(&mut self, secret: PairingProvisionSecret) {
        self.pairing_secret = Some(secret);
    }

    pub fn observe_presence(&mut self, fact: &PresenceAttributionWire) {
        self.generation = Some(fact.generation);
        self.presence = Some(fact.state);
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
        self.pending_erasure.push_back(demand);
    }

    pub fn take_pending_erasure(&mut self) -> Option<DeletionDemand> {
        self.pending_erasure.pop_front()
    }

    pub fn take_deferred_reply(&mut self, own: WireMessageId) -> Option<WirePayload> {
        let position = find_deferred_reply(&self.deferred, own)?;
        self.deferred.remove(position).map(|frame| frame.payload)
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
    AbsorbBodyHint(BodyStateHint),
    Answer(WirePayload),
    Defer,
}

#[must_use]
pub fn decide_frame(own_message_id: WireMessageId, frame: &WireFrame) -> FrameDecision {
    match &frame.payload {
        WirePayload::PresenceAttribution(fact) => FrameDecision::AbsorbPresence(fact.clone()),
        WirePayload::BodyStateHint(hint) => FrameDecision::AbsorbBodyHint(hint.clone()),
        _ if frame.envelope.correlation.reply_to == Some(own_message_id) => {
            FrameDecision::Answer(frame.payload.clone())
        }
        _ => FrameDecision::Defer,
    }
}

fn find_deferred_reply(deferred: &VecDeque<WireFrame>, own: WireMessageId) -> Option<usize> {
    deferred
        .iter()
        .position(|frame| frame.envelope.correlation.reply_to == Some(own))
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
