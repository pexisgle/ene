use async_trait::async_trait;

use crate::ToolHostError;
use ene_tool_proto::{ToolRagProfile, ToolSpec};

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

    /// Sets the call context (conversation + turn identifiers).
    async fn set_call_context(&self, _ctx: &ene_tool_proto::CallContext) {}

    /// Approves a pending destructive-operation permission request by ID.
    async fn approve_permission(&self, _request_id: &str) {}

    /// Adds a session-wide permission allow pattern (action + target glob).
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Returns the JSON Schema for the tool's config section in settings.json.
    async fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }
}
