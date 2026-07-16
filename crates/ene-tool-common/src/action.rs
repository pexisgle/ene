use async_trait::async_trait;
use ene_tool_proto::{ToolError, ToolRagProfile, ToolSpec};
use serde::de::DeserializeOwned;

/// Marker trait for tool argument structs produced by `#[derive(ToolSpec)]`.
///
/// The derive emits both an inherent `const TOOL_NAME: &'static str`
/// and a `spec() -> ToolSpec` method, plus a blanket
/// `impl ToolSpecArgs for Args`. `ToolAction` does not extend this
/// trait (to keep `dyn ToolAction` object-safe), but every action's
/// `name()` and `definition()` should be one-line forwarders to
/// `Args::TOOL_NAME` and `Args::spec()`, which makes the spec name
/// and the dispatch name the same `&'static str` by construction.
pub trait ToolSpecArgs: DeserializeOwned + Send + Sync + 'static {
    /// Canonical tool name (e.g. `"app.press_key"`).
    const TOOL_NAME: &'static str;

    /// LLM-facing tool specification.
    fn spec() -> ToolSpec;

    /// Returns the host/RAG metadata profile for this args type (#137).
    fn rag_profile() -> ToolRagProfile;
}

/// A unified trait representing a single executable tool action.
///
/// `name` and `definition` are sourced from the args struct's
/// `TOOL_NAME` const and `spec()` method — implementations should be
/// one-line forwarders. This makes the spec name and the dispatch
/// name the same `&'static str` by construction.
///
/// The `#[tool_action]` proc-macro from `ene-tool-derive` auto-fills
/// `name`, `definition`, and `rag_profile` when given an
/// `impl` block with a `run` method, leaving only the `run` body
/// to write.
#[async_trait]
pub trait ToolAction: Send + Sync {
    /// Returns the canonical tool name. Implement as `MyArgs::TOOL_NAME`.
    fn name(&self) -> &'static str;

    /// Implement as `MyArgs::spec()`.
    fn definition(&self) -> ToolSpec;

    /// Returns host/RAG metadata for this tool (#137).
    /// Implement as `MyArgs::rag_profile()`.
    fn rag_profile(&self) -> ToolRagProfile;

    /// Executes the action with a JSON argument string.
    async fn execute(&self, arguments: &str) -> Result<String, ToolError>;
}
