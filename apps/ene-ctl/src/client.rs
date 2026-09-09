//! Host transport: socket path, frame builders, handshake, request/response.
//!
//! The CLI dials the Host over a Unix-domain socket at
//! [`socket_path`] (`ene.sock` inside the resolved data directory; the socket
//! name is Stage-2 provisional). The handshake is pairing, then capability
//! advertisement, on every connect: already-paired descriptors re-pair
//! idempotently to the same device key. The issued device key plus the
//! approve-time pairing secret persist in the device file (see
//! [`crate::device`]); the secret enters through the `ENE_PAIRING_SECRET`
//! bootstrap variable (first provision, or one-shot rotation over a
//! differing or absent file secret, overwriting the file) and is never
//! logged, never rendered in `Debug`, and
//! never sent over the wire — only ownership proofs derived from it leave
//! the device.
//!
//! A first run sends [`PairingRequest`]
//! (display descriptor, pre-pairing sender with no device ID), which must
//! answer [`Paired`](ene_api::v1::handshake::PairingResult::Paired) before
//! the client continues in the same session.
//! Then [`CapabilityAdvertise`]
//! must answer
//! [`NegotiatedConnection`](ene_api::v1::handshake::NegotiatedConnection)
//! with a matching major version. The capability frame carries no device ID:
//! the Host attributes it through the connection table (which recorded the
//! paired device when this same connection paired moments earlier), so no
//! device claim is needed before authentication.
//!
//! Authentication ([`AuthChallenge`] /
//! [`AuthProof`] /
//! [`AuthResult`]) is implemented on this
//! side ([`proof_frame`], [`decide_auth`], [`Client::authenticate`]) but the
//! current Host never emits a challenge (its auth trio answers nothing yet),
//! so `connect` performs no auth exchange: it reads exactly the pairing
//! answer and the negotiated terms and returns, leaving any pipelined
//! presence fact buffered for the caller. A speculative proof today would
//! block forever waiting for an `AuthResult` that never comes. When the Host
//! starts challenging, `connect` gains a challenge read at that point; the
//! proof builder, the result decision, and the connection-ID storage need no
//! change. The accepted connection key is stored into the sender for all
//! later frames plus into the [`SessionState`] mirror.
//!
//! Request/response correlation: every [`Client::request`] stamps a fresh
//! command ID on its outgoing envelope and matches the answer by transport
//! pairing (`reply_to` against our message ID). A single read per request
//! is wrong because the Host pipelines
//! unsolicited facts ahead of answers — capability today appends the current
//! [`PresenceAttribution`](ene_api::v1::payload::WirePayload::PresenceAttribution)
//! fact right after the negotiated terms, and a leftover fact would be
//! misread as the next request's answer. So `request` consults the deferred
//! queue first and then loops: a queued or incoming frame whose `reply_to`
//! matches is the answer and returns without further I/O; presence facts
//! are absorbed into the [`SessionState`] and reading continues; any other
//! non-fact frame is pushed to the deferred queue (cap [`DEFERRED_CAP`],
//! oldest-drop) and reading continues — mismatches are never returned as
//! answers and never silently dropped. The pure
//! [`select_answer`] holds that decision over a deferred queue plus a frame
//! script; the socket loop is its streaming form. Only the fact variant is
//! absorbed for now: any future unsolicited fact kind needs a new arm here,
//! and until then such frames queue as mismatches instead of surfacing as
//! answers.
//!
//! Pairing that is still pending answers
//! [`PendingOwnerConfirmation`](ene_api::v1::handshake::PairingResult::PendingOwnerConfirmation):
//! the operator approves the device on the Host-local trusted surface (which
//! shows the one-time secret once), re-runs the client with
//! `ENE_PAIRING_SECRET` set for that one run so the secret reaches the `0600`
//! device file (first provision, or rotation overwriting a differing
//! secret), and later runs read the file. A denied pairing answers the
//! same way operationally (exit code 2 with the Host reason plus that
//! guidance). A stored device the Host no longer knows fails later at the
//! domain gate (unknown sender: close plus `DisconnectNotice`), never with a
//! dedicated capability-time outcome.
//!
//! Presence generation (see [`SessionState`]): the client keeps the latest
//! observed generation and stamps it on every [`SubmitTextInput`](ene_api::v1::round::SubmitTextInput)
//! envelope as `observed.presence_generation_view`. The value starts
//! [`None`] (pre-handshake bootstrap) and is set from the authoritative
//! [`PresenceAttribution`](ene_api::v1::payload::WirePayload::PresenceAttribution)
//! fact the Host sends post-capability, and from
//! [`StaleRound`](ene_api::v1::round::RoundIntakeOutcomeWire::StaleRound)
//! `current_generation` during normal operation. A [`None`]-stamped input
//! that the Host answers with `NeedsRevalidation` is the correct outcome,
//! never worked around by sending a default like zero.
//!
//! Framing goes through `ene-plugin-ipc` only ([`encode_frame`]/[`decode_frame`]); this module
//! owns the socket read/write loops. [`CodecError`]
//! displays carry lengths and decoder reasons only and never echo frame
//! bytes, so mapping them into [`CliError::Codec`]
//! cannot leak conversation text. All other error messages carry operations,
//! payload-kind names, refs, or generations — never bodies or secrets.
//!
//! Non-Unix platforms get stubs returning
//! [`CliError::UnsupportedPlatform`];
//! the pure builders below stay shared.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::handshake::{
    AuthChallenge, AuthProof, AuthResult, CapabilityAdvertise, PairingRequest,
};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::presence::PresenceAttributionWire;
use ene_api::v1::refs::{
    ClientIncarnationId, CommandWireId, ConnectionWireId, DeviceWireId, RequestWireId,
    WireMessageId, WireMessageType,
};
use ene_api::v1::round::RoundIntakeOutcomeWire;
use ene_credential::pairing_proof_hex;
use ene_plugin_ipc::{CodecError, MAX_FRAME_BYTES, WireFrame, decode_frame, encode_frame};

use crate::device;
use crate::errors::CliError;

/// Returns the Host socket path for `data_dir`: `<data_dir>/ene.sock`.
///
/// Pure and side-effect free; the caller decides whether the directory or
/// socket must exist (absence surfaces as [`CliError::Transport`] on dial).
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ene.sock")
}

/// Display-only platform string for pairing and capability frames, from the
/// compile-time OS and architecture (for example `"linux-x86_64"`). Display
/// only, never permission evidence.
pub fn platform_display() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Per-process incarnation sequence backing [`new_incarnation`]: `std` only,
/// process pid plus a process-local monotonic counter plus start-time
/// nanoseconds (see the function docs).
static INCARNATION_SEQ: AtomicU64 = AtomicU64::new(0);

/// Start-time nanoseconds memo for [`new_incarnation`], captured once.
static INCARNATION_START: OnceLock<u64> = OnceLock::new();

/// Mints this process's incarnation: `counter` is the process pid (unique per
/// boot per device for distinct processes), `random` folds a process-local
/// monotonic sequence into the process start-time nanoseconds.
///
/// Uniqueness needs are modest — disambiguating restarts of one device —
/// and a collision only risks a duplicate-suppression alias, never a
/// privilege change, so clock-plus-counter randomness from `std` only
/// (no OS RNG dependency) documented here is enough. This never collapses
/// with connection identity or presence generation: the three stay separate
/// envelope dimensions.
pub fn new_incarnation() -> ClientIncarnationId {
    let start = *INCARNATION_START.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos() as u64)
    });
    let seq = INCARNATION_SEQ.fetch_add(1, Ordering::Relaxed);
    ClientIncarnationId {
        counter: u64::from(std::process::id()),
        random: start.wrapping_add(seq),
    }
}

