/// Composite tool registry that aggregates multiple registries.
pub mod composite;
/// Tool registry trait and type definitions.
pub mod registry;

pub use composite::CompositeToolRegistry;
pub use registry::ToolRegistry;
pub use ene_tool_proto::{ToolCategory, ToolDefinition};

/// Computes a hash of the tool definition used for cache invalidation of tool embeddings.
pub fn compute_tool_version_hash(tool: &ToolDefinition) -> String {
    use std::hash::{Hash, Hasher};
    let mut state = std::collections::hash_map::DefaultHasher::new();
    tool.name.hash(&mut state);
    tool.description.hash(&mut state);
    tool.keywords.hash(&mut state);
    tool.parameters.to_string().hash(&mut state);
    let hash = state.finish();
    format!("{:x}", hash)
}
