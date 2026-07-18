//! # ene-tool-rag
//!
//! Tool RAG (Retrieval-Augmented Generation) pipeline for the ene AI character platform.
//!
//! Provides multi-vector tool embedding with weighted field similarity scoring
//! Provides multi-vector tool embedding with weighted field similarity scoring
//! and optional cosine reranking. LLM `HyDE` (`use_hyde`) is deprecated and a no-op.
//!
//! ## Key Types
//!
//! - [`ToolRag`] — The RAG pipeline: embedding, similarity scoring, reranking, and tool selection
//! - [`ToolRagOptions`] — Runtime options resolved from configuration
//! - [`ToolRagConfig`] — User-facing configuration (serialized into `settings.json`)
//! - [`FieldWeights`] — Per-field weight controls for multi-vector scoring
//! - [`ToolRagStats`] — Observability snapshot returned by [`ToolRag::stats`]
//!
//! ## Usage
//!
//! ```no_run
//! use ene_tool_rag::{ToolRag, ToolRagOptions, ToolRagConfig};
//! use std::sync::Arc;
//!
//! # async fn example(embedder: Arc<dyn ene_ai::EmbeddingProvider>, store: Option<Arc<ene_store::MemoryStore>>) {
//! let rag = ToolRag::new(embedder, store, ToolRagOptions::default());
//! let tools = rag.select("user query").await;
//! # }
//! ```
#![warn(missing_docs)]
#![expect(
    clippy::option_if_let_else,
    reason = "nursery style; match/if-let clarity preferred locally"
)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "RAG scoring and timing deltas use intentional arithmetic"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "ranked tool selection indexes into scored candidate lists"
)]

/// Tool RAG configuration types.
pub mod config;
/// Tool RAG error types.
pub mod error;
/// Tool RAG pipeline — multi-vector tool embedding, weighted field similarity,
/// and optional cosine reranking. LLM HyDE is deprecated (no-op).
pub mod rag;

/// Tool RAG configuration types.
pub use config::{FieldWeightsConfig, ToolRagConfig};
/// Tool RAG error types.
pub use error::ToolRagError;
/// Tool RAG pipeline types.
pub use rag::{FieldWeights, ToolRag, ToolRagOptions, ToolRagStats};
