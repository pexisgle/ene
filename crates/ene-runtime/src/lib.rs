//! # ene-runtime
//!
//! Unified runtime facade integrating LLM streaming, tool orchestration,
//! long-term memory, and session management through an actor-based
//! message-passing architecture.
#![warn(missing_docs)]
#![expect(
    clippy::option_if_let_else,
    reason = "nursery style; match/if-let clarity preferred locally"
)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "runtime turn/stream orchestration uses intentional counter arithmetic"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "streaming/db helpers index into bounded event and history buffers"
)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "unit/integration tests use unwrap/expect/panic for concise assertions"
    )
)]

/// Shared runtime bootstrap helpers (folded into [`EneHandle::open`]).
pub mod bootstrap;
pub mod connectors;
pub use connectors::{ConnectorHandle, ConnectorHandleError};
/// DB IPC request handler (re-exported from `ene-store`).
#[cfg(any(unix, windows))]
#[doc(no_inline)]
pub use ene_store::db_server;
/// Multiplexed host-service acceptor (re-exported from `ene-store`).
#[cfg(any(unix, windows))]
#[doc(no_inline)]
pub use ene_store::host_service;
/// Opt-in diagnostics facade (pipeline detail, provider health, memory).
///
/// Strictly observability — control operations (character swap,
/// compression, tools) live on [`EneHandle`] itself.
pub mod diagnostics;
mod empty_response_log;
/// Core error types.
pub mod error;
/// Actor-based runtime with message-passing architecture.
pub mod handle;
/// System prompt and message assembly helpers.
///
/// Not part of the stable public API v1 contract. Kept visible for the
/// CLI `/prompt` debug command and integration tests.
#[doc(hidden)]
pub mod message_builder;
mod proactive;
mod proactive_llm;
/// Stable public API v1 facade: version, JSON event mirrors, redaction.
pub mod public_api;
/// Read-only session and pending-candidate query handles that bypass the
/// turn-execution actor mailbox entirely.
pub mod query;
/// Persistent scheduler (timer task, policy, config).
mod scheduler;
/// Permission types and streaming engine internals.
///
/// Not part of the stable public API v1 contract. Prefer [`EneHandle`].
#[doc(hidden)]
pub mod streaming;
mod streaming_cognitive;
/// Bounded-task admission config for the turn actor's background
/// `JoinSet`s (Stage 8).
pub mod task_config;
/// Tool registry operations handle (list / search / call / invalidate).
pub mod tools;
/// Type-safe identifiers for runtime concepts.
pub mod types;
/// Actor-native undo stack and metadata.
pub mod undo;
/// Screen-image vision summarization handle, bypasses the turn-execution
/// actor mailbox entirely.
pub mod vision;
/// Workspace document indexer and its operations handle.
pub mod workspace;

// ── Bootstrap helpers ──
/// Host helpers for `ConfigStore` → card → [`EneHandle::open`].
pub use bootstrap::{open_from_disk, open_ready, open_with_config};

// ── Actor types ──
/// Actor handle, events, status, and state snapshot.
///
/// The event bus is split into three channels: [`EneEvent`] /
/// [`EneEventReceiver`] (chat bus, via [`EneHandle::subscribe`]),
/// [`AudioChunk`] / [`AudioStreamReceiver`] (audio channel, via
/// [`EneHandle::take_audio_stream`]), and [`LifecycleEvent`] /
/// [`LifecycleReceiver`] (lifecycle bus, via
/// [`EneHandle::subscribe_lifecycle`]).
pub use handle::{
    AudioChunk, AudioStreamReceiver, DeferredToolTask, EneEvent, EneEventReceiver, EneHandle,
    EneStateSnapshot, EneStatus, FeatureSettingsUpdate, LifecycleEvent, LifecycleReceiver,
    ShutdownTimeout, TerminalReason,
};

// ── Read-only query / vision handles ──
/// Pending memory-candidate approval handle and its summary DTO.
pub use query::candidates::{MemoryCandidateHandle, PendingCandidateEdit, PendingCandidateSummary};
/// Read-only session query handle (list / export / import / search / archive).
pub use query::sessions::SessionQueryHandle;
/// Screen-image vision summarization handle.
pub use vision::VisionHandle;

// ── Diagnostics ──
/// Diagnostics facade and memory query-and-mutation handle.
pub use diagnostics::{DiagnosticEvent, DiagnosticEventReceiver, EneDiagnostics, MemoryHandle};

// ── Tools ──
/// Tool registry operations handle (list / search / call / invalidate).
pub use tools::ToolHandle;
/// Workspace document index operations handle.
pub use workspace::{WorkspaceHandle, WorkspaceIndexer, WorkspaceStatusView};

// ── Public API v1 ──
/// Public API version constant, JSON chat/lifecycle-event mirrors, session
/// DTOs, and the unified [`public_api::PublicApiError`] category.
pub use public_api::{
    API_VERSION, PublicApiError, PublicChatEvent, PublicExportedMessage, PublicLifecycleEvent,
    PublicPerfCue, PublicSessionMeta, redact_text, redact_tool_arguments,
    redact_tool_arguments_json,
};

// ── Config types ──
/// Top-level application configuration (re-exported from `ene-config`).
#[doc(no_inline)]
pub use ene_config::EneConfig;
/// Bounded-task admission caps for the turn actor (`tools.*`, Stage 8).
pub use task_config::ToolRuntimeConfig;

// ── Provider types ──
/// AI provider registry and task routing config.
#[doc(no_inline)]
pub use ene_ai::AiConfig;
/// LLM message types (re-exported from `ene-ai`).
#[doc(no_inline)]
pub use ene_ai::LlmMessage;
/// LLM provider trait (re-exported from `ene-ai`).
#[doc(no_inline)]
pub use ene_ai::LlmProvider;

// ── Memory types ──
/// Persistent scheduler domain types.
pub use ene_core::{
    NewSchedule, Schedule, ScheduleAction, ScheduleConfirmation, ScheduleKind, ScheduleRun,
    ScheduleRunStatus,
};
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
/// Host observation DTO for proactive speech.
#[doc(no_inline)]
pub use ene_mind::ProactiveObservation;
/// Unique session identifier (re-exported from `ene-mind`).
#[doc(no_inline)]
pub use ene_mind::SessionId;
/// Performance cues (re-exported from `ene-mind`).
#[doc(no_inline)]
pub use ene_mind::{CueSource, MotionLayer, PerfKind, PerformanceCue};
/// Unique permission request identifier.
pub use types::RequestId;
/// Turn identity, origin, and run/cancel errors.
pub use types::{CancelError, RunError, TurnId, TurnOrigin};

// ── Tool types ──
/// `ToolSpec` type (re-exported from `ene-tool-proto`).
#[doc(no_inline)]
pub use ene_plugin_proto::ToolSpec;

// ── Core error ──
/// Runtime error type.
pub use error::EneRuntimeError;

// ── Stream types ──
/// A single answer in a multi-question interactive prompt (re-exported from `ene-tool-proto`).
pub use streaming::MultiAnswer;
/// Permission decision type.
pub use streaming::PermissionDecision;
/// User's response to an interactive tool's input request.
pub use streaming::UserInputResponse;
/// Permission grant scope and lifetime.
pub use streaming::{GrantType, PermissionScope};
/// Undo report returned by [`EneHandle::undo`].
pub use undo::UndoReport;

// ── Prompt builder ──
/// Message build context struct.
pub use message_builder::MessageBuildContext;
/// Build messages for LLM completion request.
pub use message_builder::build_messages;
