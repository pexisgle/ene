//! Host-side artifact state: catalog + CAS access for handshake injection
//! and the Engines settings page.
//!
//! The broker remains the plugin-facing artifact path (approval-gated);
//! this module is the host's own copy of the same services, used to:
//!
//! - inject catalog-managed sidecar and model paths into the config and
//!   profiles delivered to plugins at handshake (empty slots only), and
//! - expose installed / catalog state to the settings UI and perform
//!   UI-triggered installs, updates, and rollbacks.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ene_approval::PluginManifest;
use ene_artifact::catalog::compare_versions;
use ene_artifact::{
    ArtifactInstaller, ArtifactKind, CatalogMetadata, CatalogVerifier, Downloader,
    InstalledArtifact, InstallerConfig, SignedCatalog, TrustedCatalogKeys,
};

use crate::config::ArtifactConfig;

/// Built-in sidecar artifact ids per plugin name, used until the plugins'
/// signed manifests declare `sidecars` themselves.
fn builtin_sidecar_artifacts(plugin: &str) -> &'static [&'static str] {
    match plugin {
        "llama-server" => &["llama-server"],
        "whisper" => &["whisper-server"],
        _ => &[],
    }
}

/// Sidecar artifact ids a plugin may use: manifest requirements plus the
/// built-in table for plugins whose shipped manifest predates the catalog.
#[must_use]
pub fn sidecar_ids_for(plugin: &str, manifest: Option<&PluginManifest>) -> Vec<String> {
    let mut ids: Vec<String> = manifest
        .map(|m| {
            m.sidecars
                .iter()
                .map(|requirement| requirement.artifact_id.clone())
                .collect()
        })
        .unwrap_or_default();
    for id in builtin_sidecar_artifacts(plugin) {
        if !ids.iter().any(|existing| existing == id) {
            ids.push((*id).to_string());
        }
    }
    ids
}

/// Installed artifact view for the settings UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstalledArtifactView {
    /// Active version.
    pub version: String,
    /// Artifact kind (`plugin`, `sidecar`, or `model`).
    pub kind: String,
    /// Hex SHA-256 of the active object.
    pub sha256: String,
    /// Size in bytes.
    pub size: u64,
    /// CAS object path (read-only for plugins).
    pub path: String,
}

impl InstalledArtifactView {
    fn from_artifact(artifact: &InstalledArtifact, object_path: &Path) -> Self {
        Self {
            version: artifact.version.clone(),
            kind: match artifact.kind {
                ArtifactKind::Plugin => "plugin",
                ArtifactKind::Sidecar => "sidecar",
                ArtifactKind::Model => "model",
            }
            .to_string(),
            sha256: artifact.sha256.clone(),
            size: artifact.size,
            path: object_path.to_string_lossy().into_owned(),
        }
    }
}

/// Catalog target view for the settings UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CatalogTargetView {
    /// Catalog version for the artifact.
    pub version: String,
    /// Artifact kind.
    pub kind: String,
    /// Hex SHA-256 of the artifact bytes.
    pub sha256: String,
    /// Size in bytes.
    pub size: u64,
    /// Ordered mirror URLs.
    pub urls: Vec<String>,
}

/// One artifact row for the Engines page.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ArtifactSnapshot {
    /// Artifact id from the catalog.
    pub artifact_id: String,
    /// Installed generation, when present.
    pub installed: Option<InstalledArtifactView>,
    /// Catalog target, when the catalog is reachable and lists the id.
    pub catalog: Option<CatalogTargetView>,
    /// Whether the catalog offers a newer version than installed.
    pub update_available: bool,
    /// Catalog-level error (unreachable / signature / rollback).
    pub error: Option<String>,
}

/// Host-side artifact services. Built when [`ArtifactConfig`] is enabled and
/// a catalog URL plus trusted keys are configured; otherwise `None`, which
/// keeps every existing code path identical to the pre-catalog behavior.
pub struct ArtifactState {
    installer: ArtifactInstaller,
    verifier: CatalogVerifier,
    downloader: Downloader,
    http: reqwest::Client,
    config: ArtifactConfig,
    /// Cached verified catalog plus its fetch timestamp (Unix ms).
    catalog: Mutex<Option<(CatalogMetadata, u64)>>,
}

impl ArtifactState {
    /// Builds the state, or `None` when the artifact system is inactive or
    /// misconfigured.
    #[must_use]
    pub fn from_config(config: &ArtifactConfig) -> Option<Self> {
        if !config.enabled {
            return None;
        }
        let root = config.root_dir.as_ref().map_or_else(
            || ene_config::app_data_dir().join("artifacts"),
            PathBuf::from,
        );
        let keys = TrustedCatalogKeys::from_hex(
            &config
                .catalog_keys
                .iter()
                .map(|key| (key.key_id.clone(), key.public_key_hex.clone()))
                .collect::<Vec<_>>(),
        )
        .ok()?;
        if config.catalog_url.is_none() || keys.is_empty() {
            tracing::warn!("artifact system enabled but catalog_url/keys missing");
            return None;
        }
        let installer = ArtifactInstaller::new(InstallerConfig {
            cas_root: root.join("cas"),
            state_path: root.join("state.json"),
        })
        .ok()?;
        let downloader = Downloader::new(
            Some(Duration::from_secs(30)),
            Some(Duration::from_millis(config.timeout_ms)),
            config.max_redirects,
        )
        .ok()?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .ok()?;
        Some(Self {
            installer,
            verifier: CatalogVerifier::new(keys),
            downloader,
            http,
            config: config.clone(),
            catalog: Mutex::new(None),
        })
    }

