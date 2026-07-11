//! # ene-core
//!
//! Unified runtime facade integrating LLM streaming, tool orchestration,
//! long-term memory, and session management through an actor-based
//! message-passing architecture.
#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Per-tool DB IPC server.
#[cfg(any(unix, windows))]
pub mod db_server;
/// Core error types.
pub mod error;
/// Shared runtime bootstrap for application entry points.
pub mod bootstrap;
/// Actor-based runtime with message-passing architecture.
pub mod handle;
/// Typed memory recall execution for prompt context.
mod memory_recall;
mod empty_response_log;
/// System prompt and message assembly helpers.
///
/// Kept `pub` (rather than `pub(crate)`) because `ene-cli` calls the
/// module-scoped prompt builders (`build_system_prompt`,
/// `build_expression_phi`) directly for its `/prompt` debug command; see
/// [`docs/api/index.md`](../../../docs/api/index.md) for the async/error
/// conventions that apply to new additions here.
pub mod message_builder;
/// Cognition-config re-exports that exist solely to keep `ene-cognition`
/// linked for schema registration (issue #95). See the module docs for
/// details; prefer importing these from `ene-cognition` directly in new
/// code — they are re-exported at the crate root only for backward
/// compatibility with existing `ene_core::CognitionConfig`-style imports.
pub mod schema_link;
/// Permission types and streaming engine internals.
///
/// Kept `pub` for contributor/integration-test use (`ene-core`'s own
/// `tests/cognitive_streaming_integration.rs` drives `run_stream` and
/// `StreamContext` directly to exercise the legacy pipeline end-to-end).
/// Application code should prefer [`EneHandle`] instead of calling into
/// this module — it is not part of the stable host-facing API and may
/// change without a semver-relevant deprecation cycle.
pub mod streaming;
mod streaming_cognitive;
/// Type-safe identifiers for runtime concepts.
pub mod types;

// ── Bootstrap ──
/// Shared runtime initialization for desktop and CLI.
#[doc(no_inline)]
pub use bootstrap::{BootstrapOptions, bootstrap_runtime};

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
/// LLM message types (re-exported from `ene-provider`).
#[doc(no_inline)]
pub use ene_provider::LlmMessage;
/// LLM provider trait (re-exported from `ene-provider`).
#[doc(no_inline)]
pub use ene_provider::LlmProvider;
/// OpenAI-compatible provider config.
#[doc(no_inline)]
pub use ene_provider::ProviderConfig;

// ── Memory types ──
/// Memory configuration (re-exported from `ene-memory`).
#[doc(no_inline)]
pub use ene_memory::MemoryConfig;

// ── Cognition types (schema-link) ──
/// Cognition config types, re-exported at the root for backward
/// compatibility. See [`schema_link`] for why these exist.
#[doc(no_inline)]
pub use schema_link::{
    CharacterMemoryConfig, CognitionConfig, CognitionMemoryConfig, ContextConfig, EmotionConfig,
};

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
/// Session configuration (re-exported from `ene-session`).
#[doc(no_inline)]
pub use ene_session::{SessionConfig, SummarizationConfig};
/// Unique permission request identifier.
pub use types::RequestId;

// ── Tool types ──
/// `ToolSpec` type (re-exported from `ene-tool-proto`).
///
/// Returned by [`handle::EneHandle::list_tools`]; kept at the root because
/// it is part of that method's public return type.
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
