//! Authoritative presence attribution fact, Host to Client (IPC §12).
//!
//! Latest value supersedes: an older fact is dropped, and a missing fact is
//! never read as current. UI acknowledgment of this fact never constitutes
//! presence authority.

use serde::{Deserialize, Serialize};

use super::refs::{ClientWireRef, CompanionWireRef};

/// Each state is a different meaning; they never collapse into a boolean or
/// a single active field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PresenceStateWire {
    /// Present on the active Client.
    Present,
    /// Running with no active Client. A legitimate state.
    NoActive,
    /// Mid-transition. No Client-dependent starts on either side.
    InTransition,
    /// Stopped. No fallback or recovery applies.
    Stopped,
    /// Waiting on post-restart recovery. Restart restoration only.
    RecoveryWait,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PresenceAttributionWire {
    pub companion: CompanionWireRef,
    pub state: PresenceStateWire,
    pub active_client: Option<ClientWireRef>,
    /// Presence generation this fact belongs to.
    pub generation: u64,
}
