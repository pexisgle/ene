pub mod character_card;
pub mod client;
pub mod config;
pub mod conversation_manager;
pub mod embedding;
pub mod error;
pub mod mcp_client;
pub mod memory;
pub mod paths;
pub mod prompt_builder;
pub mod resources;
pub mod sandbox;
pub mod session;
pub mod special_token;
pub mod stream;
pub mod summarizer;
pub mod tool_factory;
pub mod tools;
pub mod utils;

// Re-exports for backward compatibility
pub use config::AiSettings;
pub use config::EmbeddingProviderType;
pub use conversation_manager::{
    PendingSplitTask, SessionBoundary, SplitReason, SplitResult, SplitTaskInput, check_boundary,
    execute_split, generate_session_id, poll_split_result, spawn_split_task,
};
pub use embedding::{
    ApiEmbeddingProvider, EmbeddingProvider, GgufEmbeddingProvider, create_embedding_provider,
};
pub use error::AiCoreError;
pub use memory::{ConversationSummary, KeyFact, MemoryStore, RecalledSummary};
pub use session::ConversationSession;
pub use stream::{AiStreamEvent, run_ai_with_tools};
pub use tool_factory::ToolRegistryBuilder;
pub use tools::{
    BuiltinToolRegistry, CompositeToolRegistry, ScreenshotToolRegistry, ToolCallResult,
    ToolCategory, ToolDefinition, ToolRegistry, UndoEntry, UndoManager, UndoOperation, backup_file,
};
pub use utils::{init_memory, truncate};

// Cowork Agent 新機能の公開エクスポート
pub use sandbox::permission::{
    DestructiveAction, PermissionGate, PermissionLevel, PermissionRequest,
};