/// Maximum deferred out-of-order answers held per session.
///
/// When [`Client::request`] reads a non-fact frame whose `reply_to` does not
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
    /// answer the caller can return without socket I/O.
    pub fn take_deferred_reply(&mut self, own: WireMessageId) -> Option<WirePayload> {
        let position = self
            .deferred
            .iter()
            .position(|frame| frame.envelope.correlation.reply_to == Some(own))?;
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

/// Splits a deferred queue plus an incoming frame script into the facts
/// `request` would absorb, the correlated answer, and the updated queue.
///
/// This is the pure form of the [`Client::request`] loop decision. First the
/// deferred queue is scanned for a frame whose `reply_to` equals our
/// outgoing message ID: a hit returns immediately with no absorption and
/// that frame removed, without consuming `frames` (no socket I/O in the
/// streaming form). Otherwise `frames` are walked in order:
/// [`PresenceAttribution`](ene_api::v1::payload::WirePayload::PresenceAttribution)
/// facts are collected (the caller applies each to its session); a non-fact
/// frame whose `reply_to` matches is the answer and ends the walk (later
/// script frames stay unread, as later socket reads in the streaming form);
/// any other non-fact frame is pushed to the queue (cap [`DEFERRED_CAP`],
/// oldest-drop) and the walk continues — mismatches are never returned as
/// answers and never silently dropped. No match means no answer ([`None`]);
/// the streaming caller keeps reading in that case.
///
/// Only the presence-fact variant is absorbed: a future unsolicited fact
/// kind needs a new arm here, and until then such frames queue as mismatches
/// instead of surfacing as answers.
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
    if let Some(position) = queue
        .iter()
        .position(|frame| frame.envelope.correlation.reply_to == Some(own_message_id))
    {
        let hit = queue.remove(position).map(|frame| frame.payload);
        return (Vec::new(), hit, queue);
    }
    let mut absorbed = Vec::new();
    for frame in frames {
        if let WirePayload::PresenceAttribution(fact) = &frame.payload {
            absorbed.push(fact.clone());
        } else if frame.envelope.correlation.reply_to == Some(own_message_id) {
            return (absorbed, Some(frame.payload.clone()), queue);
        } else {
            if queue.len() >= DEFERRED_CAP {
                let _ = queue.pop_front();
            }
            queue.push_back(frame.clone());
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

/// Builds the ownership proof frame for a challenge nonce: the proof is the
/// pairing-secret HMAC over the single-use nonce (hex), and the sender
/// carries no device or connection key.
///
/// No device claim is a deliberate attribution rule, not an omission: the
/// Host attributes the proof to the pending pairing this connection recorded
/// when it paired moments earlier (its connection table tracks the paired
/// device per connection; the envelope's device field is a Client claim the
/// Host must not trust for authentication). The incarnation still travels so
/// the connection's pinned owner stays attributable.
/// Builds the proof frame: the wire sender names the paired device under
/// proof (the Host attributes through its connection table and never trusts
/// the claim, but the paired sender contract carries it), echoes the
/// caller incarnation, and hides the connection id (still undisclosed
/// pre-accept).
pub fn proof_frame(
    proof: &str,
    incarnation: ClientIncarnationId,
    device_id: DeviceWireId,
) -> WireFrame {
    frame_for(
        WirePayload::AuthProof(AuthProof {
            proof: String::from(proof),
        }),
        WireSender {
            device_id: Some(device_id),
            incarnation_id: incarnation,
            connection_id: None,
        },
    )
}

/// Guidance for a still-pending pairing: approve on the Host-local trusted
/// surface, then re-run once with the shown secret in the environment so it
/// reaches the `0600` device file.
#[must_use]
pub fn pending_guidance() -> String {
    format!(
        "pairing is pending owner confirmation; approve the device on the \
         Host-local trusted surface, then re-run ene-ctl once with {} set \
         to the shown secret (it is stored to the 0600 client device file)",
        device::BOOTSTRAP_SECRET_ENV,
    )
}

/// Guidance for a challenge that arrived with no secret to prove with: the
/// operator must approve and provision before authentication can run.
#[must_use]
pub fn missing_secret_guidance() -> String {
    format!(
        "no pairing secret stored for this device; approve the device on \
         the Host-local trusted surface, then re-run ene-ctl once with {} \
         set to the shown secret",
        device::BOOTSTRAP_SECRET_ENV,
    )
}

/// Guidance for a rejected proof: the Host reason plus the re-provisioning
/// step. The reason is operational by DTO contract, so echoing it is safe.
#[must_use]
pub fn auth_rejected_guidance(reason: &str) -> String {
    format!(
        "authentication rejected: {reason}; approve the device again on the \
         Host-local trusted surface and re-run ene-ctl once with {} set to \
         the fresh secret",
        device::BOOTSTRAP_SECRET_ENV,
    )
}

/// Builds a retry frame: the caller's command id travels unchanged while
/// message and request ids go fresh for this attempt only. Same incarnation
/// only (see [`Client::retry`]): the sender, generation view, and payload
/// are reused untouched. Pure: the transport pairing in [`Client::retry`]
/// moves it unchanged.
pub fn retry_frame(
    payload: WirePayload,
    sender: WireSender,
    generation: Option<u64>,
    command: CommandWireId,
) -> WireFrame {
    let mut frame = frame_for_session(payload, sender, generation);
    frame.envelope.correlation.command_id = Some(command);
    frame.envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
    frame
}
/// `observed.presence_generation_view` with the session value on
/// [`SubmitTextInput`](ene_api::v1::round::SubmitTextInput) sends only. Other
/// payloads keep the [`None`] default: the generation view is intake
/// comparison material, not a general envelope claim.
pub fn frame_for_session(
    payload: WirePayload,
    sender: WireSender,
    generation: Option<u64>,
) -> WireFrame {
    let mut frame = frame_for(payload, sender);
    if matches!(frame.payload, WirePayload::SubmitTextInput(_)) {
        frame.envelope.observed.presence_generation_view = generation;
    }
    frame
}

/// Builds the pairing frame: display descriptor, pre-pairing sender (no
/// device ID yet — the Host issues it after Owner confirmation).
pub fn pairing_frame(descriptor: &str, incarnation: ClientIncarnationId) -> WireFrame {
    frame_for(
        WirePayload::PairingRequest(PairingRequest {
            device_descriptor: String::from(descriptor),
        }),
        WireSender {
            device_id: None,
            incarnation_id: incarnation,
            connection_id: None,
        },
    )
}

/// Builds the capability frame: speaks [`ProtocolVersion::V1`], claims no
/// optional features (text is the baseline, not a capability), and carries
/// the display platform string.
///
/// `connect` passes the paired device: the paired-sender contract names it
/// on capability and proof frames alike. The Host still attributes through
/// its connection table (paired moments earlier on this same connection)
/// and never trusts the claim — a mismatched claim drops the frame — so the
/// value here satisfies the wire contract without becoming authority.
/// Pre-pairing callers (and tests) pass [`None`].
pub fn capability_frame(
    platform: &str,
    incarnation: ClientIncarnationId,
    device_id: Option<DeviceWireId>,
) -> WireFrame {
    frame_for(
        WirePayload::CapabilityAdvertise(CapabilityAdvertise {
            supported_protocol: vec![ProtocolVersion::V1],
            features: Vec::new(),
            platform: String::from(platform),
        }),
        WireSender {
            device_id,
            incarnation_id: incarnation,
            connection_id: None,
        },
    )
}

/// Wraps `payload` in a [`ProtocolVersion::V1`] envelope for `sender`.
pub fn frame_for(payload: WirePayload, sender: WireSender) -> WireFrame {
    let message_type = message_type_for(&payload);
    let envelope = new_outgoing_envelope(ProtocolVersion::V1, sender, message_type);
    WireFrame { envelope, payload }
}

/// Stamps a fresh command ID on one outgoing request envelope and reports
/// the message ID the Host echoes in `reply_to`.
///
/// One fresh [`CommandWireId`] per send (never reused across retries at this
/// layer): the Host pairs its reply by `reply_to` against the returned
/// message ID, and the command ID keeps every request uniformly pairable as
/// command-side correlation grows. Handshake frames skip this (they rely on
/// message-ID pairing only); fire-and-forget observations skip it too (no
/// reply is ever paired to them). Transport retry of one logical send
/// reuses the ID through [`Client::retry`] instead.
fn stamp_request(frame: &mut WireFrame) -> WireMessageId {
    frame.envelope.correlation.command_id = Some(CommandWireId(uuid::Uuid::new_v4()));
    frame.envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
    frame.envelope.message_id
}

/// Static kind name of a payload, used in rejection messages (no bodies).
pub fn payload_kind(payload: &WirePayload) -> &'static str {
    match payload {
        WirePayload::PairingRequest(_) => "PairingRequest",
        WirePayload::PairingResult(_) => "PairingResult",
        WirePayload::AuthChallenge(_) => "AuthChallenge",
        WirePayload::AuthProof(_) => "AuthProof",
        WirePayload::AuthResult(_) => "AuthResult",
        WirePayload::CapabilityAdvertise(_) => "CapabilityAdvertise",
        WirePayload::NegotiatedConnection(_) => "NegotiatedConnection",
        WirePayload::ReconnectHello(_) => "ReconnectHello",
        WirePayload::RecoveryInvite(_) => "RecoveryInvite",
        WirePayload::DisconnectNotice(_) => "DisconnectNotice",
        WirePayload::SubmitTextInput(_) => "SubmitTextInput",
        WirePayload::RoundIntakeOutcome(_) => "RoundIntakeOutcome",
        WirePayload::TextStreamOpen(_) => "TextStreamOpen",
        WirePayload::TextStreamFrame(_) => "TextStreamFrame",
        WirePayload::TextStreamClose(_) => "TextStreamClose",
        WirePayload::ConfirmPresentation(_) => "ConfirmPresentation",
        WirePayload::HistoryRequest(_) => "HistoryRequest",
        WirePayload::HistoryView(_) => "HistoryView",
        WirePayload::PresenceAttribution(_) => "PresenceAttribution",
        WirePayload::ManagementIntent(_) => "ManagementIntent",
        WirePayload::ManagementOutcome(_) => "ManagementOutcome",
        WirePayload::ManagementViewRequest(_) => "ManagementViewRequest",
        WirePayload::ManagementView(_) => "ManagementView",
        WirePayload::Reject(_) => "Reject",
    }
}

/// Envelope discriminator for a payload: the variant name, matching the
/// convention the wire tests use (for example `"SubmitTextInput"`).
/// Routing hint only; the Host rejects unknown names, never guesses.
pub fn message_type_for(payload: &WirePayload) -> WireMessageType {
    WireMessageType(String::from(payload_kind(payload)))
}

/// Connected, handshaked Host session (Unix): the stream, the sender
/// identity for subsequent frames (device filled in by pairing, connection
/// filled in by authentication once the Host challenges), and the observed
/// session (presence generation, connection key, pairing secret, deferred
/// answers — see
/// [`SessionState`]).
#[cfg(unix)]
pub struct Client {
    /// Framed Host connection.
    stream: tokio::net::UnixStream,
    /// Sender identity for subsequent frames.
    sender: WireSender,
    /// Observed session: generation, connection key, pairing secret, deferred.
    state: SessionState,
}

#[cfg(unix)]
impl Client {
    /// Dials `ene.sock` under `data_dir` and runs the handshake: pairing,
    /// then capability advertisement.
    ///
    /// Pairing runs on every connect: already-paired descriptors re-pair
    /// idempotently to the same device key. The effective pairing secret is
    /// resolved by [`device::resolve_device_secret`]: a set, non-blank
    /// `ENE_PAIRING_SECRET` bootstrap rotates (it wins over a differing or
    /// absent file secret and overwrites the file); with no bootstrap value
    /// the stored file secret wins; with neither side holding a secret the
    /// session proceeds secretless. When pairing
    /// succeeds while this process holds a secret, the `{device_id, secret}`
    /// pair is persisted to the `0600` device file before capability runs
    /// (fail-closed: a store failure aborts the connect rather than running
    /// with an unpersisted secret — this covers both first provision and
    /// one-shot rotation). A successful pairing with no secret
    /// anywhere proceeds secretless — authentication simply guides later if
    /// the Host ever challenges.
    ///
    /// Capability advertises with no device ID (the Host attributes the
    /// frame through its per-connection pairing record); exactly one frame
    /// is read back and must be the negotiated terms. No further frames are
    /// read here: a pipelined presence fact stays buffered for the caller
    /// (and for [`Client::request`]'s absorbing loop), and authentication
    /// runs only through [`Client::authenticate`] once the Host actually
    /// sends a challenge — which it does not yet, so no proof is attempted
    /// here.
    ///
    /// There is no Host "unknown device" outcome on capability — an ID the
    /// Host no longer knows fails later at the domain gate (close plus
    /// `DisconnectNotice`).
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Transport`] when the socket cannot be reached, a
    /// frame cannot be moved, or the device file cannot be persisted;
    /// [`CliError::Codec`] when a frame cannot be encoded or decoded;
    /// [`CliError::ServerOutcome`] when pairing is still pending Owner
    /// confirmation or was denied (exit code 2: approve the device on the
    /// Host-local trusted surface, provision the shown secret once, then
    /// re-run — no auto-retry loop); and
    /// [`CliError::ServerRejected`] when the Host negotiates an incompatible
    /// version or answers with an unexpected payload kind.
    pub async fn connect(
        data_dir: &Path,
        descriptor: &str,
        platform: &str,
    ) -> Result<Self, CliError> {
        let path = socket_path(data_dir);
        let mut stream = tokio::net::UnixStream::connect(&path)
            .await
            .map_err(|error| {
                CliError::Transport(format!(
                    "connect to {} failed: {}",
                    path.display(),
                    error.kind()
                ))
            })?;
        let incarnation = new_incarnation();
        let stored = device::load_stored_device(data_dir);
        let (secret, source) = device::resolve_device_secret(
            stored
                .as_ref()
                .and_then(|known| known.secret().map(str::to_string)),
            device::read_bootstrap_secret(),
        );
        let device_id = {
            write_frame(&mut stream, &pairing_frame(descriptor, incarnation)).await?;
            match read_frame(&mut stream).await?.payload {
                WirePayload::PairingResult(result) => match result {
                    ene_api::v1::handshake::PairingResult::Paired { device_id } => device_id,
                    ene_api::v1::handshake::PairingResult::PendingOwnerConfirmation => {
                        return Err(CliError::ServerOutcome(pending_guidance()));
                    }
                    ene_api::v1::handshake::PairingResult::Denied { reason } => {
                        // `reason` is operational by DTO contract (never
                        // a secret or body copy), so echoing it is safe.
                        return Err(CliError::ServerOutcome(format!(
                            "pairing denied: {reason}; approve the device on the \\
                             Host-local trusted surface, then re-run ene-ctl"
                        )));
                    }
                },
                unexpected => {
                    return Err(CliError::ServerRejected(format!(
                        "unexpected {} during pairing; expected PairingResult",
                        payload_kind(&unexpected)
                    )));
                }
            }
        };
        // Persist whenever a secret is effective: first provision and
        // one-shot rotation both overwrite the `0600` file (a `Stored` secret
        // still rewrites alongside the fresh pairing device key; a `Rotated`
        // secret replaces the file secret). `Missing` holds no secret, so
        // there is nothing to persist.
        match source {
            device::SecretSource::Stored | device::SecretSource::Rotated => {
                if let Some(secret_value) = secret.as_deref() {
                    device::store_device(
                        data_dir,
                        &device::StoredDevice::new(device_id, secret_value.to_string()),
                    )?;
                }
            }
            device::SecretSource::Missing => {}
        }
        write_frame(
            &mut stream,
            &capability_frame(platform, incarnation, Some(device_id)),
        )
        .await?;
        match read_frame(&mut stream).await?.payload {
            WirePayload::NegotiatedConnection(negotiated) => {
                if !negotiated.version.shares_major_with(&ProtocolVersion::V1) {
                    return Err(CliError::ServerRejected(format!(
                        "negotiated incompatible version {}.{}; expected major 1",
                        negotiated.version.major, negotiated.version.minor
                    )));
                }
            }
            unexpected => {
                return Err(CliError::ServerRejected(format!(
                    "unexpected {} during capability negotiation; expected NegotiatedConnection",
                    payload_kind(&unexpected)
                )));
            }
        }
        let mut state = SessionState::new();
        if let Some(secret_value) = secret {
            state.set_pairing_secret(secret_value);
        }
        let mut session = Self {
            stream,
            sender: WireSender {
                device_id: Some(device_id),
                incarnation_id: incarnation,
                connection_id: None,
            },
            state,
        };
        let challenge = read_frame(&mut session.stream).await?.payload;
        let WirePayload::AuthChallenge(challenge) = challenge else {
            return Err(CliError::ServerRejected(format!(
                "unexpected {} after negotiation; expected AuthChallenge",
                payload_kind(&challenge)
            )));
        };
        session.authenticate(&challenge).await?;
        let fact = session.next_frame().await?;
        if !matches!(fact, WirePayload::PresenceAttribution(_)) {
            return Err(CliError::ServerRejected(format!(
                "unexpected {} after authentication; expected PresenceAttribution",
                payload_kind(&fact)
            )));
        }
        Ok(session)
    }

    /// Answers one authentication challenge: derives the ownership proof
    /// from the session secret and stores the accepted connection key into
    /// the sender (for all later frames) plus the session mirror.
    ///
    /// [`Client::connect`] calls this for the post-negotiation challenge;
    /// call it only with a Host-minted [`AuthChallenge`].
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Transport`] or [`CliError::Codec`] when the
    /// exchange cannot be moved or framed;
    /// [`CliError::ServerOutcome`] when no secret is provisioned (approve
    /// and provision, then re-run) or the Host rejects the proof (exit
    /// code 2: re-approve for a fresh secret and retry); and
    /// [`CliError::ServerRejected`] when the Host answers with an
    /// unexpected payload kind.
    pub async fn authenticate(&mut self, challenge: &AuthChallenge) -> Result<(), CliError> {
        let Some(secret) = self.state.pairing_secret().map(str::to_string) else {
            return Err(CliError::ServerOutcome(missing_secret_guidance()));
        };
        let proof = pairing_proof_hex(&secret, &challenge.nonce);
        let Some(device) = self.sender.device_id else {
            return Err(CliError::ServerRejected(String::from(
                "cannot prove ownership without a paired device",
            )));
        };
        write_frame(
            &mut self.stream,
            &proof_frame(&proof, self.sender.incarnation_id, device),
        )
        .await?;
        let answer = read_frame(&mut self.stream).await?.payload;
        match decide_auth(&answer) {
            AuthDecision::Accepted { connection_id } => {
                self.sender.connection_id = Some(connection_id);
                self.state.set_connection(connection_id);
                Ok(())
            }
            AuthDecision::Guidance { message } => Err(CliError::ServerOutcome(message)),
            AuthDecision::Unexpected { message } => Err(CliError::ServerRejected(message)),
        }
    }

    /// Returns the companion projection to echo on submits and history
    /// requests: the presence-learned one, or the bootstrap fallback until
    /// the first fact arrives (see
    /// [`SessionState::companion_ref`]).
    pub fn companion_ref(&self) -> String {
        self.state.companion_ref()
    }

    /// Sends one payload frame and reads the correlated answer, absorbing
    /// pipelined presence facts and deferring out-of-order frames on the way.
    ///
    /// The outgoing envelope carries a fresh command ID (one per send: the
    /// Host pairs its reply by `reply_to` against our message ID, and the
    /// command ID keeps every request uniformly pairable as command-side
    /// correlation grows). [`SubmitTextInput`](ene_api::v1::round::SubmitTextInput)
    /// sends carry the session's `observed.presence_generation_view`
    /// ([`None`] only before the first fact — the Host answers
    /// `NeedsRevalidation`, which is correct). A
    /// [`StaleRound`](ene_api::v1::round::RoundIntakeOutcomeWire::StaleRound)
    /// answer refreshes the session to its `current_generation` (normal
    /// operation, distinct from the handshake bootstrap).
    ///
    /// The read side first scans the deferred queue (the pure
    /// [`select_answer`] hit path): a queued frame whose `reply_to` matches
    /// returns without socket I/O. Otherwise it loops (the streaming form of
    /// [`select_answer`]): an
    /// authoritative
    /// [`PresenceAttribution`](ene_api::v1::payload::WirePayload::PresenceAttribution)
    /// fact refreshes the session generation (latest supersedes) and reading
    /// continues; a non-fact frame whose `reply_to` matches is the answer;
    /// any other non-fact frame is pushed to the deferred queue (cap
    /// [`DEFERRED_CAP`], oldest-drop) and reading continues — mismatches are
    /// never returned as answers and never silently dropped.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Transport`] or [`CliError::Codec`] when the
    /// exchange cannot be moved or framed. Payload semantics are the
    /// caller's job: this helper never interprets the answer beyond the
    /// generation bookkeeping above.
    pub async fn request(&mut self, payload: WirePayload) -> Result<WirePayload, CliError> {
        let mut frame = frame_for_session(payload, self.sender, self.state.generation());
        let _ = stamp_request(&mut frame);
        self.roundtrip(frame).await
    }

    /// Retries one logical send: the same command id travels (durable
    /// idempotency key on the Host), while message and request ids go fresh
    /// (transport pairing for this attempt only). Use after a lost reply,
    /// never to change what the command means — and only within one sender
    /// incarnation: the Host binds the key to its sender epoch, so a
    /// retry under a new incarnation is a conflict, not a replay. A new
    /// epoch mints a fresh command instead.
    ///
    /// # Errors
    ///
    /// Same as [`Client::request`].
    pub async fn retry(
        &mut self,
        payload: WirePayload,
        command: CommandWireId,
    ) -> Result<WirePayload, CliError> {
        self.roundtrip(retry_frame(
            payload,
            self.sender,
            self.state.generation(),
            command,
        ))
        .await
    }

    /// Moves one framed request and returns its paired answer, absorbing
    /// pipelined facts and deferring anything else.
    async fn roundtrip(&mut self, frame: WireFrame) -> Result<WirePayload, CliError> {
        let own_message_id = frame.envelope.message_id;
        write_frame(&mut self.stream, &frame).await?;
        if let Some(queued) = self.state.take_deferred_reply(own_message_id) {
            if let Some(current) = stale_generation_of(&queued) {
                self.state.note_stale_generation(current);
            }
            return Ok(queued);
        }
        loop {
            let incoming = read_frame(&mut self.stream).await?;
            // The arms mirror [`select_answer`]: facts absorb, the
            // `reply_to` match returns, and anything else defers with a cap
            // and continues reading.
            if let WirePayload::PresenceAttribution(fact) = &incoming.payload {
                self.state.observe_presence(fact);
            } else if incoming.envelope.correlation.reply_to == Some(own_message_id) {
                if let Some(current) = stale_generation_of(&incoming.payload) {
                    self.state.note_stale_generation(current);
                }
                return Ok(incoming.payload);
            } else {
                self.state.push_deferred(incoming);
            }
        }
    }

    /// Sends one observation frame with no reply expected (presentation
    /// confirmations: the Host applies them silently and answers nothing).
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Transport`] or [`CliError::Codec`] when the frame
    /// cannot be moved or encoded.
    pub async fn notify(&mut self, payload: WirePayload) -> Result<(), CliError> {
        write_frame(&mut self.stream, &frame_for(payload, self.sender)).await
    }

    /// Reads the next incoming frame payload (stream follower for `send`).
    ///
    /// An authoritative
    /// [`PresenceAttribution`](ene_api::v1::payload::WirePayload::PresenceAttribution)
    /// fact refreshes the session generation (latest supersedes) and is
    /// still returned, so the caller decides what to display.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Transport`] or [`CliError::Codec`] when the next
    /// frame cannot be read or decoded.
    pub async fn next_frame(&mut self) -> Result<WirePayload, CliError> {
        let payload = read_frame(&mut self.stream).await?.payload;
        if let WirePayload::PresenceAttribution(fact) = &payload {
            self.state.observe_presence(fact);
        }
        Ok(payload)
    }
}

