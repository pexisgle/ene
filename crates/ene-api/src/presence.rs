//! Host-to-Client presence facts.
//!
//! Presence flows Host-to-Client only: [`crate::presence::PresenceAttribution`]
//! states where a
//! companion currently stands, and each newer fact supersedes the older ones.
//! The Client never asserts presence; it observes the latest attribution and
//! reports the marks it saw back in the envelope.
//!
//! There is no move-intent message in Stage 1: with a single Client there is
//! nothing to negotiate between, so movement intent stays Host-local.

use serde::{Deserialize, Serialize};

/// Where a companion stands, as decided by the Host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresenceState {
    /// Attributed to an active Client.
    Present,
    /// Running with no active Client; waiting without moving on its own.
    NoActive,
    /// Mid-move between Clients; intake may be held (see round outcomes).
    InTransition,
    /// Stopped; fallback and recovery do not apply.
    Stopped,
    /// Waiting for recovery after a Host restart; resumes only on explicit
    /// Owner re-invitation, never automatically.
    RecoveryWait,
}

/// Host-to-Client fact attributing one companion to its current presence.
///
/// The latest attribution supersedes all earlier ones. The companion and
/// client references are opaque Host-minted strings: the Client echoes them
/// and never parses, synthesizes, or stores them as keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceAttribution {
    /// Opaque companion reference attributed; echo-only.
    pub companion: String,
    /// Where the companion stands.
    pub state: PresenceState,
    /// Opaque active-Client reference, if there is one; echo-only.
    pub active_client: Option<String>,
    /// Presence generation of this attribution; newer wins.
    pub generation: u64,
    /// Human-readable move explanation for display only, if any.
    pub move_reason: Option<String>,
}
