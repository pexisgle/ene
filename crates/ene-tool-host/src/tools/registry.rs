use async_trait::async_trait;

use crate::ToolHostError;
use ene_tool_proto::ToolSpec;

/// Unified tool registry interface — abstracts over both built-in IPC tools and MCP tools.
///
/// Implemented by [`crate::IpcToolRegistry`], [`crate::McpToolRegistry`], and [`crate::CompositeToolRegistry`].
///
/// Tool RAG indexing and selection is handled by [`ToolRag`](crate::ToolRag),
/// not by this trait.
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// Returns the list of all available tools.
    fn list_tools(&self) -> Vec<ToolSpec>;
    /// Executes a tool by name with the given JSON arguments from the LLM.
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError>;

    /// Sets the current session ID (used for undo tracking, session-scoped state).
    async fn set_session_id(&self, _session_id: &str) {}

    /// Sets the call context (conversation + turn identifiers).
    ///
    /// Default implementation forwards `conversation_id` to
    /// [`set_session_id`](ToolRegistry::set_session_id) so registries
    /// that only implement that method continue to work.
    async fn set_call_context(&self, ctx: &ene_tool_proto::CallContext) {
        self.set_session_id(&ctx.conversation_id).await;
    }

    /// Approves a pending destructive-operation permission request by ID.
    async fn approve_permission(&self, _request_id: &str) {}

    /// Adds a session-wide permission allow pattern (action + target glob).
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Returns the JSON Schema for the tool's config section in settings.json.
    async fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }
}
