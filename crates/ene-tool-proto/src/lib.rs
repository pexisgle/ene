//! # ene-tool-proto
//!
//! IPC protocol definitions and shared types for the ene tool system.
//!
//! This crate defines the contract between the core runtime and standalone tool binaries:
//!
//! - [`ToolProvider`] trait — Interface each tool implements
//! - [`IpcRequest`] / [`IpcResponse`] — Wire protocol messages (length-prefixed JSON over UDS/Named Pipe)
//! - [`ToolSpec`] — LLM-facing schema (`name`, `description`, `parameters`)
//! - [`SandboxConfigData`] — Sandbox configuration shared across tool processes
//! - [`run_tool_server`] — Helper to start an IPC server for a [`ToolProvider`]
//!
//! ## Two-layer ABI (API v2 / #135)
//!
//! - **Wire:** [`ToolProvider`] in this crate (IPC / sandbox binaries)
//! - **Host:** `ene_tool_host::ToolRegistry` (IPC + MCP + composite)
//! - Name collision is a hard error at every registry layer
//!   ([`HostRegistry`] returns [`ToolError::DuplicateName`];
//!   [`CompositeToolRegistry`] returns [`ToolHostError::DuplicateToolName`])
//! - Prefer the `ene-tool` facade crate for new tool binaries
//!
//! ## Crate Boundaries
//!
//! Enforced by [AGENTS.md §4.1](../../AGENTS.md) and reconfirmed by the
//! [API refactor plan](../../docs/architecture/api-refactor-plan.md) (item 2):
//!
//! - This crate is limited to **wire-protocol concerns only**: IPC request/response
//!   types, the [`ToolProvider`] trait, the UDS/Named-Pipe [`transport`] layer,
//!   sandbox-configuration **data** ([`SandboxConfigData`]), and the
//!   [`run_tool_server`] entry point.
//! - It must NOT gain business logic, database access, or `sea-orm` dependencies —
//!   those belong to `ene-tool-host` (orchestration) and `ene-store` (the sole
//!   SQLite owner) respectively. A future "convenience" SQL helper or policy
//!   decision does not belong here even if it seems locally useful to a tool
//!   author; it belongs in `ene-tool-host` or the calling tool binary.
//! - Depends only on `ene-config` (for the `define_tool_config!` macro used by
//!   [`SandboxConfigData`]'s schema registration). Does NOT depend on `ene-runtime`,
//!   `ene-store`, or `ene-mind`.
//!
//! ## Creating a Custom Tool
//!
//! ```rust,no_run
//! use async_trait::async_trait;
//! use ene_tool_proto::{
//!     ToolError, ToolName, ToolProvider, ToolSpec, run_tool_server,
//! };
//!
//! struct MyTool;
//!
//! #[async_trait]
//! impl ToolProvider for MyTool {
//!     fn list_specs(&self) -> Vec<ToolSpec> {
//!         vec![ToolSpec::new(
//!             ToolName::new("hello"),
//!             "Greets the user with a personalised message.",
//!             serde_json::json!({
//!                 "type": "object",
//!                 "properties": {
//!                     "name": {"type": "string", "description": "Name to greet"}
//!                 },
//!                 "required": ["name"]
//!             }),
//!         )]
//!     }
//!
//!     async fn call_tool(&self, name: &str, args: &str) -> Result<String, ToolError> {
//!         let v: serde_json::Value = serde_json::from_str(args)
//!             .map_err(|e| ToolError::InvalidArguments { message: e.to_string() })?;
//!         Ok(format!("Hello, {}!", v["name"].as_str().unwrap_or("world")))
//!     }
//! }
//! ```
#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Tool error types.
pub mod error;
/// Composite registry that aggregates multiple `ToolProvider` instances.
pub mod host_registry;
/// IPC wire protocol (request / response).
pub mod ipc;
/// Sandbox configuration types.
pub mod sandbox;
/// Server helper for running a tool provider.
pub mod server;
/// UDS / Named Pipe transport layer.
pub mod transport;
/// Shared types (`ToolCategory`, etc.).
pub mod types;

/// Tool error type.
pub use error::EneToolProtoError;
pub use error::ToolError;
/// Interactive user input prompt (used inside [`ToolError::UserInputRequired`]).
pub use error::{MultiAnswer, QuestionItem, UserInputPrompt};
/// Composite registry that aggregates multiple `ToolProvider` instances.
pub use host_registry::HostRegistry;
/// IPC message types and serialisation helpers.
pub use ipc::{
    CallContext, IPC_PROTOCOL_VERSION, IpcRequest, IpcResponse, ToolConfigAccessor,
    read_ipc_request, read_ipc_response, write_ipc_request, write_ipc_response,
};
/// Sandbox configuration data sent from the host.
pub use sandbox::SandboxConfigData;
/// Starts an IPC server for a `ToolProvider`.
pub use server::run_tool_server;
/// Shared tool types.
pub use types::{
    ActionSpec, SideEffects, ToolCategory, ToolExample, ToolName, ToolSpec, ToolVersion,
};

use async_trait::async_trait;

/// Tool provider trait — implemented by each tool crate.
///
/// After IPC separation, each provider on the host side implements this trait.
/// The host-side `IpcToolRegistry` calls through IPC to the tool binary.
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Returns the list of tool specs this provider exposes.
    ///
    /// Mega-tools return N specs, one per action (e.g. `filesystem.read`,
    /// `filesystem.write`, ...).
    fn list_specs(&self) -> Vec<ToolSpec>;

    /// Returns the per-action metadata (used for Tool RAG embedding).
    /// For individual tools, this returns an empty vec.
    fn list_action_specs(&self) -> Vec<ActionSpec> {
        Vec::new()
    }

    /// Executes a tool by name with the given JSON arguments.
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;

    /// Sets the call context (conversation + turn identifiers).
    ///
    /// Tools that want session-scoped state (e.g. undo checkpoints)
    /// should override this method and use `conversation_id` and/or
    /// `turn_id` as appropriate.
    fn set_call_context(&self, _ctx: &crate::ipc::CallContext) {}

    /// Receives sandbox configuration (used by filesystem tools; default is no-op).
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}

    /// Approves a pending destructive-operation permission request by ID.
    fn approve_permission(&self, _request_id: &str) {}

    /// Adds a session-wide permission allow pattern (action + target glob).
    fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Receives tool-specific configuration (called once during Handshake).
    fn set_config(&self, _config: &serde_json::Value) {}

    /// Returns the JSON Schema for the configuration this tool accepts.
    fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }
}

// ── DB IPC auth-token handoff ───────────────────────────────────────
//
// ene-runtime generates a per-tool pre-shared auth token when it spawns
// each per-tool DB IPC server (see `DbIpcServer::new`). The token
// must be presented by the tool binary on the very first
// `ene_tool_db::DbRequest::Handshake` to prevent a stray local
// process from connecting to the (chmod-0600) unix socket and
// declaring a schema. ene-runtime records the token here; the tool
// host reads it back when initialising the tool's sandbox config
// so the value reaches the binary inside `SandboxConfigData`.
//
// The map is process-global because the producer (ene-runtime's actor
// thread) and the consumer (ene-tool-host's startup path) are
// independent threads spawned at slightly different times.
