use serde::Serialize;

/// Formats a libgit2 timestamp as RFC3339, preserving the commit's offset.
pub fn format_time(time: git2::Time) -> String {
    let offset_seconds = time.offset_minutes().saturating_mul(60);
    let offset = chrono::FixedOffset::east_opt(offset_seconds).unwrap_or_else(|| chrono::Utc.fix());
    let dt = chrono::DateTime::from_timestamp(time.seconds(), 0)
        .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
    dt.with_timezone(&offset).to_rfc3339()
}

/// One file's status entry.
#[derive(Debug, Serialize)]
pub struct StatusFileEntry {
    /// Repository-relative file path.
    pub path: String,
    /// Index-vs-`HEAD` change letter (`A`/`M`/`D`/`R`/`T`), or `null`.
    pub staged: Option<&'static str>,
    /// Worktree-vs-index change letter (`M`/`D`/`R`/`T`), or `null`.
    pub unstaged: Option<&'static str>,
    /// Whether the file is untracked.
    pub untracked: bool,
    /// Whether the file has unresolved merge conflicts.
    pub conflicted: bool,
}

/// Result of `git.status`.
#[derive(Debug, Serialize)]
pub struct StatusOutput {
    /// Absolute working-tree path of the repository.
    pub repo: String,
    /// Current branch name, or `null` on a detached/unborn `HEAD`.
    pub branch: Option<String>,
    /// Short `HEAD` oid when the `HEAD` is detached, otherwise `null`.
    pub detached_head: Option<String>,
    /// Whether the working tree and index are clean.
    pub clean: bool,
    /// Per-file status entries.
    pub entries: Vec<StatusFileEntry>,
    /// Whether the output was truncated at the entry cap.
    pub truncated: bool,
}

/// Result of `git.diff`.
#[derive(Debug, Serialize)]
pub struct DiffOutput {
    /// Absolute working-tree path of the repository.
    pub repo: String,
    /// Whether the diff compares the index against `HEAD` (staged).
    pub staged: bool,
    /// Number of files changed.
    pub files_changed: usize,
    /// Number of added lines.
    pub insertions: usize,
    /// Number of deleted lines.
    pub deletions: usize,
    /// Human-readable stat summary.
    pub summary: String,
    /// Unified-diff patch text when requested.
    pub patch: Option<String>,
    /// Whether the patch was truncated at the output cap.
    pub truncated: bool,
}

/// Author or committer of a commit.
#[derive(Debug, Serialize)]
pub struct Person {
    /// Name from the signature.
    pub name: String,
    /// Email from the signature.
    pub email: String,
    /// RFC3339 timestamp with the original offset.
    pub time: String,
}

/// One commit in `git.log` output.
#[derive(Debug, Serialize)]
pub struct LogEntry {
    /// Full commit oid.
    pub oid: String,
    /// 7-character abbreviated oid.
    pub short_oid: String,
    /// First paragraph of the commit message.
    pub subject: String,
    /// Rest of the commit message, or `null`.
    pub body: Option<String>,
    /// Author signature.
    pub author: Person,
    /// Committer signature.
    pub committer: Person,
    /// Full parent oids.
    pub parents: Vec<String>,
}

/// Result of `git.log`.
#[derive(Debug, Serialize)]
pub struct LogOutput {
    /// Absolute working-tree path of the repository.
    pub repo: String,
    /// Branch walked, or HEAD's branch when none was requested.
    pub branch: Option<String>,
    /// Commits in reverse chronological order.
    pub entries: Vec<LogEntry>,
}

/// One branch in `git.branch` output.
#[derive(Debug, Serialize)]
pub struct BranchEntry {
    /// Branch name (without the `refs/heads/` prefix).
    pub name: String,
    /// Upstream branch name, or `null`.
    pub upstream: Option<String>,
    /// Commits in this branch not in its upstream, or `null`.
    pub ahead: Option<usize>,
    /// Commits in the upstream not in this branch, or `null`.
    pub behind: Option<usize>,
    /// Whether this is the checked-out branch.
    pub current: bool,
}

/// Result of `git.branch`.
#[derive(Debug, Serialize)]
pub struct BranchOutput {
    /// Absolute working-tree path of the repository.
    pub repo: String,
    /// Current branch name, or `null` on a detached/unborn HEAD.
    pub current: Option<String>,
    /// Short HEAD oid when the HEAD is detached, otherwise `null`.
    pub detached_head: Option<String>,
    /// Branch entries.
    pub branches: Vec<BranchEntry>,
}

/// One remote in `git.remote` output.
#[derive(Debug, Serialize)]
pub struct RemoteEntry {
    /// Remote name.
    pub name: String,
    /// Fetch URL, or `null`.
    pub fetch_url: Option<String>,
    /// Push URL, or `null`.
    pub push_url: Option<String>,
}

/// Result of `git.remote`.
#[derive(Debug, Serialize)]
pub struct RemoteOutput {
    /// Absolute working-tree path of the repository.
    pub repo: String,
    /// Configured remotes.
    pub remotes: Vec<RemoteEntry>,
}

/// One line of `git.blame` output.
#[derive(Debug, Serialize)]
pub struct BlameLine {
    /// 1-based line number in the file.
    pub line: usize,
    /// Line content from the committed version.
    pub text: String,
    /// Full oid of the commit that last changed the line.
    pub commit: String,
    /// 7-character abbreviated oid.
    pub short_commit: String,
    /// Author name.
    pub author: String,
    /// Author email.
    pub author_email: String,
    /// RFC3339 author timestamp with the original offset.
    pub author_time: String,
    /// Subject of the commit that last changed the line.
    pub subject: String,
}

/// Result of `git.blame`.
#[derive(Debug, Serialize)]
pub struct BlameOutput {
    /// Absolute working-tree path of the repository.
    pub repo: String,
    /// Repository-relative file path blamed.
    pub file: String,
    /// Per-line attribution, in line order.
    pub lines: Vec<BlameLine>,
    /// Whether the output was truncated at the line cap.
    pub truncated: bool,
}

/// Serializes a tool output struct as pretty-printed JSON.
pub fn to_json<T: Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| ToolError::internal(format!("failed to serialize tool output: {e}")))
}

/// Formats a 7-character abbreviated oid from the raw 20 bytes.
pub fn short_oid(oid: git2::Oid) -> String {
    oid.as_bytes()[..7]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