/// Encodes `frame` and writes it as one length-prefixed unit.
#[cfg(unix)]
async fn write_frame(
    stream: &mut tokio::net::UnixStream,
    frame: &WireFrame,
) -> Result<(), CliError> {
    use tokio::io::AsyncWriteExt as _;
    let bytes = encode_frame(frame)
        .map_err(|error: CodecError| CliError::Codec(format!("encode failed: {error}")))?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|error| CliError::Transport(format!("socket write failed: {}", error.kind())))?;
    Ok(())
}

/// Reads one length-prefixed frame: 4-byte big-endian body length, then the
/// body. The cap is checked before any body-sized allocation, so a hostile
/// prefix cannot drive unbounded allocation.
#[cfg(unix)]
async fn read_frame(stream: &mut tokio::net::UnixStream) -> Result<WireFrame, CliError> {
    use tokio::io::AsyncReadExt as _;
    let mut prefix = [0_u8; 4];
    stream
        .read_exact(&mut prefix)
        .await
        .map_err(|error| CliError::Transport(format!("socket read failed: {}", error.kind())))?;
    let claimed = u32::from_be_bytes(prefix) as usize;
    if claimed > MAX_FRAME_BYTES {
        return Err(CliError::Codec(format!(
            "frame body of {claimed} bytes exceeds the 256 KiB cap"
        )));
    }
    let mut body = vec![0_u8; claimed];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|error| CliError::Transport(format!("socket read failed: {}", error.kind())))?;
    let mut bytes = Vec::with_capacity(4 + claimed);
    bytes.extend_from_slice(&prefix);
    bytes.extend_from_slice(&body);
    decode_frame(&bytes)
        .map(|(frame, _consumed)| frame)
        .map_err(|error: CodecError| CliError::Codec(format!("decode failed: {error}")))
}

