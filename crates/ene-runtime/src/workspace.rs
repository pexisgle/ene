//! Workspace document indexer — scan, chunk, embed, and persist.
//!
//! The indexer is the write side of the document/workspace RAG feature:
//! it walks only the explicitly configured folders, filters files by the
//! operator's allow/ignore rules, diffs against the persisted index by
//! content hash, and embeds only changed/new files. The read side is the
//! prompt-injection search in `ene-mind` plus the CLI `/workspace search`
//! command.
//!
//! Privacy invariants (mirroring the fs-sandbox canonicalization pattern):
//! roots are canonicalized before scanning, directory symlinks are never
//! followed, and file symlinks are indexed only when their canonical target
//! stays inside the permitted root. The delete sweep runs only after a
//! *complete* walk (no cancellation, no unreadable directory, no per-entry
//! scan failures), so rows are never removed for files that were merely not
//! scanned.

use ene_ai::{EmbeddingKind, EmbeddingProvider, embed_query};
use ene_core::{
    NewWorkspaceChunk, WorkspaceChunkHit, WorkspaceDocumentPort, WorkspaceFileRow,
    WorkspaceIndexStatus, WorkspaceSearchQuery,
};
use ene_rag::{ChunkOptions, WorkspaceRagConfig, chunk_document, glob_matches};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Phase of a workspace index sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSyncPhase {
    /// Walking the permitted folders and hashing files.
    Discovering,
    /// Embedding changed/new files.
    Embedding,
    /// Removing rows for vanished files and pruned roots.
    Pruning,
    /// Sync finished (terminal snapshot).
    Done,
}

/// Live progress snapshot of a workspace sync.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceSyncProgress {
    /// Current phase.
    pub phase: WorkspaceSyncPhase,
    /// Files visited so far.
    pub files_scanned: u64,
    /// Files embedded (new or changed).
    pub files_indexed: u64,
    /// Files skipped by filters (size, extension, ignore rules, binary, ...).
    pub files_skipped: u64,
    /// Files removed because they vanished.
    pub files_deleted: u64,
    /// Files moved to a new path without re-embedding.
    pub files_renamed: u64,
    /// Chunks embedded so far.
    pub chunks_embedded: u64,
    /// File currently being processed, if any.
    pub current_file: Option<String>,
    /// Non-fatal per-file errors so far.
    pub errors: u64,
}

impl Default for WorkspaceSyncProgress {
    fn default() -> Self {
        Self {
            phase: WorkspaceSyncPhase::Discovering,
            files_scanned: 0,
            files_indexed: 0,
            files_skipped: 0,
            files_deleted: 0,
            files_renamed: 0,
            chunks_embedded: 0,
            current_file: None,
            errors: 0,
        }
    }
}

/// Final report of a workspace sync.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceSyncReport {
    /// Canonical roots that were actually scanned.
    pub roots: Vec<String>,
    /// Files visited.
    pub files_scanned: u64,
    /// Files embedded (new or changed).
    pub files_indexed: u64,
    /// Files whose content hash and model were already current.
    pub files_unchanged: u64,
    /// Files skipped by filters.
    pub files_skipped: u64,
    /// Files deleted because they vanished from the permitted roots.
    pub files_deleted: u64,
    /// Files remapped to a new path without re-embedding.
    pub files_renamed: u64,
    /// Chunks embedded.
    pub chunks_embedded: u64,
    /// Non-fatal per-file errors.
    pub errors: u64,
    /// Whether the sync was cancelled before completing (the delete sweep is
    /// skipped in that case).
    pub cancelled: bool,
    /// Wall-clock duration.
    pub elapsed: Duration,
}

/// Errors raised by the workspace indexer.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceIndexError {
    /// The document index backend rejected an operation.
    #[error("workspace index backend error: {0}")]
    Port(String),
    /// No configured folder resolved to an existing directory.
    #[error("no permitted workspace folders are configured or resolvable")]
    NoRoots,
    /// The feature is disabled in the configuration.
    #[error("workspace rag is disabled (rag.workspace.enabled)")]
    Disabled,
}

/// One file discovered during a walk (metadata only; content is re-read only
/// for files that need embedding).
#[derive(Debug, Clone)]
struct ScannedFile {
    root: String,
    path: String,
    size: u64,
    modified_at: chrono::DateTime<chrono::Utc>,
    hash: String,
}

