//! Workspace RAG configuration (settings section `rag.workspace`).

use ene_config::{ConfigTarget, HasConfigKey};

fn default_include_extensions() -> Vec<String> {
    [
        "md", "markdown", "txt", "rs", "toml", "json", "yaml", "yml", "py", "ts", "js", "tsx",
        "jsx", "html", "css", "sh", "xml", "ini", "cfg", "csv",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_ignore_globs() -> Vec<String> {
    [
        ".git/**",
        "node_modules/**",
        "target/**",
        "dist/**",
        ".venv/**",
        "**/.env",
        "**/.env.*",
        "*.gguf",
        "*.safetensors",
        "*.ckpt",
        "*.pth",
        "*.onnx",
        "*.bin",
        "*.db",
        "*.db-wal",
        "*.db-shm",
        "assets/models/**",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

const fn default_max_file_bytes() -> usize {
    1024 * 1024
}

const fn default_chunk_chars() -> usize {
    1200
}

const fn default_chunk_overlap_chars() -> usize {
    200
}

const fn default_max_chunks_per_file() -> usize {
    256
}

const fn default_top_k() -> usize {
    8
}

const fn default_final_n() -> usize {
    4
}

const fn default_min_similarity() -> f32 {
    0.20
}

/// Document/workspace RAG configuration.
///
/// Privacy-first defaults: the feature is disabled and no folders are
/// permitted until the operator explicitly enables them.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct WorkspaceRagConfig {
    /// Whether workspace indexing and prompt injection are enabled.
    pub enabled: bool,
    /// Folders allowed to be scanned and searched (canonicalized at use).
    /// Only these folders are ever read.
    pub folders: Vec<String>,
    /// File extensions (without dot, case-insensitive) eligible for indexing.
    pub include_extensions: Vec<String>,
    /// Glob patterns (relative to each folder) excluded from scanning.
    pub ignore_globs: Vec<String>,
    /// Files larger than this many bytes are skipped ("huge files").
    pub max_file_bytes: usize,
    /// Target characters per chunk.
    pub chunk_chars: usize,
    /// Characters of the previous chunk repeated at the next chunk's start.
    pub chunk_overlap_chars: usize,
    /// Hard cap on chunks per file; files exceeding it are skipped entirely.
    pub max_chunks_per_file: usize,
    /// Vector-search over-fetch before scoring/truncation.
    pub top_k: usize,
    /// Maximum chunks injected into a prompt (or returned by CLI search).
    pub final_n: usize,
    /// Minimum hybrid score for a hit to be returned.
    pub min_similarity: f32,
    /// Whether to start a background sync when the runtime opens.
    pub sync_on_startup: bool,
}

impl Default for WorkspaceRagConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            folders: Vec::new(),
            include_extensions: default_include_extensions(),
            ignore_globs: default_ignore_globs(),
            max_file_bytes: default_max_file_bytes(),
            chunk_chars: default_chunk_chars(),
            chunk_overlap_chars: default_chunk_overlap_chars(),
            max_chunks_per_file: default_max_chunks_per_file(),
            top_k: default_top_k(),
            final_n: default_final_n(),
            min_similarity: default_min_similarity(),
            sync_on_startup: false,
        }
    }
}

impl HasConfigKey for WorkspaceRagConfig {
    const KEY: &'static str = "workspace";
    const TARGET: ConfigTarget = ConfigTarget::Settings;
    fn path() -> &'static [&'static str] {
        &["rag", "workspace"]
    }
}

const _: () = {
    #[ctor::ctor(unsafe)]
    fn register() {
        ene_config::register_config_schema::<WorkspaceRagConfig>(
            ConfigTarget::Settings,
            Some("rag"),
        );
    }
};
