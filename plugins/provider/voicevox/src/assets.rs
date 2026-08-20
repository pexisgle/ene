//! Host-managed assets for `provider.voicevox` (catalog from GitHub).

use async_trait::async_trait;
use ene_plugin_ipc::{
    AssetsHandler, InstallAssetRequest, InstallAssetResult, InstallStatusRequest,
    InstallStatusResult, IpcError, ListAssetsResult, SetActiveAssetRequest, SetActiveAssetResult,
};

pub struct VoicevoxAssets;

#[async_trait]
impl AssetsHandler for VoicevoxAssets {
    async fn list_assets(&self) -> Result<ListAssetsResult, IpcError> {
        Ok(ListAssetsResult {
            assets: Vec::new(),
            error: None,
        })
    }

    async fn install_asset(
        &self,
        _request: InstallAssetRequest,
    ) -> Result<InstallAssetResult, IpcError> {
        Ok(InstallAssetResult {
            job_id: String::new(),
            error: Some("install is host-managed".to_owned()),
        })
    }

    async fn install_status(
        &self,
        _request: InstallStatusRequest,
    ) -> Result<InstallStatusResult, IpcError> {
        Ok(InstallStatusResult {
            error: Some("job not found".to_owned()),
            ..InstallStatusResult::default()
        })
    }

    async fn set_active(
        &self,
        _request: SetActiveAssetRequest,
    ) -> Result<SetActiveAssetResult, IpcError> {
        Ok(SetActiveAssetResult {
            error: Some("set_active is host-managed".to_owned()),
        })
    }
}
