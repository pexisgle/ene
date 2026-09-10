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

use super::frames::{auth_rejected_guidance, payload_kind};

/// Maximum deferred out-of-order answers held per session.
///
/// When [`super::Client::request`] reads a non-fact frame whose `reply_to` does not
/// match its send, it pushes the whole frame here and keeps reading; the
/// next request scans here first. Oldest-drop keeps a chatty or hostile Host
/// from growing the session without bound: beyond the cap the oldest queued
/// frame is discarded to make room, never the newest.
pub const DEFERRED_CAP: usize = 32;

/// Observed session: the latest generation value this process has seen,
/// the authenticated connection key, the pairing secret, and the deferred
/// out-of-order answer queue.
///
/// Latest value supersedes: an older fact never moves the session backwards
/// except by replacement (each new fact or stale answer simply overwrites).
/// A missing fact is never read as current: the session starts [`None`]
/// (pre-handshake bootstrap, before any Host fact arrived) and a
/// [`None`]-stamped input answered with `NeedsRevalidation` is the correct
/// outcome, never a reason to default the stamp (zero would claim a
/// generation the client never observed, and the Host would treat that stale
/// claim as currentness evidence it is not).
///
/// The pairing secret lives here only for the session lifetime (loaded from
/// the device file or the one-shot bootstrap at connect time): it is never
/// logged, and the custom [`core::fmt::Debug`] below renders it as
/// `[redacted]` so a debug dump cannot leak key material.
///
/// The deferred queue holds whole [`WireFrame`]s (payload plus envelope, so
/// the `reply_to` link survives for later correlation), never facts (facts
/// are absorbed into the generation on arrival). It is session-lifetime
/// only, never persisted, capped at [`DEFERRED_CAP`] with oldest-drop.
///
/// `Eq` is deliberately absent: [`WireFrame`] is `PartialEq`-only, and
/// session equality beyond tests is meaningless (generation plus queue
/// contents); callers compare dimensions, not whole sessions.
#[derive(Clone, PartialEq, Default)]
pub struct SessionState {
    /// Latest observed presence generation, if any fact arrived yet.
    generation: Option<u64>,
    /// Latest observed companion projection, echoed back on submits and
    /// history requests so the Host resolves them through its mapping.
    companion: Option<String>,
    /// Connection key the Host issued on authentication, if challenged yet.
    connection_id: Option<ConnectionWireId>,
    /// Pairing secret proving this device, if provisioned yet.
    pairing_secret: Option<String>,
    /// Out-of-order answers seen while waiting for another reply.
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
    /// Starts a bootstrap session: no generation observed, no connection
    /// authenticated, no secret provisioned, and no deferred answers yet.
    pub fn new() -> Self {
        Self {
            generation: None,
            companion: None,
            connection_id: None,
            pairing_secret: None,
            deferred: VecDeque::new(),
        }
    }

    /// Returns the latest observed generation, if any.
    pub fn generation(&self) -> Option<u64> {
        self.generation
    }

    /// Returns the authenticated connection key, if a challenge completed.
    pub fn connection_id(&self) -> Option<ConnectionWireId> {
        self.connection_id
    }

    /// Records the connection key from an accepted authentication.
    pub fn set_connection(&mut self, connection_id: ConnectionWireId) {
        self.connection_id = Some(connection_id);
    }

    /// Returns the session pairing secret, if provisioned.
    pub fn pairing_secret(&self) -> Option<&str> {
        self.pairing_secret.as_deref()
    }

    /// Holds the pairing secret for this session lifetime only: it is used
    /// for proof derivation on demand and never written anywhere from here
    /// (persistence is the device file's job at connect time).
    pub fn set_pairing_secret(&mut self, secret: String) {
        self.pairing_secret = Some(secret);
    }

    /// Records an authoritative presence fact: the fact's generation becomes
    /// current (latest supersedes; the Host sends the fact post-capability)
    /// and its companion projection becomes the ref this session echoes on
    /// submits and history requests, so the Host resolves them through its
    /// mapping instead of guessing.
    pub fn observe_presence(&mut self, fact: &PresenceAttributionWire) {
        self.generation = Some(presence_generation_of_fact(fact));
        self.companion = Some(fact.companion.0.clone());
    }

    /// Returns the companion projection to echo: the learned one, or the
    /// [`DEFAULT_COMPANION_REF`](crate::cmds::DEFAULT_COMPANION_REF)
    /// bootstrap until the first presence fact arrives (the Host
    /// revalidates the bootstrap rather than attributing through it).
    pub fn companion_ref(&self) -> String {
        self.companion
            .clone()
            .unwrap_or_else(|| String::from(crate::cmds::DEFAULT_COMPANION_REF))
    }

    /// Records a stale-round answer's current generation during normal
    /// operation. This is distinct from the handshake bootstrap: it refreshes
    /// an already-running session after the Host moved on, so the next send
    /// carries what the Host just reported.
    pub fn note_stale_generation(&mut self, current: u64) {
        self.generation = Some(current);
    }

    /// Returns how many out-of-order answers are deferred.
    pub fn deferred_len(&self) -> usize {
        self.deferred.len()
    }

    /// Defers one mismatched answer frame, enforcing the [`DEFERRED_CAP`]
    /// oldest-drop bound: when full the oldest queued frame is discarded to
    /// make room, never the newest.
    pub fn push_deferred(&mut self, frame: WireFrame) {
        if self.deferred.len() >= DEFERRED_CAP {
            let _ = self.deferred.pop_front();
        }
        self.deferred.push_back(frame);
    }

