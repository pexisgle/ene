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

/// Runs the `Stage 2` Host.
///
/// The inference transport is credential-agnostic: every provider request
/// carries the credential the admission resolved for that use, so a consent
/// reassignment bills the new credential on the next request without any
/// transport rebinding. The environment bearer store serves the `openai`
/// provider until real OS stores arrive. Socket-path assembly stays inside
/// [`crate::conn`]: this entry point passes
/// the data directory, never the socket path.
///
/// Startup is ordered around the single-writer lock (PR §6.4): the `0700`
/// data directory and the exclusive `host.lock` come first, then the store
/// open (which runs migrations), then the explicit startup mutations (the
/// presence normalization, the unapproved-pairing cleanup, the credential
/// publication reconciliation, the credential sweep, the bounded
/// retired-credential cleanup, sealed-result
/// reconciliation, orphaned usage-reservation reconciliation, and Targeted
/// Deletion recovery), and only then the
/// listener. A second Host in the same directory is refused before any of
/// that runs.
///
/// # Errors
///
/// Returns [`CoreError::AlreadyRunning`] when another Host holds the data
/// directory, [`CoreError::Store`] when the state cannot be opened, and
/// [`CoreError::Bind`] (or [`CoreError::UnsupportedPlatform`]) when the
/// listener cannot run.
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
