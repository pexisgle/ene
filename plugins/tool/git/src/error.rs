use ene_plugin_proto::ToolError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitError {
    #[error("not a git repository: {path}")]
    NotARepository { path: String },
    #[error("bare repositories are not supported: {path}")]
    BareRepository { path: String },
    #[error("path is outside the allowed workspace: {path}")]
    PathOutsideSandbox { path: String },
    #[error("invalid repository-relative path '{path}': {reason}")]
    InvalidPath { path: String, reason: String },
    #[error("{0}")]
    NotFound(String),
    #[error("repository has no commits")]
    NoCommits,
    #[error("git blob exceeds the read limit of {limit} bytes: {path}")]
    ReadLimitExceeded { path: String, limit: usize },
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
