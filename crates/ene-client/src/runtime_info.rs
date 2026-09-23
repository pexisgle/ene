use std::path::Path;

use ene_api::runtime::{HOST_RUNTIME_FILE_NAME, HostRuntimeInfo};

use crate::error::ClientError;

#[cfg(unix)]
fn verify_owner_only(path: &Path, data_dir: &Path) -> Result<(), ClientError> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = std::fs::metadata(path).map_err(|error| {
        ClientError::Transport(format!("inspect the Host runtime file: {}", error.kind()))
    })?;
    if metadata.mode() & 0o077 != 0 {
        return Err(ClientError::Transport(String::from(
            "the Host runtime file is readable by more than its owner; refuse to trust it",
        )));
    }
    let directory = std::fs::metadata(data_dir).map_err(|error| {
        ClientError::Transport(format!("inspect the data directory: {}", error.kind()))
    })?;
    if metadata.uid() != directory.uid() {
        return Err(ClientError::Transport(String::from(
            "the Host runtime file is not owned with the data directory; refuse to trust it",
        )));
    }
    Ok(())
}

pub(crate) fn load_host_runtime(data_dir: &Path) -> Result<HostRuntimeInfo, ClientError> {
    let path = data_dir.join(HOST_RUNTIME_FILE_NAME);
    #[cfg(unix)]
    verify_owner_only(&path, data_dir)?;
    let bytes = std::fs::read(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ClientError::Transport(format!(
                "no Host runtime information under {}; start `ene-core serve` first",
                data_dir.display()
            ))
        } else {
            ClientError::Transport(format!("read the Host runtime file: {}", error.kind()))
        }
    })?;
    let runtime: HostRuntimeInfo = serde_json::from_slice(&bytes)
        .map_err(|_| ClientError::Transport(String::from("the Host runtime file is malformed")))?;
    if runtime.local_port().is_none() {
        return Err(ClientError::Transport(String::from(
            "the Host runtime file does not describe a local wss://127.0.0.1 listener",
        )));
    }
    if runtime.host_pin.is_empty()
        || runtime.local_token.is_empty()
        || runtime.startup_generation.is_empty()
    {
        return Err(ClientError::Transport(String::from(
            "the Host runtime file is missing its pin, token, or startup generation",
        )));
    }
    Ok(runtime)
}
