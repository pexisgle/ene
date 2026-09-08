//! Host <-> Client wire-neutral DTOs (remote-capable only).
//!
//! Versioned wire surface lives under [`v1`]; the version is always part of
//! the path so major revisions cannot silently mix. Every reference crossing
//! this boundary is an opaque wire newtype, never a Host domain newtype,
//! secret, or durable row.
//!
//! Debug redaction rule (IPC §23): wire refs, generations, and outcomes
//! stay visible in Debug output; secrets, conversation bodies, and free-text
//! operands that may quote managed content are redacted. Each redacted type
//! documents its own boundary.
pub mod v1;
