use crate::broker::GitRun;
use crate::error::GitError;
use ene_plugin_proto::{SandboxConfigData, ToolError};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Shared handle to the latest sandbox scope delivered by the host.
pub type SandboxRef = Arc<RwLock<Option<Arc<RepoScope>>>>;

/// Default (empty) sandbox handle used before the host delivers a config.
pub fn default_sandbox() -> SandboxRef {
    Arc::new(RwLock::new(None))
}

/// Resolves the sandbox handle, falling back to a default scope.
pub fn resolve_sandbox(sandbox_ref: &SandboxRef) -> Arc<RepoScope> {
    let guard = sandbox_ref
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard
        .clone()
        .unwrap_or_else(|| Arc::new(RepoScope::default()))
}

/// Validates repository paths against the workspace allowlist.
///
/// Mirrors the fs plugin's sandbox contract: every path the caller names must
/// resolve inside an allowed directory (or a session-granted pattern), and a
/// repository discovered from that path must have its working tree and git
/// dirs inside the allowlist too — otherwise a path inside the workspace
/// could expose the history of an ancestor repository living outside it.
pub struct RepoScope {
    allowed_directories: Vec<PathBuf>,
    allowed_patterns: Arc<RwLock<HashSet<(String, String)>>>,
    max_read_bytes: usize,
}

impl Default for RepoScope {
    fn default() -> Self {
        Self::new(SandboxConfigData::default())
    }
}

impl RepoScope {
    /// Builds a scope from the host-delivered sandbox config.
    pub fn new(mut data: SandboxConfigData) -> Self {
        data.sanitize();
        Self {
            allowed_directories: data
                .allowed_directories
                .into_iter()
                .map(|s| canonicalize_existing(Path::new(&s)))
                .collect(),
            allowed_patterns: Arc::new(RwLock::new(HashSet::new())),
            max_read_bytes: data.max_read_bytes,
        }
    }

    /// Grants a session-wide pattern (action + target prefix).
    pub fn allow_pattern(&self, action: &str, target_pattern: &str) {
        if let Ok(mut guard) = self.allowed_patterns.write() {
            guard.insert((action.to_string(), target_pattern.to_string()));
        }
    }

    /// Revokes a previously granted session-wide pattern.
    pub fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        if let Ok(mut guard) = self.allowed_patterns.write() {
            guard.remove(&(action.to_string(), target_pattern.to_string()));
        }
    }

    /// Resolves `path_arg` (default `.`) to a repository whose working tree
    /// and git dirs lie inside the allowlist.
    pub async fn resolve_repo(
        &self,
        path_arg: Option<&str>,
        action: &str,
    ) -> Result<ResolvedRepo, ToolError> {
        let resolved = self.resolve_path(path_arg, action)?;
        let resolved_str = resolved.to_string_lossy().into_owned();
        let broker = crate::broker::broker();
        let run = broker
            .run_git(&resolved_str, &["rev-parse", "--show-toplevel"])
            .await?;
        let workdir = match repo_stdout(&run) {
            Some(workdir) => PathBuf::from(workdir),
            None => return Err(not_a_repository(&resolved, &run)),
        };
        let workdir = canonicalize_existing(&workdir);
        self.ensure_allowed(&workdir, action)?;
        let workdir_str = workdir.to_string_lossy().into_owned();
        let git_dir = resolve_git_path(
            &workdir,
            &broker
                .run_git(&workdir_str, &["rev-parse", "--git-dir"])
                .await?,
        );
        let common_dir = resolve_git_path(
            &workdir,
            &broker
                .run_git(&workdir_str, &["rev-parse", "--git-common-dir"])
                .await?,
        );
        self.ensure_allowed(&git_dir, action)?;
        self.ensure_allowed(&common_dir, action)?;
        Ok(ResolvedRepo {
            workdir,
            git_dir,
            common_dir,
        })
    }

    /// Maximum bytes a single committed blob may contribute to a response.
    #[must_use]
    pub const fn max_read_bytes(&self) -> usize {
        self.max_read_bytes
    }

    /// Validates a repository-relative path argument (diff path, blame file).
    pub fn validate_relative_path(&self, path: &str) -> Result<(), ToolError> {
        if path.is_empty() {
            return Err(invalid_path(path, "path must not be empty"));
        }
        let mut reason: Option<&str> = None;
        for component in Path::new(path).components() {
            match component {
                Component::ParentDir => reason = Some("path must not contain '..'"),
                Component::RootDir | Component::Prefix(_) => {
                    reason = Some("path must be relative to the repository root");
                }
                Component::Normal(_) | Component::CurDir => {}
            }
        }
        match reason {
            Some(r) => Err(invalid_path(path, r)),
            None => Ok(()),
        }
    }

    fn resolve_path(&self, path_arg: Option<&str>, action: &str) -> Result<PathBuf, ToolError> {
        let raw = path_arg.unwrap_or(".");
        let path = Path::new(raw);
        let resolved = if path.exists() {
            canonicalize_existing(path)
        } else {
            let abs = if path.is_absolute() {
                path.to_path_buf()
            } else {
                current_dir().join(path)
            };
            let Some(parent) = abs.parent() else {
                return Ok(abs);
            };
            if !parent.exists() {
                return Err(ToolError::from(GitError::NotFound(format!(
                    "parent directory does not exist: {}",
                    parent.display()
                ))));
            }
            let name = abs
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            canonicalize_existing(parent).join(name)
        };
        self.ensure_allowed(&resolved, action)?;
        Ok(resolved)
    }

    fn ensure_allowed(&self, path: &Path, action: &str) -> Result<(), ToolError> {
        for dir in &self.allowed_directories {
            if path.starts_with(dir) {
                return Ok(());
            }
        }
        if let Ok(guard) = self.allowed_patterns.read() {
            for (allowed_action, target) in guard.iter() {
                if allowed_action != action {
                    continue;
                }
                let prefix = Path::new(target);
                if path.starts_with(prefix) || path.starts_with(canonicalize_existing(prefix)) {
                    return Ok(());
                }
            }
        }
        Err(ToolError::from(GitError::PathOutsideSandbox {
            path: path.display().to_string(),
        }))
    }
}

