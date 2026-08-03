use crate::error::GitError;
use crate::output::short_oid;
use ene_plugin_proto::ToolError;
use git2::Repository;

/// Maps a `HEAD`-resolution failure to `NoCommits` for an unborn `HEAD`,
/// otherwise to the underlying git2 error.
pub(crate) fn map_head_error(e: git2::Error) -> ToolError {
    if matches!(
        e.code(),
        git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
    ) || e.class() == git2::ErrorClass::Reference
    {
        ToolError::from(GitError::NoCommits)
    } else {
        ToolError::from(GitError::Git2(e))
    }
}

/// Returns the current branch shorthand and, when detached, the short `HEAD`
/// oid.
pub(crate) fn head_info(repo: &Repository) -> (Option<String>, Option<String>) {
    let Ok(head) = repo.head() else {
        return (None, None);
    };
    if head.is_branch() {
        (head.shorthand().ok().map(str::to_string), None)
    } else {
        (None, head.target().map(short_oid))
    }
}
