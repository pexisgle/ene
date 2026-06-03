//! Tool error types re-exported from `ene-tool-proto`.
//!
//! Prior to v0.3, the host had its own `EneToolHostError` type with
//! host-specific variants like `FileNotFound`, `CommandBlocked`, etc.
//! These have all been folded into [`ene_tool_proto::EneToolProtoError`]
//! to remove the per-request boundary mapping. The legacy type alias
//! [`ToolError`] is retained for backward compatibility.

#[doc(no_inline)]
pub use ene_tool_proto::EneToolProtoError as EneToolHostError;

/// Type alias used internally across the host crate.
pub type ToolError = ene_tool_proto::ToolError;