/// A repository resolved through the host's process broker.
pub struct ResolvedRepo {
    /// Absolute working-tree path.
    pub workdir: PathBuf,
    /// Absolute git dir (worktree-specific).
    pub git_dir: PathBuf,
    /// Absolute common git dir (shared across worktrees).
    pub common_dir: PathBuf,
}

fn invalid_path(path: &str, reason: &str) -> ToolError {
    ToolError::from(GitError::InvalidPath {
        path: path.to_string(),
        reason: reason.to_string(),
    })
}

/// The trimmed first stdout line of a successful run, or `None` on a
/// non-zero exit.
fn repo_stdout(run: &GitRun) -> Option<String> {
    run.ok().then(|| {
        run.stdout
            .lines()
            .next()
            .unwrap_or_default()
            .trim_end()
            .to_string()
    })
}

/// Maps a failed `git` run to a structured error, preferring a stderr
/// snippet when present.
pub(crate) fn git_run_error(run: &GitRun) -> ToolError {
    let detail = run.stderr.trim();
    if detail.is_empty() {
        return ToolError::execution_failed("git command failed");
    }
    ToolError::execution_failed(detail.chars().take(300).collect::<String>())
}

/// Resolves a `git rev-parse --git-dir` output to an absolute path:
/// relative outputs (`.git`) are anchored at the working tree.
fn resolve_git_path(workdir: &Path, run: &GitRun) -> PathBuf {
    let Some(git_dir) = repo_stdout(run) else {
        return workdir.join(".git");
    };
    let path = PathBuf::from(git_dir);
    if path.is_absolute() {
        canonicalize_existing(&path)
    } else {
        canonicalize_existing(&workdir.join(path))
    }
}

fn not_a_repository(path: &Path, run: &GitRun) -> ToolError {
    let stderr = run.stderr.trim().to_string();
    if stderr.contains("work tree") || stderr.contains("bare") {
        return ToolError::from(GitError::BareRepository {
            path: path.display().to_string(),
        });
    }
    ToolError::from(GitError::NotARepository {
        path: path.display().to_string(),
    })
}

