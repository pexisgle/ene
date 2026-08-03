use crate::sandbox::{RepoScope, SandboxRef};
use ene_plugin_proto::SandboxConfigData;
use git2::{IndexAddOption, Oid, Repository};
use std::path::Path;
use std::sync::Arc;

/// A scope whose only allowed directory is `path`.
pub(crate) fn scope_allowing(path: &str) -> RepoScope {
    let data = SandboxConfigData {
        allowed_directories: vec![path.to_string()],
        ..SandboxConfigData::default()
    };
    RepoScope::new(data)
}

/// Wraps a scope in the same shared handle the provider uses.
pub(crate) fn sandbox_ref(scope: RepoScope) -> SandboxRef {
    Arc::new(std::sync::RwLock::new(Some(Arc::new(scope))))
}

/// A throwaway git repository with a fixed local identity, for unit tests.
pub(crate) struct RepoFixture {
    pub(crate) dir: tempfile::TempDir,
    pub(crate) repo: Repository,
}

impl RepoFixture {
    pub(crate) fn init() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();
        Self { dir, repo }
    }

    pub(crate) fn path(&self) -> String {
        self.dir.path().to_str().unwrap().to_string()
    }

    pub(crate) fn write(&self, rel: &str, content: &str) {
        let path = self.dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    pub(crate) fn delete(&self, rel: &str) {
        std::fs::remove_file(self.dir.path().join(rel)).unwrap();
    }

    pub(crate) fn stage(&self, rel: &str) {
        let mut index = self.repo.index().unwrap();
        index.add_path(Path::new(rel)).unwrap();
        index.write().unwrap();
    }

    pub(crate) fn commit_all(&self, message: &str) -> Oid {
        let mut index = self.repo.index().unwrap();
        index
            .add_all(["."].iter(), IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = self.repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = self.repo.signature().unwrap();
        let parent = self
            .repo
            .head()
            .ok()
            .and_then(|head| head.peel_to_commit().ok());
        let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
        self.repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                message,
                &tree,
                &parents,
            )
            .unwrap()
    }

    pub(crate) fn branch(&self, name: &str) {
        let head = self.repo.head().unwrap();
        let commit = head.peel_to_commit().unwrap();
        self.repo.branch(name, &commit, false).unwrap();
    }

    pub(crate) fn remote(&self, name: &str, url: &str) {
        let mut config = self.repo.config().unwrap();
        config.set_str(&format!("remote.{name}.url"), url).unwrap();
        config
            .set_str(&format!("remote.{name}.pushurl"), url)
            .unwrap();
    }
}
