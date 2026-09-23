//! Typed transport rejections (IPC §7.2, §24).
//!
//! Domain rejections stay Ok-side domain outcomes; these reject malformed
//! or unnegotiated WIRE usage without side effects and without closing the
//! connection (unless the connection itself is unusable). Never a retry
//! signal: resending the same bytes fails identically.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RejectKind {
    /// Unknown message type within the negotiated range.
    UnsupportedMessage,
    /// Unknown enum variant in a closed wire enum.
    UnsupportedFieldValue,
    MissingRequiredField,
    /// No common major version; the connection cannot proceed.
    IncompatibleProtocol,
    /// A reused command key arrived with a different request than the
    /// stored row. The durable key already owns its request fingerprint, so
    /// the conflicting send is refused without side effects; retrying the
    /// same bytes fails identically, while retrying the original request
    /// replays cleanly.
    ConflictingCommand,
    /// The connection was superseded by a newer authentication for the same
    /// device. The socket stays open (IPC §11.3), but the connection can never
    /// become current again: further service requires a new connection.
    StaleConnection,
    /// A pairing, capability, or authentication frame arrived outside the
    /// connection's current phase. The pending nonce and negotiated terms are
    /// unchanged; the frame had no effect (IPC §9.3).
    InvalidHandshakePhase,
}

/// One rejected message: kind plus operational detail only (never secrets,
/// bodies, or frame bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RejectNotice {
    pub kind: RejectKind,
    /// Operational detail (offending type name, version pair, ...).
    pub detail: String,
}
