/// Composite tool registry that aggregates multiple registries.
pub mod composite;
/// Tool registry trait and type definitions.
pub mod registry;

pub use composite::CompositeToolRegistry;
#[doc(no_inline)]
pub use ene_tool_proto::ToolCategory;
pub use registry::{DeferredCallResult, ToolRegistry};

/// Computes a stable hash of the tool definition used for cache invalidation
/// of tool embeddings. Includes name, description, and parameters so that any
/// meaningful change to the LLM-facing `ToolSpec` triggers re-embedding.
pub fn compute_tool_version_hash(tool: &ene_tool_proto::ToolSpec) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(tool.name.as_str().as_bytes());
    hasher.update(tool.description.as_bytes());
    hasher.update(tool.parameters.to_string().as_bytes());
    hasher.finalize().to_hex().to_string()
}