fn canonicalize_existing(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::{RepoScope, resolve_sandbox};
    use crate::fixture::{RepoFixture, scope_allowing};
    use ene_plugin_proto::{ErrorKind, ToolError};

    fn is_sandbox_violation(err: &ToolError) -> bool {
        matches!(
            err,
            ToolError::Generic {
                kind: ErrorKind::SandboxViolation,
                ..
            }
        )
    }

    #[tokio::test]
    async fn resolves_repository_inside_allowed_directory() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        fixture.write("a.txt", "one\n");
        fixture.commit_all("first");
        let scope = scope_allowing(&fixture.path());
        let repo = scope
            .resolve_repo(Some(&fixture.path()), "git.status")
            .await
            .unwrap();
        assert_eq!(repo.workdir.to_str().unwrap(), fixture.path());
    }

    #[tokio::test]
    async fn rejects_paths_outside_allowed_directory() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        let scope = scope_allowing(&fixture.path());
        let other = tempfile::tempdir().unwrap();
        let err = scope
            .resolve_repo(Some(other.path().to_str().unwrap()), "git.status")
            .await
            .err()
            .expect("path outside sandbox should be rejected");
        assert!(is_sandbox_violation(&err), "{err:?}");
    }

    #[tokio::test]
    async fn rejects_repository_root_outside_workspace() {
        let _serial = crate::fixture::with_broker().await;
        let parent = tempfile::tempdir().unwrap();
        let repo_path = parent.path().join("repo");
        std::fs::create_dir_all(&repo_path).unwrap();
        let repo = git2::Repository::init(&repo_path).unwrap();
        let inner = repo_path.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        drop(repo);

        let scope = scope_allowing(inner.to_str().unwrap());
        let err = scope
            .resolve_repo(Some(inner.to_str().unwrap()), "git.status")
            .await
            .err()
            .expect("repository root outside workspace should be rejected");
        assert!(is_sandbox_violation(&err), "{err:?}");
    }

    #[tokio::test]
    async fn rejects_non_repository_paths() {
        let _serial = crate::fixture::with_broker().await;
        let dir = tempfile::tempdir().unwrap();
        let scope = scope_allowing(dir.path().to_str().unwrap());
        let err = scope
            .resolve_repo(Some(dir.path().to_str().unwrap()), "git.status")
            .await
            .err()
            .expect("non-repository path should be rejected");
        assert!(err.to_string().contains("not a git repository"), "{err}");
    }

    #[tokio::test]
    async fn allow_pattern_grants_and_revokes_access() {
        let _serial = crate::fixture::with_broker().await;
        let fixture = RepoFixture::init();
        let other = tempfile::tempdir().unwrap();
        let repo_path = other.path().join("repo");
        std::fs::create_dir_all(&repo_path).unwrap();
        let outside_repo = git2::Repository::init(&repo_path).unwrap();
        drop(outside_repo);

        let scope = scope_allowing(&fixture.path());
        let err = scope
            .resolve_repo(Some(repo_path.to_str().unwrap()), "git.status")
            .await
            .err()
            .expect("ungranted repository should be rejected");
        assert!(is_sandbox_violation(&err));

        scope.allow_pattern("git.status", repo_path.to_str().unwrap());
        let repo = scope
            .resolve_repo(Some(repo_path.to_str().unwrap()), "git.status")
            .await
            .unwrap();
        assert_eq!(repo.workdir.to_str().unwrap(), repo_path.to_str().unwrap());

        scope.revoke_pattern("git.status", repo_path.to_str().unwrap());
        let err = scope
            .resolve_repo(Some(repo_path.to_str().unwrap()), "git.status")
            .await
            .err()
            .expect("revoked repository should be rejected");
        assert!(is_sandbox_violation(&err));
    }

    #[test]
    fn relative_path_validation() {
        let fixture = RepoFixture::init();
        let scope = scope_allowing(&fixture.path());
        scope.validate_relative_path("src/main.rs").unwrap();
        scope.validate_relative_path("./src/main.rs").unwrap();
        for bad in ["", "..", "../x", "/abs/path", "a/../../b"] {
            let err = scope.validate_relative_path(bad).unwrap_err();
            assert!(
                matches!(err, ToolError::InvalidArguments { .. }),
                "{bad}: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn missing_parent_is_reported() {
        let _serial = crate::fixture::with_broker().await;
        let scope = RepoScope::default();
        let err = scope
            .resolve_repo(Some("/definitely/not/a/repo"), "git.status")
            .await
            .err()
            .expect("missing parent should be rejected");
        assert!(
            err.to_string().contains("parent directory does not exist"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn resolve_sandbox_falls_back_to_default() {
        let _serial = crate::fixture::with_broker().await;
        let scope = resolve_sandbox(&crate::sandbox::default_sandbox());
        drop(scope.resolve_repo(None, "git.status").await);
    }
}