/// Orchestrates workspace document indexing against an
/// [`WorkspaceDocumentPort`] using an [`EmbeddingProvider`].
pub struct WorkspaceIndexer {
    store: Arc<dyn WorkspaceDocumentPort>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl WorkspaceIndexer {
    /// Creates an indexer over the given persistence port and embedder.
    pub fn new(
        store: Arc<dyn WorkspaceDocumentPort>,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self { store, embedder }
    }

    /// Runs one full sync: walk, diff, embed, sweep, prune.
    ///
    /// Progress events are sent to `progress` when provided. Cancellation is
    /// checked between files and between walk iterations; the delete sweep is
    /// skipped when the walk did not complete.
    pub async fn sync(
        &self,
        config: &WorkspaceRagConfig,
        cancel: &CancellationToken,
        progress: Option<mpsc::Sender<WorkspaceSyncProgress>>,
    ) -> Result<WorkspaceSyncReport, WorkspaceIndexError> {
        let started = std::time::Instant::now();
        if config.folders.is_empty() {
            // An empty allowlist permits nothing: prune every row so search
            // and status agree with the config immediately.
            let deleted = self
                .store
                .prune_workspace_roots(&[])
                .await
                .map_err(|e| WorkspaceIndexError::Port(e.to_string()))?;
            return Ok(WorkspaceSyncReport {
                roots: Vec::new(),
                files_scanned: 0,
                files_indexed: 0,
                files_unchanged: 0,
                files_skipped: 0,
                files_deleted: u64::try_from(deleted).unwrap_or(u64::MAX),
                files_renamed: 0,
                chunks_embedded: 0,
                errors: 0,
                cancelled: false,
                elapsed: started.elapsed(),
            });
        }
        let roots = canonicalize_roots(&config.folders).await;
        if roots.is_empty() {
            return Err(WorkspaceIndexError::NoRoots);
        }

        let mut state = SyncState::new();
        emit_progress(
            progress.as_ref(),
            state.snapshot(WorkspaceSyncPhase::Discovering),
        );

        let mut walk_complete = true;
        for root in &roots {
            walk_dir(
                Path::new(root),
                Path::new(root),
                &mut state,
                config,
                cancel,
                progress.as_ref(),
                &mut walk_complete,
            )
            .await;
            if cancel.is_cancelled() {
                break;
            }
        }

        let existing = self
            .store
            .list_workspace_files()
            .await
            .map_err(|e| WorkspaceIndexError::Port(e.to_string()))?;

        emit_progress(
            progress.as_ref(),
            state.snapshot(WorkspaceSyncPhase::Embedding),
        );

        // Hash-preserving rename remap: a file that appeared at a new path
        // with the same content hash and model as exactly one vanished row is
        // moved in place instead of re-embedded.
        let current_paths: HashSet<&str> = state.files.keys().map(String::as_str).collect();
        let mut hash_candidates: HashMap<&str, Vec<&WorkspaceFileRow>> = HashMap::new();
        for row in &existing {
            if !current_paths.contains(row.path.as_str()) {
                hash_candidates
                    .entry(row.content_hash.as_str())
                    .or_default()
                    .push(row);
            }
        }
        for (path, file) in &state.files {
            if state.processed_paths.contains(path) {
                continue;
            }
            let candidates = hash_candidates.get(file.hash.as_str());
            let Some(candidates) = candidates else {
                continue;
            };
            if candidates.len() == 1
                && candidates[0].model_name == self.embedder.model_name()
                && !state.remapped_paths.contains(&candidates[0].path)
            {
                let old = &candidates[0].path;
                let new_row = WorkspaceFileRow {
                    root: file.root.clone(),
                    path: file.path.clone(),
                    size: file.size,
                    modified_at: file.modified_at,
                    content_hash: file.hash.clone(),
                    model_name: self.embedder.model_name().to_string(),
                    chunk_count: 0,
                };
                match self.store.rename_workspace_file(old, &new_row).await {
                    Ok(true) => {
                        state.files_renamed = state.files_renamed.saturating_add(1);
                        state.remapped_paths.insert(old.clone());
                        state.processed_paths.insert(file.path.clone());
                        tracing::info!(
                            component = "WorkspaceRag",
                            old_path = %old,
                            new_path = %file.path,
                            "Workspace file renamed in index"
                        );
                    }
                    Ok(false) => {}
                    Err(e) => {
                        state.errors = state.errors.saturating_add(1);
                        tracing::warn!(
                            component = "WorkspaceRag",
                            old_path = %old,
                            error = %e,
                            "Workspace rename remap failed; will re-embed"
                        );
                    }
                }
            }
        }

        // Embed files that are new or whose hash/model changed.
        let mut indexed = 0u64;
        let mut unchanged = 0u64;
        for (path, file) in &state.files {
            if cancel.is_cancelled() {
                break;
            }
            if state.processed_paths.contains(path) {
                continue;
            }
            let existing_row = existing.iter().find(|row| row.path == *path);
            if let Some(row) = existing_row
                && row.content_hash == file.hash
                && row.model_name == self.embedder.model_name()
                && row.chunk_count > 0
            {
                unchanged = unchanged.saturating_add(1);
                state.processed_paths.insert(path.clone());
                continue;
            }

            state.current_file = Some(path.clone());
            emit_progress(
                progress.as_ref(),
                state.snapshot(WorkspaceSyncPhase::Embedding),
            );
            match self.index_file(config, file, cancel).await {
                Ok(chunk_count) => {
                    indexed = indexed.saturating_add(1);
                    state.chunks_embedded = state.chunks_embedded.saturating_add(chunk_count);
                }
                Err(IndexFileError::Cancelled) => {
                    state.current_file = None;
                    break;
                }
                Err(IndexFileError::Skipped(reason)) => {
                    state.files_skipped = state.files_skipped.saturating_add(1);
                    tracing::debug!(
                        component = "WorkspaceRag",
                        path = %path,
                        reason = %reason,
                        "Workspace file skipped"
                    );
                }
                Err(IndexFileError::Failed(e)) => {
                    state.errors = state.errors.saturating_add(1);
                    tracing::warn!(
                        component = "WorkspaceRag",
                        path = %path,
                        error = %e,
                        "Workspace file indexing failed"
                    );
                }
            }
            state.current_file = None;
            emit_progress(
                progress.as_ref(),
                state.snapshot(WorkspaceSyncPhase::Embedding),
            );
        }
        state.files_indexed = state.files_indexed.saturating_add(indexed);
        state.files_unchanged = unchanged;

        let cancelled = cancel.is_cancelled();
        let mut deleted = 0u64;
        if walk_complete && !cancelled {
            emit_progress(
                progress.as_ref(),
                state.snapshot(WorkspaceSyncPhase::Pruning),
            );
            let vanished: Vec<String> = existing
                .iter()
                .filter(|row| {
                    roots.contains(&row.root)
                        && !current_paths.contains(row.path.as_str())
                        && !state.remapped_paths.contains(&row.path)
                        && !state.failed_paths.contains(&row.path)
                })
                .map(|row| row.path.clone())
                .collect();
            if !vanished.is_empty() {
                match self.store.delete_workspace_files(&vanished).await {
                    Ok(n) => deleted = u64::try_from(n).unwrap_or(u64::MAX),
                    Err(e) => {
                        state.errors = state.errors.saturating_add(1);
                        tracing::warn!(
                            component = "WorkspaceRag",
                            error = %e,
                            "Workspace delete sweep failed"
                        );
                    }
                }
            }
            // Roots removed from configuration no longer own any rows.
            match self.store.prune_workspace_roots(&roots).await {
                Ok(_) => {}
                Err(e) => {
                    state.errors = state.errors.saturating_add(1);
                    tracing::warn!(
                        component = "WorkspaceRag",
                        error = %e,
                        "Workspace root prune failed"
                    );
                }
            }
        }
        state.files_deleted = deleted;

        let report = WorkspaceSyncReport {
            roots,
            files_scanned: state.files_scanned,
            files_indexed: state.files_indexed,
            files_unchanged: state.files_unchanged,
            files_skipped: state.files_skipped,
            files_deleted: state.files_deleted,
            files_renamed: state.files_renamed,
            chunks_embedded: state.chunks_embedded,
            errors: state.errors,
            cancelled,
            elapsed: started.elapsed(),
        };
        emit_progress(progress.as_ref(), state.snapshot(WorkspaceSyncPhase::Done));
        tracing::info!(
            component = "WorkspaceRag",
            files_scanned = report.files_scanned,
            files_indexed = report.files_indexed,
            files_unchanged = report.files_unchanged,
            files_skipped = report.files_skipped,
            files_deleted = report.files_deleted,
            files_renamed = report.files_renamed,
            chunks_embedded = report.chunks_embedded,
            errors = report.errors,
            cancelled = report.cancelled,
            elapsed_ms = report.elapsed.as_millis(),
            "Workspace index sync finished"
        );
        Ok(report)
    }

    /// Hybrid search over the permitted roots (vector + lexical).
    pub async fn search(
        &self,
        config: &WorkspaceRagConfig,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<WorkspaceChunkHit>, WorkspaceIndexError> {
        let roots = canonicalize_roots(&config.folders).await;
        if roots.is_empty() {
            return Err(WorkspaceIndexError::NoRoots);
        }
        // Lexical-only fallback when the embedder is unavailable or fails:
        // search must not hard-depend on a live embedding model.
        let embedding = match embed_query(self.embedder.as_ref(), query_text).await {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(
                    component = "WorkspaceRag",
                    error = %e,
                    "Workspace query embedding failed; falling back to lexical search"
                );
                None
            }
        };
        let query = WorkspaceSearchQuery {
            query_text,
            embedding: embedding.as_deref(),
            model_name: self.embedder.model_name(),
            allowed_roots: &roots,
            top_k: limit.max(1),
            min_similarity: config.min_similarity,
        };
        self.store
            .search_workspace(&query)
            .await
            .map_err(|e| WorkspaceIndexError::Port(e.to_string()))
    }

    /// Persisted index counts.
    pub async fn index_status(&self) -> Result<WorkspaceIndexStatus, WorkspaceIndexError> {
        self.store
            .workspace_index_status()
            .await
            .map_err(|e| WorkspaceIndexError::Port(e.to_string()))
    }

    async fn index_file(
        &self,
        config: &WorkspaceRagConfig,
        file: &ScannedFile,
        cancel: &CancellationToken,
    ) -> Result<u64, IndexFileError> {
        if cancel.is_cancelled() {
            return Err(IndexFileError::Cancelled);
        }
        // Re-resolve the path immediately before reading: a concurrent swap
        // of an intermediate directory to a symlink between the walk-time
        // canonicalize and this read must not redirect the read outside the
        // permitted root.
        match tokio::fs::canonicalize(&file.path).await {
            Ok(canonical) if canonical == Path::new(&file.path) => {}
            Ok(_) => return Err(IndexFileError::Skipped("file changed during sync".into())),
            Err(e) => return Err(IndexFileError::Failed(format!("canonicalize {e}"))),
        }
        let bytes = tokio::fs::read(&file.path)
            .await
            .map_err(|e| IndexFileError::Failed(format!("read {e}")))?;
        if bytes.len() > config.max_file_bytes {
            return Err(IndexFileError::Skipped(
                "file grew past max_file_bytes".into(),
            ));
        }
        if bytes.contains(&0) {
            return Err(IndexFileError::Skipped("binary file".into()));
        }
        let text =
            std::str::from_utf8(&bytes).map_err(|_| IndexFileError::Skipped("not UTF-8".into()))?;
        let chunked = chunk_document(
            text,
            Path::new(&file.path)
                .file_stem()
                .map_or("document", |s| s.to_str().unwrap_or("document")),
            &ChunkOptions {
                chunk_chars: config.chunk_chars,
                overlap_chars: config.chunk_overlap_chars,
                max_chunks_per_file: config.max_chunks_per_file,
            },
        );
        if chunked.truncated {
            return Err(IndexFileError::Skipped(
                "exceeds max_chunks_per_file".into(),
            ));
        }
        if chunked.chunks.is_empty() {
            return Err(IndexFileError::Skipped("empty document".into()));
        }
        if cancel.is_cancelled() {
            return Err(IndexFileError::Cancelled);
        }

        let items: Vec<(&str, EmbeddingKind)> = chunked
            .chunks
            .iter()
            .map(|chunk| (chunk.content.as_str(), EmbeddingKind::Description))
            .collect();
        let embeddings = self
            .embedder
            .embed_batch(&items)
            .await
            .map_err(|e| IndexFileError::Failed(format!("embed {e}")))?;

        let new_chunks: Vec<NewWorkspaceChunk> = chunked
            .chunks
            .into_iter()
            .zip(embeddings)
            .map(|(chunk, embedding)| NewWorkspaceChunk {
                chunk_index: chunk.chunk_index,
                heading: chunk.heading,
                content: chunk.content,
                start_line: chunk.start_line,
                end_line: chunk.end_line,
                embedding,
            })
            .collect();
        let row = WorkspaceFileRow {
            root: file.root.clone(),
            path: file.path.clone(),
            size: file.size,
            modified_at: file.modified_at,
            content_hash: file.hash.clone(),
            model_name: self.embedder.model_name().to_string(),
            chunk_count: u64::try_from(new_chunks.len()).unwrap_or(u64::MAX),
        };
        self.store
            .replace_workspace_file(&row, &new_chunks)
            .await
            .map_err(|e| IndexFileError::Failed(format!("persist {e}")))?;
        Ok(u64::try_from(new_chunks.len()).unwrap_or(u64::MAX))
    }
}

enum IndexFileError {
    Cancelled,
    Skipped(String),
    Failed(String),
}

struct SyncState {
    files: HashMap<String, ScannedFile>,
    processed_paths: HashSet<String>,
    remapped_paths: HashSet<String>,
    failed_paths: HashSet<String>,
    files_scanned: u64,
    files_indexed: u64,
    files_unchanged: u64,
    files_skipped: u64,
    files_deleted: u64,
    files_renamed: u64,
    chunks_embedded: u64,
    current_file: Option<String>,
    errors: u64,
}

impl SyncState {
    fn new() -> Self {
        Self {
            files: HashMap::new(),
            processed_paths: HashSet::new(),
            remapped_paths: HashSet::new(),
            failed_paths: HashSet::new(),
            files_scanned: 0,
            files_indexed: 0,
            files_unchanged: 0,
            files_skipped: 0,
            files_deleted: 0,
            files_renamed: 0,
            chunks_embedded: 0,
            current_file: None,
            errors: 0,
        }
    }

