use crate::output::{LogEntry, LogOutput, Person, format_time, short_oid, to_json};
use crate::sandbox::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;
use git2::{Commit, Sort};

const MAX_BODY_CHARS: usize = 4096;

/// Lists the commit history of a git repository.
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[serde(rename_all = "camelCase")]
#[tool(
    namespace = "git",
    name = "log",
    summary = "Show the commit history of a git repository.",
    description = "Returns recent commits of a git repository as structured JSON with full and short oids, subjects and bodies, author/committer names, emails and RFC3339 timestamps, and parent oids. The repository path must lie inside the configured workspace.",
    category = "Utility",
    keywords_primary = "git, log, history, commits, author, repository",
    side_effects = "ReadOnly"
)]
pub struct LogAction {
    /// Path to the git repository working tree (default: current directory).
    #[serde(default)]
    #[arg(
        default = ".",
        description = "Path to the git repository (default: current directory)."
    )]
    path: Option<String>,
    /// Maximum number of commits to return (1-100, default 30).
    #[serde(default)]
    #[arg(default = "30", minimum = 1, maximum = 100)]
    max_count: Option<u32>,
    /// Branch or ref to walk (default: `HEAD`).
    #[serde(default)]
    branch: Option<String>,

    #[tool(skip)]
    #[serde(skip, default = "default_sandbox")]
    sandbox: SandboxRef,
}

impl LogAction {
    pub const fn new(sandbox: SandboxRef) -> Self {
        Self {
            path: None,
            max_count: None,
            branch: None,
            sandbox,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let scope = resolve_sandbox(&self.sandbox);
        let (repo, workdir) = scope.resolve_repo(self.path.as_deref())?;
        let max_count = usize::try_from(self.max_count.unwrap_or(30))
            .unwrap_or(30)
            .clamp(1, 100);

        let mut walk = repo.revwalk()?;
        let branch = match &self.branch {
            Some(name) => {
                let commit = repo.revparse_single(name)?.peel_to_commit()?;
                walk.push(commit.id())?;
                Some(name.clone())
            }
            None => {
                walk.push_head().map_err(super::common::map_head_error)?;
                super::common::head_info(&repo).0
            }
        };
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;

        let mut entries: Vec<LogEntry> = Vec::new();
        for oid in walk.take(max_count) {
            let commit = repo.find_commit(oid?)?;
            entries.push(log_entry(&commit));
        }

        to_json(&LogOutput {
            repo: workdir.display().to_string(),
            branch,
            entries,
        })
    }
}

fn log_entry(commit: &Commit<'_>) -> LogEntry {
    LogEntry {
        oid: commit.id().to_string(),
        short_oid: short_oid(commit.id()),
        subject: commit
            .summary()
            .ok()
            .flatten()
            .unwrap_or_default()
            .to_string(),
        body: commit.body().ok().flatten().map(truncate_body),
        author: person(&commit.author()),
        committer: person(&commit.committer()),
        parents: commit.parent_ids().map(|id| id.to_string()).collect(),
    }
}

fn person(sig: &git2::Signature<'_>) -> Person {
    Person {
        name: sig.name().unwrap_or_default().to_string(),
        email: sig.email().unwrap_or_default().to_string(),
        time: format_time(sig.when()),
    }
}

fn truncate_body(body: &str) -> String {
    if body.chars().count() > MAX_BODY_CHARS {
        let head: String = body.chars().take(MAX_BODY_CHARS).collect();
        format!("{head}\n... (body truncated)")
    } else {
        body.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{LogAction, truncate_body};
    use crate::fixture::{RepoFixture, sandbox_ref, scope_allowing};
    use serde_json::Value;

    fn action(fixture: &RepoFixture, max_count: Option<u32>, branch: Option<&str>) -> LogAction {
        LogAction {
            path: Some(fixture.path()),
            max_count,
            branch: branch.map(str::to_string),
            sandbox: sandbox_ref(scope_allowing(&fixture.path())),
        }
    }

    async fn run_json(
        fixture: &RepoFixture,
        max_count: Option<u32>,
        branch: Option<&str>,
    ) -> Value {
        let out = action(fixture, max_count, branch).run().await.unwrap();
        serde_json::from_str(&out).unwrap()
    }

    #[test]
    fn body_truncation_appends_marker() {
        let short = "short body";
        assert_eq!(truncate_body(short), short);
        let long = "x".repeat(5000);
        let out = truncate_body(&long);
        assert!(out.ends_with("... (body truncated)"));
        assert!(out.chars().count() < 4200);
    }

    #[tokio::test]
    async fn commits_are_newest_first_with_dates_and_parents() {
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        let first = fixture.commit_all("first commit");
        fixture.write("a.txt", "two\n");
        let second = fixture.commit_all("second commit");

        let out = run_json(&fixture, None, None).await;
        let entries = out["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["subject"], "second commit");
        assert_eq!(entries[0]["oid"], second.to_string());
        assert_eq!(entries[0]["parents"][0], first.to_string());
        assert_eq!(entries[1]["subject"], "first commit");
        assert_eq!(entries[1]["parents"].as_array().unwrap().len(), 0);
        assert!(entries[0]["author"]["time"].as_str().unwrap().contains('T'));
        assert_eq!(entries[0]["author"]["name"], "Test User");
        assert_eq!(entries[0]["author"]["email"], "test@example.com");
    }

    #[tokio::test]
    async fn max_count_limits_entries() {
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        fixture.write("a.txt", "two\n");
        fixture.commit_all("second");

        let out = run_json(&fixture, Some(1), None).await;
        assert_eq!(out["entries"].as_array().unwrap().len(), 1);
        assert_eq!(out["entries"][0]["subject"], "second commit");
    }

    #[tokio::test]
    async fn branch_argument_selects_the_walk_root() {
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        fixture.branch("feature");

        let out = run_json(&fixture, None, Some("feature")).await;
        assert_eq!(out["branch"], "feature");
        assert_eq!(out["entries"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn empty_repository_is_an_error() {
        let fixture = RepoFixture::init();
        let result = action(&fixture, None, None).run().await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no commits"), "{err}");
    }
}
