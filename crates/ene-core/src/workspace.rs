//! Workspace document index vocabulary (persistence-agnostic).
//!
//! The document/workspace RAG index is the third RAG consumer after memory
//! recall and tool selection. Its DTOs and port live here so `ene-rag`
//! (policy), `ene-store` (persistence), `ene-mind` (prompt injection), and
//! `ene-runtime` (indexing service) can share the vocabulary without
//! depending on each other.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFileRow {
    /// Canonical path of the configured root this file was scanned from.
    pub root: String,
    /// Canonical absolute path of the file (the index identity).
    pub path: String,
    /// File size in bytes at scan time.
    pub size: u64,
    /// Filesystem modification time at scan time.
    pub modified_at: DateTime<Utc>,
    /// Blake3 hash of the file bytes (hex), used for change/rename detection.
    pub content_hash: String,
    /// Embedding model that produced this file's chunk vectors.
    pub model_name: String,
    /// Number of chunks currently indexed for this file. `0` means the file
    /// row exists but needs (re-)embedding.
    pub chunk_count: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewWorkspaceChunk {
    /// Zero-based position within the file.
    pub chunk_index: u32,
    /// Nearest heading at chunk start (empty when the file has none).
    pub heading: String,
    pub content: String,
    /// First line of the chunk (1-based, inclusive).
    pub start_line: u32,
    /// Last line of the chunk (1-based, inclusive).
    pub end_line: u32,
    /// Embedding vector for [`Self::content`].
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceChunkHit {
    /// Zero-based chunk position within the file.
    pub chunk_index: u32,
    /// Canonical path of the configured root the file belongs to.
    pub root: String,
    /// Canonical absolute file path (citation location).
    pub path: String,
    /// Nearest heading at chunk start (citation heading).
    pub heading: String,
    /// First line of the chunk (1-based, inclusive; citation range).
    pub start_line: u32,
    /// Last line of the chunk (1-based, inclusive; citation range).
    pub end_line: u32,
    pub content: String,
    /// Hybrid relevance score in `[0, 1]`.
    pub similarity: f32,
}

#[derive(Debug, Clone)]
pub struct WorkspaceSearchQuery<'a> {
    /// Raw query text used for lexical matching.
    pub query_text: &'a str,
    /// Query embedding for vector matching (`None` for lexical-only search).
    pub embedding: Option<&'a [f32]>,
    /// Embedding model that produced [`Self::embedding`]; vector hits are
    /// filtered to chunks stored by the same model.
    pub model_name: &'a str,
    /// Canonical paths of the currently permitted roots. **Empty means no
    /// roots are permitted and the search returns nothing** (fail closed).
    pub allowed_roots: &'a [String],
    pub top_k: usize,
    pub min_similarity: f32,
}

/// Runtime sync state is reported separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceIndexStatus {
    pub indexed_files: u64,
    pub indexed_chunks: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspacePortError {
    /// The backing store rejected or failed an operation.
    #[error("workspace index backend error: {0}")]
    Backend(String),
}

/// Persistence abstraction for the workspace document index.
///
/// `ene-runtime`'s indexer and `ene-mind`'s prompt injection use this port so
/// they never depend on the concrete store crate — the same arrangement as
/// [`EmbeddingStorePort`](crate::EmbeddingStorePort).
#[async_trait::async_trait]
pub trait WorkspaceDocumentPort: Send + Sync {
    /// All indexed file rows, used by the indexer for change/rename
    /// detection. Intentionally without vectors or chunk text.
    async fn list_workspace_files(&self) -> Result<Vec<WorkspaceFileRow>, WorkspacePortError>;

    /// Atomically replace one file's rows (delete old chunks, insert the file
    /// row and all new chunk rows plus their vec0 entries).
    async fn replace_workspace_file(
        &self,
        file: &WorkspaceFileRow,
        chunks: &[NewWorkspaceChunk],
    ) -> Result<(), WorkspacePortError>;

    /// Move an indexed file to a new path, keeping its chunks (rename
    /// remap). Returns `false` when no row had `old_path`.
    async fn rename_workspace_file(
        &self,
        old_path: &str,
        file: &WorkspaceFileRow,
    ) -> Result<bool, WorkspacePortError>;

    /// Delete the rows (and vec0 entries) for the given paths.
    async fn delete_workspace_files(&self, paths: &[String]) -> Result<usize, WorkspacePortError>;

    /// Delete every file row whose root is not in `keep_roots` (roots removed
    /// from configuration). An empty list wipes the whole index.
    async fn prune_workspace_roots(
        &self,
        keep_roots: &[String],
    ) -> Result<usize, WorkspacePortError>;

    /// Hybrid search (vector KNN + lexical overlap) over chunks belonging to
    /// the permitted roots, scored by `ene-rag` policy.
    async fn search_workspace(
        &self,
        query: &WorkspaceSearchQuery<'_>,
    ) -> Result<Vec<WorkspaceChunkHit>, WorkspacePortError>;

    async fn workspace_index_status(&self) -> Result<WorkspaceIndexStatus, WorkspacePortError>;
}
