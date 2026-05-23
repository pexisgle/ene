pub mod client;
pub mod error;
pub mod prompt_builder;
pub mod runtime;
pub mod stream;

pub use error::AiCoreError;
pub use ene_memory::{ConversationSummary, KeyFact, MemoryStore, RecalledSummary};
pub use ene_session::{
    ConversationSession, PendingSplitTask, SessionBoundary, SplitReason, SplitResult,
    SplitTaskInput, check_boundary, execute_split, generate_session_id, poll_split_result,
    spawn_split_task, SessionError, init_embedding, init_memory, init_memory_store, truncate,
    extract_emotion_from_token, split_text_and_special_tokens,
};
pub use ene_tool_host::{
    CompositeToolRegistry, IpcToolRegistry, McpToolRegistry, ToolCategory, ToolDefinition,
    ToolHostManager, ToolRegistry, ToolError,
};
pub use runtime::{AiRuntime, build_tool_registry};
pub use stream::{AiStreamEvent, run_ai_with_tools};




