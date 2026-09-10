//! Host <-> Client wire-neutral DTOs (remote-capable only).
//!
//! The wire version is always part of the path ([`v1`]) so major revisions
//! cannot silently mix. Every reference crossing this boundary is an opaque
//! wire newtype, never a Host domain newtype, secret, or durable row.
//!
//! Debug redaction rule (IPC §23): wire refs, generations, and outcomes
//! stay visible in Debug output; secrets, conversation bodies, and free-text
//! operands that may quote managed content are redacted.
pub mod v1;
