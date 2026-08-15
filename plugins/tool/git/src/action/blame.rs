use crate::error::GitError;
use crate::output::{BlameLine, BlameOutput, format_time, short_oid, to_json};
use crate::sandbox::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;

const MAX_BLAME_LINES: usize = 2000;

/// Shows per-line commit attribution for a file in a git repository.
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[serde(rename_all = "camelCase")]
#[tool(
    namespace = "git",
    name = "blame",
    summary = "Show per-line commit attribution for a file in a git repository.",
    description = "Returns each line of a committed file with the commit that last changed it (full and short oid, author, timestamp, subject) as structured JSON. The blame reflects the committed HEAD version, not uncommitted working tree edits. The repository path must lie inside the configured workspace.",
    category = "Utility",
    keywords_primary = "git, blame, annotate, lines, author, commit",
    side_effects = "ReadOnly"
)]
pub struct BlameAction {
    /// Path to the git repository working tree (default: current directory).
    #[serde(default)]
    #[arg(
        default = ".",
        description = "Path to the git repository (default: current directory)."
    )]
    path: Option<String>,
    /// Repository-relative path to the file to blame.
    file: String,
    /// First line to report (1-based, inclusive).
    #[serde(default)]
    #[arg(minimum = 1)]
    start_line: Option<u32>,
    /// Last line to report (1-based, inclusive; default: last line of the file).
    #[serde(default)]
    #[arg(minimum = 1)]
    end_line: Option<u32>,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl BlameAction {
    /// Creates a blame action using the shared sandbox scope.
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            path: None,
            file: String::new(),
            start_line: None,
            end_line: None,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let scope = resolve_sandbox(&self.sandbox);
        let repo = scope
            .resolve_repo(self.path.as_deref(), "git.blame")
            .await?;
        let workdir = repo.workdir.to_string_lossy().into_owned();
        scope.validate_relative_path(&self.file)?;

        let broker = crate::broker::broker();
        let size_run = broker
            .run_git(
                &workdir,
                &["cat-file", "-s", &format!("HEAD:{}", self.file)],
            )
            .await?;
        if !size_run.ok() {
            return Err(ToolError::from(GitError::NotFound(format!(
                "{file} not found in HEAD",
                file = self.file
            ))));
        }
        let blob_size = size_run.stdout.trim().parse::<usize>().unwrap_or(0);
        if blob_size > scope.max_read_bytes() {
            return Err(ToolError::from(GitError::ReadLimitExceeded {
                path: self.file.clone(),
                limit: scope.max_read_bytes(),
            }));
        }
        let total = blob_size_count(&workdir, &self.file, &broker).await?;

        let start = usize::try_from(self.start_line.unwrap_or(1)).unwrap_or(1);
        let end = self
            .end_line
            .map_or(total, |n| usize::try_from(n).unwrap_or(total));
        if start < 1 || end < start || end > total {
            return Err(ToolError::InvalidArguments {
                message: format!("invalid line range {start}-{end}: file has {total} lines"),
            });
        }

        let count = end.saturating_sub(start).saturating_add(1);
        let (reported_end, truncated) = if count > MAX_BLAME_LINES {
            (
                start.saturating_add(MAX_BLAME_LINES).saturating_sub(1),
                true,
            )
        } else {
            (end, false)
        };

        let range = format!("-L {start},{reported_end}");
        let blame_run = broker
            .run_git(
                &workdir,
                &["blame", "--porcelain", &range, "--", &self.file],
            )
            .await?;
        if !blame_run.ok() {
            return Err(crate::sandbox::git_run_error(&blame_run));
        }
        let out_lines = parse_porcelain(&blame_run.stdout, start, reported_end);

        to_json(&BlameOutput {
            repo: workdir,
            file: self.file.clone(),
            lines: out_lines,
            truncated,
        })
    }
}

/// Counts the lines of the committed file via `git show` output.
async fn blob_size_count(
    workdir: &str,
    file: &str,
    broker: &crate::broker::GitBroker,
) -> Result<usize, ToolError> {
    let show = broker
        .run_git(workdir, &["show", &format!("HEAD:{file}")])
        .await?;
    if !show.ok() {
        return Err(ToolError::from(GitError::NotFound(format!(
            "{file} not found in HEAD"
        ))));
    }
    Ok(show.stdout.lines().count())
}

