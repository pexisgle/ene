//! Host transport: socket path, frame builders, handshake, request/response.
//!
//! The CLI dials the Host over a Unix-domain socket at [`socket_path`]
//! (`ene.sock` inside the resolved data directory). A run without a stored
//! device pairs first while retaining its original connection, then
//! advertises capability; a run with a stored device skips pairing and
//! resolves its DeviceWireId at capability time, never by descriptor (#1389).
//! Device identity and secret provisioning live in the `device` module.
//!
//! A first run sends [`ene_api::v1::handshake::PairingRequest`]
//! (display descriptor, pre-pairing sender with no device ID), which must
//! answer with a pending identity and later deliver
//! [`PairingProvision`](ene_api::v1::handshake::PairingProvision) on that same
//! connection before the client continues.
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
//! the Host sends a challenge, this side answers with `frames::proof_frame` (the
//! proof names the paired device, never the connection), and the pure
//! `session::decide_auth` step classifies the answer while the accepted
//! connection key is stored into the sender for all later frames. The trailing
//! presence fact is consumed as the
//! session's first attribution before returning.
//!
//! Identity: pure requests ([`ene_api::v1::management::ManagementViewRequest`],
//! [`ene_api::v1::round::HistoryRequest`]) carry only a fresh `request_id`;
//! command payloads carry exactly one `command_id`
//! ([`ene_api::v1::round::SubmitTextInput`] mints one, a
//! [`ManagementIntent`](ene_api::v1::management::ManagementIntent) keeps its
//! `intent_id`). [`Client::prepare`] retains that command identity caller-side
//! so a lost reply is re-sent through [`Client::execute`] as the same command
//! with fresh message/request ids; [`Client::request`] is the one-shot
//! convenience that does not expose the identity.
//!
//! Request/response correlation: every send matches the answer by transport
//! pairing (`reply_to` against our message ID). A single read per request is
//! wrong because the Host pipelines unsolicited facts ahead of answers (the
//! accepted-connection answer appends the current presence fact, then any
//! absence backlog), so `request` loops: an incoming frame whose `reply_to`
//! matches is the answer and returns without further I/O; presence facts are
//! absorbed into the session state and reading continues; any
//! other non-fact frame is pushed to the deferred queue (cap
//! `session::DEFERRED_CAP`, oldest-drop), which only buffers
//! auto-presented summaries for the session's `take_undelivered`, and reading
//! continues — mismatches are never returned as answers, and auto-presented
//! summaries are never silently dropped from the deferred queue.
//! `session::decide_frame` is the pure per-frame step of that loop;
//! the deferred queue holds the rest.
//!
//! A pairing first answers
//! [`PendingOwnerConfirmation`](ene_api::v1::handshake::PairingResult::PendingOwnerConfirmation)
//! with the opaque pending ID: the operator approves that ID on the
//! Host-local trusted surface while the Client retains the connection. The
//! Host then provisions that connection and the Client persists only after
//! authentication succeeds. A denied pairing exits 2 with the Host reason.
//! A stored device the Host no longer knows is refused at capability
//! time: the Host answers a `DisconnectNotice` instead of the negotiated
//! terms, surfaced as an unexpected-frame `ServerRejected`. A peer with no
//! protocol major in common gets the typed terminal
//! [`IncompatibleProtocol`](ene_api::v1::reject::IncompatibleProtocol) naming
//! both maxima and the upgrade hint, also surfaced as `ServerRejected` (no
//! retry can intersect majors). Only a device that resolves but cannot prove
//! its secret fails later at authentication with `AuthResult::Rejected`.
//!
//! Framing goes through `ene-plugin-ipc` only ([`ene_plugin_ipc::encode_frame`]/[`ene_plugin_ipc::decode_frame`]); this module
//! owns the socket read/write loops. [`ene_plugin_ipc::CodecError`]
//! displays carry lengths and decoder reasons only and never echo frame
//! bytes, so mapping them into [`crate::error::ClientError::Codec`]
//! cannot leak conversation text. All other error messages carry operations,
//! payload-kind names, refs, or generations — never bodies or secrets.
//!
//! Platforms without a supported transport get stubs returning
//! [`crate::error::ClientError::UnsupportedPlatform`];
//! the pure builders below stay shared.

use std::path::{Path, PathBuf};

pub(crate) mod device;
pub mod error;
pub(crate) mod frames;
pub(crate) mod incarnation;
mod pairing;
pub(crate) mod session;
mod transport;

pub use error::ClientError;
pub use frames::PreparedRequest;
pub use transport::{Client, ConnectProgress, PendingPairingClient};

pub const DEFAULT_COMPANION_REF: &str = "default";

pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ene.sock")
}

pub fn platform_display() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests;
