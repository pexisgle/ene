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
//! - **Tool RAG**: Embedding-based tool selection (stored in `tool_embeddings` table)
//! - **Conversation logging**: Full conversation history in `conversation_logs` for audit and replay
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use ene_memory::MemoryStore;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let store = MemoryStore::open(std::path::Path::new(":memory:"), 768)?;
//! // Use store.search_summaries(), store.insert_summary(), etc.
//! # Ok(())
//! # }
//! ```
#![warn(missing_docs)]

/// Memory configuration types.
pub mod config;
/// Memory-related error types.
pub mod error;
/// Summary recall and prompt formatting utilities.
pub mod recall;
/// Diesel-generated database schema.
pub mod schema;
/// Core memory store (SQLite + sqlite-vec).
pub mod store;
/// LLM-driven conversation summarization.
pub mod summarizer;

/// Memory feature toggle configuration.
pub use config::MemoryConfig;
/// Memory error type.
pub use error::EneMemoryError;
pub use error::MemoryError;
/// Formats recalled summaries for prompt injection.
pub use recall::format_summaries_for_prompt;
/// Core memory types.
pub use store::{ConversationSummary, KeyFact, MemoryStore, RecalledSummary};
/// LLM summarization result type and entry-point.
pub use summarizer::{ConversationSummaryResult, summarize_conversation};
