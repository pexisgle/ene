use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::cas::{Cas, CasEntry};
use crate::catalog::{ArtifactKind, ArtifactTarget};
use crate::download::Downloader;
use crate::error::{ArtifactError, Result};

/// One installed generation of an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledArtifact {
    /// Artifact id from the catalog.
    pub id: String,
    /// Installed version.
    pub version: String,
    /// Artifact kind.
    pub kind: ArtifactKind,
    /// Hex SHA-256 of the active object.
    pub sha256: String,
    /// Exact size in bytes.
    pub size: u64,
    /// Unix millisecond timestamp of activation.
    pub activated_at_ms: u64,
    /// The generation this one replaced. Rollback restores it; the previous
    /// generation's own `previous` is dropped, so exactly one generation of
    /// rollback is kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Box<InstalledArtifact>>,
}

/// Persisted installation state: artifact id → active generation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledState {
    /// Active generations by artifact id.
    pub artifacts: BTreeMap<String, InstalledArtifact>,
}

/// Constructor inputs for [`ArtifactInstaller`].
#[derive(Debug, Clone)]
pub struct InstallerConfig {
    /// CAS root directory.
    pub cas_root: PathBuf,
    /// Where `state.json` lives (survives restarts).
    pub state_path: PathBuf,
}

/// Installs, switches, rolls back, and garbage-collects artifacts.
///
/// Every state mutation is written atomically (temp file + fsync + rename),
/// so a crash never leaves a half-written state. A failed update never
/// touches the active generation.
#[derive(Debug)]
pub struct ArtifactInstaller {
    cas: Cas,
    state_path: PathBuf,
    state: Mutex<InstalledState>,
}

impl ArtifactInstaller {
    /// Opens (creating if needed) the installer and its state file.
    pub fn new(config: InstallerConfig) -> Result<Self> {
        let cas = Cas::new(&config.cas_root)?;
        let state = if config.state_path.is_file() {
            let bytes = std::fs::read(&config.state_path)?;
            serde_json::from_slice(&bytes)?
        } else {
            InstalledState::default()
        };
        Ok(Self {
            cas,
            state_path: config.state_path,
            state: Mutex::new(state),
        })
    }

    /// Snapshot of the current installation state.
    #[must_use]
    pub fn state(&self) -> InstalledState {
        self.state.lock().clone()
    }

    /// The active generation for `id`, if installed.
    #[must_use]
    pub fn installed(&self, id: &str) -> Option<InstalledArtifact> {
        self.state.lock().artifacts.get(id).cloned()
    }

    /// id → `(version, digest)` of the active generation, for catalog
    /// rollback and digest-change checks.
    #[must_use]
    pub fn installed_refs(&self) -> BTreeMap<String, (String, String)> {
        self.state
            .lock()
            .artifacts
            .iter()
            .map(|(id, artifact)| {
                (
                    id.clone(),
                    (artifact.version.clone(), artifact.sha256.clone()),
                )
            })
            .collect()
    }

    /// Downloads `target` (via the signed catalog's URL/digest/size) and
    /// activates it. Mirrors are tried in order; the first successful
    /// download wins.
    pub async fn install(
        &self,
        id: &str,
        target: &ArtifactTarget,
        downloader: &Downloader,
        max_bytes: u64,
        on_redirect: &(dyn Fn(&str) -> Result<()> + Sync),
    ) -> Result<InstalledArtifact> {
        crate::digest::validate_digest(&target.sha256)?;
        if self.cas.contains(&target.sha256)? {
            let entry = CasEntry {
                sha256: target.sha256.clone(),
                size: target.size,
                path: self.cas.object_path(&target.sha256),
            };
            return self.activate(id, target, &entry);
        }

        let part_dir = self.cas.root().join(".tmp");
        std::fs::create_dir_all(&part_dir)?;
        let part_path = part_dir.join(format!("{id}-{}.part", target.version));
        let mut last_error: Option<ArtifactError> = None;
        for url in &target.urls {
            match downloader
                .download_to(
                    url,
                    &part_path,
                    &target.sha256,
                    target.size,
                    max_bytes,
                    on_redirect,
                )
                .await
            {
                Ok(_) => {
                    let entry = self.cas.put(
                        std::fs::File::open(&part_path)?,
                        &target.sha256,
                        target.size,
                        max_bytes,
                    )?;
                    drop(std::fs::remove_file(&part_path));
                    return self.activate(id, target, &entry);
                }
                Err(e) => {
                    tracing::warn!(
                        artifact = %id,
                        url = %url,
                        error = %e,
                        "artifact mirror failed; trying next"
                    );
                    last_error = Some(e);
                }
            }
        }
        Err(last_error.unwrap_or(ArtifactError::ArtifactNotFound(id.to_string())))
    }

