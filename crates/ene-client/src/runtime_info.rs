use std::path::Path;

use ene_api::runtime::{HOST_RUNTIME_FILE_NAME, HostRuntimeInfo};

use crate::error::ClientError;

#[cfg(unix)]
pub(crate) fn verify_owner_only(
    path: &Path,
    data_dir: &Path,
    what: &str,
) -> Result<(), ClientError> {
    use std::os::unix::fs::MetadataExt as _;

    let metadata = std::fs::metadata(path)
        .map_err(|error| ClientError::Transport(format!("inspect the {what}: {}", error.kind())))?;
    if metadata.mode() & 0o077 != 0 {
        return Err(ClientError::Transport(format!(
            "the {what} is readable by more than its owner; refuse to trust it"
        )));
    }
    let directory = std::fs::metadata(data_dir).map_err(|error| {
        ClientError::Transport(format!("inspect the data directory: {}", error.kind()))
    })?;
    if metadata.uid() != directory.uid() {
        return Err(ClientError::Transport(format!(
            "the {what} is not owned with the data directory; refuse to trust it"
        )));
    }
    Ok(())
}

/// Fail closed on Windows: ownership must be this user and no allow ACE may
/// grant anybody else (IPC §10.4 never trusts the default ACL).
#[cfg(windows)]
pub(crate) fn verify_owner_only(
    path: &Path,
    _data_dir: &Path,
    what: &str,
) -> Result<(), ClientError> {
    match crate::win_acl::owner_only_ok(path) {
        Ok(true) => Ok(()),
        Ok(false) => Err(ClientError::Transport(format!(
            "the {what} is not restricted to its owner; refuse to trust it"
        ))),
        Err(_) => Err(ClientError::Transport(format!(
            "the {what} protection could not be verified; refuse to trust it"
        ))),
    }
}

pub(crate) fn load_host_runtime(data_dir: &Path) -> Result<HostRuntimeInfo, ClientError> {
    let path = data_dir.join(HOST_RUNTIME_FILE_NAME);
    #[cfg(any(unix, windows))]
    verify_owner_only(&path, data_dir, "Host runtime file")?;
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

/// Replaces `path` with `bytes` behind an owner-only protection from the
/// first byte: staged file, write, sync, atomic rename (IPC §10.4).
pub(crate) fn write_protected_file(
    path: &Path,
    bytes: &[u8],
    context: &str,
) -> Result<(), ClientError> {
    #[cfg(not(windows))]
    {
        crate::device::atomic_replace(path, bytes, Some(0o600), context)
    }
    #[cfg(windows)]
    {
        windows_write_protected(path, bytes, context)
    }
}

#[cfg(windows)]
fn windows_write_protected(path: &Path, bytes: &[u8], context: &str) -> Result<(), ClientError> {
    use std::io::Write as _;

    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let name = path.file_name().map_or_else(
        || String::from("protected"),
        |n| n.to_string_lossy().into_owned(),
    );
    let staged = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let write = (|| -> std::io::Result<()> {
        let mut file = crate::win_acl::create_owner_only(&staged)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&staged, path)
    })();
    if let Err(error) = write {
        let _ = std::fs::remove_file(&staged);
        return Err(ClientError::Transport(format!(
            "{context}: {}",
            error.kind()
        )));
    }
    Ok(())
}