    /// CAS root directory (sandboxed plugins get read access to it).
    #[must_use]
    pub fn cas_root(&self) -> PathBuf {
        self.config
            .root_dir
            .as_ref()
            .map_or_else(
                || ene_config::app_data_dir().join("artifacts"),
                PathBuf::from,
            )
            .join("cas")
    }

    /// CAS object path for a digest (`root/objects/{2}/{rest}`).
    fn object_path(&self, sha256: &str) -> PathBuf {
        let (prefix, rest) = sha256.split_at(2);
        self.cas_root().join("objects").join(prefix).join(rest)
    }

    /// Path of the installed artifact's active object, if installed.
    #[must_use]
    pub fn installed_path(&self, id: &str) -> Option<PathBuf> {
        self.installer
            .installed(id)
            .map(|artifact| self.object_path(&artifact.sha256))
    }

    /// Snapshot of every installed artifact plus catalog targets.
    pub async fn snapshot(&self) -> Vec<ArtifactSnapshot> {
        let installed = self.installer.state();
        let (metadata, error) = match self.catalog_metadata(false).await {
            Ok(metadata) => (Some(metadata), None),
            Err(e) => (None, Some(e)),
        };
        let mut ids: Vec<String> = installed.artifacts.keys().cloned().collect();
        if let Some(metadata) = &metadata {
            for id in metadata.artifacts.keys() {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
        ids.sort();
        ids.into_iter()
            .map(|id| {
                let current = installed.artifacts.get(&id);
                let target = metadata.as_ref().and_then(|m| m.artifacts.get(&id));
                let update_available = match (current, target) {
                    (Some(installed), Some(target)) => {
                        compare_versions(&target.version, &installed.version)
                            == std::cmp::Ordering::Greater
                    }
                    (None, Some(_)) => true,
                    _ => false,
                };
                ArtifactSnapshot {
                    artifact_id: id.clone(),
                    installed: current.map(|artifact| {
                        InstalledArtifactView::from_artifact(
                            artifact,
                            &self.object_path(&artifact.sha256),
                        )
                    }),
                    catalog: target.map(|target| CatalogTargetView {
                        version: target.version.clone(),
                        kind: match target.kind {
                            ArtifactKind::Plugin => "plugin",
                            ArtifactKind::Sidecar => "sidecar",
                            ArtifactKind::Model => "model",
                        }
                        .to_string(),
                        sha256: target.sha256.clone(),
                        size: target.size,
                        urls: target.urls.clone(),
                    }),
                    update_available,
                    error: error.clone(),
                }
            })
            .collect()
    }

    /// Installs (or updates) `id` from the catalog and returns the new view.
    pub async fn install(
        &self,
        id: &str,
        version: Option<&str>,
    ) -> Result<InstalledArtifactView, String> {
        let metadata = self.catalog_metadata(true).await?;
        let target = metadata
            .artifacts
            .get(id)
            .ok_or_else(|| format!("artifact '{id}' not in catalog"))?;
        if let Some(version) = version
            && target.version != version
        {
            return Err(format!(
                "artifact '{id}' version {version} not in catalog ({} available)",
                target.version
            ));
        }
        let installed = self
            .installer
            .install(id, target, &self.downloader, self.config.max_bytes, &|_| {
                Err(ene_artifact::ArtifactError::RedirectRejected(
                    "artifact redirects are not followed".to_string(),
                ))
            })
            .await
            .map_err(|e| e.to_string())?;
        Ok(InstalledArtifactView::from_artifact(
            &installed,
            &self.object_path(&installed.sha256),
        ))
    }

    /// Rolls the artifact back one generation.
    pub fn rollback(&self, id: &str) -> Result<InstalledArtifactView, String> {
        let installed = self.installer.rollback(id).map_err(|e| e.to_string())?;
        Ok(InstalledArtifactView::from_artifact(
            &installed,
            &self.object_path(&installed.sha256),
        ))
    }

    /// Force-refreshes the catalog and returns its version.
    pub async fn refresh_catalog(&self) -> Result<u64, String> {
        self.catalog_metadata(true).await.map(|m| m.version)
    }

    /// Injects catalog-managed sidecar/model paths into delivered config and
    /// profiles. Only empty slots are filled; user-configured values win.
    pub fn inject(
        &self,
        sidecar_ids: &[String],
        config: &mut Option<serde_json::Value>,
        profiles: &mut Option<serde_json::Value>,
    ) {
        if let Some(cfg) = config.as_mut().and_then(serde_json::Value::as_object_mut) {
            for id in sidecar_ids {
                let Some(path) = self.installed_path(id) else {
                    continue;
                };
                let already = cfg
                    .get("server_path")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty());
                if !already {
                    cfg.insert(
                        "server_path".to_string(),
                        serde_json::Value::String(path.to_string_lossy().into_owned()),
                    );
                }
            }
        }
        if let Some(profiles_obj) = profiles.as_mut().and_then(serde_json::Value::as_object_mut) {
            for profile in profiles_obj.values_mut() {
                let Some(obj) = profile.as_object_mut() else {
                    continue;
                };
                let Some(artifact_id) = obj
                    .get("artifact_id")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                else {
                    continue;
                };
                let already = obj
                    .get("model_path")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty());
                if already {
                    continue;
                }
                if let Some(path) = self.installed_path(artifact_id) {
                    obj.insert(
                        "model_path".to_string(),
                        serde_json::Value::String(path.to_string_lossy().into_owned()),
                    );
                }
            }
        }
        if let Some(cfg) = config.as_mut().and_then(serde_json::Value::as_object_mut) {
            let Some(mmproj_id) = cfg
                .get("mmproj_artifact_id")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty())
            else {
                return;
            };
            let already = cfg
                .get("mmproj_path")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| !s.trim().is_empty());
            if already {
                return;
            }
            if let Some(path) = self.installed_path(mmproj_id) {
                cfg.insert(
                    "mmproj_path".to_string(),
                    serde_json::Value::String(path.to_string_lossy().into_owned()),
                );
            }
        }
    }

    async fn catalog_metadata(&self, force: bool) -> Result<CatalogMetadata, String> {
        let now = now_ms();
        let refresh_ms = self.config.refresh_hours.max(1).saturating_mul(3600 * 1000);
        if !force
            && let Some((catalog, fetched_at)) = self
                .catalog
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            && now.saturating_sub(*fetched_at) < refresh_ms
        {
            return Ok(catalog.clone());
        }
        let url = self
            .config
            .catalog_url
            .as_deref()
            .ok_or_else(|| "no catalog URL configured".to_string())?;
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| format!("catalog fetch failed: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("catalog fetch failed: {}", response.status()));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("catalog fetch failed: {e}"))?;
        let signed: SignedCatalog =
            serde_json::from_slice(&bytes).map_err(|e| format!("invalid catalog JSON: {e}"))?;
        let metadata = self
            .verifier
            .verify(&signed, &self.installer.installed_refs(), now)
            .map_err(|e| format!("catalog verification failed: {e}"))?;
        self.installer
            .record_catalog_version(metadata.version)
            .map_err(|e| e.to_string())?;
        *self
            .catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((metadata.clone(), now));
        Ok(metadata)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().try_into().unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_sidecar_table_covers_sidecar_plugins() {
        assert_eq!(builtin_sidecar_artifacts("llama-server"), &["llama-server"]);
        assert_eq!(builtin_sidecar_artifacts("whisper"), &["whisper-server"]);
        assert!(builtin_sidecar_artifacts("kokoro").is_empty());
    }

    #[test]
    fn sidecar_ids_merge_manifest_and_builtin() {
        let manifest = PluginManifest {
            schema_version: 1,
            plugin_id: "llama-server".to_string(),
            name: "llama-server".to_string(),
            publisher: "ene".to_string(),
            version: "1".to_string(),
            description: None,
            fs_slots: Vec::new(),
            fixed_origins: Vec::new(),
            dynamic_web: false,
            artifacts: Vec::new(),
            sidecars: vec![ene_approval::manifest::SidecarRequirement {
                artifact_id: "llama-server".to_string(),
                version_constraint: ">=1".to_string(),
            }],
            host_services: Vec::new(),
            side_effects: ene_approval::ManifestSideEffects::default(),
            resource_limits: ene_approval::ResourceLimits::default(),
            permissions: Vec::new(),
        };
        let ids = sidecar_ids_for("llama-server", Some(&manifest));
        assert_eq!(ids, vec!["llama-server".to_string()]);
    }

    #[test]
    fn object_path_matches_cas_layout() {
        let config = ArtifactConfig {
            enabled: true,
            catalog_url: Some("https://example.test/catalog.json".to_string()),
            catalog_keys: vec![],
            root_dir: Some("/tmp/ene-artifacts".to_string()),
            ..ArtifactConfig::default()
        };
        // from_config requires keys; construct the path logic directly via a
        // stubbed state to lock the layout used by injection.
        let state = ArtifactState {
            installer: ArtifactInstaller::new(InstallerConfig {
                cas_root: "/tmp/ene-artifacts/cas".into(),
                state_path: "/tmp/ene-artifacts/state.json".into(),
            })
            .expect("installer"),
            verifier: CatalogVerifier::new(TrustedCatalogKeys::default()),
            downloader: Downloader::new(None, None, 5).expect("downloader"),
            http: reqwest::Client::new(),
            config,
            catalog: Mutex::new(None),
        };
        let digest = "ab".repeat(32);
        let path = state.object_path(&digest);
        assert!(path.ends_with(format!("objects/ab/{}", "ab".repeat(31))));
    }
}