/// Non-Unix placeholder: same surface, always unsupported.
#[cfg(windows)]
pub struct Client {
    /// Unconstructible: there is no socket to hold.
    _sealed: (),
}

#[cfg(windows)]
impl Client {
    /// Always reports unsupported: transport needs a Unix-domain socket.
    ///
    /// # Errors
    ///
    /// Always returns [`CliError::UnsupportedPlatform`].
    pub async fn connect(
        _data_dir: &Path,
        _descriptor: &str,
        _platform: &str,
    ) -> Result<Self, CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }

    /// Always reports unsupported: transport needs a Unix-domain socket.
    ///
    /// # Errors
    ///
    /// Always returns [`CliError::UnsupportedPlatform`].
    pub async fn request(&mut self, _payload: WirePayload) -> Result<WirePayload, CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }

    /// Always reports unsupported: transport needs a Unix-domain socket.
    ///
    /// # Errors
    ///
    /// Always returns [`CliError::UnsupportedPlatform`].
    pub async fn authenticate(
        &mut self,
        _challenge: &ene_api::v1::handshake::AuthChallenge,
    ) -> Result<(), CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }

    /// Always reports unsupported: transport needs a Unix-domain socket.
    ///
    /// # Errors
    ///
    /// Always returns [`CliError::UnsupportedPlatform`].
    pub async fn next_frame(&mut self) -> Result<WirePayload, CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }

    /// Always reports unsupported: transport needs a Unix-domain socket.
    ///
    /// # Errors
    ///
    /// Always returns [`CliError::UnsupportedPlatform`].
    pub async fn notify(&mut self, _payload: WirePayload) -> Result<(), CliError> {
        Err(CliError::UnsupportedPlatform("unix socket transport"))
    }
}

