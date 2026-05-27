use async_trait::async_trait;

use super::ToolDefinition;

/// Unified tool registry interface — abstracts over both built-in IPC tools and MCP tools.
///
/// Implemented by [`crate::IpcToolRegistry`], [`crate::McpToolRegistry`], and [`crate::CompositeToolRegistry`].
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// Returns the list of all available tools.
    fn list_tools(&self) -> Vec<ToolDefinition>;

    /// Returns only the tools relevant to the given query (defaults to all tools).
    /// Takes an optional query embedding for cosine-similarity filtering.
    fn list_relevant_tools(
        &self,
        _query_embedding: Option<&[f32]>,
        _limit: usize,
    ) -> Vec<ToolDefinition> {
        self.list_tools()
    }

    /// Executes a tool by name with the given JSON arguments from the LLM.
    async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<String, crate::error::ToolError>;

    /// Sets the current session ID (used for undo tracking, session-scoped state).
    async fn set_session_id(&self, _session_id: &str) {}

    /// Approves a pending destructive-operation permission request by ID.
    async fn approve_permission(&self, _request_id: &str) {}

    /// Adds a session-wide permission allow pattern (action + target glob).
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Returns the JSON Schema for the tool's config section in settings.json.
    async fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }

    /// Builds the vector embedding index for Tool RAG.
    /// Only the [`crate::CompositeToolRegistry`] implementation does meaningful work here.
    async fn ensure_index_built(
        &self,
        _embedder: &dyn ene_embedding::EmbeddingProvider,
        _store: Option<&ene_memory::MemoryStore>,
    ) -> Result<(), crate::error::ToolError> {
        Ok(())
    }
}