    fn snapshot(&self, phase: WorkspaceSyncPhase) -> WorkspaceSyncProgress {
        WorkspaceSyncProgress {
            phase,
            files_scanned: self.files_scanned,
            files_indexed: self.files_indexed,
            files_skipped: self.files_skipped,
            files_deleted: self.files_deleted,
            files_renamed: self.files_renamed,
            chunks_embedded: self.chunks_embedded,
            current_file: self.current_file.clone(),
            errors: self.errors,
        }
    }
}

async fn canonicalize_roots(folders: &[String]) -> Vec<String> {
    let mut roots = Vec::with_capacity(folders.len());
    for folder in folders {
        match tokio::fs::canonicalize(folder).await {
            Ok(canonical) => roots.push(canonical.to_string_lossy().into_owned()),
            Err(e) => {
                tracing::warn!(
                    component = "WorkspaceRag",
                    folder = %folder,
                    error = %e,
                    "Configured workspace folder not found; skipping it"
                );
            }
        }
    }
    roots
}

fn emit_progress(
    tx: Option<&mpsc::Sender<WorkspaceSyncProgress>>,
    progress: WorkspaceSyncProgress,
) {
    if let Some(tx) = tx {
        drop(tx.try_send(progress));
    }
}

async fn walk_dir(
    dir: &Path,
    root: &Path,
    state: &mut SyncState,
    config: &WorkspaceRagConfig,
    cancel: &CancellationToken,
    progress: Option<&mpsc::Sender<WorkspaceSyncProgress>>,
    walk_complete: &mut bool,
) {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) => {
            *walk_complete = false;
            state.errors = state.errors.saturating_add(1);
            tracing::warn!(
                component = "WorkspaceRag",
                dir = %dir.display(),
                error = %e,
                "Workspace directory unreadable; delete sweep disabled"
            );
            return;
        }
    };

    while let Some(result) = entries.next_entry().await.transpose() {
        if cancel.is_cancelled() {
            return;
        }
        let entry = match result {
            Ok(entry) => entry,
            Err(e) => {
                // The failing entry cannot be identified, so its rows cannot
                // be excluded from the sweep individually: disable the sweep
                // entirely rather than deleting rows for files that may
                // still exist.
                *walk_complete = false;
                state.errors = state.errors.saturating_add(1);
                tracing::warn!(component = "WorkspaceRag", error = %e, "Workspace walk entry failed");
                continue;
            }
        };
        let entry_path = entry.path();
        let Ok(rel) = entry_path.strip_prefix(root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let entry_path_str = entry_path.to_string_lossy().into_owned();

        let file_type = match entry.file_type().await {
            Ok(t) => t,
            Err(e) => {
                state.failed_paths.insert(entry_path_str.clone());
                state.errors = state.errors.saturating_add(1);
                tracing::warn!(component = "WorkspaceRag", error = %e, "Workspace file_type failed");
                continue;
            }
        };

        // `DirEntry::file_type` reports symlinks as symlinks (it does not
        // follow them), so resolve the target once: directory symlinks are
        // never followed; file symlinks proceed to the regular file path,
        // where canonical-path containment decides whether they are indexed.
        let mut followed_meta: Option<std::fs::Metadata> = None;
        let is_dir = if file_type.is_symlink() {
            match tokio::fs::metadata(&entry_path).await {
                Ok(meta) if meta.is_dir() => continue,
                Ok(meta) => {
                    followed_meta = Some(meta);
                    false
                }
                Err(e) => {
                    state.failed_paths.insert(entry_path_str.clone());
                    state.errors = state.errors.saturating_add(1);
                    tracing::warn!(
                        component = "WorkspaceRag",
                        error = %e,
                        "Workspace symlink metadata failed"
                    );
                    continue;
                }
            }
        } else {
            file_type.is_dir()
        };

        if is_dir {
            if is_ignored(&rel_str, config) {
                continue;
            }
            Box::pin(walk_dir(
                &entry.path(),
                root,
                state,
                config,
                cancel,
                progress,
                walk_complete,
            ))
            .await;
            continue;
        }
        if !file_type.is_file() && followed_meta.is_none() {
            continue;
        }
        if is_ignored(&rel_str, config) {
            state.files_skipped = state.files_skipped.saturating_add(1);
            continue;
        }

        let Some(ext) = entry
            .path()
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
        else {
            state.files_skipped = state.files_skipped.saturating_add(1);
            continue;
        };
        if !config
            .include_extensions
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(&ext))
        {
            state.files_skipped = state.files_skipped.saturating_add(1);
            continue;
        }

        let meta = match followed_meta.take() {
            Some(meta) => meta,
            None => match tokio::fs::metadata(entry.path()).await {
                Ok(meta) => meta,
                Err(e) => {
                    state
                        .failed_paths
                        .insert(best_effort_canonical(&entry_path).await);
                    state.errors = state.errors.saturating_add(1);
                    tracing::warn!(component = "WorkspaceRag", error = %e, "Workspace metadata failed");
                    continue;
                }
            },
        };
        if meta.len() > u64::try_from(config.max_file_bytes).unwrap_or(u64::MAX) {
            state.files_skipped = state.files_skipped.saturating_add(1);
            continue;
        }

        let canonical = match tokio::fs::canonicalize(entry.path()).await {
            Ok(c) => c,
            Err(e) => {
                state
                    .failed_paths
                    .insert(best_effort_canonical(&entry_path).await);
                state.errors = state.errors.saturating_add(1);
                tracing::warn!(component = "WorkspaceRag", error = %e, "Workspace canonicalize failed");
                continue;
            }
        };
        // Symlinked files pointing outside every permitted root are skipped.
        if !is_within(&canonical, root) {
            state.files_skipped = state.files_skipped.saturating_add(1);
            continue;
        }

        let hash = match hash_file(&canonical).await {
            Ok(hash) => hash,
            Err(e) => {
                state
                    .failed_paths
                    .insert(canonical.to_string_lossy().into_owned());
                state.errors = state.errors.saturating_add(1);
                tracing::warn!(component = "WorkspaceRag", error = %e, "Workspace hashing failed");
                continue;
            }
        };
        let modified_at: chrono::DateTime<chrono::Utc> = meta
            .modified()
            .map_or_else(|_| chrono::Utc::now(), chrono::DateTime::from);
        state.files.insert(
            canonical.to_string_lossy().into_owned(),
            ScannedFile {
                root: root.to_string_lossy().into_owned(),
                path: canonical.to_string_lossy().into_owned(),
                size: meta.len(),
                modified_at,
                hash,
            },
        );
        state.files_scanned = state.files_scanned.saturating_add(1);
        state.current_file = Some(canonical.to_string_lossy().into_owned());
        emit_progress(progress, state.snapshot(WorkspaceSyncPhase::Discovering));
    }
}