#[cfg(test)]
mod tests {
    //! Builder shapes, socket path, in-memory codec roundtrips, the pure
    //! request-correlation decision, auth builders and decisions, guidance
    //! text, and session secrecy.
    //!
    //! No sockets are opened, the environment is never mutated, and frames
    //! go through the in-memory codec or plain in-memory scripts only.

    use std::collections::VecDeque;

    use ene_api::v1::envelope::ProtocolVersion;
    use ene_api::v1::handshake::AuthResult;
    use ene_api::v1::payload::WirePayload;
    use ene_api::v1::presence::{PresenceAttributionWire, PresenceStateWire};
    use ene_api::v1::refs::{CompanionWireRef, ConnectionWireId, RoundWireId, WireMessageId};
    use ene_plugin_ipc::WireFrame;

    use super::{AuthDecision, ClientIncarnationId, DEFERRED_CAP, SessionState, WireSender};
    use super::{
        auth_rejected_guidance, capability_frame, decide_auth, frame_for, frame_for_session,
        message_type_for, missing_secret_guidance, new_incarnation, pairing_frame, payload_kind,
        pending_guidance, platform_display, presence_generation_of_fact, proof_frame, retry_frame,
        select_answer, socket_path, stale_generation_of, stamp_request,
    };
    use ene_api::v1::refs::CommandWireId;

    /// Fixed incarnation so built frames are deterministic.
    fn incarnation() -> ClientIncarnationId {
        ClientIncarnationId {
            counter: 0,
            random: 7,
        }
    }

    /// Yields `Ok` values without `unwrap`/`expect` (both denied): the
    /// `assert!` fails the test first, so the `else` branch is only a
    /// type-level fallback, never a silent pass.
    fn require_ok<T: core::fmt::Debug, E: core::fmt::Debug>(
        result: Result<T, E>,
        what: &str,
    ) -> Option<T> {
        assert!(result.is_ok(), "{what} unexpectedly failed: {result:?}");
        result.ok()
    }

    #[test]
    fn socket_path_appends_ene_sock() {
        let dir = std::path::Path::new("/tmp/ene-data");
        assert!(
            socket_path(dir) == dir.join("ene.sock"),
            "socket path must be ene.sock under the data dir"
        );
    }

    #[test]
    fn platform_display_names_os_and_arch() {
        let display = platform_display();
        assert!(
            display == format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            "platform display must name OS and arch, got {display:?}"
        );
    }

    #[test]
    fn pairing_frame_is_pre_pairing_v1() {
        let frame = pairing_frame("Owner laptop", incarnation());
        let WirePayload::PairingRequest(request) = &frame.payload else {
            return;
        };
        assert!(
            request.device_descriptor == "Owner laptop",
            "pairing keeps the display descriptor"
        );
        assert!(
            frame.envelope.protocol == ProtocolVersion::V1,
            "pairing speaks V1"
        );
        assert!(
            frame.envelope.sender.device_id.is_none(),
            "pre-pairing sender carries no device ID"
        );
        assert!(
            frame.envelope.message_type.0 == "PairingRequest",
            "pairing names its payload shape"
        );
    }

    #[test]
    fn capability_frame_claims_no_features_and_threads_device() {
        let sender_device = ene_api::v1::refs::DeviceWireId(uuid::Uuid::new_v4());
        let frame = capability_frame("linux-x86_64", incarnation(), Some(sender_device));
        let WirePayload::CapabilityAdvertise(advertise) = &frame.payload else {
            return;
        };
        assert!(
            advertise.supported_protocol == vec![ProtocolVersion::V1],
            "capability speaks V1"
        );
        assert!(
            advertise.features.is_empty(),
            "text is the baseline, not a claimed feature"
        );
        assert!(
            advertise.platform == "linux-x86_64",
            "capability carries the display platform"
        );
        assert!(
            frame.envelope.sender.device_id == Some(sender_device),
            "capability threads the paired device ID"
        );
        assert!(
            frame.envelope.message_type.0 == "CapabilityAdvertise",
            "capability names its payload shape"
        );
    }

