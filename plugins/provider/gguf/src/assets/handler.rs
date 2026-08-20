//! `provider.assets` face for `provider.gguf`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use ene_plugin_ipc::{
    AssetVersionView, AssetView, AssetsHandler, InstallAssetRequest, InstallAssetResult,
    InstallPhase, InstallStatusRequest, InstallStatusResult, IpcError, ListAssetsResult,
    SetActiveAssetRequest, SetActiveAssetResult,
};
use ene_provider_assets::{
    AssetKind, DownloadProgress, Manifest, current_platform, install_version, resolve_active_path,
    resolve_installed_path, store_root, version_matches_platform,
};
use parking_lot::Mutex;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::catalog::{self, CATALOG, PLUGIN_ID};

#[derive(Debug)]
struct InstallJob {
    progress: Arc<Mutex<DownloadProgress>>,
    receiver: Option<oneshot::Receiver<Result<PathBuf, String>>>,
}

pub struct GgufAssets {
    jobs: Mutex<HashMap<String, InstallJob>>,
}

impl GgufAssets {
    #[must_use]
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
        }
    }

    fn build_list(&self) -> ListAssetsResult {
        let manifest = Manifest::load(PLUGIN_ID);
        let platform = current_platform();
        let assets = CATALOG
            .all()
            .iter()
            .map(|row| {
                let installed_path = resolve_installed_path(PLUGIN_ID, row.id)
                    .or_else(|| {
                        if row.kind == AssetKind::Sidecar {
                            manifest.active_version(row.id).and_then(|version| {
                                row.versions
                                    .iter()
                                    .find(|v| v.version == version)
                                    .map(|ver| {
                                        store_root(PLUGIN_ID)
                                            .join(row.id)
                                            .join(version)
                                            .join(ver.filename)
                                    })
                            })
                        } else {
                            None
                        }
                    })
                    .filter(|path| path.is_file());
                let installed = installed_path.is_some();
                let active_version = manifest.active_version(row.id).map(str::to_owned);
                let active = active_version.is_some() && installed;
                AssetView {
                    id: row.id.to_owned(),
                    kind: row.kind.as_str().to_owned(),
                    label: row.label.to_owned(),
                    description: row.description.to_owned(),
                    recommended: row.recommended,
                    installed,
                    active,
                    active_version,
                    local_path: installed_path.map(|path| path.display().to_string()),
                    versions: row
                        .versions
                        .iter()
                        .filter(|version| version_matches_platform(version, platform))
                        .map(|version| AssetVersionView {
                            version: version.version.to_owned(),
                            size_bytes: version.size_bytes,
                            recommended: version.recommended,
                            installed: installed
                                && manifest
                                    .active_version(row.id)
                                    .is_some_and(|active| active == version.version),
                            variant_id: String::new(),
                            label: String::new(),
                            backend: String::new(),
                            release_tag: String::new(),
                        })
                        .collect(),
                    seams: row.seams.iter().map(|seam| (*seam).to_owned()).collect(),
                }
            })
            .collect();
        ListAssetsResult {
            assets,
            error: None,
        }
    }
}

impl Default for GgufAssets {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AssetsHandler for GgufAssets {
    async fn list_assets(&self) -> Result<ListAssetsResult, IpcError> {
        Ok(self.build_list())
    }

