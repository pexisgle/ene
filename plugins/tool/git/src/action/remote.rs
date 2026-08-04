use crate::error::from_git2;
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
    /// Creates a remote action using the shared sandbox scope.
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            path: None,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let scope = resolve_sandbox(&self.sandbox);
        let (repo, workdir) = scope.resolve_repo(self.path.as_deref(), "git.remote")?;

        let mut remotes: Vec<RemoteEntry> = Vec::new();
        let remote_names = repo.remotes().map_err(from_git2)?;
        for name in &remote_names {
            let Some(name) = name.map_err(from_git2)? else {
                continue;
            };
            let remote = repo.find_remote(name).map_err(from_git2)?;
            remotes.push(RemoteEntry {
                name: name.to_string(),
                fetch_url: redact_remote_url_bytes(remote.url_bytes()),
                push_url: remote.pushurl_bytes().and_then(redact_remote_url_bytes),
            });
        }
        remotes.sort_by(|a, b| a.name.cmp(&b.name));

        to_json(&RemoteOutput {
            repo: workdir.display().to_string(),
            remotes,
        })
    }
}

fn redact_remote_url_bytes(url: &[u8]) -> Option<String> {
    (!url.is_empty()).then(|| redact_remote_url(&String::from_utf8_lossy(url)))
}

fn redact_remote_url(url: &str) -> String {
    if let Ok(mut parsed) = url::Url::parse(url) {
        if parsed.set_username("").is_err() || parsed.set_password(None).is_err() {
            return "[redacted]".to_string();
        }
        parsed.set_query(None);
        parsed.set_fragment(None);
        return parsed.to_string();
    }
    url.split_once('@')
        .map_or_else(|| url.to_string(), |(_, rest)| format!("[redacted]@{rest}"))
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

    #[test]
    fn remote_urls_redact_credentials_and_query() {
        assert_eq!(
            super::redact_remote_url("https://user:secret@example.com/repo.git?token=hidden#frag"),
            "https://example.com/repo.git"
        );
        assert_eq!(
            super::redact_remote_url("git@github.com:owner/repo.git"),
            "[redacted]@github.com:owner/repo.git"
        );
    }
}
