//! Plugin IPC: independent `core` / `tool` subprotocols (D-22).
//!
//! Provider and capability subprotocols are added in later waves. Frames are
//! 32-bit big-endian length + `MessagePack`. `id` is required on every
//! request/response (never defaulted).

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests fail fast"))]
#![deny(unsafe_code)]

mod error;
mod frame;
mod host;
mod plugin;
mod protocol;

pub use error::IpcError;
pub use frame::{MAX_FRAME_BYTES, read_frame, write_frame};
pub use host::{HostConn, negotiate};
pub use plugin::{BuiltinKind, ToolHandler, serve_from_env, serve_plugin};
pub use protocol::{
    CORE_VERSION, HostHello, Negotiated, ProtoId, ProtocolRanges, TOOL_VERSION, ToolCall,
    ToolResult, ToolSpecWire, VersionRange,
};

#[cfg(test)]
mod tests;
