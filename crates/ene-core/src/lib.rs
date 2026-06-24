//! # ene-core
//!
//! Unified runtime facade integrating LLM streaming, tool orchestration,
//! long-term memory, and session management through an actor-based
//! message-passing architecture.
#![warn(missing_docs)]

/// Per-tool DB IPC server.
#[cfg(any(unix, windows))]
pub mod db_server;
/// Core error types.
pub mod error;
/// Actor-based runtime with message-passing architecture.
pub mod handle;
/// System prompt and message assembly helpers.
pub mod message_builder;
/// Permission types and streaming engine internals.
pub mod streaming;
/// Type-safe identifiers for runtime concepts.
pub mod types;

// ── Actor types ──
/// Actor handle, events, status, and state snapshot.
pub use handle::{
    ActorDeadError, ConversationEntry, EneEvent, EneEventReceiver, EneHandle, EneStateSnapshot,
    EneStatus, MemoryQueryHandle, TerminalReason,
};

// ── Config types ──
/// Top-level application configuration (re-exported from `ene-config`).
#[doc(no_inline)]
pub use ene_config::EneConfig;

// ── Provider types ──
/// Configuration for API key retrieval.
#[doc(no_inline)]
pub use ene_provider::ApiKeyConfig;
/// Configuration for the cloud embedding backend.
#[doc(no_inline)]
pub use ene_provider::CloudEmbeddingConfig;
/// Configuration for the embedding system.
#[doc(no_inline)]
pub use ene_provider::EmbeddingConfig;
/// LLM message types (re-exported from `ene-provider`).
#[doc(no_inline)]
pub use ene_provider::LlmMessage;
/// LLM provider trait (re-exported from `ene-provider`).
#[doc(no_inline)]
pub use ene_provider::LlmProvider;
/// Configuration for the local GGUF embedding backend.
#[doc(no_inline)]
pub use ene_provider::LocalEmbeddingConfig;
/// OpenAI-compatible provider config.
#[doc(no_inline)]
pub use ene_provider::ProviderConfig;

// ── Memory types ──
/// Memory configuration (re-exported from `ene-memory`).
#[doc(no_inline)]
pub use ene_memory::MemoryConfig;

// ── Session types ──
/// Truncate text utility (re-exported from `ene-common`).
#[doc(no_inline)]
pub use ene_common::Truncate;
/// Character card type (re-exported from `ene-config`).
#[doc(no_inline)]
pub use ene_config::CharacterCardV3;
/// Role enum for conversation history (re-exported from `ene-provider`).
#[doc(no_inline)]
pub use ene_provider::Role;
/// Character card name (re-exported from `ene-session`).
#[doc(no_inline)]
pub use ene_session::CardName;
/// Unique session identifier (re-exported from `ene-session`).
#[doc(no_inline)]
pub use ene_session::SessionId;
/// Session split reason (re-exported from `ene-session`).
#[doc(no_inline)]
pub use ene_session::SplitReason;
/// Session split result (re-exported from `ene-session`).
#[doc(no_inline)]
pub use ene_session::SplitResult;
/// Extract emotion name from special token (re-exported from `ene-session`).
#[doc(no_inline)]
pub use ene_session::extract_emotion_from_token;
/// Split text and special tokens (re-exported from `ene-session`).
#[doc(no_inline)]
pub use ene_session::split_text_and_special_tokens;
/// Session configuration (re-exported from `ene-session`).
#[doc(no_inline)]
pub use ene_session::{SessionConfig, SummarizationConfig};
/// Unique permission request identifier.
pub use types::RequestId;

// ── Tool types ──
/// Tool registry trait (re-exported from `ene-tool-host`).
#[doc(no_inline)]
pub use ene_tool_host::ToolRegistry;
/// `ToolSpec` type (re-exported from `ene-tool-proto`).
#[doc(no_inline)]
pub use ene_tool_proto::ToolSpec;

// ── Core error ──
/// Core AI error type.
pub use error::EneCoreError;

// ── Stream types ──
pub use streaming::MultiAnswer;
/// Permission decision type (re-exported from `ene-core::streaming`).
pub use streaming::PermissionDecision;
pub use streaming::UserInputResponse;

// ── Prompt builder ──
/// Message build context struct.
pub use message_builder::MessageBuildContext;
/// Build messages for LLM completion request.
pub use message_builder::build_messages;
