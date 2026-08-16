use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::cas::{Cas, CasEntry};
use crate::catalog::{ArtifactKind, ArtifactPayload, ArtifactTarget};
use crate::download::{ArtifactProgress, Downloader, InstallStage};
use crate::error::{ArtifactError, Result};
use crate::extract;

/// One installed generation of an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledArtifact {
    /// Artifact id from the catalog.
    pub id: String,
    pub version: String,
    pub kind: ArtifactKind,
    /// Hex SHA-256 of the active object.
    pub sha256: String,
    /// Exact size in bytes.
    pub size: u64,
    /// Unix millisecond timestamp of activation.
    pub activated_at_ms: u64,
    /// Payload format of the active object (raw file or extracted archive).
    #[serde(default)]
    pub payload: ArtifactPayload,
    /// Generation root for extracted payloads; `None` for raw files (the
    /// CAS object itself is the artifact).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_root: Option<PathBuf>,
    /// Executable path inside `install_root` for extracted payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<String>,
    /// The generation this one replaced. Rollback restores it; the previous
    /// generation's own `previous` is dropped, so exactly one generation of
    /// rollback is kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Box<InstalledArtifact>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledState {
    pub artifacts: BTreeMap<String, InstalledArtifact>,
    /// Highest verified catalog version seen by this installer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_version: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct InstallerConfig {
    pub cas_root: PathBuf,
    /// The `state.json` file survives restarts.
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

    #[must_use]
    pub fn state(&self) -> InstalledState {
        self.state.lock().clone()
    }

    #[must_use]
    pub fn installed(&self, id: &str) -> Option<InstalledArtifact> {
        self.state.lock().artifacts.get(id).cloned()
    }

    /// Used for catalog rollback and digest-change checks.
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

    #[must_use]
    pub fn catalog_version(&self) -> Option<u64> {
        self.state.lock().catalog_version
    }

    /// Persists a verified catalog version, rejecting rollbacks across host
    /// restarts as well as during one process lifetime.
    pub fn record_catalog_version(&self, version: u64) -> Result<()> {
        let mut state = self.state.lock();
        if let Some(current) = state.catalog_version {
            if version < current {
                return Err(ArtifactError::Rollback {
                    artifact: "catalog".to_string(),
                    detail: format!("catalog version {version} is older than {current}"),
                });
            }
            if version == current {
                return Ok(());
            }
        }
        state.catalog_version = Some(version);
        self.persist_locked(&state)
    }

    pub async fn install(
        &self,
        id: &str,
        target: &ArtifactTarget,
        downloader: &Downloader,
        max_bytes: u64,
        on_redirect: &(dyn Fn(&str) -> Result<()> + Sync),
        progress: Option<&(dyn Fn(ArtifactProgress) + Sync)>,
        cancel: Option<&CancellationToken>,
    ) -> Result<InstalledArtifact> {
        let target = target.for_platform(&ArtifactTarget::current_platform());
        crate::digest::validate_digest(&target.sha256)?;
        if self.cas.contains(&target.sha256)?
            && std::fs::metadata(self.cas.object_path(&target.sha256))
                .is_ok_and(|metadata| metadata.len() == target.size)
        {
            let entry = CasEntry {
                sha256: target.sha256.clone(),
                size: target.size,
                path: self.cas.object_path(&target.sha256),
            };
            let generation_dir = self.generation_dir(id, &target.sha256);
            let root = self
                .materialize_payload(&entry, target, &generation_dir, progress, cancel)
                .await?;
            return self.activate_payload(id, target, &entry, root.as_deref());
        }

        let part_dir = self.cas.root().join(".tmp");
        std::fs::create_dir_all(&part_dir)?;
        let generation_dir = self.generation_dir(id, &target.sha256);
        let part_path = part_dir.join(format!("{}.part", target.sha256));
        let mut last_error: Option<ArtifactError> = None;
        for url in &target.urls {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(ArtifactError::Cancelled);
            }
            match downloader
                .download_to(
                    url,
                    &part_path,
                    &target.sha256,
                    target.size,
                    max_bytes,
                    on_redirect,
                    progress,
                    cancel,
                )
                .await
            {
                Ok(_) => {
                    if let Some(report) = progress {
                        report(ArtifactProgress {
                            downloaded_bytes: target.size,
                            total_bytes: Some(target.size),
                            stage: InstallStage::Verify,
                        });
                    }
                    let entry = self.cas.put(
                        std::fs::File::open(&part_path)?,
                        &target.sha256,
                        target.size,
                        max_bytes,
                    )?;
                    drop(std::fs::remove_file(&part_path));
                    let root = self
                        .materialize_payload(&entry, target, &generation_dir, progress, cancel)
                        .await?;
                    let activated = self.activate_payload(id, target, &entry, root.as_deref())?;
                    if let Some(report) = progress {
                        report(ArtifactProgress {
                            downloaded_bytes: target.size,
                            total_bytes: Some(target.size),
                            stage: InstallStage::Activate,
                        });
                    }
                    return Ok(activated);
                }
                Err(e) => {
                    if matches!(e, ArtifactError::Cancelled) {
                        return Err(e);
                    }
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
        self.activate_payload(id, target, entry, None)
    }

    /// Activates a verified CAS object as the new generation for `id`.
    ///
    /// `install_root` names the materialized generation directory for
    /// extracted payloads (already verified by the caller); `None` keeps the
    /// raw CAS object as the artifact.
    fn activate_payload(
        &self,
        id: &str,
        target: &ArtifactTarget,
        entry: &CasEntry,
        install_root: Option<&Path>,
    ) -> Result<InstalledArtifact> {
        if entry.sha256 != target.sha256 || entry.size != target.size {
            return Err(ArtifactError::DigestMismatch {
                artifact: id.to_string(),
                expected: target.sha256.clone(),
                actual: entry.sha256.clone(),
            });
        }
        if entry.path != self.cas.object_path(&entry.sha256)
            || !self.cas.contains(&entry.sha256)?
            || std::fs::metadata(&entry.path).map_or(true, |metadata| metadata.len() != entry.size)
        {
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
        if target.kind == ArtifactKind::Sidecar
            && target
                .payload
                .as_ref()
                .is_none_or(|payload| payload.format == crate::catalog::PayloadFormat::Raw)
        {
            set_executable(&entry.path)?;
        }
        let installed = InstalledArtifact {
            id: id.to_string(),
            version: target.version.clone(),
            kind: target.kind,
            sha256: target.sha256.clone(),
            size: target.size,
            activated_at_ms: now_ms(),
            previous,
            payload: target.payload.clone().unwrap_or_default(),
            install_root: install_root.map(Path::to_path_buf),
            entrypoint: target
                .payload
                .as_ref()
                .and_then(|payload| payload.entrypoint.clone()),
        };
        state.artifacts.insert(id.to_string(), installed.clone());
        self.persist_locked(&state)?;
        Ok(installed)
    }

    /// One-step rollback to the previous generation.
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
        drop(self.remove_generation_dir(&restored.id, &current.sha256));
        state.artifacts.insert(id.to_string(), restored.clone());
        self.persist_locked(&state)?;
        Ok(restored)
    }

    /// Removes the installed generation for `id` (state entry, materialized
    /// files, and the CAS object), keeping the previous generation intact.
    pub fn uninstall(&self, id: &str) -> Result<()> {
        let mut state = self.state.lock();
        let Some(artifact) = state.artifacts.remove(id) else {
            return Err(ArtifactError::ArtifactNotFound(id.to_string()));
        };
        if let Some(previous) = &artifact.previous {
            drop(self.remove_generation_dir(id, &previous.sha256));
        }
        drop(self.remove_generation_dir(id, &artifact.sha256));
        let mut keep = HashSet::new();
        for remaining in state.artifacts.values() {
            keep.insert(remaining.sha256.clone());
            if let Some(previous) = &remaining.previous {
                keep.insert(previous.sha256.clone());
            }
        }
        self.persist_locked(&state)?;
        self.cas.gc(&keep)?;
        Ok(())
    }

    /// Materializes an extracted payload (zip-vvpp) into a fresh generation
    /// directory, returning its root. Raw payloads return `None`.
    ///
    /// On any failure the partial directory is removed so no half-extracted
    /// payload can ever be activated.
    async fn materialize_payload(
        &self,
        entry: &CasEntry,
        target: &ArtifactTarget,
        generation_dir: &Path,
        progress: Option<&(dyn Fn(ArtifactProgress) + Sync)>,
        cancel: Option<&CancellationToken>,
    ) -> Result<Option<PathBuf>> {
        let Some(payload) = &target.payload else {
            return Ok(None);
        };
        if !extract::needs_extraction(payload) {
            return Ok(None);
        }
        let cancel = cancel.cloned();
        let object_path = entry.path.clone();
        let extract_dir = generation_dir.to_path_buf();
        let cleanup_dir = generation_dir.to_path_buf();
        let unpack_limit = payload
            .unpack_limit
            .unwrap_or(crate::catalog::DEFAULT_UNPACK_LIMIT);
        let declared_entrypoint = payload.entrypoint.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
            if let Some(token) = &cancel
                && token.is_cancelled()
            {
                return Err(ArtifactError::Cancelled);
            }
            if extract_dir.exists() {
                std::fs::remove_dir_all(&extract_dir)?;
            }
            std::fs::create_dir_all(&extract_dir)?;
            // Stream from disk: official VOICEVOX VVPP archives are
            // gigabytes, so the archive is never loaded into memory.
            extract::extract_zip_file(
                &object_path,
                &extract_dir,
                declared_entrypoint.as_deref(),
                unpack_limit,
                cancel.as_ref(),
            )?;
            Ok(extract_dir)
        })
        .await
        .map_err(|e| ArtifactError::UnsafeArchive(format!("extraction task panicked: {e}")))?;
        if let Some(report) = progress {
            report(ArtifactProgress {
                downloaded_bytes: entry.size,
                total_bytes: Some(entry.size),
                stage: InstallStage::Extract,
            });
        }
        if result.is_err() {
            drop(std::fs::remove_dir_all(&cleanup_dir));
        }
        result.map(Some)
    }

    /// Absolute generation directory for `(id, sha256)`.
    fn generation_dir(&self, id: &str, sha256: &str) -> PathBuf {
        self.cas.root().join("generations").join(id).join(sha256)
    }

    /// Removes the materialized generation directory for a digest, ignoring
    /// missing directories (raw payloads have no generation directory).
    fn remove_generation_dir(&self, id: &str, sha256: &str) -> Result<()> {
        let dir = self.generation_dir(id, sha256);
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    /// Removes CAS objects and generation directories not referenced by any
    /// current or previous generation, returning the number of removed
    /// objects.
    pub fn gc(&self) -> Result<usize> {
        let state = self.state.lock();
        let mut keep = HashSet::new();
        let mut referenced_dirs: Vec<(String, String)> = Vec::new();
        for artifact in state.artifacts.values() {
            keep.insert(artifact.sha256.clone());
            referenced_dirs.push((artifact.id.clone(), artifact.sha256.clone()));
            if let Some(previous) = &artifact.previous {
                keep.insert(previous.sha256.clone());
                referenced_dirs.push((artifact.id.clone(), previous.sha256.clone()));
            }
        }
        let generations = self.cas.root().join("generations");
        if generations.is_dir() {
            for id_entry in std::fs::read_dir(&generations)? {
                let id_entry = id_entry?;
                let id = id_entry.file_name().to_string_lossy().into_owned();
                for sha_entry in std::fs::read_dir(id_entry.path())? {
                    let sha_entry = sha_entry?;
                    let sha = sha_entry.file_name().to_string_lossy().into_owned();
                    let referenced = referenced_dirs
                        .iter()
                        .any(|(rid, rsha)| rid == &id && rsha == &sha);
                    if !referenced {
                        drop(std::fs::remove_dir_all(sha_entry.path()));
                    }
                }
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

/// Makes a raw sidecar binary executable on Unix (no-op elsewhere). Models
/// and plugin archives never take this path: raw payloads are executable
/// only for `Sidecar` artifacts.
#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
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
            payload: None,
            platforms: BTreeMap::new(),
        }
    }

    /// Builds zip bytes from `entries`.
    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("start file");
            writer.write_all(bytes).expect("write file");
        }
        writer.finish().expect("finish zip").into_inner()
    }

    /// A zip-vvpp fixture: target (with `entrypoint` declared in the
    /// payload) plus the exact archive bytes the digest covers.
    fn zip_fixture(
        version: &str,
        entries: &[(&str, &[u8])],
        entrypoint: &str,
    ) -> (ArtifactTarget, Vec<u8>) {
        let bytes = zip_bytes(entries);
        let digest = crate::digest::sha256_hex(&bytes);
        (
            ArtifactTarget {
                version: version.to_string(),
                kind: ArtifactKind::Sidecar,
                urls: vec!["https://example.test/a.vvpp".to_string()],
                sha256: digest,
                size: bytes.len() as u64,
                payload: Some(crate::catalog::ArtifactPayload {
                    format: crate::catalog::PayloadFormat::ZipVvpp,
                    entrypoint: Some(entrypoint.to_string()),
                    unpack_limit: None,
                }),
                platforms: BTreeMap::new(),
            },
            bytes,
        )
    }

    async fn activate_zip(
        installer: &ArtifactInstaller,
        id: &str,
        target: &ArtifactTarget,
        bytes: &[u8],
    ) {
        let entry = crate::cas::put_bytes(&installer.cas, bytes).expect("cas put");
        assert_eq!(entry.sha256, target.sha256, "fixture digest mismatch");
        let generation = installer.generation_dir(id, &target.sha256);
        let root = installer
            .materialize_payload(&entry, target, &generation, None, None)
            .await
            .expect("materialize")
            .expect("zip payload materializes");
        installer
            .activate_payload(id, target, &entry, Some(&root))
            .expect("activate");
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

    /// A failed payload extraction (hostile archive) leaves the current
    /// generation and the persisted state untouched, and removes the
    /// partial extraction directory.
    #[tokio::test]
    async fn failed_payload_extraction_keeps_current_generation() {
        let (_dir, installer) = installer();
        let (v1, v1_bytes) = zip_fixture(
            "1.0.0",
            &[
                ("engine_manifest.json", br#"{"command":"run.sh"}"#),
                ("run.sh", b"v1"),
            ],
            "run.sh",
        );
        activate_zip(&installer, "voicevox", &v1, &v1_bytes).await;
        let before = installer.state();

        let bad = zip_bytes(&[
            ("engine_manifest.json", br#"{"command":"run.sh"}"#),
            ("../escape", b"evil"),
        ]);
        let digest = crate::digest::sha256_hex(&bad);
        let entry = crate::cas::put_bytes(&installer.cas, &bad).expect("cas put");
        let target = ArtifactTarget {
            version: "2.0.0".to_string(),
            kind: ArtifactKind::Sidecar,
            urls: vec!["https://example.test/bad.vvpp".to_string()],
            sha256: digest.clone(),
            size: bad.len() as u64,
            payload: Some(crate::catalog::ArtifactPayload {
                format: crate::catalog::PayloadFormat::ZipVvpp,
                entrypoint: Some("run.sh".to_string()),
                unpack_limit: None,
            }),
            platforms: BTreeMap::new(),
        };
        let generation = installer.generation_dir("voicevox", &digest);
        let result = installer
            .materialize_payload(&entry, &target, &generation, None, None)
            .await;
        assert!(result.is_err(), "traversal payload must fail extraction");
        assert_eq!(installer.state(), before, "state is unchanged");
        assert!(
            installer.installed("voicevox").expect("installed").version == "1.0.0",
            "the active generation is unchanged"
        );
        assert!(!generation.exists(), "partial extraction is removed");
    }

    /// A cancelled install never touches the installed state.
    #[tokio::test]
    async fn cancelled_install_leaves_state_unchanged() {
        use crate::download::Downloader;
        use tokio_util::sync::CancellationToken;

        let (_dir, installer) = installer();
        install_bytes(&installer, "fs", "1.0.0", b"v1");
        let before = installer.state();

        let target = target("2.0.0", &crate::digest::sha256_hex(b"v2"), 2);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = installer
            .install(
                "fs",
                &target,
                &Downloader::test_new(),
                1024,
                &|_| Err(ArtifactError::RedirectRejected("unexpected".to_string())),
                None,
                Some(&cancel),
            )
            .await
            .expect_err("pre-cancelled install aborts");
        assert!(matches!(err, ArtifactError::Cancelled));
        assert_eq!(installer.state(), before);
        assert_eq!(
            installer.installed("fs").expect("installed").version,
            "1.0.0"
        );
    }

    #[tokio::test]
    async fn zip_payload_materializes_and_entrypoint_is_executable() {
        let (_dir, installer) = installer();
        let (target, bytes) = zip_fixture(
            "1.0.0",
            &[
                ("engine_manifest.json", br#"{"command":"run.sh"}"#),
                ("run.sh", b"#!/bin/sh\n"),
            ],
            "run.sh",
        );
        activate_zip(&installer, "voicevox", &target, &bytes).await;

        let active_artifact = installer.installed("voicevox").expect("installed");
        assert_eq!(active_artifact.version, "1.0.0");
        let root = active_artifact.install_root.as_ref().expect("install root");
        assert!(root.join("run.sh").is_file());
        assert!(root.join("engine_manifest.json").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join("run.sh"))
                .expect("metadata")
                .permissions()
                .mode();
            assert_ne!(mode & 0o111, 0, "entrypoint must be executable");
        }
    }

    #[tokio::test]
    async fn zip_payload_update_rolls_back_and_gc_removes_orphans() {
        let (_dir, installer) = installer();
        let (v1, v1_bytes) = zip_fixture(
            "1.0.0",
            &[
                ("engine_manifest.json", br#"{"command":"run.sh"}"#),
                ("run.sh", b"v1"),
            ],
            "run.sh",
        );
        let (v2, v2_bytes) = zip_fixture(
            "1.1.0",
            &[
                ("engine_manifest.json", br#"{"command":"run.sh"}"#),
                ("run.sh", b"v2"),
            ],
            "run.sh",
        );
        activate_zip(&installer, "voicevox", &v1, &v1_bytes).await;
        activate_zip(&installer, "voicevox", &v2, &v2_bytes).await;

        let rolled = installer.rollback("voicevox").expect("rollback");
        assert_eq!(rolled.version, "1.0.0");
        assert_eq!(
            rolled.sha256, v1.sha256,
            "rollback restores the previous generation bytes"
        );
        // The rolled-away generation directory is removed by gc.
        installer.gc().expect("gc");
        assert!(
            !installer.generation_dir("voicevox", &v2.sha256).exists(),
            "orphaned generation directory must be gc'd"
        );
        assert!(
            installer.generation_dir("voicevox", &v1.sha256).exists(),
            "active generation directory is kept"
        );
    }

    #[tokio::test]
    async fn uninstall_removes_state_and_generation() {
        let (_dir, installer) = installer();
        let (target, bytes) = zip_fixture(
            "1.0.0",
            &[
                ("engine_manifest.json", br#"{"command":"run.sh"}"#),
                ("run.sh", b"x"),
            ],
            "run.sh",
        );
        activate_zip(&installer, "voicevox", &target, &bytes).await;
        installer.uninstall("voicevox").expect("uninstall");
        assert!(installer.installed("voicevox").is_none());
        assert!(
            !installer
                .generation_dir("voicevox", &target.sha256)
                .exists()
        );
        assert!(
            !installer
                .cas
                .contains(&target.sha256)
                .expect("object removed")
        );
        assert!(matches!(
            installer.uninstall("voicevox"),
            Err(ArtifactError::ArtifactNotFound(_))
        ));
    }

    #[tokio::test]
    async fn install_picks_platform_variant_and_reports_progress() {
        // The test never hits the network: the platform variant's URL is
        // only reached when the digest is absent, so this locks the
        // selection logic and the progress callback plumbing instead.
        let (_dir, installer) = installer();
        let (base, _base_bytes) = zip_fixture(
            "1.0.0",
            &[
                ("engine_manifest.json", br#"{"command":"run.sh"}"#),
                ("run.sh", b"base"),
            ],
            "run.sh",
        );
        let platform = ArtifactTarget::current_platform();
        let (variant, variant_bytes) = zip_fixture(
            "1.0.0",
            &[
                ("engine_manifest.json", br#"{"command":"run.sh"}"#),
                ("run.sh", b"platform"),
            ],
            "run.sh",
        );
        let mut target = base.clone();
        target.platforms.insert(platform.clone(), variant);

        let selected = target.for_platform(&platform);
        assert_eq!(selected.sha256, target.platforms[&platform].sha256);
        let fallback = target.for_platform("s390x-unknown-linux");
        assert_eq!(fallback.sha256, base.sha256);

        // Activating through the extracted path (as install does) reports
        // nothing to the progress callback — extraction is synchronous —
        // but the payload is still materialized.
        let entry = crate::cas::put_bytes(&installer.cas, &variant_bytes).expect("cas put");
        assert_eq!(entry.sha256, selected.sha256, "variant digest mismatch");
        let generation = installer.generation_dir("voicevox", &selected.sha256);
        let root = installer
            .materialize_payload(&entry, selected, &generation, None, None)
            .await
            .expect("materialize")
            .expect("zip materializes");
        let active_artifact = installer
            .activate_payload("voicevox", selected, &entry, Some(&root))
            .expect("activate");
        assert_eq!(active_artifact.entrypoint.as_deref(), Some("run.sh"));
    }
}
