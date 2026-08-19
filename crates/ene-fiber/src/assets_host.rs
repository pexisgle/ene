use std::collections::HashMap;
use std::sync::Arc;

use ene_plugin_ipc::{
    InstallAssetRequest, InstallAssetResult, InstallPhase, InstallStatusRequest,
    InstallStatusResult, ListAssetsResult, SetActiveAssetRequest, SetActiveAssetResult,
};
use ene_provider_assets::{
    CatalogRegistry, DownloadProgress, Manifest, RuntimeCatalog, install_variant,
    merge_host_catalog, runtime_asset_hosted,
};
use parking_lot::Mutex;
use tokio::sync::{Mutex as AsyncMutex, oneshot};
use uuid::Uuid;

use crate::supervisor::SupervisorError;

struct HostInstallJob {
    progress: Arc<Mutex<DownloadProgress>>,
    receiver: Option<oneshot::Receiver<Result<std::path::PathBuf, String>>>,
}

pub struct HostAssets {
    catalog: Arc<CatalogRegistry>,
    install_jobs: AsyncMutex<HashMap<String, HostInstallJob>>,
}

impl HostAssets {
    #[must_use]
    pub fn new(catalog: Arc<CatalogRegistry>) -> Self {
        Self {
            catalog,
            install_jobs: AsyncMutex::new(HashMap::new()),
        }
    }

    pub async fn list_assets(
        &self,
        plugin: &str,
        probe: impl std::future::Future<Output = Result<ListAssetsResult, SupervisorError>>,
    ) -> Result<ListAssetsResult, SupervisorError> {
        if !ene_provider_assets::host_catalog_plugin(plugin) {
            return probe.await;
        }
        let catalog = self
            .catalog
            .ensure_fresh(plugin, false)
            .await
            .map_err(|err| map_asset_err(&err))?;
        let probed = probe.await.unwrap_or_default();
        Ok(merge_host_catalog(plugin, &catalog, probed))
    }

    pub async fn refresh_catalog(&self, plugin: &str) -> Result<RuntimeCatalog, SupervisorError> {
        self.catalog
            .refresh(plugin)
            .await
            .map_err(|err| map_asset_err(&err))
    }

    pub async fn install_asset(
        &self,
        plugin: &str,
        request: InstallAssetRequest,
        probe: impl std::future::Future<Output = Result<InstallAssetResult, SupervisorError>>,
    ) -> Result<InstallAssetResult, SupervisorError> {
        if !runtime_asset_hosted(plugin, &request.asset_id) {
            return probe.await;
        }
        let catalog = self
            .catalog
            .ensure_fresh(plugin, false)
            .await
            .map_err(|err| map_asset_err(&err))?;
        let (release_tag, variant_id) = resolve_install_target(&request)?;
        let Some((_, _, variant)) =
            catalog.find_variant(&request.asset_id, &release_tag, &variant_id)
        else {
            return Ok(InstallAssetResult {
                job_id: String::new(),
                error: Some("variant not found in catalog".to_owned()),
            });
        };
        let job_id = Uuid::now_v7().to_string();
        let progress = Arc::new(Mutex::new(DownloadProgress::default()));
        let (tx, rx) = oneshot::channel();
        let progress_task = Arc::clone(&progress);
        let plugin_id = plugin.to_owned();
        let asset_id = request.asset_id.clone();
        let release = release_tag.clone();
        let variant = variant.clone();
        tokio::spawn(async move {
            let result = install_variant(&plugin_id, &asset_id, &release, &variant, progress_task)
                .await
                .map_err(|err| err.to_string());
            drop(tx.send(result));
        });
        self.install_jobs.lock().await.insert(
            job_id.clone(),
            HostInstallJob {
                progress,
                receiver: Some(rx),
            },
        );
        Ok(InstallAssetResult {
            job_id,
            error: None,
        })
    }

    pub async fn install_status(
        &self,
        request: InstallStatusRequest,
    ) -> Result<InstallStatusResult, SupervisorError> {
        let mut jobs = self.install_jobs.lock().await;
        let Some(job) = jobs.get_mut(&request.job_id) else {
            return Ok(InstallStatusResult {
                error: Some("job not found".to_owned()),
                ..InstallStatusResult::default()
            });
        };
        let snap = *job.progress.lock();
        let Some(receiver) = job.receiver.as_mut() else {
            return Ok(InstallStatusResult {
                phase: Some(InstallPhase::Done),
                received: snap.received,
                total: snap.total,
                error: None,
                local_path: None,
            });
        };
        match receiver.try_recv() {
            Ok(Ok(path)) => {
                job.receiver = None;
                Ok(InstallStatusResult {
                    phase: Some(InstallPhase::Done),
                    received: snap.received,
                    total: snap.total,
                    local_path: Some(path.display().to_string()),
                    error: None,
                })
            }
            Ok(Err(err)) => {
                job.receiver = None;
                Ok(InstallStatusResult {
                    phase: Some(InstallPhase::Failed),
                    received: snap.received,
                    total: snap.total,
                    error: Some(err),
                    ..InstallStatusResult::default()
                })
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
    }

    pub async fn set_active_asset(
        &self,
        plugin: &str,
        request: SetActiveAssetRequest,
        probe: impl std::future::Future<Output = Result<SetActiveAssetResult, SupervisorError>>,
    ) -> Result<SetActiveAssetResult, SupervisorError> {
        if !runtime_asset_hosted(plugin, &request.asset_id) {
            return probe.await;
        }
        let Some(version) = request.version.filter(|row| !row.is_empty()) else {
            return Ok(SetActiveAssetResult {
                error: Some("version required".to_owned()),
            });
        };
        let mut manifest = Manifest::load(plugin);
        if !manifest.is_installed(&request.asset_id) {
            return Ok(SetActiveAssetResult {
                error: Some("asset is not installed".to_owned()),
            });
        }
        manifest.set_active(&request.asset_id, version);
        manifest.save(plugin).map_err(SupervisorError::Io)?;
        Ok(SetActiveAssetResult { error: None })
    }
}

fn resolve_install_target(
    request: &InstallAssetRequest,
) -> Result<(String, String), SupervisorError> {
    if let Some(version) = &request.version {
        if let Some((tag, variant)) = RuntimeCatalog::split_install_key(version) {
            return Ok((tag.to_owned(), variant.to_owned()));
        }
        let variant = request
            .variant
            .clone()
            .ok_or_else(|| SupervisorError::Spawn("variant required".to_owned()))?;
        return Ok((version.clone(), variant));
    }
    Err(SupervisorError::Spawn("version required".to_owned()))
}

fn map_asset_err(err: &ene_provider_assets::AssetError) -> SupervisorError {
    SupervisorError::Spawn(err.to_string())
}