    async fn install_asset(
        &self,
        request: InstallAssetRequest,
    ) -> Result<InstallAssetResult, IpcError> {
        let Some(asset) = CATALOG.get(&request.asset_id) else {
            return Ok(InstallAssetResult {
                job_id: String::new(),
                error: Some("asset not found".to_owned()),
            });
        };
        let version = request
            .version
            .as_deref()
            .or_else(|| CATALOG.best_version(asset).map(|row| row.version))
            .ok_or_else(|| IpcError::Call("no version for platform".to_owned()))?;
        let Some((_, catalog_version)) = CATALOG.version(&request.asset_id, version) else {
            return Ok(InstallAssetResult {
                job_id: String::new(),
                error: Some("version not found".to_owned()),
            });
        };
        if !CATALOG.is_allowlisted_url(catalog_version.url) {
            return Ok(InstallAssetResult {
                job_id: String::new(),
                error: Some("url not allowlisted".to_owned()),
            });
        }
        let job_id = Uuid::now_v7().to_string();
        let progress = Arc::new(Mutex::new(DownloadProgress::default()));
        let (tx, rx) = oneshot::channel();
        let asset_id = request.asset_id.clone();
        let kind = asset.kind;
        let version_row = catalog_version.clone();
        let progress_task = Arc::clone(&progress);
        tokio::spawn(async move {
            let result = install_version(PLUGIN_ID, kind, &asset_id, &version_row, progress_task)
                .await
                .map_err(|err| err.to_string());
            drop(tx.send(result));
        });
        self.jobs.lock().insert(
            job_id.clone(),
            InstallJob {
                progress,
                receiver: Some(rx),
            },
        );
        Ok(InstallAssetResult {
            job_id,
            error: None,
        })
    }

    async fn install_status(
        &self,
        request: InstallStatusRequest,
    ) -> Result<InstallStatusResult, IpcError> {
        let mut jobs = self.jobs.lock();
        let Some(job) = jobs.get_mut(&request.job_id) else {
            return Ok(InstallStatusResult {
                error: Some("job not found".to_owned()),
                ..InstallStatusResult::default()
            });
        };
        let snap = *job.progress.lock();
        if let Some(receiver) = job.receiver.as_mut() {
            match receiver.try_recv() {
                Ok(Ok(path)) => {
                    job.receiver = None;
                    return Ok(InstallStatusResult {
                        phase: Some(InstallPhase::Done),
                        received: snap.received,
                        total: snap.total,
                        local_path: Some(path.display().to_string()),
                        error: None,
                    });
                }
                Ok(Err(err)) => {
                    job.receiver = None;
                    return Ok(InstallStatusResult {
                        phase: Some(InstallPhase::Failed),
                        received: snap.received,
                        total: snap.total,
                        error: Some(err),
                        ..InstallStatusResult::default()
                    });
                }
                Err(oneshot::error::TryRecvError::Empty) => Ok(InstallStatusResult {
                    phase: Some(InstallPhase::Downloading),
                    received: snap.received,
                    total: snap.total,
                    error: None,
                    local_path: None,
                }),
                Err(oneshot::error::TryRecvError::Closed) => Ok(InstallStatusResult {
                    phase: Some(InstallPhase::Failed),
                    error: Some("download cancelled".to_owned()),
                    ..InstallStatusResult::default()
                }),
            }
        } else {
            Ok(InstallStatusResult {
                phase: Some(InstallPhase::Done),
                received: snap.received,
                total: snap.total,
                error: None,
                local_path: None,
            })
        }
    }

    async fn set_active(
        &self,
        request: SetActiveAssetRequest,
    ) -> Result<SetActiveAssetResult, IpcError> {
        let Some(asset) = CATALOG.get(&request.asset_id) else {
            return Ok(SetActiveAssetResult {
                error: Some("asset not found".to_owned()),
            });
        };
        let version = request
            .version
            .as_deref()
            .or_else(|| CATALOG.best_version(asset).map(|row| row.version))
            .ok_or_else(|| IpcError::Call("no version".to_owned()))?;
        let mut manifest = Manifest::load(PLUGIN_ID);
        if !manifest.is_installed(&request.asset_id) {
            let path = if asset.kind == AssetKind::Sidecar {
                resolve_active_path(PLUGIN_ID, &request.asset_id, catalog::sidecar_binary_name())
            } else {
                resolve_installed_path(PLUGIN_ID, &request.asset_id)
            };
            if path.is_none() {
                return Ok(SetActiveAssetResult {
                    error: Some("asset is not installed".to_owned()),
                });
            }
        }
        manifest.set_active(&request.asset_id, version);
        manifest
            .save(PLUGIN_ID)
            .map_err(|err| IpcError::Call(err.to_string()))?;
        Ok(SetActiveAssetResult { error: None })
    }
}
