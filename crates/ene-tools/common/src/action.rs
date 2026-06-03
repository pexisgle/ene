use async_trait::async_trait;
use ene_tool_proto::{ToolError, ToolSpec};

/// A unified trait representing a single executable tool action.
///
/// Implementors represent standalone tool operations, encapsulating their
/// metadata definitions and their runtime execution dependencies.
#[async_trait]
pub trait ToolAction: Send + Sync {
    /// Returns the metadata definition of this tool.
    fn definition(&self) -> ToolSpec;

    /// Returns the canonical tool name (e.g. `"filesystem.read"`).
    ///
    /// Override this in concrete types to avoid cloning the entire
    /// `ToolSpec` during `call_tool` dispatch.
    fn tool_name(&self) -> &'static str;

    /// Executes the action with JSON arguments string and returns the result.
    async fn execute(&self, arguments: &str) -> Result<String, ToolError>;
}
