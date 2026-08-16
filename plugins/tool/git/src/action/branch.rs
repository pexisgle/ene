use crate::output::{BranchEntry, BranchOutput, to_json};
use crate::sandbox::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[serde(rename_all = "camelCase")]
#[tool(
    namespace = "git",
    name = "branch",
    summary = "List the branches of a git repository.",
    description = "Returns the current branch (or detached HEAD) and the repository's branches as structured JSON, including each branch's upstream and ahead/behind counts when it has one. The repository path must lie inside the configured workspace.",
    category = "Utility",
    keywords_primary = "git, branch, upstream, ahead, behind, repository",
    side_effects = "ReadOnly"
)]
pub struct BranchAction {
    #[serde(default)]
    #[arg(
        default = ".",
        description = "Path to the git repository (default: current directory)."
    )]
    path: Option<String>,
    /// Whether to include remote-tracking branches.
    #[serde(default)]
    #[arg(default = "false")]
    include_remote: bool,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl BranchAction {
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            path: None,
            include_remote: false,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let scope = resolve_sandbox(&self.sandbox);
        let repo = scope
            .resolve_repo(self.path.as_deref(), "git.branch")
            .await?;
        let workdir = repo.workdir.to_string_lossy().into_owned();
        let (current, detached_head) = super::common::head_info(&workdir).await;

        let format = "%(HEAD)%09%(refname:short)%09%(upstream:short)%09%(upstream:track)";
        let format_arg = format!("--format={format}");
        let ref_args: &[&str] = if self.include_remote {
            &["refs/heads", "refs/remotes"]
        } else {
            &["refs/heads"]
        };
        let mut args: Vec<&str> = vec!["for-each-ref", &format_arg];
        args.extend(ref_args.iter().copied());
        let run = crate::broker::broker().run_git(&workdir, &args).await?;
        if !run.ok() {
            return Err(crate::sandbox::git_run_error(&run));
        }
        let mut branches = parse_branches(&run.stdout, current.as_deref());
        branches.sort_by(|a, b| a.name.cmp(&b.name));

        to_json(&BranchOutput {
            repo: workdir,
            current,
            detached_head,
            branches,
        })
    }
}

/// Parses `for-each-ref` rows (`HEAD \t name \t upstream \t track`) into
/// branch entries.
fn parse_branches(output: &str, current: Option<&str>) -> Vec<BranchEntry> {
    let mut branches = Vec::new();
    for line in output.lines().filter(|line| !line.is_empty()) {
        let mut columns = line.split('\t');
        let head_marker = columns.next().unwrap_or_default();
        let Some(name) = columns.next() else {
            continue;
        };
        let upstream = columns
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        let track = columns.next().unwrap_or_default();
        let (ahead, behind) = {
            let (ahead, behind) = parse_track(track);
            // With an upstream, a missing count means up-to-date on that
            // side; a `[gone]` upstream reports neither.
            if upstream.is_some() && track.trim() != "[gone]" {
                (ahead.or(Some(0)), behind.or(Some(0)))
            } else {
                (ahead, behind)
            }
        };
        branches.push(BranchEntry {
            name: name.to_string(),
            upstream,
            ahead,
            behind,
            current: head_marker == "*" || current == Some(name),
        });
    }
    branches
}

/// Parses `%(upstream:track)` output (`[ahead 1]`, `[behind 2]`,
/// `[ahead 1, behind 2]`, `[gone]`).
fn parse_track(track: &str) -> (Option<usize>, Option<usize>) {
    let mut ahead = None;
    let mut behind = None;
    let rest = track.trim().trim_start_matches('[').trim_end_matches(']');
    if rest.is_empty() || rest == "gone" {
        return (ahead, behind);
    }
    for part in rest.split(',') {
        let part = part.trim();
        if let Some(count) = part.strip_prefix("ahead ") {
            ahead = count.parse().ok();
        } else if let Some(count) = part.strip_prefix("behind ") {
            behind = count.parse().ok();
        }
    }
    (ahead, behind)
}

#[cfg(test)]
mod tests {
    use super::BranchAction;
    use crate::fixture::{RepoFixture, sandbox_ref, scope_allowing};
    use serde_json::Value;

    fn action(fixture: &RepoFixture, include_remote: bool) -> BranchAction {
        BranchAction {
            path: Some(fixture.path()),
            include_remote,
            sandbox: sandbox_ref(scope_allowing(&fixture.path())),
        }
    }

    #[tokio::test]
    async fn lists_branches_with_current_flag() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        fixture.branch("feature");

        let out: Value =
            serde_json::from_str(&action(&fixture, false).run().await.unwrap()).unwrap();
        let default_branch = fixture
            .repo
            .head()
            .unwrap()
            .shorthand()
            .unwrap()
            .to_string();
        assert_eq!(out["current"], default_branch);
        let branches = out["branches"].as_array().unwrap();
        assert_eq!(branches.len(), 2);
        let current = branches
            .iter()
            .find(|b| b["name"] == default_branch)
            .unwrap();
        assert_eq!(current["current"], true);
        assert!(current["upstream"].is_null());
        assert!(current["ahead"].is_null());
        let feature = branches.iter().find(|b| b["name"] == "feature").unwrap();
        assert_eq!(feature["current"], false);
        assert!(feature["upstream"].is_null());
    }

    #[tokio::test]
    async fn ahead_behind_are_reported_against_upstream() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("base");
        fixture.branch("feature");
        fixture.write("a.txt", "two\n");
        fixture.commit_all("master ahead");
        let default_branch = fixture
            .repo
            .head()
            .unwrap()
            .shorthand()
            .unwrap()
            .to_string();
        let mut feature = fixture
            .repo
            .find_branch("feature", git2::BranchType::Local)
            .unwrap();
        feature.set_upstream(Some(&default_branch)).unwrap();

        let out: Value =
            serde_json::from_str(&action(&fixture, false).run().await.unwrap()).unwrap();
        let branches = out["branches"].as_array().unwrap();
        let current = branches
            .iter()
            .find(|b| b["name"] == default_branch)
            .unwrap();
        assert!(current["upstream"].is_null());
        assert!(current["ahead"].is_null());
        let feature = branches.iter().find(|b| b["name"] == "feature").unwrap();
        assert_eq!(feature["upstream"], default_branch);
        assert_eq!(feature["ahead"], 0);
        assert_eq!(feature["behind"], 1);
    }
}
