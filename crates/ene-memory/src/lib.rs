//! # ene-memory
//!
//! SQLite-vec powered episodic memory store for the ene AI character platform.
//!
//! ## Features
//!
//! - **Conversation summaries**: Persistent storage of conversation summaries with vector embeddings
//! - **Key facts**: User-specific key-value facts with upsert and latest-value retrieval
//! - **Vector similarity search**: Cosine-similarity-based recall of semantically relevant past conversations
//! - **LLM-driven summarization**: Automatic conversation summarization via structured LLM output
//! - **Tool RAG**: Embedding-based tool selection (stored in `tool_embedding_index` table, multi-vector per tool)
//! - **Conversation logging**: Full conversation history in `conversation_logs` for audit and replay
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use ene_memory::MemoryStore;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let store = MemoryStore::open(std::path::Path::new(":memory:"), 768).await?;
//!     // Use store.search_summaries(), store.insert_summary(), etc.
//!     Ok(())
//! }
//! ```
#![warn(missing_docs)]

/// Affect state domain model.
pub mod affect;
/// Memory configuration types.
pub mod config;
/// SeaORM entities representation.
pub mod entities;
/// Memory-related error types.
pub mod error;
/// SeaORM schema migrations.
pub mod migrator;
/// Summary recall and prompt formatting utilities.
pub mod recall;
/// Core memory store (`SQLite` + sqlite-vec).
pub mod store;
/// LLM-driven conversation summarization.
pub mod summarizer;
/// Typed memory domain model.
pub mod typed_memory;

/// Affect state types.
pub use affect::{AffectState, DiscreteEmotion};
/// Memory feature toggle configuration.
pub use config::MemoryConfig;
/// Memory error type.
pub use error::EneMemoryError;
pub use error::MemoryError;
/// Formats recalled summaries for prompt injection.
pub use recall::{format_summaries_for_prompt, format_summaries_with_library};
/// Core memory types.
pub use store::{ConversationSummary, KeyFact, MemoryStore, RecalledSummary};
/// LLM summarization result type and entry-point.
pub use summarizer::{ConversationSummaryResult, summarize_conversation};
/// Typed memory domain types.
pub use typed_memory::{
    AffectAnnotation, MemoryConfidence, MemoryItem, MemoryKind, MemorySalience, MemoryScope,
    MemorySource, MemoryStatus, NewMemoryItem,
};
