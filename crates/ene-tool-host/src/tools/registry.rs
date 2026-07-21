use async_trait::async_trait;

use crate::ToolHostError;
use ene_tool_proto::{ToolRagProfile, ToolSpec};

/// Result of a deferred (background) tool call (#196).
///
/// Mirrors [`ene_tool_proto::DeferredOutcome`] at the host registry layer.
/// A background-capable tool returns [`DeferredCallResult::Deferred`] with a
/// unique `task_id`; any other tool falls back to [`DeferredCallResult::Sync`],
/// carrying the ordinary synchronous result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeferredCallResult {
    /// The call ran synchronously and produced its final result now.
    Sync(String),
    /// The call was accepted for background execution under `task_id`.
    Deferred {
        /// Unique identifier for the queued background task.
        task_id: String,
    },
}

/// Unified tool registry interface — abstracts over both built-in IPC tools and MCP tools.
///
/// Implemented by [`crate::IpcToolRegistry`], [`crate::McpToolRegistry`], and [`crate::CompositeToolRegistry`].
///
/// Tool RAG indexing and selection is handled by [`ToolRag`](crate::ToolRag),
/// not by this trait.
#[async_trait]
#[expect(missing_docs, reason = "method docs are on the trait definition")]
pub trait ToolRegistry: Send + Sync {
    /// All currently registered tools.
    fn list_tools(&self) -> Vec<ToolSpec>;

    /// Returns host/RAG metadata profiles for indexed tools (#137).
    ///
    /// Default synthesizes minimal profiles from [`list_tools`](Self::list_tools)
    /// so MCP / legacy registries keep working without an IPC round-trip.
    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> {
        self.list_tools()
            .iter()
            .map(ToolRagProfile::from_tool_spec)
            .collect()
    }

    /// Executes a tool by name with the given JSON arguments from the LLM.
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError>;

    /// Executes a tool in deferred (background) mode (#196).
    ///
    /// A background-capable tool returns [`DeferredCallResult::Deferred`]
    /// with a `task_id` and delivers the result later out-of-band. The
    /// default implementation runs the call synchronously and wraps the
    /// result in [`DeferredCallResult::Sync`], so registries that do not
    /// support deferral keep working unchanged.
    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<DeferredCallResult, ToolHostError> {
        Ok(DeferredCallResult::Sync(
            self.call_tool(name, arguments).await?,
        ))
    }

    /// Polls the status of a deferred (background) task by id (#196).
    ///
    /// `tool_name` identifies the owning tool so composite registries can
    /// route the poll to the correct sub-registry (task ids are assigned
    /// per tool process and are not globally unique). The default returns
    /// [`ene_tool_proto::DeferredStatus::Unknown`] for registries that do
    /// not support deferral.
    async fn poll_deferred(
        &self,
        _tool_name: &str,
        _task_id: &str,
    ) -> ene_tool_proto::DeferredStatus {
        ene_tool_proto::DeferredStatus::Unknown
    }

    /// Cancels a deferred (background) task by id (#196).
    ///
    /// `tool_name` identifies the owning tool for routing in composite
    /// registries. The default is a no-op for registries that do not
    /// support deferral.
    async fn cancel_deferred(&self, _tool_name: &str, _task_id: &str) {}

    async fn set_call_context(&self, _ctx: &ene_tool_proto::CallContext) {}

    /// Approves a pending destructive-operation permission request by ID.
    async fn approve_permission(&self, _request_id: &str) {}

    /// Adds a session-wide permission allow pattern (action + target glob).
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Revokes a previously granted session-wide permission allow pattern (#177).
    async fn revoke_pattern(&self, _action: &str, _target_pattern: &str) {}

    async fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }
}