/// Best-effort canonical path for sweep-exclusion bookkeeping when
/// `canonicalize` itself failed: canonicalize the parent and re-attach the
/// file name so the recorded path matches the stored index key.
async fn best_effort_canonical(path: &Path) -> String {
    if let Some(parent) = path.parent()
        && let Some(name) = path.file_name()
        && let Ok(parent) = tokio::fs::canonicalize(parent).await
    {
        return parent.join(name).to_string_lossy().into_owned();
    }
    path.to_string_lossy().into_owned()
}

fn is_ignored(rel_path: &str, config: &WorkspaceRagConfig) -> bool {
    config
        .ignore_globs
        .iter()
        .any(|pattern| glob_matches(pattern, rel_path))
}

/// Component-wise containment check (`/a/b` is inside `/a`, `/a/bb` is not).
fn is_within(canonical: &Path, root: &Path) -> bool {
    canonical.starts_with(root)
}

async fn hash_file(path: &Path) -> Result<String, std::io::Error> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Handle for workspace document index operations.
///
/// Obtained via [`crate::EneHandle::workspace`]. Routes through the actor
/// mailbox: sync is single-flight (a second `start_sync` returns
/// [`crate::error::EneRuntimeError::Busy`]) and carries a cancellation token
/// that `cancel_sync` aborts.
#[derive(Clone)]
pub struct WorkspaceHandle {
    cmd_tx: Arc<mpsc::UnboundedSender<crate::handle::EneCommand>>,
}

