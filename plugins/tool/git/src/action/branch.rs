use crate::output::{BranchEntry, BranchOutput, to_json};
use crate::sandbox::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;
use git2::BranchType;

/// Lists the branches of a git repository.
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
    /// Path to the git repository working tree (default: current directory).
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
        let (repo, workdir) = scope.resolve_repo(self.path.as_deref())?;
        let (current, detached_head) = super::common::head_info(&repo);

        let filter = if self.include_remote {
            Some(BranchType::All)
        } else {
            Some(BranchType::Local)
        };
        let mut branches: Vec<BranchEntry> = Vec::new();
        for item in repo.branches(filter)? {
            let (branch, _kind) = item?;
            let name = branch.name()?.unwrap_or_default().to_string();
            let upstream_branch = branch.upstream().ok();
            let upstream = upstream_branch
                .as_ref()
                .and_then(|up| up.name().ok().flatten())
                .map(str::to_string);
            let (ahead, behind) = match (
                branch.get().target(),
                upstream_branch.as_ref().and_then(|up| up.get().target()),
            ) {
                (Some(local), Some(remote)) => {
                    let (a, b) = repo.graph_ahead_behind(local, remote)?;
                    (Some(a), Some(b))
                }
                _ => (None, None),
            };
            branches.push(BranchEntry {
                name,
                upstream,
                ahead,
                behind,
                current: false,
            });
        }

        for entry in &mut branches {
            entry.current = current.as_deref() == Some(entry.name.as_str());
        }
        branches.sort_by(|a, b| a.name.cmp(&b.name));

        to_json(&BranchOutput {
            repo: workdir.display().to_string(),
            current,
            detached_head,
            branches,
        })
    }
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
