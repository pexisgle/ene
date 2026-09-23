use std::io::Write as _;
use std::path::{Path, PathBuf};

use ene_api::v1::handshake::PairingProvisionSecret;
use ene_api::v1::refs::DeviceWireId;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::ClientError;

pub const DEVICE_FILE_NAME: &str = "client-device.json";

static STAGE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredDevice {
    pub device_id: DeviceWireId,
    pub pairing_secret: PairingProvisionSecret,
}

impl core::fmt::Debug for StoredDevice {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("StoredDevice")
            .field("device_id", &self.device_id)
            .field("pairing_secret", &"[redacted]")
            .finish()
    }
}

impl StoredDevice {
    #[must_use]
    pub fn new(device_id: DeviceWireId, pairing_secret: String) -> Self {
        Self {
            device_id,
            pairing_secret: PairingProvisionSecret::new(pairing_secret),
        }
    }

    #[must_use]
    pub fn secret(&self) -> Option<&str> {
        if self.pairing_secret.expose_secret().is_empty() {
            None
        } else {
            Some(self.pairing_secret.expose_secret())
        }
    }
}

#[must_use]
pub fn device_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DEVICE_FILE_NAME)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceFileState {
    Missing,
    Unreadable,
    Malformed,
    Loaded(StoredDevice),
}

#[must_use]
pub fn load_stored_device(data_dir: &Path) -> DeviceFileState {
    let bytes = match std::fs::read(device_file_path(data_dir)) {
        Ok(bytes) => Zeroizing::new(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return DeviceFileState::Missing;
        }
        Err(_) => return DeviceFileState::Unreadable,
    };
    let Ok(stored) = serde_json::from_slice::<StoredDevice>(&bytes) else {
        return DeviceFileState::Malformed;
    };
    if stored.secret().is_none() {
        return DeviceFileState::Malformed;
    }
    DeviceFileState::Loaded(stored)
}

pub fn store_device(data_dir: &Path, device: &StoredDevice) -> Result<(), ClientError> {
    let path = device_file_path(data_dir);
    let bytes = Zeroizing::new(serde_json::to_vec(device).map_err(|error| {
        ClientError::Transport(format!("client device encode failed: {error}"))
    })?);
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        Some(_) | None => PathBuf::from("."),
    };
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from(DEVICE_FILE_NAME));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let seq = STAGE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let staged = parent.join(format!(
        ".{file_name}.{}.{nanos}.{seq}.tmp",
        std::process::id()
    ));
    if let Err(error) = stage_and_replace(&staged, &path, &bytes) {
        if std::fs::remove_file(&staged).is_err() {
            // Best effort: the file lives in the owner-only data directory,
            // and the reported store failure stays authoritative.
        }
        return Err(error);
    }
    Ok(())
}

fn stage_and_replace(staged: &Path, target: &Path, bytes: &[u8]) -> Result<(), ClientError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(staged).map_err(|error| store_error(&error))?;
    file.write_all(bytes).map_err(|error| store_error(&error))?;
    file.sync_all().map_err(|error| store_error(&error))?;
    drop(file);
    std::fs::rename(staged, target).map_err(|error| store_error(&error))
}

fn store_error(error: &std::io::Error) -> ClientError {
    ClientError::Transport(format!("client device store failed: {}", error.kind()))
}
