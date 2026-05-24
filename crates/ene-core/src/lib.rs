//! # Ene Core (`ene-core`)
//!
//! Ene Core is the unified Rust-based library powering the Ene application ecosystem.
//! It serves as a modular, high-performance facade encapsulating LLM integration,
//! conversational memory, auto session-splitting, and active tool hosts (via both local IPC and MCP).
//!
//! By using `ene-core`, third-party developers can embed the entire Ene system in their own applications
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
//! Here is a minimal example initializing the `AiRuntime` and launching an AI streaming completion loop:
//!
//! ```rust,no_run
//! use ene_core::{AiRuntime, run_ai_with_tools, AiStreamEvent};
//! use ene_config::load_full_settings;
//! use tokio_stream::StreamExt;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // 1. Load workspace settings
//!     let settings = load_full_settings()?;
//!
//!     // 2. Initialize the core runtime
//!     let mut runtime = AiRuntime::init(settings).await?;
//!
//!     // 3. Prepare user input
//!     let user_input = "Hello, what tools can you use?";
//!     let _ = runtime.embed_input(user_input).await?;
//!
//!     // 4. Run the AI agent stream with active sandboxed tools
//!     let mut stream = run_ai_with_tools(
//!         &runtime.settings,
//!         &runtime.session,
//!         user_input,
//!         runtime.registry.clone(),
//!     ).await?;
//!
//!     // 5. Consume events dynamically (e.g., streaming text, tool executions)
//!     while let Some(event) = stream.next().await {
//!         match event {
//!             AiStreamEvent::TextDelta(delta) => print!("{}", delta),
//!             AiStreamEvent::ToolCallStart { name, arguments } => {
//!                 println!("\n[Executing tool '{}' with {}]", name, arguments);
//!             }
//!             AiStreamEvent::ToolCallResult { name, result } => {
//!                 println!("[Tool '{}' returned: {}]", name, result);
//!             }
//!             AiStreamEvent::Finished => {
//!                 println!("\n[Stream complete]");
//!             }
//!             AiStreamEvent::Error(err) => {
//!                 eprintln!("Error in stream: {}", err);
//!             }
//!             _ => {}
//!         }
//!     }
//!
//!     Ok(())
//! }
//! ```

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