    /// Removes and returns the first deferred frame whose `reply_to` equals
    /// `own`, if any. Facts never sit in the queue, so a hit is always an
    /// answer the caller can return without socket I/O. Shares the scan
    /// with [`select_answer`] through the same position function, so both
    /// find the same frame.
    pub fn take_deferred_reply(&mut self, own: WireMessageId) -> Option<WirePayload> {
        let position = find_deferred_reply(&self.deferred, own)?;
        self.deferred.remove(position).map(|frame| frame.payload)
    }
}

/// Reads the generation out of a presence fact. A free function so the frame
/// loop and the session update stay testable without a socket.
pub fn presence_generation_of_fact(fact: &PresenceAttributionWire) -> u64 {
    fact.generation
}

/// Reads the current generation out of a stale-round intake answer, if the
/// answer is one. A free function so request handling stays testable without
/// a socket.
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

/// Ruling for one incoming frame against our outgoing message ID.
///
/// The single decision behind both the pure [`select_answer`] script form
/// and [`super::Client::request`]'s socket loop: the loop classifies every read
/// frame here and only applies session effects, so the pure tests below
/// verify the production ruling directly instead of a mirror. Queue-cap
/// handling stays with each caller (session push vs. script queue).
#[derive(Debug, Clone, PartialEq)]
pub enum FrameDecision {
    /// An authoritative presence fact to absorb into the session.
    AbsorbPresence(PresenceAttributionWire),
    /// The correlated answer to return to the caller.
    Answer(WirePayload),
    /// Anything else: defer under the caller's oldest-drop cap.
    Defer,
}

/// Classifies one incoming frame: presence facts absorb, a `reply_to`
/// match answers, anything else defers. Total and pure: no I/O, no session
/// access, so both the script form and the socket loop rule identically.
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

/// Position of the first deferred frame whose `reply_to` equals `own`, if
/// any. Shared by the session pop ([`SessionState::take_deferred_reply`])
/// and the script scan ([`select_answer`]) so both find the same frame.
fn find_deferred_reply(deferred: &VecDeque<WireFrame>, own: WireMessageId) -> Option<usize> {
    deferred
        .iter()
        .position(|frame| frame.envelope.correlation.reply_to == Some(own))
}

/// Splits a deferred queue plus an incoming frame script into the facts
/// `request` would absorb, the correlated answer, and the updated queue.
///
/// This is the pure form of the [`super::Client::request`] loop decision. First the
/// deferred queue is scanned for a frame whose `reply_to` equals our
/// outgoing message ID: a hit returns immediately with no absorption and
/// that frame removed, without consuming `frames` (no socket I/O in the
/// streaming form). Otherwise `frames` are walked in order, each classified
/// by [`decide_frame`]: absorbed facts accumulate (the caller applies each
/// to its session); the first answer ends the walk (later script frames stay
/// unread, as later socket reads in the streaming form); deferred frames
/// push to the queue (cap [`DEFERRED_CAP`], oldest-drop) and the walk
/// continues — mismatches are never returned as answers and never silently
/// dropped. No match means no answer ([`None`]); the streaming caller keeps
/// reading in that case.
///
/// Only the presence-fact variant is absorbed: a future unsolicited fact
/// kind needs a new arm in [`decide_frame`], and until then such frames
/// queue as mismatches instead of surfacing as answers.
///
/// The function is total: every combination of queue and script yields a
/// (possibly empty) absorption, a (possibly absent) answer, and a bounded
/// queue, with no I/O and no failure.
#[must_use]
pub fn select_answer(
    own_message_id: WireMessageId,
    deferred: &VecDeque<WireFrame>,
    frames: &[WireFrame],
) -> (
    Vec<PresenceAttributionWire>,
    Option<WirePayload>,
    VecDeque<WireFrame>,
) {
    let mut queue = deferred.clone();
    if let Some(position) = find_deferred_reply(&queue, own_message_id) {
        let hit = queue.remove(position).map(|frame| frame.payload);
        return (Vec::new(), hit, queue);
    }
    let mut absorbed = Vec::new();
    for frame in frames {
        match decide_frame(own_message_id, frame) {
            FrameDecision::AbsorbPresence(fact) => absorbed.push(fact),
            FrameDecision::Answer(payload) => return (absorbed, Some(payload), queue),
            FrameDecision::Defer => {
                if queue.len() >= DEFERRED_CAP {
                    let _ = queue.pop_front();
                }
                queue.push_back(frame.clone());
            }
        }
    }
    (absorbed, None, queue)
}

/// Authentication outcome decision for one inbound payload: either the
/// accepted connection key, or a ready-made failure.
///
/// [`AuthResult::Rejected`]
/// maps to [`AuthDecision::Guidance`] (exit code 2: the operator can
/// re-provision a fresh secret and retry), while an unexpected payload kind
/// maps to [`AuthDecision::Unexpected`] (a wire-shape violation, exit
/// code 1). The Host's rejection reason is operational by DTO contract
/// (never a secret or body copy), so carrying it into the guidance is safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthDecision {
    /// Authenticated: this connection key governs later messages.
    Accepted {
        /// Newly issued connection key for this connection.
        connection_id: ConnectionWireId,
    },
    /// Rejected with operator guidance (exit code 2 at the crate root).
    Guidance {
        /// What to do next: carries the Host reason, never secrets.
        message: String,
    },
    /// Wrong payload kind entirely (exit code 1 at the crate root).
    Unexpected {
        /// Names the received kind and the expected one, never bodies.
        message: String,
    },
}

/// Maps one inbound payload to its [`AuthDecision`]: accepted keys pass
/// through, rejections become provisioning guidance, and anything else names
/// its kind.
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
                payload_kind(unexpected)
            ),
        },
    }
}
