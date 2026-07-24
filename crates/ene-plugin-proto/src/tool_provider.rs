//! Tool provider trait and deferred outcome types.

use crate::sandbox::SandboxConfigData;
use crate::tool_error::ToolError;
use crate::tool_ipc::{CallContext, DeferredStatus};
use crate::tool_types::{ToolRagProfile, ToolSpec};
use async_trait::async_trait;

/// Outcome of a deferred (background) tool call (#196).
///
/// Returned by [`ToolProvider::call_tool_deferred`]. A background-capable
/// tool returns [`DeferredOutcome::Deferred`] with a unique `task_id` and
/// delivers the result later out-of-band; any other tool falls back to
/// [`DeferredOutcome::Sync`], carrying the ordinary synchronous result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeferredOutcome {
    /// The call ran synchronously and produced its final result now.
    Sync(String),
    /// The call was accepted for background execution under `task_id`.
    Deferred {
        /// Unique identifier for the queued background task.
        task_id: String,
    },
}

/// Tool provider trait — implemented by each tool crate.
///
/// After IPC separation, each provider on the host side implements this trait.
/// The host-side `IpcToolRegistry` calls through IPC to the tool binary.
///
/// ## Setter-call ordering contract
///
/// During the IPC handshake the server calls [`set_sandbox`](Self::set_sandbox)
/// and [`set_config`](Self::set_config) **once**, synchronously, before the
/// first [`call_tool`](Self::call_tool) on the same connection. Because each
/// connection is spawned as a separate Tokio task, interior mutability (e.g.
/// `RwLock`) is required for any state these setters write and `call_tool`
/// reads. Implementors may assume that after `set_sandbox`/`set_config`
/// return, subsequent `call_tool` calls on the *same* connection will observe
/// the values written by those setters.
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Returns the list of tool specs this provider exposes.
    ///
    /// Mega-tools return N specs, one per action (e.g. `filesystem.read`,
    /// `filesystem.write`, ...).
    fn list_specs(&self) -> Vec<ToolSpec>;

    /// Returns host/RAG metadata for each callable tool (#137).
    ///
    /// Default is empty so hand-written providers keep compiling; prefer
    /// emitting profiles from `#[derive(ToolSpec)]` / `ActionSetProvider`.
    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> {
        Vec::new()
    }

    /// Executes a tool by name with the given JSON arguments.
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;

    /// Executes a tool in deferred (background) mode (#196).
    ///
    /// A background-capable tool should start the work asynchronously and
    /// return [`DeferredOutcome::Deferred`] with a unique `task_id`; the
    /// completion is delivered later out-of-band. Tools that do not support
    /// deferral keep the default implementation, which runs the call
    /// synchronously and returns [`DeferredOutcome::Sync`] — the host then
    /// surfaces the result exactly like an ordinary call.
    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<DeferredOutcome, ToolError> {
        Ok(DeferredOutcome::Sync(
            self.call_tool(name, arguments).await?,
        ))
    }

    /// Polls the status of a deferred (background) task by id (#196).
    ///
    /// The default returns [`DeferredStatus::Unknown`] for tools that do
    /// not support deferral; background-capable tools should track their
    /// tasks and report the real status.
    fn poll_deferred(&self, _task_id: &str) -> DeferredStatus {
        DeferredStatus::Unknown
    }

    /// Cancels a deferred (background) task by id (#196).
    ///
    /// The default is a no-op for tools that do not support deferral.
    fn cancel_deferred(&self, _task_id: &str) {}

    /// Sets the call context (conversation + turn identifiers).
    ///
    /// Tools that want session-scoped state (e.g. undo checkpoints)
    /// should override this method and use `conversation_id` and/or
    /// `turn_id` as appropriate.
    fn set_call_context(&self, _ctx: &CallContext) {}

    /// Receives sandbox configuration (used by filesystem tools; default is no-op).
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}

    /// Approves a pending destructive-operation permission request by ID.
    fn approve_permission(&self, _request_id: &str) {}

    /// Adds a session-wide permission allow pattern (action + target glob).
    fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Revokes a previously granted session-wide permission allow pattern (#177).
    fn revoke_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Receives tool-specific configuration (called once during Handshake).
    fn set_config(&self, _config: &serde_json::Value) {}

    /// Returns the JSON Schema for the configuration this tool accepts.
    fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }
}