    #[test]
    fn message_type_names_the_variant() {
        let sender = WireSender {
            device_id: None,
            incarnation_id: incarnation(),
            connection_id: None,
        };
        let frame = frame_for(
            WirePayload::HistoryRequest(crate::cmds::history_request("companion-1", 3)),
            sender,
        );
        assert!(
            message_type_for(&frame.payload).0 == "HistoryRequest",
            "discriminator must name the variant"
        );
        assert!(
            payload_kind(&frame.payload) == "HistoryRequest",
            "kind name must match the discriminator"
        );
        assert!(
            frame.envelope.protocol == ProtocolVersion::V1,
            "built frames speak V1"
        );
    }

    #[test]
    fn pairing_frame_survives_the_wire_codec() {
        let frame = pairing_frame("Owner laptop", incarnation());
        let Some(encoded) =
            require_ok(ene_plugin_ipc::encode_frame(&frame), "encode pairing frame")
        else {
            return;
        };
        let Some((decoded, consumed)) = require_ok(
            ene_plugin_ipc::decode_frame(&encoded),
            "decode pairing frame",
        ) else {
            return;
        };
        assert!(consumed == encoded.len(), "decode must consume the frame");
        assert!(decoded == frame, "codec must preserve the pairing frame");
    }

    #[test]
    fn capability_frame_survives_the_wire_codec() {
        let frame = capability_frame("linux-x86_64", incarnation(), None);
        let Some(encoded) = require_ok(
            ene_plugin_ipc::encode_frame(&frame),
            "encode capability frame",
        ) else {
            return;
        };
        let Some((decoded, _consumed)) = require_ok(
            ene_plugin_ipc::decode_frame(&encoded),
            "decode capability frame",
        ) else {
            return;
        };
        assert!(decoded == frame, "codec must preserve the capability frame");
    }

    /// Builds a presence fact carrying `generation`.
    fn presence_fact(generation: u64) -> PresenceAttributionWire {
        PresenceAttributionWire {
            companion: CompanionWireRef(String::from("default")),
            state: PresenceStateWire::Present,
            active_client: None,
            generation,
            move_reason: None,
        }
    }

    /// Builds a stale-round answer carrying `current_generation`.
    fn stale_answer(current_generation: u64) -> WirePayload {
        WirePayload::RoundIntakeOutcome(ene_api::v1::round::RoundIntakeOutcomeWire::StaleRound {
            current_round: None,
            current_generation,
        })
    }

    #[test]
    fn session_starts_unobserved_and_tracks_latest() {
        let mut session = SessionState::new();
        assert!(
            session.generation().is_none(),
            "a new session observed nothing yet"
        );
        session.observe_presence(&presence_fact(4));
        assert!(
            session.generation() == Some(4),
            "the fact generation becomes current"
        );
        session.observe_presence(&presence_fact(7));
        assert!(
            session.generation() == Some(7),
            "a newer fact supersedes: {:?}",
            session.generation()
        );
        session.note_stale_generation(9);
        assert!(
            session.generation() == Some(9),
            "a stale answer refreshes the running session"
        );
    }

    #[test]
    fn presence_generation_of_fact_reads_the_fact() {
        assert!(
            presence_generation_of_fact(&presence_fact(12)) == 12,
            "the fact always carries its generation"
        );
    }

    #[test]
    fn stale_generation_of_reads_only_stale_answers() {
        assert!(
            stale_generation_of(&stale_answer(21)) == Some(21),
            "a stale answer yields its current generation"
        );
        let accepted = WirePayload::RoundIntakeOutcome(
            ene_api::v1::round::RoundIntakeOutcomeWire::AcceptedForRound {
                round: RoundWireId(String::from("round-1")),
            },
        );
        assert!(
            stale_generation_of(&accepted).is_none(),
            "a non-stale answer yields nothing"
        );
        let history = WirePayload::HistoryRequest(crate::cmds::history_request("companion-1", 1));
        assert!(
            stale_generation_of(&history).is_none(),
            "an unrelated payload yields nothing"
        );
    }

    #[test]
    fn session_frames_stamp_only_text_inputs() {
        let sender = WireSender {
            device_id: None,
            incarnation_id: incarnation(),
            connection_id: None,
        };
        let input = WirePayload::SubmitTextInput(crate::cmds::submit_input(
            "companion-1",
            None,
            String::from("hello"),
            String::from("en"),
        ));
        let stamped = frame_for_session(input, sender, Some(6));
        assert!(
            stamped.envelope.observed.presence_generation_view == Some(6),
            "text input carries the session generation"
        );
        let bootstrap = frame_for_session(
            WirePayload::SubmitTextInput(crate::cmds::submit_input(
                "companion-1",
                None,
                String::from("hello"),
                String::from("en"),
            )),
            sender,
            None,
        );
        assert!(
            bootstrap
                .envelope
                .observed
                .presence_generation_view
                .is_none(),
            "pre-fact bootstrap stamps None (NeedsRevalidation is correct)"
        );
        let history = frame_for_session(
            WirePayload::HistoryRequest(crate::cmds::history_request("companion-1", 1)),
            sender,
            Some(6),
        );
        assert!(
            history.envelope.observed.presence_generation_view.is_none(),
            "non-input payloads keep the None default"
        );
    }

    #[test]
    fn first_run_capability_carries_no_device() {
        let frame = capability_frame("linux-x86_64", incarnation(), None);
        assert!(
            frame.envelope.sender.device_id.is_none(),
            "first-run capability advertises with no device yet"
        );
    }

    #[test]
    fn incarnation_names_this_process_and_advances() {
        let first = new_incarnation();
        let second = new_incarnation();
        assert!(
            first.counter == u64::from(std::process::id()),
            "incarnation counter is this process pid: {first:?}"
        );
        assert!(
            first.random != second.random,
            "successive incarnations differ: {first:?} vs {second:?}"
        );
    }

    /// Builds a script frame with a controlled message ID and reply link.
    fn script_frame(
        payload: WirePayload,
        message_id: WireMessageId,
        reply_to: Option<WireMessageId>,
    ) -> WireFrame {
        let mut frame = frame_for(
            payload,
            WireSender {
                device_id: None,
                incarnation_id: incarnation(),
                connection_id: None,
            },
        );
        frame.envelope.message_id = message_id;
        frame.envelope.correlation.reply_to = reply_to;
        frame
    }

    /// Builds a deterministic message ID from one integer.
    fn message_id(value: u128) -> WireMessageId {
        WireMessageId(uuid::Uuid::from_u128(value))
    }

    /// Builds an answer payload (the kind never matters to selection).
    fn answer_payload() -> WirePayload {
        WirePayload::HistoryRequest(crate::cmds::history_request("companion-1", 1))
    }

    #[test]
    fn request_stamps_a_fresh_command_id_per_send() {
        let sender = WireSender {
            device_id: None,
            incarnation_id: incarnation(),
            connection_id: None,
        };
        let mut first = frame_for(answer_payload(), sender);
        let mut second = frame_for(answer_payload(), sender);
        assert!(
            first.envelope.correlation.command_id.is_none(),
            "builders stamp no command ID by themselves"
        );
        let first_id = stamp_request(&mut first);
        stamp_request(&mut second);
        assert!(
            first_id == first.envelope.message_id,
            "the stamp reports the echoed message ID"
        );
        let (Some(first_command), Some(second_command)) = (
            first.envelope.correlation.command_id,
            second.envelope.correlation.command_id,
        ) else {
            return;
        };
        assert!(
            first_command != second_command,
            "every send mints a fresh command ID: {first_command:?} vs {second_command:?}"
        );
        assert!(
            first.envelope.correlation.request_id.is_some(),
            "every send carries a request ID for pairing"
        );
    }

    #[test]
    fn session_echoes_the_learned_companion_projection() {
        use super::SessionState;

        let mut state = SessionState::new();
        assert_eq!(
            state.companion_ref(),
            String::from(crate::cmds::DEFAULT_COMPANION_REF),
            "bootstrap echoes the fallback until the first fact"
        );
        let mut fact = presence_fact(3);
        fact.companion = CompanionWireRef(String::from("host-issued-projection"));
        state.observe_presence(&fact);
        assert_eq!(
            state.companion_ref(),
            String::from("host-issued-projection"),
            "after presence the session echoes the learned projection"
        );
        assert_eq!(
            state.generation(),
            Some(3),
            "generation bookkeeping is untouched"
        );
    }