    /// Activates a verified CAS object as the new generation for `id`.
    ///
    /// The previous generation is kept as `previous`; the generation before
    /// that is dropped. State is persisted before returning.
    pub fn activate(
        &self,
        id: &str,
        target: &ArtifactTarget,
        entry: &CasEntry,
    ) -> Result<InstalledArtifact> {
        if entry.sha256 != target.sha256 || entry.size != target.size {
            return Err(ArtifactError::DigestMismatch {
                artifact: id.to_string(),
                expected: target.sha256.clone(),
                actual: entry.sha256.clone(),
            });
        }
        let mut state = self.state.lock();
        let previous = state.artifacts.get(id).cloned().map(|old| {
            Box::new(InstalledArtifact {
                previous: None,
                ..old
            })
        });
        let installed = InstalledArtifact {
            id: id.to_string(),
            version: target.version.clone(),
            kind: target.kind,
            sha256: target.sha256.clone(),
            size: target.size,
            activated_at_ms: now_ms(),
            previous,
        };
        state.artifacts.insert(id.to_string(), installed.clone());
        self.persist_locked(&state)?;
        Ok(installed)
    }

    /// Restores the previous generation for `id` (one-step rollback).
    pub fn rollback(&self, id: &str) -> Result<InstalledArtifact> {
        let mut state = self.state.lock();
        let Some(current) = state.artifacts.get(id) else {
            return Err(ArtifactError::ArtifactNotFound(id.to_string()));
        };
        let Some(previous) = current.previous.clone() else {
            return Err(ArtifactError::Rollback {
                artifact: id.to_string(),
                detail: "no previous generation to roll back to".to_string(),
            });
        };
        let mut restored = *previous;
        restored.previous = None;
        state.artifacts.insert(id.to_string(), restored.clone());
        self.persist_locked(&state)?;
        Ok(restored)
    }

    /// Removes CAS objects not referenced by any current or previous
    /// generation, returning the number of removed objects.
    pub fn gc(&self) -> Result<usize> {
        let state = self.state.lock();
        let mut keep = HashSet::new();
        for artifact in state.artifacts.values() {
            keep.insert(artifact.sha256.clone());
            if let Some(previous) = &artifact.previous {
                keep.insert(previous.sha256.clone());
            }
        }
        self.cas.gc(&keep)
    }

    fn persist_locked(&self, state: &InstalledState) -> Result<()> {
        if let Some(parent) = self.state_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut temp = tempfile::NamedTempFile::new_in(
            self.state_path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(".")),
        )?;
        serde_json::to_writer(&mut temp, state)?;
        temp.as_file().sync_all()?;
        temp.persist(&self.state_path)
            .map_err(|e| ArtifactError::Io(e.error))?;
        if let Some(parent) = self.state_path.parent()
            && let Ok(dir) = std::fs::File::open(parent)
        {
            drop(dir.sync_all());
        }
        Ok(())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().try_into().unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(version: &str, digest: &str, size: u64) -> ArtifactTarget {
        ArtifactTarget {
            version: version.to_string(),
            kind: ArtifactKind::Plugin,
            urls: vec!["https://example.test/a.bin".to_string()],
            sha256: digest.to_string(),
            size,
        }
    }

