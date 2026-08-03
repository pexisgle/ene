use crate::output::{RemoteEntry, RemoteOutput, to_json};
use crate::sandbox::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;

/// Lists the configured remotes of a git repository.
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[serde(rename_all = "camelCase")]
#[tool(
    namespace = "git",
    name = "remote",
    summary = "List the configured remotes of a git repository.",
    description = "Returns the repository's configured remotes as structured JSON with their fetch and push URLs. Only reads local configuration; it never contacts the network. The repository path must lie inside the configured workspace.",
    category = "Utility",
    keywords_primary = "git, remote, url, origin, repository",
    side_effects = "ReadOnly"
)]
pub struct RemoteAction {
    /// Path to the git repository working tree (default: current directory).
    #[serde(default)]
    #[arg(
        default = ".",
        description = "Path to the git repository (default: current directory)."
    )]
    path: Option<String>,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl RemoteAction {
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            path: None,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let scope = resolve_sandbox(&self.sandbox);
        let (repo, workdir) = scope.resolve_repo(self.path.as_deref())?;

        let mut remotes: Vec<RemoteEntry> = Vec::new();
        for name in repo.remotes()? {
            let Some(name) = name? else {
                continue;
            };
            let remote = repo.find_remote(&name)?;
            remotes.push(RemoteEntry {
                name: name.to_string(),
                fetch_url: remote.url().ok().map(str::to_string),
                push_url: remote.pushurl().ok().flatten().map(str::to_string),
            });
        }
        remotes.sort_by(|a, b| a.name.cmp(&b.name));

        to_json(&RemoteOutput {
            repo: workdir.display().to_string(),
            remotes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteAction;
    use crate::fixture::{RepoFixture, sandbox_ref, scope_allowing};
    use serde_json::Value;

    #[tokio::test]
    async fn lists_configured_remotes() {
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        fixture.remote("origin", "https://example.com/repo.git");
        fixture.remote("upstream", "https://example.com/upstream.git");

        let action = RemoteAction {
            path: Some(fixture.path()),
            sandbox: sandbox_ref(scope_allowing(&fixture.path())),
        };
        let out: Value = serde_json::from_str(&action.run().await.unwrap()).unwrap();
        let remotes = out["remotes"].as_array().unwrap();
        assert_eq!(remotes.len(), 2);
        let origin = remotes.iter().find(|r| r["name"] == "origin").unwrap();
        assert_eq!(origin["fetch_url"], "https://example.com/repo.git");
        assert_eq!(origin["push_url"], "https://example.com/repo.git");
    }

    #[tokio::test]
    async fn repo_without_remotes_is_empty() {
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");

        let action = RemoteAction {
            path: Some(fixture.path()),
            sandbox: sandbox_ref(scope_allowing(&fixture.path())),
        };
        let out: Value = serde_json::from_str(&action.run().await.unwrap()).unwrap();
        assert_eq!(out["remotes"].as_array().unwrap().len(), 0);
    }
}