impl WorkspaceHandle {
    pub(crate) fn new(cmd_tx: Arc<mpsc::UnboundedSender<crate::handle::EneCommand>>) -> Self {
        Self { cmd_tx }
    }

    /// Start a background index sync. Returns
    /// [`EneRuntimeError::Busy`](crate::error::EneRuntimeError::Busy) when a
    /// sync is already running.
    pub async fn start_sync(&self) -> Result<(), crate::error::EneRuntimeError> {
        use crate::handle::EneCommand;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx
            .send(EneCommand::WorkspaceStartSync { reply: tx })
            .map_err(|_| crate::error::EneRuntimeError::ChannelClosed)?;
        rx.await
            .map_err(|_| crate::error::EneRuntimeError::ChannelClosed)?
    }

    /// Cancel the in-flight background sync, if any.
    pub fn cancel_sync(&self) -> Result<(), crate::public_api::PublicApiError> {
        use crate::handle::EneCommand;
        self.cmd_tx
            .send(EneCommand::WorkspaceCancelSync)
            .map_err(|_| crate::public_api::PublicApiError::ActorDead)
    }

    /// Current index + sync status.
    pub async fn status(&self) -> Result<WorkspaceStatusView, crate::public_api::PublicApiError> {
        use crate::handle::EneCommand;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx
            .send(EneCommand::WorkspaceStatus { reply: tx })
            .map_err(|_| crate::public_api::PublicApiError::ActorDead)?;
        rx.await
            .map_err(|_| crate::public_api::PublicApiError::ActorDead)
    }

