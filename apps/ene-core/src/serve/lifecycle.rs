use std::path::Path;
use std::sync::Arc;

use super::{CoreError, HostHandle};
use ene_credential::{CredentialRef, CredentialStore, CredentialTechnicalError};
use ene_inference::provider::{DEFAULT_BASE_URL, OpenAiResponsesTransport};

struct ServingCredentials(Arc<HostHandle>);

impl CredentialStore for ServingCredentials {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        self.0.cred_store.with_bearer(cred, f)
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        self.0.cred_store.contains(cred)
    }

    fn put(&self, _cred: &CredentialRef, _secret: &str) -> Result<(), CredentialTechnicalError> {
        Err(CredentialTechnicalError::StorageUnavailable {
            reason: String::from("transport credentials are read-only; use Host publication"),
        })
    }
}

#[cfg(unix)]
pub(crate) fn ensure_data_dir(data_dir: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(data_dir)
        .map_err(|error| CoreError::Store(format!("create data directory: {error}")))?;
    let mode = std::fs::metadata(data_dir)
        .map_err(|error| CoreError::Store(format!("stat data directory: {error}")))?
        .permissions()
        .mode()
        & 0o777;
    if mode & 0o077 != 0 {
        std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| CoreError::Store(format!("protect data directory: {error}")))?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn ensure_data_dir(data_dir: &Path) -> Result<(), CoreError> {
    std::fs::create_dir_all(data_dir)
        .map_err(|error| CoreError::Store(format!("create data directory: {error}")))?;
    Ok(())
}

pub async fn serve(data_dir: &Path) -> Result<(), CoreError> {
    let _lock = crate::host_lock::HostLock::acquire(data_dir)?;
    let handle = HostHandle::open(data_dir).await?;
    handle.run_startup_mutations().await?;
    let base_url = std::env::var("ENE_OPENAI_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let handle = Arc::new(handle);
    let transport =
        OpenAiResponsesTransport::new(base_url, ServingCredentials(Arc::clone(&handle)))
            .map_err(|error| CoreError::Inference(error.to_string()))?;
    crate::conn::run(data_dir.to_path_buf(), handle, Arc::new(transport)).await
}
