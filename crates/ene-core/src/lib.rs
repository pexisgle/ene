//! # ene Core (`ene-core`)
//!
//! ene Core is the unified Rust-based library powering the ene application ecosystem.
//! It serves as a modular, high-performance facade encapsulating LLM integration,
//! conversational memory, auto session-splitting, and active tool hosts (via both local IPC and MCP).
//!
//! By using `ene-core`, third-party developers can embed the entire ene system in their own applications
//! (CUI, GUI, or server backends) without duplicate boilerplate.
//!
//! ## Architecture Overview
//!
//! The core engine ties together several specialized subsystems:
//!
//! - **Configuration (`ene_config`)**: Flexible JSON-based settings registration and validation.
//! - **Embedding (`ene_embedding`)**: Vector representation generation via remote APIs or local Candle models.
//! - **Memory (`ene_memory`)**: An SQLite-vec powered episodic memory store for facts and summaries.
//! - **Session (`ene_session`)**: Conversation history, character card configurations, and auto-session splits.
//! - **Tool Host (`ene_tool_host`)**: Spawns and manages sandboxed tool binaries (via UDS/IPC) and external MCP servers.
//!
//! ## Quick Start Example
//!
//! Here is a minimal example initializing the `EneRuntime` and launching an AI streaming completion loop:
//!
//! ```rust,no_run
//! use ene_core::{EneRuntime, EneStreamEvent};
//! use tokio_stream::StreamExt;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // 1. Create the runtime
//!     let mut runtime = EneRuntime::init().await?;
//!
//!     // 2. Load workspace settings and initialize subsystems
//!     runtime.config().load().await?;
//!
//!     // 3. Load the character card (default from config)
//!     runtime.character().load()?;
//!
//!     // 4. Run the AI agent stream with tool support
//!     let user_input = "Hello, what tools can you use?";
//!     let stream = runtime.run(user_input).await?;
//!     tokio::pin!(stream);
//!
//!     // 5. Consume events dynamically (e.g., streaming text, tool executions)
//!     while let Some(event) = stream.next().await {
//!         match event {
//!             EneStreamEvent::TextDelta(delta) => print!("{}", delta),
//!             EneStreamEvent::ToolCallStart { name, arguments } => {
//!                 println!("\n[Executing tool '{}' with {}]", name, arguments);
//!             }
//!             EneStreamEvent::ToolCallResult { name, result } => {
//!                 println!("[Tool '{}' returned: {}]", name, result);
//!             }
//!             EneStreamEvent::Finished => {
//!                 println!("\n[Stream complete]");
//!             }
//!             EneStreamEvent::Error(err) => {
//!                 eprintln!("Error in stream: {}", err);
//!             }
//!             _ => {}
//!         }
//!     }
//!
//!     Ok(())
//! }
//! ```
#![warn(missing_docs)]

/// Configuration types for the AI provider.
pub mod config;
/// Core error types.
pub mod error;
/// System prompt and message assembly helpers.
pub mod prompt_builder;
/// Runtime initialization and lifecycle management.
pub mod runtime;
/// AI streaming engine and tool-call loop.
pub mod stream;

/// OpenAI-compatible provider config.
pub use config::ProviderConfig;

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
/// Runtime and tool-registry builder.
pub use runtime::{CharacterHandle, ConfigHandle, EneRuntime, build_tool_registry};
/// Streaming event type and entry-point.
pub use stream::{
    EneStreamEvent, PermissionDecision, register_permission_request, run_ene_with_tools,
    submit_permission_decision,
};
