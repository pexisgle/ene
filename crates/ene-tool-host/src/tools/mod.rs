/// Composite tool registry that aggregates multiple registries.
pub mod composite;
/// Tool registry trait and type definitions.
pub mod registry;

pub use composite::CompositeToolRegistry;
#[doc(no_inline)]
pub use ene_tool_proto::ToolCategory;
pub use registry::ToolRegistry;

/// Computes a stable hash of the tool definition used for cache invalidation
/// of tool embeddings. Includes `ToolVersion`, all `KeywordSet` tiers, and
/// other semantically relevant fields so that any meaningful change to the
/// tool spec triggers re-embedding.
pub fn compute_tool_version_hash(tool: &ene_tool_proto::ToolSpec) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(tool.name.as_str().as_bytes());
    hasher.update(tool.version.to_string().as_bytes());
    hasher.update(tool.display_name.as_bytes());
    hasher.update(tool.summary.as_bytes());
    hasher.update(tool.description.as_bytes());
    hasher.update(tool.parameters.to_string().as_bytes());
    for k in &tool.keywords.primary {
        hasher.update(k.as_bytes());
    }
    for k in &tool.keywords.secondary {
        hasher.update(k.as_bytes());
    }
    for k in &tool.keywords.domain {
        hasher.update(k.as_bytes());
    }
    for k in &tool.keywords.negative {
        hasher.update(k.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}
