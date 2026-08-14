use crate::output::{LogEntry, LogOutput, Person, short_oid, to_json};
use crate::sandbox::{SandboxRef, default_sandbox, resolve_sandbox};
use ene_plugin::prelude::*;

const MAX_BODY_CHARS: usize = 4096;
/// Field separator (`%x00`) and record separator (`%x1e`) for the log
/// format below.
const FIELD_SEP: char = '\u{0}';
const RECORD_SEP: char = '\u{1e}';

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
    /// Creates a log action using the shared sandbox scope.
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
        let repo = scope.resolve_repo(self.path.as_deref(), "git.log").await?;
        let workdir = repo.workdir.to_string_lossy().into_owned();
        let max_count = usize::try_from(self.max_count.unwrap_or(30))
            .unwrap_or(30)
            .clamp(1, 100);

        let branch = match &self.branch {
            Some(name) => Some(name.clone()),
            None => super::common::head_info(&workdir).await.0,
        };
        let format = "%H%x00%P%x00%an%x00%ae%x00%aI%x00%cn%x00%ce%x00%cI%x00%s%x00%b%x1e";
        let max_count_arg = format!("-n {max_count}");
        let format_arg = format!("--format={format}");
        let mut args: Vec<&str> = vec!["log", &max_count_arg, &format_arg];
        if let Some(name) = &self.branch {
            args.push(name);
        }
        let run = crate::broker::broker().run_git(&workdir, &args).await?;
        if !run.ok() {
            return Err(super::common::no_commits_or(&run));
        }
        let entries = parse_log(&run.stdout);

        to_json(&LogOutput {
            repo: workdir,
            branch,
            entries,
        })
    }
}

/// Parses the `--format` output into log entries.
fn parse_log(output: &str) -> Vec<LogEntry> {
    output
        .split(RECORD_SEP)
        .filter_map(|record| {
            let record = record.trim();
            (!record.is_empty()).then(|| parse_record(record)).flatten()
        })
        .collect()
}

fn parse_record(record: &str) -> Option<LogEntry> {
    let mut fields = record.split(FIELD_SEP);
    let oid = fields.next()?.to_string();
    let parents = fields
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let author = Person {
        name: fields.next().unwrap_or_default().to_string(),
        email: fields.next().unwrap_or_default().to_string(),
        // `%aI` is strict ISO 8601 with the original offset (RFC3339).
        time: fields.next().unwrap_or_default().to_string(),
    };
    let committer = Person {
        name: fields.next().unwrap_or_default().to_string(),
        email: fields.next().unwrap_or_default().to_string(),
        time: fields.next().unwrap_or_default().to_string(),
    };
    let subject = fields.next().unwrap_or_default().to_string();
    let body = fields
        .next()
        .map(truncate_body)
        .filter(|body| !body.is_empty());
    Some(LogEntry {
        oid: oid.clone(),
        short_oid: short_oid(&oid),
        subject,
        body,
        author,
        committer,
        parents,
    })
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
        let _serial = crate::fixture::with_broker().await;
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
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        fixture.write("a.txt", "two\n");
        fixture.commit_all("second commit");

        let out = run_json(&fixture, Some(1), None).await;
        assert_eq!(out["entries"].as_array().unwrap().len(), 1);
        assert_eq!(out["entries"][0]["subject"], "second commit");
    }

    #[tokio::test]
    async fn branch_argument_selects_the_walk_root() {
        let _serial = crate::fixture::with_broker().await;
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
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        let result = action(&fixture, None, None).run().await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no commits"), "{err}");
    }
}
