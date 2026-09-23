use std::collections::VecDeque;

use ene_api::v1::deletion::{ClientTempClass, DeletionDemand};
use ene_api::v1::handshake::AuthResult;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::presence::PresenceAttributionWire;
use ene_api::v1::refs::{ConnectionWireId, WireMessageId};
use ene_api::v1::round::RoundIntakeOutcomeWire;
use ene_plugin_ipc::WireFrame;

use super::frames::auth_rejected_guidance;

pub const DEFERRED_CAP: usize = 32;

/// Beyond this cap the oldest stashed Host demand is discarded to make room,
/// never the newest; the Host holds and re-demands a dropped condition.
pub const PENDING_ERASURE_CAP: usize = 32;

/// Observed session: latest presence generation, companion projection, and
/// the deferred out-of-order answer queue.
///
/// Latest value supersedes: each new fact or stale answer overwrites. A
/// missing generation is never read as current — a [`None`]-stamped input
/// answered with `NeedsRevalidation` is the correct outcome; defaulting it
/// (zero) would claim a generation the client never observed, and the Host
/// would treat that stale claim as currentness evidence it is not.
///
/// The deferred queue holds whole [`WireFrame`]s (payload plus envelope),
/// never facts (absorbed on arrival); it is session-lifetime only, never
/// persisted, and capped at [`DEFERRED_CAP`] with oldest-drop. It only
/// buffers auto-presented summaries drained by [`Self::take_undelivered`];
/// answers are correlated on the read path, never recovered from here.
///
/// `PartialEq` and `Eq` are deliberately absent: [`WireFrame`] is
/// `PartialEq`-only, and whole-session equality beyond tests is meaningless;
/// callers compare dimensions.
#[derive(Default)]
pub struct SessionState {
    generation: Option<u64>,
    /// Companion projection to echo on submits and history requests so the
    /// Host resolves them through its mapping.
    companion: Option<String>,
    deferred: VecDeque<WireFrame>,
    defer_erasure: bool,
    /// Host erasure demands stashed for the GUI participant, bounded at
    /// [`PENDING_ERASURE_CAP`] with oldest-drop so a chatty or hostile Host
    /// cannot grow the session without bound.
    pending_erasure: VecDeque<DeletionDemand>,
}

impl core::fmt::Debug for SessionState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SessionState")
            .field("generation", &self.generation)
            .field("companion", &self.companion)
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

    /// Applies an authoritative presence fact: its generation and companion
    /// projection supersede what the session held, so later sends echo the
    /// Host's current mapping instead of guessing.
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

    /// Drains deferred auto-presented summaries (unsolicited facts the Host
    /// pushed without `reply_to`). The caller paints them and ACKs each
    /// receipt it fully painted; unpainted ones stay Unknown Host-side.
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
    /// Host → Client activity hint (IPC M-21). Never an answer.
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

/// [`AuthResult::Rejected`] maps to [`AuthDecision::Guidance`] (exit code 2:
/// re-provision a fresh secret and retry) while an unexpected payload kind
/// maps to [`AuthDecision::Unexpected`] (a wire-shape violation, exit code 1).
/// The Host's rejection reason is operational by DTO contract (never a secret
/// or body copy), so carrying it into the guidance is safe.
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