/// Parses `git blame --porcelain` output for the requested line range.
fn parse_porcelain(output: &str, start: usize, end: usize) -> Vec<BlameLine> {
    let mut lines = Vec::new();
    let mut commit = String::new();
    let mut author = String::new();
    let mut author_email = String::new();
    let mut author_time_unix = 0_i64;
    let mut author_tz = String::new();
    let mut subject = String::new();
    let mut final_line = 0_usize;
    for raw in output.lines() {
        if let Some(rest) = raw.strip_prefix('\t') {
            if (start..=end).contains(&final_line) {
                lines.push(BlameLine {
                    line: final_line,
                    text: rest.to_string(),
                    commit: commit.clone(),
                    short_commit: short_oid(&commit),
                    author: author.clone(),
                    author_email: author_email.clone(),
                    author_time: format_time(author_time_unix, &author_tz),
                    subject: subject.clone(),
                });
            }
            continue;
        }
        let first = raw.split_whitespace().next().unwrap_or_default();
        if first.len() == 40 && first.chars().all(|c| c.is_ascii_hexdigit()) {
            // Commit line: `<oid> <orig> <final> [<count>]`.
            let mut parts = raw.split_whitespace();
            commit = parts.next().unwrap_or_default().to_string();
            final_line = parts.nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
            continue;
        }
        if let Some((key, value)) = raw.split_once(' ') {
            // Header line: `<key> <value>`.
            match key {
                "author" => author = value.to_string(),
                "author-mail" => {
                    author_email = value
                        .trim()
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_string();
                }
                "author-time" => {
                    author_time_unix = value.trim().parse().unwrap_or(0);
                }
                "author-tz" => author_tz = value.trim().to_string(),
                "summary" => subject = value.to_string(),
                _ => {}
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::BlameAction;
    use crate::fixture::{RepoFixture, sandbox_ref, scope_allowing};
    use serde_json::Value;

    fn action(
        fixture: &RepoFixture,
        file: &str,
        start_line: Option<u32>,
        end_line: Option<u32>,
    ) -> BlameAction {
        BlameAction {
            path: Some(fixture.path()),
            file: file.to_string(),
            start_line,
            end_line,
            sandbox: sandbox_ref(scope_allowing(&fixture.path())),
        }
    }

    #[tokio::test]
    async fn attributes_every_line_to_the_introducing_commit() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\ntwo\nthree\n");
        let oid = fixture.commit_all("initial");

        let out: Value =
            serde_json::from_str(&action(&fixture, "a.txt", None, None).run().await.unwrap())
                .unwrap();
        let lines = out["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["line"], 1);
        assert_eq!(lines[0]["text"], "one");
        assert_eq!(lines[0]["commit"], oid.to_string());
        assert_eq!(lines[2]["text"], "three");
        assert_eq!(lines[2]["author"], "Test User");
        assert_eq!(lines[2]["subject"], "initial");
    }

    #[tokio::test]
    async fn reattribution_after_second_commit() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\ntwo\nthree\n");
        fixture.commit_all("initial");
        fixture.write("a.txt", "one\nTWO\nthree\n");
        let second = fixture.commit_all("rewrite line two");

        let out: Value =
            serde_json::from_str(&action(&fixture, "a.txt", None, None).run().await.unwrap())
                .unwrap();
        let lines = out["lines"].as_array().unwrap();
        assert_eq!(lines[1]["commit"], second.to_string());
        assert_ne!(lines[0]["commit"], lines[1]["commit"]);
    }

    #[tokio::test]
    async fn line_range_limits_output() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\ntwo\nthree\n");
        fixture.commit_all("initial");

        let out: Value = serde_json::from_str(
            &action(&fixture, "a.txt", Some(2), Some(2))
                .run()
                .await
                .unwrap(),
        )
        .unwrap();
        let lines = out["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["line"], 2);
        assert_eq!(lines[0]["text"], "two");
    }

    #[tokio::test]
    async fn invalid_line_range_is_rejected() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("initial");
        let result = action(&fixture, "a.txt", Some(5), None).run().await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid line range"), "{err}");
    }

    #[tokio::test]
    async fn missing_file_is_rejected() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("initial");
        let result = action(&fixture, "missing.txt", None, None).run().await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "{err}");
    }
}
