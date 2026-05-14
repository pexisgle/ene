pub mod character_card;
pub mod client;
pub mod config;
pub mod conversation_manager;
pub mod embedding;
pub mod error;
pub mod mcp_client;
pub mod paths;
pub mod resources;
pub mod memory;
pub mod prompt_builder;
pub mod sandbox;
pub mod session;
pub mod special_token;
pub mod stream;
pub mod summarizer;
pub mod tools;
pub mod tool_factory;
pub mod utils;

// Re-exports for backward compatibility
pub use config::AiSettings;
pub use conversation_manager::{
    check_boundary, execute_split, generate_session_id, poll_split_result,
    spawn_split_task, PendingSplitTask, SessionBoundary, SplitReason, SplitResult,
};
pub use embedding::{ApiEmbeddingProvider, EmbeddingProvider, GgufEmbeddingProvider, create_embedding_provider};
pub use config::EmbeddingProviderType;
pub use error::AiCoreError;
pub use memory::{MemoryStore, ConversationSummary, RecalledSummary, KeyFact};
pub use session::ConversationSession;
pub use stream::{run_ai_with_tools, AiStreamEvent};
pub use tools::{ToolDefinition, ToolRegistry, ToolCallResult};
pub use tool_factory::ToolRegistryBuilder;
pub use utils::{truncate, init_memory};

// Legacy module aliases for external code using old paths
pub mod memory_store {
    pub use super::memory::store::*;
}
pub mod memory_recall {
    pub use super::memory::recall::*;
}
pub mod tool {
    pub use super::tools::definition::*;
}
pub mod builtin_tools {
    pub use super::tools::builtin::*;
}
pub mod screenshot_tool {
    pub use super::tools::screenshot::*;
}
pub mod composite_registry {
    pub use super::tools::composite::*;
}
pub mod undo {
    pub use super::tools::undo_manager::*;
}
