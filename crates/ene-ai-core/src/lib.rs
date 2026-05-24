pub mod client;
pub mod config;
pub mod error;
pub mod prompt_builder;
pub mod runtime;
pub mod stream;

pub use config::ProviderSettings;

pub use ene_memory::{ConversationSummary, KeyFact, MemoryStore, RecalledSummary, MemoryConfig};
pub use ene_session::{
    ConversationSession, PendingSplitTask, SessionBoundary, SessionError, SplitReason, SplitResult,
    SplitTaskInput, check_boundary, execute_split, extract_emotion_from_token, generate_session_id,
    poll_split_result, spawn_split_task, split_text_and_special_tokens, truncate,
    expand_cbs_macros, SessionConfig,
};
pub use ene_embedding::{EmbeddingProviderType, EmbeddingConfig};
pub use ene_tool_host::{
    CompositeToolRegistry, IpcToolRegistry, McpToolRegistry, ToolCategory, ToolDefinition,
    ToolError, ToolHostManager, ToolRegistry,
};
pub use error::AiCoreError;
pub use runtime::{AiRuntime, build_tool_registry};
pub use stream::{AiStreamEvent, run_ai_with_tools};
