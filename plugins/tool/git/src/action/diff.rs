use crate::error::from_git2;
use crate::output::{DiffOutput, to_json};
use crate::sandbox::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;
use git2::{DiffFormat, DiffOptions};

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
        let (repo, workdir) = scope.resolve_repo(self.path.as_deref(), "git.diff")?;
        if let Some(filter) = &self.path_filter {
            scope.validate_relative_path(filter)?;
        }

        let mut opts = DiffOptions::new();
        if let Some(filter) = &self.path_filter {
            opts.pathspec(filter);
        }
        let index = repo.index().map_err(from_git2)?;
        let head_tree = repo
            .head()
            .ok()
            .map(|head| head.peel_to_tree().map_err(from_git2))
            .transpose()?;
        let diff = if self.staged {
            repo.diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut opts))
                .map_err(from_git2)?
        } else {
            repo.diff_index_to_workdir(Some(&index), Some(&mut opts))
                .map_err(from_git2)?
        };

        let stats = diff.stats().map_err(from_git2)?;
        let files_changed = stats.files_changed();
        let insertions = stats.insertions();
        let deletions = stats.deletions();
        let summary = format!(
            "{files_changed} {} changed, {insertions} {}(+), {deletions} {}(-)",
            plural(files_changed, "file", "files"),
            plural(insertions, "insertion", "insertions"),
            plural(deletions, "deletion", "deletions"),
        );
        let (patch, truncated) = if wants_text {
            print_patch(&diff)?
        } else {
            (None, false)
        };

        to_json(&DiffOutput {
            repo: workdir.display().to_string(),
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

/// Renders the diff as unified patch text, stopping at the output cap.
fn print_patch(diff: &git2::Diff<'_>) -> Result<(Option<String>, bool), ToolError> {
    let mut buf = String::new();
    let mut truncated = false;
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        if truncated || buf.len() >= MAX_PATCH_CHARS {
            truncated = true;
            return false;
        }
        let origin = line.origin();
        let content = String::from_utf8_lossy(line.content());
        match origin {
            ' ' | '+' | '-' => {
                let remaining = MAX_PATCH_CHARS.saturating_sub(buf.len());
                let addition = format!("{origin}{content}");
                if addition.len() > remaining {
                    buf.push_str(&String::from_utf8_lossy(&addition.as_bytes()[..remaining]));
                    truncated = true;
                    return false;
                }
                buf.push_str(&addition);
            }
            _ => {
                let remaining = MAX_PATCH_CHARS.saturating_sub(buf.len());
                if content.len() > remaining {
                    buf.push_str(&String::from_utf8_lossy(&content.as_bytes()[..remaining]));
                    truncated = true;
                    return false;
                }
                buf.push_str(&content);
            }
        }
        true
    })
    .map_err(from_git2)?;
    let patch = if buf.is_empty() {
        None
    } else {
        (!truncated).then_some(buf)
    };
    Ok((patch, truncated))
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
        let fixture = RepoFixture::init();
        let result = action(&fixture, false, Some("nope"), None).run().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn traversal_filter_is_rejected() {
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
