//! # ene-runtime
//!
//! Unified runtime facade integrating LLM streaming, tool orchestration,
//! long-term memory, and session management through an actor-based
//! message-passing architecture.
#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Shared runtime bootstrap helpers (folded into [`EneHandle::open`]).
pub mod bootstrap;
/// Per-tool DB IPC server.
#[cfg(any(unix, windows))]
pub mod db_server;
/// Opt-in diagnostics facade (pipeline detail, memory, tools).
pub mod diagnostics;
mod empty_response_log;
/// Core error types.
pub mod error;
/// Actor-based runtime with message-passing architecture.
pub mod handle;
/// System prompt and message assembly helpers.
///
/// Kept `pub` (rather than `pub(crate)`) because `ene-cli` calls the
/// module-scoped prompt builders (`build_system_prompt`,
/// `build_expression_phi`) directly for its `/prompt` debug command.
pub mod message_builder;
/// Permission types and streaming engine internals.
///
/// Kept `pub` for contributor/integration-test use. Application code should
/// prefer [`EneHandle`] instead of calling into this module.
pub mod streaming;
mod streaming_cognitive;
/// Type-safe identifiers for runtime concepts.
pub mod types;

// ── Bootstrap helpers ──
/// Host helpers for ConfigStore → card → [`EneHandle::open`].
pub use bootstrap::{open_from_disk, open_ready, open_with_config};

// ── Actor types ──
/// Actor handle, events, status, and state snapshot.
pub use handle::{
    ActorDeadError, EneEvent, EneEventReceiver, EneHandle, EneStateSnapshot, EneStatus,
    ShutdownTimeout, TerminalReason,
};

// ── Diagnostics ──
/// Diagnostics facade and memory query handle.
pub use diagnostics::{
    DiagnosticEvent, DiagnosticEventReceiver, EneDiagnostics, MemoryQueryHandle,
};

// ── Config types ──
/// Top-level application configuration (re-exported from `ene-config`).
#[doc(no_inline)]
pub use ene_config::EneConfig;

// ── Provider types ──
/// LLM message types (re-exported from `ene-ai`).
#[doc(no_inline)]
pub use ene_ai::LlmMessage;
/// LLM provider trait (re-exported from `ene-ai`).
#[doc(no_inline)]
pub use ene_ai::LlmProvider;
/// OpenAI-compatible provider config.
#[doc(no_inline)]
pub use ene_ai::ProviderConfig;

// ── Memory types ──
/// Memory configuration (re-exported from `ene-store`).
#[doc(no_inline)]
pub use ene_store::StoreConfig;

// ── Session / history ──
/// Role enum for conversation history (re-exported from `ene-ai`).
#[doc(no_inline)]
pub use ene_ai::Role;
/// Character card type (re-exported from `ene-config`).
#[doc(no_inline)]
pub use ene_config::CharacterCardV3;
/// Character card name (re-exported from `ene-mind`).
#[doc(no_inline)]
pub use ene_mind::CardName;
/// Unified history entry (re-exported from `ene-mind`).
#[doc(no_inline)]
pub use ene_mind::HistoryEntry;
/// Unique session identifier (re-exported from `ene-mind`).
#[doc(no_inline)]
pub use ene_mind::SessionId;
/// Performance cues (re-exported from `ene-mind`).
#[doc(no_inline)]
pub use ene_mind::{CueSource, PerformanceCue};
/// Unique permission request identifier.
pub use types::RequestId;
/// Turn identity and run/cancel errors.
pub use types::{CancelError, RunError, TurnId};

// ── Tool types ──
/// `ToolSpec` type (re-exported from `ene-tool-proto`).
#[doc(no_inline)]
pub use ene_tool_proto::ToolSpec;

// ── Core error ──
/// Runtime error type.
pub use error::EneRuntimeError;

// ── Stream types ──
pub use streaming::MultiAnswer;
/// Permission decision type.
pub use streaming::PermissionDecision;
pub use streaming::UserInputResponse;

// ── Prompt builder ──
/// Message build context struct.
pub use message_builder::MessageBuildContext;
/// Build messages for LLM completion request.
pub use message_builder::build_messages;