    #[test]
    fn retry_frame_reuses_command_with_fresh_transport_ids() {
        let sender = WireSender {
            device_id: None,
            incarnation_id: incarnation(),
            connection_id: None,
        };
        let command = CommandWireId(uuid::Uuid::new_v4());
        let input = || {
            WirePayload::SubmitTextInput(crate::cmds::submit_input(
                "companion-1",
                None,
                String::from("hi"),
                String::from("en"),
            ))
        };
        let first = retry_frame(input(), sender, Some(3), command);
        let second = retry_frame(input(), sender, Some(3), command);
        assert_eq!(
            first.envelope.correlation.command_id,
            Some(command),
            "retry reuses the logical command ID"
        );
        assert_eq!(
            second.envelope.correlation.command_id,
            Some(command),
            "retry reuses the logical command ID"
        );
        assert!(
            first.envelope.message_id != second.envelope.message_id,
            "retries pair transport-fresh"
        );
        assert!(
            first.envelope.correlation.request_id.is_some(),
            "retries carry request IDs"
        );
        assert_eq!(
            first.envelope.observed.presence_generation_view,
            second.envelope.observed.presence_generation_view,
            "retries preserve the observed premise"
        );
    }

    #[test]
    fn select_answer_returns_a_lone_correlated_answer() {
        let own = message_id(1);
        let frames = [script_frame(answer_payload(), message_id(2), Some(own))];
        let (absorbed, answer, deferred) = select_answer(own, &VecDeque::new(), &frames);
        assert!(
            absorbed.is_empty(),
            "no fact means no absorption: {absorbed:?}"
        );
        assert!(
            answer == Some(answer_payload()),
            "the correlated frame is the answer, got {answer:?}"
        );
        assert!(
            deferred.is_empty(),
            "a direct hit queues nothing: {deferred:?}"
        );
    }

    #[test]
    fn select_answer_absorbs_facts_then_answers() {
        let own = message_id(7);
        let frames = [
            script_frame(
                WirePayload::PresenceAttribution(presence_fact(3)),
                message_id(8),
                Some(own),
            ),
            script_frame(
                WirePayload::PresenceAttribution(presence_fact(5)),
                message_id(9),
                Some(own),
            ),
            script_frame(answer_payload(), message_id(10), Some(own)),
        ];
        let (absorbed, answer, deferred) = select_answer(own, &VecDeque::new(), &frames);
        assert!(
            absorbed
                .iter()
                .map(super::presence_generation_of_fact)
                .collect::<Vec<u64>>()
                == vec![3, 5],
            "pipelined facts absorb in order, got {absorbed:?}"
        );
        assert!(
            answer == Some(answer_payload()),
            "the correlated non-fact ends the wait, got {answer:?}"
        );
        assert!(
            deferred.is_empty(),
            "facts and the hit queue nothing: {deferred:?}"
        );
    }

    #[test]
    fn select_answer_without_an_answer_absorbs_only() {
        let own = message_id(11);
        let frames = [
            script_frame(
                WirePayload::PresenceAttribution(presence_fact(2)),
                message_id(12),
                Some(own),
            ),
            script_frame(
                WirePayload::PresenceAttribution(presence_fact(4)),
                message_id(13),
                None,
            ),
        ];
        let (absorbed, answer, deferred) = select_answer(own, &VecDeque::new(), &frames);
        assert!(
            absorbed.len() == 2,
            "facts absorb even without a reply link, got {absorbed:?}"
        );
        assert!(
            answer.is_none(),
            "facts alone are never an answer: {answer:?}"
        );
        assert!(
            deferred.is_empty(),
            "facts alone queue nothing: {deferred:?}"
        );
    }

    #[test]
    fn select_answer_defers_mismatches_instead_of_answering() {
        let own = message_id(21);
        let other = message_id(22);
        for frames in [
            [script_frame(answer_payload(), message_id(23), Some(other))].as_slice(),
            [script_frame(answer_payload(), message_id(24), None)].as_slice(),
        ] {
            let (absorbed, answer, deferred) = select_answer(own, &VecDeque::new(), frames);
            assert!(
                absorbed.is_empty(),
                "a non-fact absorbs nothing: {absorbed:?}"
            );
            assert!(
                answer.is_none(),
                "an uncorrelated frame is never the answer, got {answer:?}"
            );
            assert!(
                deferred.len() == 1,
                "the mismatch is deferred, not dropped: {deferred:?}"
            );
        }
    }

    #[test]
    fn select_answer_defers_a_mismatch_then_answers() {
        let own = message_id(31);
        let other = message_id(32);
        let frames = [
            script_frame(answer_payload(), message_id(33), Some(other)),
            script_frame(answer_payload(), message_id(34), Some(own)),
        ];
        let (absorbed, answer, deferred) = select_answer(own, &VecDeque::new(), &frames);
        assert!(
            absorbed.is_empty(),
            "no fact means no absorption: {absorbed:?}"
        );
        assert!(
            answer == Some(answer_payload()),
            "the correlated frame answers after the mismatch, got {answer:?}"
        );
        assert!(
            deferred.len() == 1,
            "the mismatch stays deferred: {deferred:?}"
        );
        assert!(
            deferred[0].envelope.correlation.reply_to == Some(other),
            "the deferred frame is the mismatch: {deferred:?}"
        );
    }

    /// Builds a history answer carrying `limit`, so out-of-order answers
    /// stay distinguishable by payload.
    fn history_answer(limit: u64) -> WirePayload {
        WirePayload::HistoryRequest(crate::cmds::history_request("companion-1", limit))
    }

    #[test]
    fn select_answer_serves_the_second_request_from_the_queue() {
        let first = message_id(41);
        let second = message_id(42);
        let script = [
            script_frame(history_answer(2), message_id(43), Some(second)),
            script_frame(history_answer(1), message_id(44), Some(first)),
        ];
        let (absorbed, answer, deferred) = select_answer(first, &VecDeque::new(), &script);
        assert!(
            absorbed.is_empty(),
            "no fact means no absorption: {absorbed:?}"
        );
        assert!(
            answer == Some(history_answer(1)),
            "the first request takes its own reply: {answer:?}"
        );
        assert!(
            deferred.len() == 1,
            "the future answer stays queued: {deferred:?}"
        );
        let (absorbed_next, queued, deferred_next) = select_answer(second, &deferred, &[]);
        assert!(
            absorbed_next.is_empty(),
            "a queue hit absorbs nothing: {absorbed_next:?}"
        );
        assert!(
            queued == Some(history_answer(2)),
            "the second request finds its answer already queued: {queued:?}"
        );
        assert!(
            deferred_next.is_empty(),
            "the hit removes the queued frame: {deferred_next:?}"
        );
    }

    #[test]
    fn select_answer_prefers_the_queue_over_new_frames() {
        let own = message_id(51);
        let queued_frame = script_frame(history_answer(9), message_id(52), Some(own));
        let queued: VecDeque<WireFrame> = [queued_frame].into_iter().collect();
        let fresh = [script_frame(history_answer(8), message_id(53), Some(own))];
        let (absorbed, answer, deferred) = select_answer(own, &queued, &fresh);
        assert!(
            absorbed.is_empty(),
            "a queue hit absorbs nothing: {absorbed:?}"
        );
        assert!(
            answer == Some(history_answer(9)),
            "the queued answer wins without socket I/O: {answer:?}"
        );
        assert!(
            deferred.is_empty(),
            "the hit drains the queue and ignores fresh frames: {deferred:?}"
        );
    }

