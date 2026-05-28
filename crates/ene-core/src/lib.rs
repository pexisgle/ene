//! # ene-core
//!
//! Unified runtime facade integrating LLM streaming, tool orchestration,
//! long-term memory, and session management through an actor-based
//! message-passing architecture.
#![warn(missing_docs)]

/// Actor-based runtime with message-passing architecture.
pub mod actor;
/// Configuration types for the AI provider.
pub mod config;
/// Core error types.
pub mod error;
/// System prompt and message assembly helpers.
pub mod prompt_builder;
/// AI streaming engine and tool-call loop.
pub mod stream;

/// OpenAI-compatible provider config.
pub use config::ProviderConfig;

/// Actor handle, commands, events, status, and state snapshot.
pub use actor::{EneCommand, EneEvent, EneHandle, EneStateSnapshot, EneStatus};

/// Provider types (re-exported from `ene-provider`).
pub use ene_provider::{
    LlmMessage, LlmProvider, LlmProviderFactory, LlmProviderRegistry, LlmResponseChunk,
    LlmToolCall, LlmToolCallChunk, UserMessagePart,
};

/// Embedding provider trait (re-exported from `ene-provider`).
pub use ene_provider::EmbeddingProvider;
/// Local embedding config (re-exported from `ene-provider`).
pub use ene_provider::LocalEmbeddingConfig;

/// Memory types (re-exported from `ene-memory`).
pub use ene_memory::{ConversationSummary, KeyFact, MemoryConfig, MemoryStore, RecalledSummary};
/// Session types and utilities (re-exported from `ene-session`).
pub use ene_session::{
    ConversationSession, PendingSplitTask, SessionBoundary, SessionConfig, SessionError,
    SplitReason, SplitResult, SplitTaskInput, check_boundary, execute_split, expand_cbs_macros,
    extract_emotion_from_token, generate_session_id, poll_split_result, spawn_split_task,
    split_text_and_special_tokens, truncate,
};
/// Tool host types (re-exported from `ene-tool-host`).
pub use ene_tool_host::{
    CompositeToolRegistry, IpcToolRegistry, McpToolRegistry, ToolCategory, ToolDefinition,
    ToolError, ToolHostManager, ToolRegistry,
};
/// Core AI error type.
pub use error::EneCoreError;

/// Resource directory initialization (re-exported from `ene-config`).
pub use ene_config::ensure_resource_dirs;
/// Prompt builder utilities.
pub use prompt_builder::{build_expression_phi, build_messages, build_system_prompt};
/// Permission decision type.
pub use stream::PermissionDecision;
/// Tool RAG configuration.
pub use stream::ToolRagConfig;