    fn installer() -> (tempfile::TempDir, ArtifactInstaller) {
        let dir = tempfile::tempdir().expect("tempdir");
        let installer = ArtifactInstaller::new(InstallerConfig {
            cas_root: dir.path().join("cas"),
            state_path: dir.path().join("state.json"),
        })
        .expect("installer");
        (dir, installer)
    }

    fn install_bytes(installer: &ArtifactInstaller, id: &str, version: &str, bytes: &[u8]) {
        let digest = crate::digest::sha256_hex(bytes);
        let entry = crate::cas::put_bytes(&installer.cas, bytes).expect("cas put");
        let target = target(version, &digest, bytes.len() as u64);
        installer.activate(id, &target, &entry).expect("activate");
    }

    #[test]
    fn activate_keeps_one_generation_and_rolls_back() {
        let (_dir, installer) = installer();
        install_bytes(&installer, "fs", "1.0.0", b"v1");
        install_bytes(&installer, "fs", "1.1.0", b"v2");
        install_bytes(&installer, "fs", "1.2.0", b"v3");

        let current = installer.installed("fs").expect("installed");
        assert_eq!(current.version, "1.2.0");
        let previous = current.previous.as_ref().expect("previous");
        assert_eq!(previous.version, "1.1.0");
        assert!(
            previous.previous.is_none(),
            "only one rollback generation is kept"
        );

        let rolled = installer.rollback("fs").expect("rollback");
        assert_eq!(rolled.version, "1.1.0");
        assert!(rolled.previous.is_none());
        // Rolling back twice is not supported (one generation only).
        assert!(installer.rollback("fs").is_err());
    }

    #[test]
    fn state_survives_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = InstallerConfig {
            cas_root: dir.path().join("cas"),
            state_path: dir.path().join("state.json"),
        };
        {
            let installer = ArtifactInstaller::new(config.clone()).expect("installer");
            install_bytes(&installer, "model", "2.0", b"weights");
        }
        let reopened = ArtifactInstaller::new(config).expect("reopen");
        let installed = reopened.installed("model").expect("installed");
        assert_eq!(installed.version, "2.0");
        assert_eq!(installed.sha256, crate::digest::sha256_hex(b"weights"));
    }

    #[test]
    fn gc_keeps_current_and_previous() {
        let (_dir, installer) = installer();
        install_bytes(&installer, "a", "1", b"a1");
        install_bytes(&installer, "a", "2", b"a2");
        install_bytes(&installer, "a", "3", b"a3");
        install_bytes(&installer, "b", "1", b"b1");
        let removed = installer.gc().expect("gc");
        assert_eq!(removed, 1); // a1's first generation was dropped
        assert!(
            !installer
                .cas
                .contains(&crate::digest::sha256_hex(b"a1"))
                .expect("a1 gone")
        );
        assert!(
            installer
                .cas
                .contains(&crate::digest::sha256_hex(b"a2"))
                .expect("a2")
        );
        assert!(
            installer
                .cas
                .contains(&crate::digest::sha256_hex(b"a3"))
                .expect("a3 current")
        );
        assert!(
            installer
                .cas
                .contains(&crate::digest::sha256_hex(b"b1"))
                .expect("b1")
        );
    }

    #[test]
    fn activate_rejects_mismatched_entry() {
        let (_dir, installer) = installer();
        let digest = crate::digest::sha256_hex(b"data");
        let entry = crate::cas::put_bytes(&installer.cas, b"other").expect("cas");
        let err = installer
            .activate("x", &target("1", &digest, 4), &entry)
            .expect_err("digest mismatch must fail");
        assert!(matches!(err, ArtifactError::DigestMismatch { .. }));
    }

    #[test]
    fn rollback_unknown_artifact_fails() {
        let (_dir, installer) = installer();
        assert!(matches!(
            installer.rollback("missing"),
            Err(ArtifactError::ArtifactNotFound(_))
        ));
    }
}
