//! Session state and pure frame-decision rules.
//!
//! Kept transport-free: the socket loop feeds decoded frames in and acts on
//! these decisions.

use std::collections::VecDeque;

use ene_api::v1::handshake::AuthResult;
use ene_api::v1::payload::WirePayload;
use ene_api::v1::presence::PresenceAttributionWire;
use ene_api::v1::refs::{ConnectionWireId, WireMessageId};
use ene_api::v1::round::RoundIntakeOutcomeWire;
use ene_plugin_ipc::WireFrame;

use super::frames::auth_rejected_guidance;

/// Beyond this cap the oldest queued frame is discarded to make room, never
/// the newest, so a chatty or hostile Host cannot grow the session without
/// bound.
pub const DEFERRED_CAP: usize = 32;

/// Observed session: latest presence generation, companion projection,
/// authenticated connection key, pairing secret, and the deferred
/// out-of-order answer queue.
///
/// Latest value supersedes: each new fact or stale answer overwrites. A
/// missing generation is never read as current — a [`None`]-stamped input
/// answered with `NeedsRevalidation` is the correct outcome; defaulting it
/// (zero) would claim a generation the client never observed, and the Host
/// would treat that stale claim as currentness evidence it is not.
///
/// The pairing secret lives here for the session lifetime only (loaded from
/// the device file or the one-shot bootstrap at connect time) and is never
/// logged; the custom [`core::fmt::Debug`] below renders it as `[redacted]`
/// so a debug dump cannot leak key material.
///
/// The deferred queue holds whole [`WireFrame`]s (payload plus envelope, so
/// the `reply_to` link survives for later correlation), never facts (absorbed
/// on arrival); it is session-lifetime only, never persisted, and capped at
/// [`DEFERRED_CAP`] with oldest-drop.
///
/// `Eq` is deliberately absent: [`WireFrame`] is `PartialEq`-only, and
/// whole-session equality beyond tests is meaningless; callers compare
/// dimensions.
#[derive(Clone, PartialEq, Default)]
pub struct SessionState {
    generation: Option<u64>,
    /// Companion projection to echo on submits and history requests so the
    /// Host resolves them through its mapping.
    companion: Option<String>,
    connection_id: Option<ConnectionWireId>,
    pairing_secret: Option<String>,
    deferred: VecDeque<WireFrame>,
}

impl core::fmt::Debug for SessionState {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SessionState")
            .field("generation", &self.generation)
            .field("companion", &self.companion)
            .field("connection_id", &self.connection_id)
            .field(
                "pairing_secret",
                &self.pairing_secret.as_ref().map(|_| "[redacted]"),
            )
            .field("deferred_len", &self.deferred.len())
            .finish()
    }
}

impl SessionState {
    pub fn generation(&self) -> Option<u64> {
        self.generation
    }

    pub fn set_connection(&mut self, connection_id: ConnectionWireId) {
        self.connection_id = Some(connection_id);
    }

    pub fn pairing_secret(&self) -> Option<&str> {
        self.pairing_secret.as_deref()
    }

    /// Session-lifetime only, used for proof derivation on demand: never
    /// written anywhere from here (persistence is the device file's job at
    /// connect time).
    pub fn set_pairing_secret(&mut self, secret: String) {
        self.pairing_secret = Some(secret);
    }

    /// Applies an authoritative presence fact: its generation and companion
    /// projection supersede what the session held, so later sends echo the
    /// Host's current mapping instead of guessing.
    pub fn observe_presence(&mut self, fact: &PresenceAttributionWire) {
        self.generation = Some(fact.generation);
        self.companion = Some(fact.companion.0.clone());
    }

    /// Falls back to the
    /// [`DEFAULT_COMPANION_REF`](crate::cmds::DEFAULT_COMPANION_REF)
    /// bootstrap until the first presence fact arrives; the Host revalidates
    /// that fallback rather than attributing through it.
    pub fn companion_ref(&self) -> String {
        self.companion
            .clone()
            .unwrap_or_else(|| String::from(crate::cmds::DEFAULT_COMPANION_REF))
    }

    /// Normal-operation refresh from a stale-round answer, distinct from the
    /// handshake bootstrap: the next send carries what the Host just reported.
    pub fn note_stale_generation(&mut self, current: u64) {
        self.generation = Some(current);
    }

    pub fn push_deferred(&mut self, frame: WireFrame) {
        if self.deferred.len() >= DEFERRED_CAP {
            let _ = self.deferred.pop_front();
        }
        self.deferred.push_back(frame);
    }

    /// Facts never sit in the queue, so a hit is always an answer the caller
    /// can return without socket I/O.
    pub fn take_deferred_reply(&mut self, own: WireMessageId) -> Option<WirePayload> {
        let position = find_deferred_reply(&self.deferred, own)?;
        self.deferred.remove(position).map(|frame| frame.payload)
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

/// One ruling shared by [`super::Client::request`]'s socket loop; queue-cap
/// handling stays with the caller.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameDecision {
    AbsorbPresence(PresenceAttributionWire),
    Answer(WirePayload),
    Defer,
}

/// Total and pure: no I/O, no session access, so tests rule on the same
/// function the socket loop uses. Only presence facts absorb — a future
/// unsolicited fact kind needs a new arm here, and until then such frames
/// defer instead of surfacing as answers.
#[must_use]
pub fn decide_frame(own_message_id: WireMessageId, frame: &WireFrame) -> FrameDecision {
    if let WirePayload::PresenceAttribution(fact) = &frame.payload {
        FrameDecision::AbsorbPresence(fact.clone())
    } else if frame.envelope.correlation.reply_to == Some(own_message_id) {
        FrameDecision::Answer(frame.payload.clone())
    } else {
        FrameDecision::Defer
    }
}

/// Finds the frame the session pop ([`SessionState::take_deferred_reply`])
/// correlates to `own`.
fn find_deferred_reply(deferred: &VecDeque<WireFrame>, own: WireMessageId) -> Option<usize> {
    deferred
        .iter()
        .position(|frame| frame.envelope.correlation.reply_to == Some(own))
}

/// [`AuthResult::Rejected`] maps to [`AuthDecision::Guidance`] (exit code 2:
/// re-provision a fresh secret and retry) while an unexpected payload kind
/// maps to [`AuthDecision::Unexpected`] (a wire-shape violation, exit code 1).
/// The Host's rejection reason is operational by DTO contract (never a secret
/// or body copy), so carrying it into the guidance is safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthDecision {
    /// Authenticated: this connection key governs later messages.
    Accepted { connection_id: ConnectionWireId },
    /// Rejected: operator guidance carrying the Host reason, never secrets
    /// (exit code 2 at the crate root).
    Guidance { message: String },
    /// Wrong payload kind entirely (exit code 1 at the crate root); names the
    /// received and expected kinds, never bodies.
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
