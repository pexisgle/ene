//! Command correlation and idempotency verification rejects (IPC §24).
//!
//! These are typed wire rejects for command identity violations, judged at
//! the communication boundary before domain dispatch. They are neither a
//! transport error nor a generic domain error, and they never invite a
//! resend under a fresh command id: a conflict is refused as-is, and a
//! command whose detailed result is no longer retained reports
//! [`AlreadyProcessed`](CommandReplayRejectWire::AlreadyProcessed).

use serde::{Deserialize, Serialize};

use super::refs::CommandWireId;

/// A reused command id is refused without side effects; the recorded row
/// keeps its request fingerprint, so retrying the original request replays
/// cleanly while retrying different content fails identically.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommandReplayRejectWire {
    /// The command id was first seen with a different request.
    CommandIdConflict { command_id: CommandWireId },
    /// A non-ID-issuing command was already processed and its detailed
    /// result is no longer retained. Never used for ID-issuing commands.
    AlreadyProcessed { command_id: CommandWireId },
}
