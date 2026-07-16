//! # ene-tool-rag
//!
//! Tool RAG (Retrieval-Augmented Generation) pipeline for the ene AI character platform.
//!
//! Provides multi-vector tool embedding with weighted field similarity scoring,
//! optional `HyDE` (Hypothetical Document Embedding), and optional LLM-based reranking.
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
#![allow(clippy::option_if_let_else)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Tool RAG configuration types.
pub mod config;
/// Tool RAG error types.
pub mod error;
/// Tool RAG pipeline — multi-vector tool embedding, weighted field similarity,
/// optional HyDE and LLM rerank.
pub mod rag;

/// Tool RAG configuration types.
pub use config::{FieldWeightsConfig, ToolRagConfig};
/// Tool RAG error types.
pub use error::ToolRagError;
/// Tool RAG pipeline types.
pub use rag::{FieldWeights, ToolRag, ToolRagOptions, ToolRagStats};