    #[test]
    fn select_answer_bounds_the_queue_oldest_drop() {
        let own = message_id(61);
        let mut queued: VecDeque<WireFrame> = VecDeque::new();
        for index in 0..DEFERRED_CAP {
            let id = u128::try_from(index).map_or(0, |value| value + 100);
            queued.push_back(script_frame(history_answer(7), message_id(id), None));
        }
        assert!(
            queued.len() == DEFERRED_CAP,
            "the fixture queue starts full: {queued:?}"
        );
        let overflow = [script_frame(
            history_answer(7),
            message_id(999),
            Some(message_id(998)),
        )];
        let (_, answer, deferred) = select_answer(own, &queued, &overflow);
        assert!(
            answer.is_none(),
            "a lone mismatch never answers: {answer:?}"
        );
        assert!(
            deferred.len() == DEFERRED_CAP,
            "the queue stays bounded: {deferred:?}"
        );
        assert!(
            deferred[0].envelope.message_id != queued[0].envelope.message_id,
            "the oldest frame drops first"
        );
    }

    #[test]
    fn decide_auth_accepts_connection_keys() {
        let connection = ConnectionWireId(uuid::Uuid::from_u128(0xaa));
        let decision = decide_auth(&WirePayload::AuthResult(AuthResult::Accepted {
            connection_id: connection,
        }));
        assert!(
            decision
                == AuthDecision::Accepted {
                    connection_id: connection,
                },
            "acceptance must carry the connection key, got {decision:?}"
        );
    }

    #[test]
    fn decide_auth_rejection_guides_reprovisioning() {
        let decision = decide_auth(&WirePayload::AuthResult(AuthResult::Rejected {
            reason: String::from("unknown proof"),
        }));
        let AuthDecision::Guidance { message } = decision else {
            return;
        };
        assert!(
            message.contains("unknown proof"),
            "guidance keeps the operational Host reason: {message:?}"
        );
        assert!(
            message.contains(crate::device::BOOTSTRAP_SECRET_ENV),
            "guidance names the provisioning step: {message:?}"
        );
    }

    #[test]
    fn decide_auth_names_unexpected_kinds() {
        let decision = decide_auth(&answer_payload());
        let AuthDecision::Unexpected { message } = decision else {
            return;
        };
        assert!(
            message.contains("HistoryRequest") && message.contains("AuthResult"),
            "the refusal must name both kinds: {message:?}"
        );
    }

    #[test]
    fn proof_frame_names_the_paired_device() {
        use ene_api::v1::refs::DeviceWireId;
        let device = DeviceWireId(uuid::Uuid::new_v4());
        let frame = proof_frame("proof-hex-abc", incarnation(), device);
        let WirePayload::AuthProof(proof) = &frame.payload else {
            return;
        };
        assert!(
            proof.proof == "proof-hex-abc",
            "the proof value travels in the auth frame"
        );
        assert!(
            frame.envelope.sender.device_id == Some(device)
                && frame.envelope.sender.connection_id.is_none()
                && frame.envelope.sender.incarnation_id == incarnation(),
            "the proof names the paired device but no connection: {:?}",
            frame.envelope.sender
        );
        let rendered = format!("{frame:?}");
        assert!(
            !rendered.contains("proof-hex-abc"),
            "frame Debug must not leak the proof: {rendered:?}"
        );
        let Some(encoded) = require_ok(ene_plugin_ipc::encode_frame(&frame), "encode proof frame")
        else {
            return;
        };
        let Some((decoded, _consumed)) =
            require_ok(ene_plugin_ipc::decode_frame(&encoded), "decode proof frame")
        else {
            return;
        };
        assert!(decoded == frame, "codec must preserve the proof frame");
    }

    #[test]
    fn proof_derives_from_the_secret_and_the_single_use_nonce() {
        use ene_api::v1::refs::DeviceWireId;
        let proof = ene_credential::pairing_proof_hex("pairing-secret", "nonce-1");
        let frame = proof_frame(&proof, incarnation(), DeviceWireId(uuid::Uuid::new_v4()));
        let WirePayload::AuthProof(carried) = &frame.payload else {
            return;
        };
        assert!(
            ene_credential::verify_pairing_proof("pairing-secret", "nonce-1", &carried.proof),
            "the carried proof must verify against the secret and nonce"
        );
        assert!(
            !ene_credential::verify_pairing_proof("pairing-secret", "nonce-2", &carried.proof),
            "the proof must not verify against another nonce (single-use)"
        );
    }

    #[test]
    fn guidance_names_provisioning_without_secrets() {
        let pending = pending_guidance();
        assert!(
            pending.contains("pending owner confirmation")
                && pending.contains(crate::device::BOOTSTRAP_SECRET_ENV),
            "pending guidance must name the approval plus the provisioning step: {pending:?}"
        );
        let missing = missing_secret_guidance();
        assert!(
            missing.contains("no pairing secret") && missing.contains("approve"),
            "missing-secret guidance must direct approval and provisioning: {missing:?}"
        );
        let rejected = auth_rejected_guidance("unknown proof");
        assert!(
            rejected.contains("unknown proof"),
            "rejection guidance keeps the Host reason: {rejected:?}"
        );
    }

    #[test]
    fn session_tracks_connection_and_holds_secret() {
        let mut session = SessionState::new();
        assert!(
            session.connection_id().is_none() && session.pairing_secret().is_none(),
            "a new session is unauthenticated and unprovisioned: {session:?}"
        );
        let connection = ConnectionWireId(uuid::Uuid::from_u128(0xbb));
        session.set_connection(connection);
        session.set_pairing_secret(String::from("secret-hex"));
        assert!(
            session.connection_id() == Some(connection),
            "the accepted connection key is stored"
        );
        assert!(
            session.pairing_secret() == Some("secret-hex"),
            "the provisioned secret is held for the session"
        );
    }

    #[test]
    fn session_debug_redacts_the_secret() {
        let mut session = SessionState::new();
        session.set_pairing_secret(String::from("secret-hex-marker-9d4e"));
        let rendered = format!("{session:?}");
        assert!(
            !rendered.contains("secret-hex-marker-9d4e"),
            "session Debug must not leak the secret: {rendered:?}"
        );
        assert!(
            rendered.contains("[redacted]"),
            "session Debug must mark the redaction: {rendered:?}"
        );
    }

    #[test]
    fn session_debug_reports_the_queue_length_without_bodies() {
        let mut session = SessionState::new();
        session.push_deferred(script_frame(history_answer(3), message_id(71), None));
        let rendered = format!("{session:?}");
        assert!(
            rendered.contains("deferred_len"),
            "session Debug must name the queue length: {rendered:?}"
        );
        assert!(
            !rendered.contains("HistoryRequest"),
            "session Debug must not dump queued payloads: {rendered:?}"
        );
    }

    #[test]
    fn session_deferred_queue_takes_only_the_matching_reply() {
        let mut session = SessionState::new();
        assert!(
            session.deferred_len() == 0,
            "a new session defers nothing: {session:?}"
        );
        let first = message_id(81);
        let second = message_id(82);
        session.push_deferred(script_frame(
            history_answer(2),
            message_id(83),
            Some(second),
        ));
        session.push_deferred(script_frame(history_answer(1), message_id(84), Some(first)));
        assert!(
            session.deferred_len() == 2,
            "both mismatches queue: {session:?}"
        );
        assert!(
            session.take_deferred_reply(first) == Some(history_answer(1)),
            "the take finds the matching reply out of order"
        );
        assert!(
            session.deferred_len() == 1,
            "the hit removes only its frame: {session:?}"
        );
        assert!(
            session.take_deferred_reply(message_id(85)).is_none(),
            "an unknown reply finds nothing"
        );
        assert!(
            session.take_deferred_reply(second) == Some(history_answer(2)),
            "the remaining reply is still queued"
        );
        assert!(session.deferred_len() == 0, "the queue drains: {session:?}");
    }

    #[test]
    fn session_deferred_queue_drops_oldest_at_the_cap() {
        let mut session = SessionState::new();
        for index in 0..DEFERRED_CAP + 2 {
            let id = u128::try_from(index).map_or(0, |value| value + 200);
            session.push_deferred(script_frame(history_answer(4), message_id(id), None));
        }
        assert!(
            session.deferred_len() == DEFERRED_CAP,
            "the queue stays bounded at the cap: {session:?}"
        );
    }
}
