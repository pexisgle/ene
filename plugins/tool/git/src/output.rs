use ene_plugin_proto::ToolError;
use serde::Serialize;

/// Formats a Unix timestamp plus an RFC 822-style offset (`+0900`) as
/// RFC3339, preserving the commit's offset.
pub fn format_time(unix_seconds: i64, offset: &str) -> String {
    let offset_minutes: i32 = parse_tz_offset(offset);
    let offset_seconds = offset_minutes.saturating_mul(60);
    let dt = chrono::DateTime::from_timestamp(unix_seconds, 0)
        .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
    match chrono::FixedOffset::east_opt(offset_seconds) {
        Some(offset) => dt.with_timezone(&offset).to_rfc3339(),
        None => dt.with_timezone(&chrono::Utc).to_rfc3339(),
    }
}

/// Parses `+HHMM` / `-HHMM` into minutes east of UTC.
fn parse_tz_offset(raw: &str) -> i32 {
    let raw = raw.trim();
    if raw.len() != 5 {
        return 0;
    }
    let (sign, digits) = raw.split_at(1);
    let hours: i32 = digits[..2].parse().unwrap_or(0);
    let minutes: i32 = digits[2..].parse().unwrap_or(0);
    let total = hours.saturating_mul(60).saturating_add(minutes);
    match sign {
        "-" => -total,
        _ => total,
    }
}

#[derive(Debug, Serialize)]
pub struct StatusFileEntry {
    /// Repository-relative file path.
    pub path: String,
    /// Index-vs-`HEAD` change letter (`A`/`M`/`D`/`R`/`T`), or `null`.
    pub staged: Option<&'static str>,
    /// Worktree-vs-index change letter (`M`/`D`/`R`/`T`), or `null`.
    pub unstaged: Option<&'static str>,
    pub untracked: bool,
    pub conflicted: bool,
}

#[derive(Debug, Serialize)]
pub struct StatusOutput {
    /// Absolute working-tree path of the repository.
    pub repo: String,
    /// Current branch name, or `null` on a detached/unborn `HEAD`.
    pub branch: Option<String>,
    /// Short `HEAD` oid when the `HEAD` is detached, otherwise `null`.
    pub detached_head: Option<String>,
    pub clean: bool,
    pub entries: Vec<StatusFileEntry>,
    /// Whether the output was truncated at the entry cap.
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct DiffOutput {
    /// Absolute working-tree path of the repository.
    pub repo: String,
    /// Whether the diff compares the index against `HEAD` (staged).
    pub staged: bool,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
    pub summary: String,
    /// Unified-diff patch text when requested.
    pub patch: Option<String>,
    /// Whether the patch was truncated at the output cap.
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct Person {
    pub name: String,
    pub email: String,
    /// RFC3339 timestamp with the original offset.
    pub time: String,
}

#[derive(Debug, Serialize)]
pub struct LogEntry {
    pub oid: String,
    pub short_oid: String,
    /// First paragraph of the commit message.
    pub subject: String,
    /// Rest of the commit message, or `null`.
    pub body: Option<String>,
    pub author: Person,
    pub committer: Person,
    pub parents: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LogOutput {
    /// Absolute working-tree path of the repository.
    pub repo: String,
    /// Branch walked, or HEAD's branch when none was requested.
    pub branch: Option<String>,
    /// Commits in reverse chronological order.
    pub entries: Vec<LogEntry>,
}

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
    pub current: bool,
}

#[derive(Debug, Serialize)]
pub struct BranchOutput {
    /// Absolute working-tree path of the repository.
    pub repo: String,
    /// Current branch name, or `null` on a detached/unborn HEAD.
    pub current: Option<String>,
    /// Short HEAD oid when the HEAD is detached, otherwise `null`.
    pub detached_head: Option<String>,
    pub branches: Vec<BranchEntry>,
}

#[derive(Debug, Serialize)]
pub struct RemoteEntry {
    pub name: String,
    /// Fetch URL, or `null`.
    pub fetch_url: Option<String>,
    /// Push URL, or `null`.
    pub push_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RemoteOutput {
    /// Absolute working-tree path of the repository.
    pub repo: String,
    pub remotes: Vec<RemoteEntry>,
}

#[derive(Debug, Serialize)]
pub struct BlameLine {
    /// 1-based line number in the file.
    pub line: usize,
    /// Line content from the committed version.
    pub text: String,
    /// Full oid of the commit that last changed the line.
    pub commit: String,
    pub short_commit: String,
    pub author: String,
    pub author_email: String,
    /// RFC3339 author timestamp with the original offset.
    pub author_time: String,
    /// Subject of the commit that last changed the line.
    pub subject: String,
}

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

pub fn to_json<T: Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| ToolError::internal(format!("failed to serialize tool output: {e}")))
}

pub fn short_oid(oid: &str) -> String {
    oid.chars().take(7).collect()
}
