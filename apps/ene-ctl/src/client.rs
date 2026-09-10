//! Host transport: socket path, frame builders, handshake, request/response.
//!
//! The CLI dials the Host over a Unix-domain socket at [`socket_path`]
//! (`ene.sock` inside the resolved data directory). Every connect runs pairing
//! first, then capability advertisement: already-paired descriptors re-pair
//! idempotently to the same device key. Device identity and secret
//! provisioning live in [`crate::device`].
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
//! pairing (`reply_to` against our message ID). A single read per request is
//! wrong because the Host pipelines unsolicited facts ahead of answers
//! (capability appends the current presence fact right after the negotiated
//! terms), so `request` consults the deferred queue first and then loops: a
//! queued or incoming frame whose `reply_to` matches is the answer and
//! returns without further I/O; presence facts are absorbed into the
//! [`session::SessionState`] and reading continues; any other non-fact frame
//! is pushed to the deferred queue (cap [`session::DEFERRED_CAP`],
//! oldest-drop) and reading continues — mismatches are never returned as
//! answers and never silently dropped. The pure [`session::select_answer`]
//! holds that decision over a deferred queue plus a frame script; the socket
//! loop is its streaming form.
//!
//! A still-pending pairing answers
//! [`PendingOwnerConfirmation`](ene_api::v1::handshake::PairingResult::PendingOwnerConfirmation):
//! the operator approves the device on the Host-local trusted surface, re-runs
//! the client once with the shown secret in the environment so it reaches the
//! `0600` device file, and later runs read the file. A denied pairing exits 2
//! with the Host reason plus that guidance. A stored device the Host no longer
//! knows fails later at the domain gate (unknown sender: close plus
//! `DisconnectNotice`), never with a dedicated capability-time outcome.
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

/// Pure: the caller decides whether the directory or socket must exist;
/// absence surfaces as [`crate::errors::CliError::Transport`] on dial.
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ene.sock")
}

/// Compile-time OS/arch string (for example `"linux-x86_64"`), display only —
/// never permission evidence.
pub fn platform_display() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests;
