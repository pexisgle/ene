use crate::error::GitError;
use crate::output::{BlameLine, BlameOutput, format_time, short_oid, to_json};
use crate::sandbox::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;
use std::path::Path;

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
        let (repo, workdir) = scope.resolve_repo(self.path.as_deref())?;
        scope.validate_relative_path(&self.file)?;

        let head = repo.head().map_err(super::common::map_head_error)?;
        let tree = head.peel_to_tree()?;
        let blob = tree
            .get_path(Path::new(&self.file))
            .map_err(|e| {
                if e.code() == git2::ErrorCode::NotFound {
                    ToolError::from(GitError::NotFound(format!(
                        "{file} not found in HEAD",
                        file = self.file
                    )))
                } else {
                    ToolError::from(GitError::Git2(e))
                }
            })?
            .to_object(&repo)?
            .peel_to_blob()?;
        let lines: Vec<&str> = String::from_utf8_lossy(blob.content()).lines().collect();
        let total = lines.len();

        let start = usize::try_from(self.start_line.unwrap_or(1)).unwrap_or(1);
        let end = self
            .end_line
            .map(|n| usize::try_from(n).unwrap_or(total))
            .unwrap_or(total);
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

        let blame = repo.blame_file(Path::new(&self.file), None)?;
        let mut out_lines: Vec<BlameLine> = Vec::new();
        for n in start..=reported_end {
            let hunk = blame.get_line(n).ok_or_else(|| {
                ToolError::from(GitError::NotFound(format!(
                    "no blame data for line {n} in {file}",
                    file = self.file
                )))
            })?;
            let commit = hunk.orig_commit_id();
            let (author, author_email, author_time) =
                match hunk.orig_signature().or_else(|| hunk.final_signature()) {
                    Some(sig) => (
                        sig.name().unwrap_or_default().to_string(),
                        sig.email().unwrap_or_default().to_string(),
                        format_time(sig.when()),
                    ),
                    None => (String::new(), String::new(), String::new()),
                };
            out_lines.push(BlameLine {
                line: n,
                text: lines
                    .get(n.saturating_sub(1))
                    .copied()
                    .unwrap_or_default()
                    .to_string(),
                commit: commit.to_string(),
                short_commit: short_oid(commit),
                author,
                author_email,
                author_time,
                subject: hunk
                    .summary()
                    .ok()
                    .flatten()
                    .unwrap_or_default()
                    .to_string(),
            });
        }

        to_json(&BlameOutput {
            repo: workdir.display().to_string(),
            file: self.file.clone(),
            lines: out_lines,
            truncated,
        })
    }
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
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("initial");
        let result = action(&fixture, "a.txt", Some(5), None).run().await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid line range"), "{err}");
    }

    #[tokio::test]
    async fn missing_file_is_rejected() {
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("initial");
        let result = action(&fixture, "missing.txt", None, None).run().await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "{err}");
    }
}
