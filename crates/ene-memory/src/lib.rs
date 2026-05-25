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
//! let store = MemoryStore::open(":memory:", 768)?;
//! // Use store.search_summaries(), store.insert_summary(), etc.
//! # Ok(())
//! # }
//! ```
#![warn(missing_docs)]

pub mod config;
pub mod error;
pub mod recall;
pub mod schema;
pub mod store;
pub mod summarizer;
pub mod utils;

pub use error::MemoryError;
pub use recall::format_summaries_for_prompt;
pub use store::{ConversationSummary, KeyFact, MemoryStore, RecalledSummary};
pub use summarizer::{ConversationSummaryResult, summarize_conversation};
pub use utils::truncate;
pub use config::MemoryConfig;
