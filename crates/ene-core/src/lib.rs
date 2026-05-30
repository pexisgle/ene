//! # ene-core
//!
//! Unified runtime facade integrating LLM streaming, tool orchestration,
//! long-term memory, and session management through an actor-based
//! message-passing architecture.
#![warn(missing_docs)]

/// Actor-based runtime with message-passing architecture.
pub mod actor;
/// Core error types.
pub mod error;
/// System prompt and message assembly helpers.
pub mod prompt_builder;
/// AI streaming engine and tool-call loop.
pub mod stream;

// ── Actor types ──
/// Actor handle, events, status, and state snapshot.
pub use actor::{
    ActorDeadError, ConversationEntry, EneEvent, EneEventReceiver, EneHandle, EneStateSnapshot,
    EneStatus, MemoryQueryHandle,
};

// ── Provider types ──
/// OpenAI-compatible provider config.
pub use ene_provider::ProviderConfig;
/// LLM provider trait (re-exported from `ene-provider`).
pub use ene_provider::LlmProvider;
/// LLM message types (re-exported from `ene-provider`).
pub use ene_provider::LlmMessage;

// ── Memory types ──
/// Memory configuration (re-exported from `ene-memory`).
pub use ene_memory::MemoryConfig;

// ── Session types ──
/// Character card type (re-exported from `ene-session`).
pub use ene_session::CharacterCardV3;
/// Session configuration (re-exported from `ene-session`).
pub use ene_session::SessionConfig;
/// Session split reason (re-exported from `ene-session`).
pub use ene_session::SplitReason;
/// Session split result (re-exported from `ene-session`).
pub use ene_session::SplitResult;
/// Extract emotion name from special token (re-exported from `ene-session`).
pub use ene_session::extract_emotion_from_token;
/// Split text and special tokens (re-exported from `ene-session`).
pub use ene_session::split_text_and_special_tokens;
/// Truncate text utility (re-exported from `ene-session`).
pub use ene_session::truncate;

// ── Tool types ──
/// Tool definition type (re-exported from `ene-tool-host`).
pub use ene_tool_host::ToolDefinition;
/// Tool registry trait (re-exported from `ene-tool-host`).
pub use ene_tool_host::ToolRegistry;

// ── Core error ──
/// Core AI error type.
pub use error::EneCoreError;

// ── Stream types ──
/// Permission decision type (re-exported from `ene-core::stream`).
pub use stream::PermissionDecision;

// ── Prompt builder ──
/// Message build context struct.
pub use prompt_builder::MessageBuildContext;
/// Build messages for LLM completion request.
pub use prompt_builder::build_messages;
