use crate::output::{StatusFileEntry, StatusOutput, to_json};
use crate::sandbox::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;

const MAX_STATUS_ENTRIES: usize = 2000;

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[serde(rename_all = "camelCase")]
#[tool(
    namespace = "git",
    name = "status",
    summary = "Show the working tree status of a git repository.",
    description = "Returns the current branch (or detached HEAD) and per-file status of a git repository as structured JSON: staged/unstaged change letters, untracked, and conflict flags. The repository path must lie inside the configured workspace.",
    category = "Utility",
    keywords_primary = "git, status, branch, staged, untracked, conflicted, repository",
    side_effects = "ReadOnly"
)]
pub struct StatusAction {
    #[serde(default)]
    #[arg(
        default = ".",
        description = "Path to the git repository (default: current directory)."
    )]
    path: Option<String>,
    #[serde(default = "default_true")]
    #[arg(default = "true")]
    include_untracked: bool,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

const fn default_true() -> bool {
    true
}

impl StatusAction {
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            path: None,
            include_untracked: true,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let scope = resolve_sandbox(&self.sandbox);
        let repo = scope
            .resolve_repo(self.path.as_deref(), "git.status")
            .await?;
        let workdir = repo.workdir.to_string_lossy().into_owned();

        let untracked_mode = if self.include_untracked {
            "--untracked-files=normal"
        } else {
            "--untracked-files=no"
        };
        let status_run = crate::broker::broker()
            .run_git(&workdir, &["status", "--porcelain=v1", untracked_mode])
            .await?;
        let status_text = if status_run.ok() {
            status_run.stdout
        } else {
            return Err(crate::sandbox::git_run_error(&status_run));
        };
        let (branch, detached_head) = super::common::head_info(&workdir).await;

        let mut entries: Vec<StatusFileEntry> = Vec::new();
        let status_lines = status_text.lines();
        let total = status_lines.clone().count();
        for line in status_lines {
            if entries.len() >= MAX_STATUS_ENTRIES {
                break;
            }
            let Some(parsed) = parse_status_line(line) else {
                continue;
            };
            entries.push(StatusFileEntry {
                path: parsed.path,
                staged: parsed.staged,
                unstaged: parsed.unstaged,
                untracked: parsed.untracked,
                conflicted: parsed.conflicted,
            });
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        to_json(&StatusOutput {
            repo: workdir,
            branch,
            detached_head,
            clean: status_text.is_empty(),
            entries,
            truncated: total > MAX_STATUS_ENTRIES,
        })
    }
}

struct ParsedStatus {
    path: String,
    staged: Option<&'static str>,
    unstaged: Option<&'static str>,
    untracked: bool,
    conflicted: bool,
}

fn parse_status_line(line: &str) -> Option<ParsedStatus> {
    let bytes = line.as_bytes();
    if bytes.len() < 4 || bytes[2] != b' ' {
        return None;
    }
    let index = code_for(bytes[0]);
    let worktree = code_for(bytes[1]);
    // Renames use `XY orig -> dest`; report the destination like libgit2's
    // `entry.path()` did.
    let path = line[3..]
        .split_once(" -> ")
        .map_or(line[3..].to_string(), |(_, dest)| dest.to_string());
    Some(ParsedStatus {
        path,
        staged: index,
        unstaged: worktree,
        untracked: worktree == Some("?"),
        conflicted: matches!(
            (bytes[0], bytes[1]),
            (b'U', b'U' | b'A' | b'D') | (b'A', b'A' | b'U') | (b'D', b'D' | b'U')
        ),
    })
}

/// Maps a porcelain status letter to the output's change-letter contract.
fn code_for(byte: u8) -> Option<&'static str> {
    match byte {
        b'A' => Some("A"),
        b'M' => Some("M"),
        b'D' => Some("D"),
        b'R' => Some("R"),
        b'T' => Some("T"),
        b'C' => Some("C"),
        b'U' => Some("U"),
        b'?' => Some("?"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::StatusAction;
    use crate::fixture::{RepoFixture, sandbox_ref, scope_allowing};
    use serde_json::Value;

    async fn run(fixture: &RepoFixture, include_untracked: bool) -> Value {
        let action = StatusAction {
            path: Some(fixture.path()),
            include_untracked,
            sandbox: sandbox_ref(scope_allowing(&fixture.path())),
        };
        let out = action.run().await.unwrap();
        serde_json::from_str(&out).unwrap()
    }

    #[tokio::test]
    async fn clean_repo_reports_clean() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        let out = run(&fixture, true).await;
        assert_eq!(out["clean"], true);
        assert_eq!(out["entries"].as_array().unwrap().len(), 0);
        assert!(
            out["branch"]
                .as_str()
                .is_some_and(|branch| !branch.is_empty()),
            "the current branch name is reported (its default depends on the host git config)"
        );
    }

    #[tokio::test]
    async fn untracked_and_modified_files_are_listed() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("tracked.txt", "v1\n");
        fixture.commit_all("first");
        fixture.write("tracked.txt", "v2\n");
        fixture.write("new.txt", "hello\n");

        let out = run(&fixture, true).await;
        let entries = out["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        let tracked = entries.iter().find(|e| e["path"] == "tracked.txt").unwrap();
        assert_eq!(tracked["unstaged"], "M");
        let untracked = entries.iter().find(|e| e["path"] == "new.txt").unwrap();
        assert_eq!(untracked["untracked"], true);

        let out = run(&fixture, false).await;
        let entries = out["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["path"], "tracked.txt");
    }

    #[tokio::test]
    async fn staged_and_deleted_files_are_listed() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.write("gone.txt", "bye\n");
        fixture.commit_all("first");
        fixture.write("a.txt", "two\n");
        fixture.stage("a.txt");
        fixture.delete("gone.txt");

        let out = run(&fixture, true).await;
        let entries = out["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        let staged = entries.iter().find(|e| e["path"] == "a.txt").unwrap();
        assert_eq!(staged["staged"], "M");
        let deleted = entries.iter().find(|e| e["path"] == "gone.txt").unwrap();
        assert_eq!(deleted["unstaged"], "D");
    }
}
