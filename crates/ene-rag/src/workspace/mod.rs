//! Document/workspace indexing policy — the third RAG consumer after memory
//! recall and tool selection.
//!
//! This module is pure policy: text chunking with citation metadata (heading,
//! line ranges), ignore-rule glob matching, hybrid chunk scoring, and the
//! operator-facing configuration. Filesystem I/O, embedding, and persistence
//! orchestration live in the runtime indexer; the store owns the SQL.

mod chunk;
mod config;
mod glob;
mod score;

pub use chunk::{ChunkOptions, ChunkedDocument, DocumentChunk, chunk_document};
pub use config::WorkspaceRagConfig;
pub use glob::glob_matches;
pub use score::score_chunk;
