//! Host data-directory preparation and the `serve` entry point.

use std::path::Path;
use std::sync::Arc;

use super::{CoreError, HostHandle};
use ene_credential::EnvCredentialStore;
use ene_inference::provider::{DEFAULT_BASE_URL, OpenAiResponsesTransport};

/// Ensures the Host data directory exists.
#[cfg(unix)]
pub(super) fn ensure_data_dir(data_dir: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(data_dir)
        .map_err(|error| CoreError::Store(format!("create data directory: {error}")))?;
    // Creation mode applies only to created directories: a pre-existing dir
    // keeps whatever mode it had, which may predate this Host. The
    // same-machine socket trust premise needs owner-only, so tighten rather
    // than serve exposed; a tighten failure fails startup (fail-closed).
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

/// Ensures the Host data directory exists.
#[cfg(not(unix))]
pub(super) fn ensure_data_dir(data_dir: &Path) -> Result<(), CoreError> {
    std::fs::create_dir_all(data_dir)
        .map_err(|error| CoreError::Store(format!("create data directory: {error}")))?;
    Ok(())
}

/// Runs the `Stage 2` Host: opens state, builds transport, serves the socket.
///
/// The inference transport is credential-agnostic: every provider request
/// carries the credential the admission resolved for that use, so a consent
/// reassignment bills the new credential on the next request without any
/// transport rebinding. The environment bearer store serves the `openai`
/// provider until real OS stores arrive. Socket-path assembly stays inside
/// [`crate::conn`]: this entry point passes
/// the data directory, never the socket path.
///
/// # Errors
///
/// Returns [`CoreError::Store`] when the state cannot be opened and
/// [`CoreError::Bind`] (or [`CoreError::UnsupportedPlatform`]) when the
/// listener cannot run.
pub async fn serve(data_dir: &Path) -> Result<(), CoreError> {
    let handle = HostHandle::open(data_dir).await?;
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
