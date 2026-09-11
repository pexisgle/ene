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
//! - [`serve`] holds the frame dispatch, pairing/capability handshake, and
//!   [`serve::serve`] entry point.
//! - [`dialogue`] holds the one-to-one text round trip.
//! - [`setup`] holds the `Stage 2` setup management inlet.
//! - [`conn`] holds the Unix socket listener. The wire close convention is shared:
//!   a [`ene_api::v1::handshake::DisconnectNotice`] in the response vector is
//!   terminal and the connection closes after it is written.
//!
//! `message_type` convention (`Stage 2`, Host-side): outgoing envelopes name the
//! [`ene_api::v1::payload::WirePayload`] variant. The envelope value is a routing
//! hint only; the `MessagePack` body already carries the same variant name through
//! its externally-tagged encoding, so the two can never disagree silently.

pub mod conn;
pub mod dialogue;
pub mod serve;
pub mod setup;

#[cfg(test)]
pub(crate) mod test_support;

use std::sync::{Mutex as StdMutex, MutexGuard};

/// Locks a `std` mutex, recovering from poisoning.
///
/// Poisoning only follows a panic inside a critical section; sections here
/// run plain map and queue operations that never panic while holding the
/// guard, so recovery preserves the committed state.
pub(crate) fn lock_unpoison<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
