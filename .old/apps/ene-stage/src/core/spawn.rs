use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use ene_api::ApiClient;
use ene_ctl::core::{
    CtlError, find_ene_core_binary, pid_file_path, read_api_ready, wait_for_api_json,
};
use thiserror::Error;

use crate::settings::DesktopSettings;

pub const CLIENT_ID: &str = "stage";

/// Guards a spawned `ene-core` child; kills it on drop when lifetime is `app`.
pub struct StageCore {
    child: Option<Child>,
    kill_on_drop: bool,
}

impl StageCore {
    #[must_use]
    pub const fn detached() -> Self {
        Self {
            child: None,
            kill_on_drop: false,
        }
    }

    #[must_use]
    pub fn child(&self) -> Option<&Child> {
        self.child.as_ref()
    }
}

impl Drop for StageCore {
    fn drop(&mut self) {
        if !self.kill_on_drop {
            return;
        }
        if let Some(mut child) = self.child.take() {
            drop(child.kill());
            drop(child.wait());
        }
    }
}

#[derive(Debug, Error)]
pub enum StageSpawnError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("ctl: {0}")]
    Ctl(#[from] CtlError),
    #[error("api: {0}")]
    Api(#[from] ene_api::ApiError),
    #[error("spawn: {0}")]
    Spawn(String),
    #[error("ready: {0}")]
    Ready(String),
    #[error("token missing from api.json")]
    TokenMissing,
}

/// Attach to an existing core or spawn `ene-core --data-dir`.
///
/// Returns the API client and an optional child handle. When `desktop.core_lifetime`
/// is `app`, the returned [`StageCore`] kills the child on drop; when `detached`,
/// the child keeps running and a pid file is written like `ene-ctl`.
pub async fn attach_or_spawn_core(
    settings: &DesktopSettings,
) -> Result<(ApiClient, StageCore), StageSpawnError> {
    if let Some(client) = try_env_attach().await? {
        return Ok((client, StageCore::detached()));
    }

    let data_dir = stage_data_dir();
    if let Some(client) = try_data_dir_attach(&data_dir).await? {
        return Ok((client, StageCore::detached()));
    }

    let (client, child) = spawn_core(&data_dir, settings).await?;
    let kill_on_drop = settings.core_lifetime == "app";
    let guard = StageCore {
        child: Some(child),
        kill_on_drop,
    };
    Ok((client, guard))
}

#[must_use]
pub fn stage_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ENE_DATA_DIR") {
        return PathBuf::from(dir);
    }
    ene_config::paths::app_data_dir()
}

async fn try_env_attach() -> Result<Option<ApiClient>, StageSpawnError> {
    let Ok(url) = std::env::var("ENE_API_URL") else {
        return Ok(None);
    };
    let Ok(token) = std::env::var("ENE_API_TOKEN") else {
        return Ok(None);
    };
    let client = ApiClient::new(url, token, CLIENT_ID);
    match client.health().await {
        Ok(_) => Ok(Some(client)),
        Err(err) => {
            tracing::debug!(error = %err, "ENE_API_URL health check failed");
            Ok(None)
        }
    }
}

async fn try_data_dir_attach(data_dir: &Path) -> Result<Option<ApiClient>, StageSpawnError> {
    let api_json = data_dir.join("api.json");
    if !api_json.is_file() {
        return Ok(None);
    }
    let ready = match read_api_ready(&api_json) {
        Ok(ready) => ready,
        Err(err) => {
            tracing::debug!(error = %err, path = %api_json.display(), "api.json not ready");
            return Ok(None);
        }
    };
    let Some(token) = ready.token else {
        return Ok(None);
    };
    let client = ApiClient::new(ready.url, token, CLIENT_ID);
    match client.health().await {
        Ok(_) => Ok(Some(client)),
        Err(err) => {
            tracing::debug!(error = %err, "existing core health check failed");
            Ok(None)
        }
    }
}

async fn spawn_core(
    data_dir: &Path,
    settings: &DesktopSettings,
) -> Result<(ApiClient, Child), StageSpawnError> {
    std::fs::create_dir_all(data_dir)?;
    let api_json = data_dir.join("api.json");
    if api_json.is_file() {
        std::fs::remove_file(&api_json)?;
    }

    let bin = find_ene_core_binary()?;
    let mut cmd = Command::new(&bin);
    cmd.arg("--data-dir")
        .arg(data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child = cmd
        .spawn()
        .map_err(|err| StageSpawnError::Spawn(err.to_string()))?;
    let pid = child.id();

    let ready = wait_for_api_json(&api_json, Duration::from_mins(1))
        .await
        .map_err(StageSpawnError::Ctl)?;
    let token = ready.token.ok_or(StageSpawnError::TokenMissing)?;
    let client = ApiClient::new(ready.url, token, CLIENT_ID);
    client.health().await?;

    if settings.core_lifetime != "app" {
        let pid_path = pid_file_path(data_dir);
        std::fs::write(&pid_path, format!("{pid}\n"))?;
        tracing::info!(pid, pid_file = %pid_path.display(), "detached ene-core");
    }

    if child.try_wait()?.is_some() {
        return Err(StageSpawnError::Spawn(
            "ene-core exited before becoming ready".to_owned(),
        ));
    }

    Ok((client, child))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENE_DATA_DIR_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn pid_file_name_matches_ctl() {
        assert_eq!(ene_ctl::core::PID_FILE, "ene-core.pid");
    }

    #[test]
    fn stage_data_dir_honors_ene_data_dir() {
        let _lock = ENE_DATA_DIR_LOCK.lock().expect("env lock");
        let dir = tempfile::tempdir().expect("tempdir");
        // SAFETY: exclusive lock serializes ENE_DATA_DIR mutations in these tests.
        unsafe { std::env::set_var("ENE_DATA_DIR", dir.path()) };
        assert_eq!(stage_data_dir(), dir.path());
        // SAFETY: paired with set_var above in the same test.
        unsafe { std::env::remove_var("ENE_DATA_DIR") };
    }

    #[test]
    fn stage_data_dir_default_is_not_repo_assets() {
        let _lock = ENE_DATA_DIR_LOCK.lock().expect("env lock");
        // SAFETY: exclusive lock serializes ENE_DATA_DIR mutations in these tests.
        unsafe { std::env::remove_var("ENE_DATA_DIR") };
        let dir = stage_data_dir();
        assert_eq!(dir, ene_config::paths::app_data_dir());
        let repo_assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        if let (Ok(got), Ok(assets)) = (dir.canonicalize(), repo_assets.canonicalize()) {
            assert_ne!(got, assets);
        }
    }
}
