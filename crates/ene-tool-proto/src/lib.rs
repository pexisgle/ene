//! # ene-tool-proto
//!
//! IPC protocol definitions and shared types for the ene tool system.
//!
//! This crate defines the contract between the core runtime and standalone tool binaries:
//!
//! - [`ToolProvider`] trait — Interface each tool implements
//! - [`IpcRequest`] / [`IpcResponse`] — Wire protocol messages (length-prefixed JSON over UDS/Named Pipe)
//! - [`ToolSpec`] — Structured schema describing a tool (name, parameters, category, keywords, side effects, ...)
//! - [`SandboxConfigData`] — Sandbox configuration shared across tool processes
//! - [`run_tool_server`] — Helper to start an IPC server for a [`ToolProvider`]
//!
//! ## Creating a Custom Tool
//!
//! ```rust,no_run
//! use async_trait::async_trait;
//! use ene_tool_proto::{
//!     KeywordSet, SideEffects, ToolCategory, ToolError, ToolName, ToolProvider, ToolSpec,
//!     ToolVersion, run_tool_server,
//! };
//!
//! struct MyTool;
//!
//! #[async_trait]
//! impl ToolProvider for MyTool {
//!     fn list_specs(&self) -> Vec<ToolSpec> {
//!         vec![ToolSpec {
//!             name: ToolName::new("hello"),
//!             version: ToolVersion::new(1, 0, 0),
//!             display_name: "Hello".into(),
//!             summary: "Greets the user".into(),
//!             description: "Greets the user with a personalised message.".into(),
//!             category: ToolCategory::Utility,
//!             keywords: KeywordSet::primary_only(["greet", "hello", "greeting"]),
//!             parameters: serde_json::json!({
//!                 "type": "object",
//!                 "properties": {
//!                     "name": {"type": "string", "description": "Name to greet"}
//!                 },
//!                 "required": ["name"]
//!             }),
//!             examples: vec![],
//!             caveats: vec![],
//!             side_effects: SideEffects::ReadOnly,
//!             preconditions: vec![],
//!             related: vec![],
//!         }]
//!     }
//!
//!     async fn call_tool(&self, name: &str, args: &str) -> Result<String, ToolError> {
//!         let v: serde_json::Value = serde_json::from_str(args)
//!             .map_err(|e| ToolError::InvalidArguments { message: e.to_string() })?;
//!         Ok(format!("Hello, {}!", v["name"].as_str().unwrap_or("world")))
//!     }
//!
//!     fn set_session_id(&self, _sid: &str) {}
//! }
//! ```
#![warn(missing_docs)]

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
    IPC_PROTOCOL_VERSION, IpcRequest, IpcResponse, read_ipc_request, read_ipc_response,
    write_ipc_request, write_ipc_response,
};
/// Sandbox configuration data sent from the host.
pub use sandbox::SandboxConfigData;
/// Starts an IPC server for a `ToolProvider`.
pub use server::run_tool_server;
/// Shared tool types.
pub use types::{
    ActionSpec, EmbeddingField, KeywordSet, SideEffects, ToolCategory, ToolExample, ToolName,
    ToolSpec, ToolVersion,
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

    /// Sets the current session ID (used for undo tracking, session-scoped state, etc.).
    fn set_session_id(&self, session_id: &str);

    /// Receives sandbox configuration (used by filesystem tools; default is no-op).
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}

    /// Approves a pending destructive-operation permission request by ID.
    fn approve_permission(&self, _request_id: &str) {}

    /// Adds a session-wide permission allow pattern (action + target glob).
    fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Receives tool-specific configuration (called once during Initialize).
    fn set_config(&self, _config: &serde_json::Value) {}

    /// Returns the JSON Schema for the configuration this tool accepts.
    fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }
}
