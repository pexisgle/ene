use crate::output::{DiffOutput, to_json};
use crate::sandbox::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;

const MAX_PATCH_CHARS: usize = 1_000_000;

/// Shows the changes in a git repository as a unified diff and/or stat summary.
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[serde(rename_all = "camelCase")]
#[tool(
    namespace = "git",
    name = "diff",
    summary = "Show working tree or staged changes in a git repository.",
    description = "Returns the unified diff text and/or a stat summary for a git repository: by default the unstaged working tree changes, or the staged index changes when `staged` is true. An optional repository-relative path restricts the diff to a single file or directory. The repository path must lie inside the configured workspace.",
    category = "Utility",
    keywords_primary = "git, diff, patch, changes, staged, stat",
    side_effects = "ReadOnly"
)]
pub struct DiffAction {
    /// Path to the git repository working tree (default: current directory).
    #[serde(default)]
    #[arg(
        default = ".",
        description = "Path to the git repository (default: current directory)."
    )]
    path: Option<String>,
    /// Compare the index against `HEAD` (staged changes) instead of the
    /// working tree.
    #[serde(default)]
    #[arg(default = "false")]
    staged: bool,
    /// Output format: unified diff text, stat summary, or both.
    #[serde(default)]
    #[arg(default = "text", enum_values = "text, stat, both")]
    format: Option<String>,
    /// Repository-relative path to restrict the diff to one file or directory.
    #[serde(default)]
    path_filter: Option<String>,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl DiffAction {
    /// Creates a diff action using the shared sandbox scope.
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            path: None,
            staged: false,
            format: None,
            path_filter: None,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let format = self.format.as_deref().unwrap_or("text");
        if !matches!(format, "text" | "stat" | "both") {
            return Err(ToolError::InvalidArguments {
                message: format!("unknown format '{format}' (expected text, stat, or both)"),
            });
        }
        let wants_text = format != "stat";

        let scope = resolve_sandbox(&self.sandbox);
        let repo = scope.resolve_repo(self.path.as_deref(), "git.diff").await?;
        let workdir = repo.workdir.to_string_lossy().into_owned();
        if let Some(filter) = &self.path_filter {
            scope.validate_relative_path(filter)?;
        }

        let broker = crate::broker::broker();
        let mut base_args: Vec<&str> = vec!["diff"];
        if self.staged {
            base_args.push("--cached");
        }
        let mut numstat_args: Vec<&str> = base_args.clone();
        numstat_args.push("--numstat");
        let pathspec: Vec<String> = self
            .path_filter
            .as_deref()
            .into_iter()
            .map(str::to_string)
            .collect();
        numstat_args.extend(pathspec.iter().map(String::as_str));
        let numstat = broker.run_git(&workdir, &numstat_args).await?;
        if !numstat.ok() {
            return Err(crate::sandbox::git_run_error(&numstat));
        }
        let (files_changed, insertions, deletions) = sum_numstat(&numstat.stdout);
        let summary = format!(
            "{files_changed} {} changed, {insertions} {}(+), {deletions} {}(-)",
            plural(files_changed, "file", "files"),
            plural(insertions, "insertion", "insertions"),
            plural(deletions, "deletion", "deletions"),
        );
        let (patch, truncated) = if wants_text {
            let mut patch_args: Vec<&str> = Vec::new();
            patch_args.extend(base_args.iter().copied());
            patch_args.extend(pathspec.iter().map(String::as_str));
            let patch_run = broker.run_git(&workdir, &patch_args).await?;
            if !patch_run.ok() {
                return Err(crate::sandbox::git_run_error(&patch_run));
            }
            let text = patch_run.stdout;
            if text.is_empty() {
                (None, false)
            } else if text.chars().count() > MAX_PATCH_CHARS {
                let head: String = text.chars().take(MAX_PATCH_CHARS).collect();
                (Some(format!("{head}\n... (patch truncated)")), true)
            } else {
                (Some(text), false)
            }
        } else {
            (None, false)
        };

        to_json(&DiffOutput {
            repo: workdir,
            staged: self.staged,
            files_changed,
            insertions,
            deletions,
            summary,
            patch,
            truncated,
        })
    }
}

/// Sums `git diff --numstat` output into (files, insertions, deletions).
/// Binary entries (`-`) contribute zero line counts but count as a file.
fn sum_numstat(output: &str) -> (usize, usize, usize) {
    let mut files = 0_usize;
    let mut insertions = 0_usize;
    let mut deletions = 0_usize;
    for line in output.lines() {
        let mut columns = line.split('\t');
        let Some(added) = columns.next() else {
            continue;
        };
        let Some(removed) = columns.next() else {
            continue;
        };
        files = files.saturating_add(1);
        if added != "-" {
            insertions = insertions.saturating_add(added.parse().unwrap_or(0));
        }
        if removed != "-" {
            deletions = deletions.saturating_add(removed.parse().unwrap_or(0));
        }
    }
    (files, insertions, deletions)
}

const fn plural<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

#[cfg(test)]
mod tests {
    use super::DiffAction;
    use crate::fixture::{RepoFixture, sandbox_ref, scope_allowing};
    use serde_json::Value;

    fn action(
        fixture: &RepoFixture,
        staged: bool,
        format: Option<&str>,
        filter: Option<&str>,
    ) -> DiffAction {
        DiffAction {
            path: Some(fixture.path()),
            staged,
            format: format.map(str::to_string),
            path_filter: filter.map(str::to_string),
            sandbox: sandbox_ref(scope_allowing(&fixture.path())),
        }
    }

    #[tokio::test]
    async fn unstaged_diff_contains_patch_text() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        fixture.write("a.txt", "one\ntwo\n");

        let out: Value = serde_json::from_str(
            &action(&fixture, false, Some("text"), None)
                .run()
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(out["files_changed"], 1);
        assert_eq!(out["insertions"], 1);
        assert_eq!(out["deletions"], 0);
        let patch = out["patch"].as_str().unwrap();
        assert!(patch.contains("+two"));
        assert!(patch.contains("@@"));
    }

    #[tokio::test]
    async fn staged_diff_requires_staged_flag() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        fixture.write("a.txt", "two\n");
        fixture.stage("a.txt");

        let unstaged: Value =
            serde_json::from_str(&action(&fixture, false, None, None).run().await.unwrap())
                .unwrap();
        assert_eq!(unstaged["files_changed"], 0);
        let staged: Value =
            serde_json::from_str(&action(&fixture, true, None, None).run().await.unwrap()).unwrap();
        assert_eq!(staged["files_changed"], 1);
        assert_eq!(staged["staged"], true);
    }

    #[tokio::test]
    async fn stat_format_omits_patch() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        fixture.write("a.txt", "two\n");

        let out: Value = serde_json::from_str(
            &action(&fixture, false, Some("stat"), None)
                .run()
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(out["patch"].is_null());
        assert_eq!(
            out["summary"],
            "1 file changed, 1 insertion(+), 1 deletion(-)"
        );
    }

    #[tokio::test]
    async fn clean_repo_yields_empty_diff() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        let out: Value =
            serde_json::from_str(&action(&fixture, false, None, None).run().await.unwrap())
                .unwrap();
        assert_eq!(out["files_changed"], 0);
        assert!(out["patch"].is_null());
    }

    #[tokio::test]
    async fn invalid_format_is_rejected() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        let result = action(&fixture, false, Some("nope"), None).run().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn traversal_filter_is_rejected() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        let result = action(&fixture, false, None, Some("../etc/passwd"))
            .run()
            .await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("'..'"), "{err}");
    }
}
