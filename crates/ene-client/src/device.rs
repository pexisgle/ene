//! Client device identity at rest.
//!
//! The Host delivers the device key and secret together on the originating
//! pairing connection. The file is written only after that connection proves
//! ownership and receives `AuthResult::Accepted`.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use ene_api::v1::handshake::PairingProvisionSecret;
use ene_api::v1::refs::DeviceWireId;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::ClientError;

pub const DEVICE_FILE_NAME: &str = "client-device.json";

static STAGE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The Host-issued device key plus the approve-time pairing secret, serialized
/// as `{device_id: <uuid>, pairing_secret: <hex>}`.
///
/// The secret uses a zeroize-on-drop wrapper in memory; the file permission
/// (`0600` on Unix) is its at-rest protection. `Debug` is custom and redacts
/// the secret while leaving the device key visible for operator correlation.
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

    /// A blank secret is treated as absent: it can prove nothing, so returning
    /// it would only postpone the provisioning guidance to proof time.
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

/// State of the client device file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceFileState {
    /// No file at all: first run, or after a full reset.
    Missing,
    /// The file exists but could not be read (I/O error other than
    /// not-found).
    Unreadable,
    /// The file exists but is not a usable device document (invalid JSON or
    /// a blank secret).
    Malformed,
    /// A usable stored identity.
    Loaded(StoredDevice),
}

/// Reads the device file, distinguishing a genuinely missing file from
/// unreadable or malformed state. Failure messages carry no path or content.
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

/// Atomically replaces the device file: the new document is staged to an
/// owner-only temp in the same directory, synced, and renamed over the
/// target, so a crash or write failure leaves either the old or the new
/// document whole, never a torn or empty file. Failure messages carry the
/// operation and I/O kind only, never the secret, the device key, or the
/// path.
///
/// Callers persist only after the Host accepted the ownership proof, so a
/// failed pairing attempt never replaces a working file.
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
        // The temp carries secret material; best-effort removal after a
        // failure, without masking the real error.
        if std::fs::remove_file(&staged).is_err() {
            // Best effort: the file lives in the owner-only data directory,
            // and the reported store failure stays authoritative.
        }
        return Err(error);
    }
    Ok(())
}

/// Unix creates the staging temp owner-only so secret bytes are never
/// briefly readable by other users; `sync_all` keeps a crash from leaving a
/// truncated temp that a later rename could publish.
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
