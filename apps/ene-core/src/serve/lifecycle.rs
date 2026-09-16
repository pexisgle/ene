//! Host data-directory preparation and the `serve` entry point.

use std::path::Path;
use std::sync::Arc;

use super::{CoreError, HostHandle};
use ene_credential::EnvCredentialStore;
use ene_inference::provider::{DEFAULT_BASE_URL, OpenAiResponsesTransport};

#[cfg(unix)]
pub(crate) fn ensure_data_dir(data_dir: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(data_dir)
        .map_err(|error| CoreError::Store(format!("create data directory: {error}")))?;
    // Creation mode applies only to created directories: a pre-existing dir
    // keeps whatever mode it had. The same-machine socket trust premise needs
    // owner-only, so tighten rather than serve exposed; a tighten failure
    // fails startup (fail-closed).
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
/// sweep, and sealed-result reconciliation), and only then the
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
    // The serving startup mutations (PR §6.4 steps 2-5) run after the state
    // open and before the listener binds; see
    // [`HostHandle::run_startup_mutations`] for the order contract.
    handle.run_startup_mutations().await?;
    // Base URL override for self-hosted endpoints and tests: production
    // keeps [`DEFAULT_BASE_URL`]. The test harness points a real `serve`
    // binary at a local fake Responses server through this variable (child
    // process env only; no test ever mutates its own environment).
    let base_url = std::env::var("ENE_OPENAI_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let transport = OpenAiResponsesTransport::new(base_url, EnvCredentialStore::new())
        .map_err(|error| CoreError::Inference(error.to_string()))?;
    crate::conn::run(
        data_dir.to_path_buf(),
        Arc::new(handle),
        Arc::new(transport),
    )
    .await
}