    /// Search the permitted workspace folders.
    pub async fn search(
        &self,
        query: String,
        limit: usize,
    ) -> Result<Vec<WorkspaceChunkHit>, crate::error::EneRuntimeError> {
        use crate::handle::EneCommand;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx
            .send(EneCommand::WorkspaceSearch {
                query,
                limit,
                reply: tx,
            })
            .map_err(|_| crate::error::EneRuntimeError::ChannelClosed)?;
        rx.await
            .map_err(|_| crate::error::EneRuntimeError::ChannelClosed)?
    }
}

/// User-facing workspace index status (config + persisted + live sync state).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceStatusView {
    /// Whether the feature is enabled in the current config.
    pub enabled: bool,
    /// Configured folders.
    pub folders: Vec<String>,
    /// Indexed file count in the store.
    pub indexed_files: u64,
    /// Indexed chunk count in the store.
    pub indexed_chunks: u64,
    /// Whether a sync is currently running.
    pub in_progress: bool,
    /// Latest progress snapshot.
    pub progress: WorkspaceSyncProgress,
    /// Most recent completed sync report.
    pub last_report: Option<WorkspaceSyncReport>,
    /// Most recent sync failure message.
    pub last_error: Option<String>,
}

/// Mutable sync state shared between the actor and its background sync task.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceActorState {
    /// Whether a sync is currently running.
    pub in_progress: bool,
    /// Latest progress snapshot.
    pub progress: WorkspaceSyncProgress,
    /// Most recent completed sync report.
    pub last_report: Option<WorkspaceSyncReport>,
    /// Most recent sync failure message.
    pub last_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_core::{WorkspaceIndexStatus, WorkspacePortError};
    use parking_lot::Mutex;

    struct FakeEmbedder;

    #[async_trait::async_trait]
    impl EmbeddingProvider for FakeEmbedder {
        async fn embed_batch(
            &self,
            items: &[(&str, EmbeddingKind)],
        ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
            // Deterministic content-hash-derived vector so tests can assert
            // what was embedded without a real model.
            Ok(items
                .iter()
                .map(|(text, _)| {
                    let digest = blake3::hash(text.as_bytes());
                    let bytes = digest.as_bytes();
                    bytes
                        .iter()
                        .take(4)
                        .flat_map(|b| {
                            let v = f32::from(*b) / 255.0;
                            [v, 1.0 - v]
                        })
                        .collect()
                })
                .collect())
        }

        fn dimensions(&self) -> usize {
            8
        }

        fn model_name(&self) -> &'static str {
            "fake-model"
        }
    }

    struct MemoryPort {
        files: Mutex<Vec<WorkspaceFileRow>>,
        chunks: Mutex<Vec<(String, NewWorkspaceChunk)>>,
    }

    impl MemoryPort {
        fn new() -> Self {
            Self {
                files: Mutex::new(Vec::new()),
                chunks: Mutex::new(Vec::new()),
            }
        }

        fn paths(&self) -> Vec<String> {
            self.files.lock().iter().map(|f| f.path.clone()).collect()
        }
    }

    #[async_trait::async_trait]
    impl WorkspaceDocumentPort for MemoryPort {
        async fn list_workspace_files(&self) -> Result<Vec<WorkspaceFileRow>, WorkspacePortError> {
            Ok(self.files.lock().clone())
        }

        async fn replace_workspace_file(
            &self,
            file: &WorkspaceFileRow,
            chunks: &[NewWorkspaceChunk],
        ) -> Result<(), WorkspacePortError> {
            self.files.lock().retain(|f| f.path != file.path);
            self.files.lock().push(file.clone());
            self.chunks.lock().retain(|(path, _)| path != &file.path);
            for chunk in chunks {
                self.chunks.lock().push((file.path.clone(), chunk.clone()));
            }
            Ok(())
        }

        async fn rename_workspace_file(
            &self,
            old_path: &str,
            file: &WorkspaceFileRow,
        ) -> Result<bool, WorkspacePortError> {
            let mut files = self.files.lock();
            let Some(row) = files.iter_mut().find(|f| f.path == old_path) else {
                return Ok(false);
            };
            row.path = file.path.clone();
            row.root = file.root.clone();
            row.content_hash = file.content_hash.clone();
            Ok(true)
        }

        async fn delete_workspace_files(
            &self,
            paths: &[String],
        ) -> Result<usize, WorkspacePortError> {
            let mut files = self.files.lock();
            let before = files.len();
            files.retain(|f| !paths.contains(&f.path));
            Ok(before.saturating_sub(files.len()))
        }

        async fn prune_workspace_roots(
            &self,
            keep_roots: &[String],
        ) -> Result<usize, WorkspacePortError> {
            let mut files = self.files.lock();
            let before = files.len();
            files.retain(|f| keep_roots.contains(&f.root));
            Ok(before.saturating_sub(files.len()))
        }

        async fn search_workspace(
            &self,
            _query: &WorkspaceSearchQuery<'_>,
        ) -> Result<Vec<WorkspaceChunkHit>, WorkspacePortError> {
            Ok(Vec::new())
        }

        async fn workspace_index_status(&self) -> Result<WorkspaceIndexStatus, WorkspacePortError> {
            Ok(WorkspaceIndexStatus {
                indexed_files: u64::try_from(self.files.lock().len()).unwrap_or(u64::MAX),
                indexed_chunks: u64::try_from(self.chunks.lock().len()).unwrap_or(u64::MAX),
            })
        }
    }

    fn test_config(folders: Vec<String>) -> WorkspaceRagConfig {
        WorkspaceRagConfig {
            enabled: true,
            folders,
            ..WorkspaceRagConfig::default()
        }
    }

    async fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        tokio::fs::write(path, content).await.unwrap();
    }

    fn indexer(store: Arc<MemoryPort>) -> WorkspaceIndexer {
        WorkspaceIndexer::new(
            store as Arc<dyn WorkspaceDocumentPort>,
            Arc::new(FakeEmbedder),
        )
    }

    #[tokio::test]
    async fn sync_indexes_and_updates_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        write_file(&dir.path().join("notes.md"), "# Notes\nfirst content\n").await;
        let store = Arc::new(MemoryPort::new());
        let indexer = indexer(store.clone());

        let report = indexer
            .sync(
                &test_config(vec![root.clone()]),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(report.files_indexed, 1);
        assert_eq!(store.paths().len(), 1);
        assert!(store.paths()[0].ends_with("notes.md"));

        // Unchanged file is not re-embedded.
        let report = indexer
            .sync(
                &test_config(vec![root.clone()]),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(report.files_indexed, 0);
        assert_eq!(report.files_unchanged, 1);

        // Edited file is re-indexed.
        write_file(&dir.path().join("notes.md"), "# Notes\nchanged content\n").await;
        let report = indexer
            .sync(
                &test_config(vec![root.clone()]),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(report.files_indexed, 1);
        assert_eq!(report.files_unchanged, 0);
    }

    #[tokio::test]
    async fn sync_deletes_vanished_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        write_file(&dir.path().join("a.md"), "alpha\n").await;
        write_file(&dir.path().join("b.md"), "beta\n").await;
        let store = Arc::new(MemoryPort::new());
        let indexer = indexer(store.clone());
        indexer
            .sync(
                &test_config(vec![root.clone()]),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        tokio::fs::remove_file(dir.path().join("a.md"))
            .await
            .unwrap();
        let report = indexer
            .sync(
                &test_config(vec![root.clone()]),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(report.files_deleted, 1);
        assert_eq!(store.paths().len(), 1);
        assert!(store.paths()[0].ends_with("b.md"));
    }

    #[tokio::test]
    async fn sync_remaps_renames_without_reembedding() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        write_file(&dir.path().join("old.md"), "# Doc\nsame content\n").await;
        let store = Arc::new(MemoryPort::new());
        let indexer = indexer(store.clone());
        indexer
            .sync(
                &test_config(vec![root.clone()]),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        tokio::fs::rename(dir.path().join("old.md"), dir.path().join("new.md"))
            .await
            .unwrap();
        let report = indexer
            .sync(
                &test_config(vec![root.clone()]),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(report.files_renamed, 1);
        assert_eq!(report.files_indexed, 0);
        assert_eq!(store.paths().len(), 1);
        assert!(store.paths()[0].ends_with("new.md"));
    }

    #[tokio::test]
    async fn sync_respects_ignore_rules_and_binary_sniff() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        write_file(&dir.path().join("keep.md"), "keep me\n").await;
        write_file(&dir.path().join(".env"), "SECRET=1\n").await;
        write_file(&dir.path().join("blob.bin"), "a\x00b\n").await;
        write_file(&dir.path().join("ignored.exe"), "nope\n").await;
        let store = Arc::new(MemoryPort::new());
        let indexer = indexer(store.clone());
        let report = indexer
            .sync(
                &test_config(vec![root.clone()]),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(report.files_indexed, 1);
        assert_eq!(report.files_skipped, 3);
        assert_eq!(store.paths().len(), 1);
        assert!(store.paths()[0].ends_with("keep.md"));
    }

    #[tokio::test]
    async fn sync_skips_symlink_escape_and_dir_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        write_file(&outside.path().join("secret.md"), "outside secret\n").await;
        write_file(&dir.path().join("inside.md"), "inside\n").await;
        write_file(&dir.path().join("real.md"), "real target\n").await;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), dir.path().join("escape_dir")).unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("secret.md"),
                dir.path().join("escape_file.md"),
            )
            .unwrap();
            // An in-root file symlink is indexed: canonical-path containment
            // passes, so the symlink resolution must not drop it before the
            // check.
            std::os::unix::fs::symlink(dir.path().join("real.md"), dir.path().join("alias.md"))
                .unwrap();
        }
        let store = Arc::new(MemoryPort::new());
        let indexer = indexer(store.clone());
        let report = indexer
            .sync(
                &test_config(vec![root.clone()]),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        // The in-root alias resolves to real.md's canonical path, so both
        // entries collapse into one row (canonical identity); the escaping
        // file and directory symlinks are skipped.
        assert_eq!(report.files_indexed, 2);
        assert_eq!(store.paths().len(), 2);
        assert!(store.paths().iter().any(|p| p.ends_with("inside.md")));
        assert!(store.paths().iter().any(|p| p.ends_with("real.md")));
    }

    #[tokio::test]
    async fn sync_skips_oversized_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        write_file(&dir.path().join("big.md"), &"x".repeat(200)).await;
        let mut config = test_config(vec![root.clone()]);
        config.max_file_bytes = 100;
        let store = Arc::new(MemoryPort::new());
        let indexer = indexer(store.clone());
        let report = indexer
            .sync(&config, &CancellationToken::new(), None)
            .await
            .unwrap();
        assert_eq!(report.files_indexed, 0);
        assert_eq!(report.files_skipped, 1);
        assert!(store.paths().is_empty());
    }

    #[tokio::test]
    async fn empty_folders_wipe_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        write_file(&dir.path().join("a.md"), "alpha\n").await;
        let store = Arc::new(MemoryPort::new());
        let indexer = indexer(store.clone());
        indexer
            .sync(
                &test_config(vec![root.clone()]),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(store.paths().len(), 1);

        // An empty allowlist permits nothing: syncing prunes every row so
        // search and status agree with the configuration immediately.
        let report = indexer
            .sync(&test_config(vec![]), &CancellationToken::new(), None)
            .await
            .unwrap();
        assert_eq!(report.files_deleted, 1);
        assert!(store.paths().is_empty());
    }

    #[tokio::test]
    async fn unresolvable_roots_error_without_wiping() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        write_file(&dir.path().join("a.md"), "alpha\n").await;
        let store = Arc::new(MemoryPort::new());
        let indexer = indexer(store.clone());
        indexer
            .sync(
                &test_config(vec![root.clone()]),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();

        // A configured-but-missing folder is a config error, not a wipe: a
        // typo must never delete the index.
        let result = indexer
            .sync(
                &test_config(vec![
                    dir.path()
                        .join("does-not-exist")
                        .to_string_lossy()
                        .into_owned(),
                ]),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert!(matches!(result, Err(WorkspaceIndexError::NoRoots)));
        assert_eq!(store.paths().len(), 1);
    }

    #[tokio::test]
    async fn cancelled_sync_skips_delete_sweep() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_string_lossy().into_owned();
        write_file(&dir.path().join("a.md"), "alpha\n").await;
        let store = Arc::new(MemoryPort::new());
        let indexer = indexer(store.clone());
        indexer
            .sync(
                &test_config(vec![root.clone()]),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();

        // Cancel before the second sync: the vanished file's row must survive
        // (the sweep is gated on a complete, uncancelled walk).
        tokio::fs::remove_file(dir.path().join("a.md"))
            .await
            .unwrap();
        let token = CancellationToken::new();
        token.cancel();
        let report = indexer
            .sync(&test_config(vec![root.clone()]), &token, None)
            .await
            .unwrap();
        assert!(report.cancelled);
        assert_eq!(report.files_deleted, 0);
        assert_eq!(store.paths().len(), 1);
    }
}
