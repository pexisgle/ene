use ene_plugin_proto::ToolError;
use thiserror::Error;

/// Errors surfaced by the read-only git tools.
#[derive(Debug, Error)]
pub enum GitError {
    /// The resolved path is not inside a git working tree.
    #[error("not a git repository: {path}")]
    NotARepository {
        /// Path the caller asked to inspect.
        path: String,
    },
    /// The discovered repository has no working tree.
    #[error("bare repositories are not supported: {path}")]
    BareRepository {
        /// Repository path reported by libgit2.
        path: String,
    },
    /// The path or discovered repository root lies outside the workspace.
    #[error("path is outside the allowed workspace: {path}")]
    PathOutsideSandbox {
        /// Offending path.
        path: String,
    },
    /// A repository-relative argument (diff path, blame file) is malformed.
    #[error("invalid repository-relative path '{path}': {reason}")]
    InvalidPath {
        /// Path as passed by the caller.
        path: String,
        /// Why the path was rejected.
        reason: String,
    },
    /// A repository object (file, ref, commit) does not exist.
    #[error("{0}")]
    NotFound(String),
    /// The repository has no commits yet (unborn `HEAD`).
    #[error("repository has no commits")]
    NoCommits,
    /// Underlying libgit2 failure.
    #[error(transparent)]
    Git2(#[from] git2::Error),
}

impl From<GitError> for ToolError {
    fn from(e: GitError) -> Self {
        match e {
            GitError::InvalidPath { .. } => ToolError::InvalidArguments {
                message: e.to_string(),
            },
            GitError::PathOutsideSandbox { .. } => ToolError::sandbox_violation(e.to_string()),
            other => ToolError::execution_failed(other.to_string()),
        }
    }
}

impl From<git2::Error> for ToolError {
    fn from(e: git2::Error) -> Self {
        ToolError::execution_failed(format!("git operation failed: {e}"))
    }
}
