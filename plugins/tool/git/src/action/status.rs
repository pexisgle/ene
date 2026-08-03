use crate::error::from_git2;
use crate::output::{StatusFileEntry, StatusOutput, to_json};
use crate::sandbox::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;
use git2::{Status, StatusOptions};

const MAX_STATUS_ENTRIES: usize = 2000;

/// Shows the working tree status of a git repository.
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
    /// Path to the git repository working tree (default: current directory).
    #[serde(default)]
    #[arg(
        default = ".",
        description = "Path to the git repository (default: current directory)."
    )]
    path: Option<String>,
    /// Whether to include untracked files in the output.
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
    /// Creates a status action using the shared sandbox scope.
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            path: None,
            include_untracked: true,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let scope = resolve_sandbox(&self.sandbox);
        let (repo, workdir) = scope.resolve_repo(self.path.as_deref(), "git.status")?;

        let mut opts = StatusOptions::new();
        opts.include_untracked(self.include_untracked)
            .recurse_untracked_dirs(self.include_untracked)
            .include_ignored(false);
        let statuses = repo.statuses(Some(&mut opts)).map_err(from_git2)?;

        let mut entries: Vec<StatusFileEntry> = Vec::new();
        for entry in statuses.iter() {
            if entries.len() >= MAX_STATUS_ENTRIES {
                break;
            }
            let status = entry.status();
            entries.push(StatusFileEntry {
                path: entry.path().map(str::to_string).unwrap_or_default(),
                staged: index_code(status),
                unstaged: worktree_code(status),
                untracked: status.is_wt_new(),
                conflicted: status.is_conflicted(),
            });
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        let (branch, detached_head) = super::common::head_info(&repo);
        to_json(&StatusOutput {
            repo: workdir.display().to_string(),
            branch,
            detached_head,
            clean: statuses.is_empty(),
            entries,
            truncated: statuses.len() > MAX_STATUS_ENTRIES,
        })
    }
}

fn index_code(status: Status) -> Option<&'static str> {
    if status.is_index_new() {
        Some("A")
    } else if status.is_index_modified() {
        Some("M")
    } else if status.is_index_deleted() {
        Some("D")
    } else if status.is_index_renamed() {
        Some("R")
    } else if status.is_index_typechange() {
        Some("T")
    } else {
        None
    }
}

fn worktree_code(status: Status) -> Option<&'static str> {
    if status.is_wt_modified() {
        Some("M")
    } else if status.is_wt_deleted() {
        Some("D")
    } else if status.is_wt_renamed() {
        Some("R")
    } else if status.is_wt_typechange() {
        Some("T")
    } else {
        None
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
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        let out = run(&fixture, true).await;
        assert_eq!(out["clean"], true);
        assert_eq!(out["entries"].as_array().unwrap().len(), 0);
        assert_eq!(out["branch"], "master");
    }

    #[tokio::test]
    async fn untracked_and_modified_files_are_listed() {
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
