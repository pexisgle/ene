//! Host composition root library surface (`Stage 2`).
//!
//! This library is the testable seam of the `ene-core` Host: [`serve::HostHandle`]
//! owns the durable [`ene_store::Store`], the [`ene_permission::EvaluationTracker`],
//! and the per-process Host maps (open rounds, issued round refs), while
//! [`serve::HostHandle::handle_frame`]
//! runs the full transport-free orchestration pipeline over
//! [`ene_plugin_ipc::WireFrame`] values. Pairing requests and input replay
//! keys are durable in the store; presence clients map deterministically from
//! paired device strings.
//!
//! Module layout:
//!
//! - [`serve`] holds [`serve::CoreError`], [`serve::CredStore`], [`serve::LiveInput`],
//!   [`serve::HostHandle`], the frame dispatch, the pairing/capability handshake, and
//!   the [`serve::serve`] entry point.
//! - [`dialogue`] holds the one-to-one text round trip: intake, history appends,
//!   authorization, inference dispatch, streaming frames, presentation observations,
//!   and timeline restore.
//! - [`setup`] holds the `Stage 2` setup management inlet: credential registration,
//!   consent assignment, setup completion, and filtered views.
//! - [`conn`] holds the Unix socket listener. The wire close convention is shared:
//!   a [`ene_api::v1::handshake::DisconnectNotice`] in the response vector is
//!   terminal and the connection closes after it is written.
//!
//! `message_type` convention (`Stage 2`, Host-side): outgoing envelopes name the
//! [`ene_api::v1::payload::WirePayload`] variant (`"PairingResult"`,
//! `"NegotiatedConnection"`, `"AuthChallenge"`, `"AuthResult"`,
//! `"PresenceAttribution"`, `"RoundIntakeOutcome"`, `"TextStreamOpen"`,
//! `"TextStreamFrame"`, `"TextStreamClose"`, `"HistoryView"`, `"ManagementOutcome"`,
//! `"ManagementView"`, `"DisconnectNotice"`). The envelope value is a routing hint
//! only; the `MessagePack` body already carries the same variant name through its
//! externally-tagged encoding, so the two can never disagree silently.

pub mod conn;
pub mod dialogue;
pub mod serve;
pub mod setup;

#[cfg(test)]
pub(crate) mod test_support;
