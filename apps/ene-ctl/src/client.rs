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
//! A first run sends [`ene_api::v1::handshake::PairingRequest`]
//! (display descriptor, pre-pairing sender with no device ID), which must
//! answer [`Paired`](ene_api::v1::handshake::PairingResult::Paired) before
//! the client continues in the same session.
//! Then [`ene_api::v1::handshake::CapabilityAdvertise`]
//! must answer
//! [`NegotiatedConnection`](ene_api::v1::handshake::NegotiatedConnection)
//! with a matching major version. The capability frame names the paired
//! device ID (paired-sender contract); the Host attributes it through the
//! connection table (which recorded the paired device when this same
//! connection paired moments earlier) and never trusts the claim —
//! a mismatched claim drops the frame.
//!
//! Authentication ([`ene_api::v1::handshake::AuthChallenge`] /
//! [`ene_api::v1::handshake::AuthProof`] /
//! [`ene_api::v1::handshake::AuthResult`]) runs inside `connect`: after the negotiated terms arrive,
//! the Host sends a challenge, this side answers with [`frames::proof_frame`] (the
//! proof names the paired device, never the connection), and
//! [`session::decide_auth`] plus [`Client::authenticate`] store the accepted
//! connection key into the sender for all later frames plus into the
//! [`session::SessionState`] mirror. The trailing presence fact is consumed as the
//! session's first attribution before returning.
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
//! are absorbed into the [`session::SessionState`] and reading continues; any other
//! non-fact frame is pushed to the deferred queue (cap [`session::DEFERRED_CAP`],
//! oldest-drop) and reading continues — mismatches are never returned as
//! answers and never silently dropped. The pure
//! [`session::select_answer`] holds that decision over a deferred queue plus a frame
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
//! Presence generation (see [`session::SessionState`]): the client keeps the latest
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
//! Framing goes through `ene-plugin-ipc` only ([`ene_plugin_ipc::encode_frame`]/[`ene_plugin_ipc::decode_frame`]); this module
//! owns the socket read/write loops. [`ene_plugin_ipc::CodecError`]
//! displays carry lengths and decoder reasons only and never echo frame
//! bytes, so mapping them into [`crate::errors::CliError::Codec`]
//! cannot leak conversation text. All other error messages carry operations,
//! payload-kind names, refs, or generations — never bodies or secrets.
//!
//! Non-Unix platforms get stubs returning
//! [`crate::errors::CliError::UnsupportedPlatform`];
//! the pure builders below stay shared.

use std::path::{Path, PathBuf};

pub mod frames;
pub mod session;
mod transport;

pub use frames::payload_kind;
pub use transport::Client;

/// Returns the Host socket path for `data_dir`: `<data_dir>/ene.sock`.
///
/// Pure and side-effect free; the caller decides whether the directory or
/// socket must exist (absence surfaces as [`crate::errors::CliError::Transport`] on dial).
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ene.sock")
}

/// Display-only platform string for pairing and capability frames, from the
/// compile-time OS and architecture (for example `"linux-x86_64"`). Display
/// only, never permission evidence.
pub fn platform_display() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests;
