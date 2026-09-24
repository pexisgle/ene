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
    if stored.pairing_secret.expose_secret().is_empty() {
        return DeviceFileState::Malformed;
    }
    DeviceFileState::Loaded(stored)
}

pub fn store_device(data_dir: &Path, device: &StoredDevice) -> Result<(), ClientError> {
    let bytes = Zeroizing::new(serde_json::to_vec(device).map_err(|error| {
        ClientError::Transport(format!("client device encode failed: {error}"))
    })?);
    atomic_replace(
        &device_file_path(data_dir),
        &bytes,
        Some(0o600),
        "client device store failed",
    )
}

pub(crate) fn atomic_replace(
    target: &Path,
    bytes: &[u8],
    mode: Option<u32>,
    context: &str,
) -> Result<(), ClientError> {
    let parent = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        Some(_) | None => PathBuf::from("."),
    };
    let file_name = target.file_name().map_or_else(
        || String::from("staged"),
        |name| name.to_string_lossy().into_owned(),
    );
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let seq = STAGE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let staged = parent.join(format!(
        ".{file_name}.{}.{nanos}.{seq}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            if let Some(mode) = mode {
                options.mode(mode);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = mode;
        }
        let mut file = options
            .open(&staged)
            .map_err(|error| store_error(context, &error))?;
        file.write_all(bytes)
            .map_err(|error| store_error(context, &error))?;
        file.sync_all()
            .map_err(|error| store_error(context, &error))?;
        drop(file);
        std::fs::rename(&staged, target).map_err(|error| store_error(context, &error))
    })();
    if result.is_err() {
        // The temp can carry secret material; best-effort removal without
        // masking the real error.
        if std::fs::remove_file(&staged).is_err() {
            // Best effort only.
        }
    }
    result
}

fn store_error(context: &str, error: &std::io::Error) -> ClientError {
    ClientError::Transport(format!("{context}: {}", error.kind()))
}

#[cfg(test)]
mod tests {
    use ene_api::v1::refs::DeviceWireId;

    use super::{
        DeviceFileState, StoredDevice, device_file_path, load_stored_device, store_device,
    };

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ene-ctl-device-{}-{}-{name}",
            std::process::id(),
            crate::platform_display(),
        ));
        let created = std::fs::create_dir_all(&dir);
        assert!(created.is_ok(), "scratch dir must create: {created:?}");
        dir
    }

    fn remove_dir(dir: &std::path::Path) {
        if std::fs::remove_dir_all(dir).is_err() {}
    }

    fn stored() -> StoredDevice {
        StoredDevice::new(
            DeviceWireId(uuid::Uuid::from_u128(
                0x1234_5678_9abc_def0_1234_5678_9abc_def0,
            )),
            String::from("abcdef0123456789"),
        )
    }

    #[test]
    fn device_storage_preserves_private_atomic_documents() {
        {
            let dir = scratch_dir("states");
            assert!(
                !device_file_path(&dir).exists(),
                "the scratch file must start absent"
            );
            assert!(
                load_stored_device(&dir) == DeviceFileState::Missing,
                "a missing file is the first-run state"
            );

            let written = std::fs::write(device_file_path(&dir), b"{not json");
            assert!(written.is_ok(), "the corrupt fixture must write");
            assert!(
                load_stored_device(&dir) == DeviceFileState::Malformed,
                "a corrupt file is a degraded state, never a first run"
            );

            let blank = StoredDevice::new(DeviceWireId(uuid::Uuid::from_u128(1)), String::new());
            let stored_result = store_device(&dir, &blank);
            assert!(
                stored_result.is_ok(),
                "storing must succeed: {stored_result:?}"
            );
            assert!(
                load_stored_device(&dir) == DeviceFileState::Malformed,
                "a blank secret cannot prove anything and is degraded state"
            );
            remove_dir(&dir);
        }

        {
            let dir = scratch_dir("roundtrip-private");
            let stored_result = store_device(&dir, &stored());
            assert!(
                stored_result.is_ok(),
                "storing must succeed: {stored_result:?}"
            );
            let loaded = load_stored_device(&dir);
            assert!(
                loaded == DeviceFileState::Loaded(stored()),
                "the stored identity must load back, got {loaded:?}"
            );

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;

                let metadata = std::fs::metadata(device_file_path(&dir));
                assert!(metadata.is_ok(), "the device file must exist");
                let metadata = metadata.expect("the device file must exist");
                assert!(
                    metadata.permissions().mode() & 0o777 == 0o600,
                    "the device file must be owner-only, got {:o}",
                    metadata.permissions().mode() & 0o777
                );
            }

            let rendered = format!("{:?}", stored());
            assert!(
                !rendered.contains("abcdef0123456789"),
                "stored Debug must not leak the secret: {rendered:?}"
            );
            assert!(
                rendered.contains("[redacted]"),
                "stored Debug must mark the redaction: {rendered:?}"
            );
            assert!(
                rendered.contains("StoredDevice"),
                "stored Debug must name the type: {rendered:?}"
            );
            remove_dir(&dir);
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let dir = scratch_dir("staging-failure");
            let stored_result = store_device(&dir, &stored());
            assert!(
                stored_result.is_ok(),
                "the fixture must store: {stored_result:?}"
            );
            let before = std::fs::read(device_file_path(&dir)).unwrap_or_default();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500))
                .expect("the scratch directory must be made read-only");

            let enforced = std::fs::write(dir.join("probe"), b"x").is_err();
            if enforced {
                let replacement = StoredDevice::new(
                    DeviceWireId(uuid::Uuid::from_u128(3)),
                    String::from("replacement-secret"),
                );
                let failed = store_device(&dir, &replacement);
                assert!(
                    failed.is_err(),
                    "a staging failure must be reported, got {failed:?}"
                );
            }
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .expect("the scratch directory must be restored");
            let after = std::fs::read(device_file_path(&dir)).unwrap_or_default();
            assert!(
                after == before,
                "a failed store must leave the previous document untouched"
            );
            assert!(
                load_stored_device(&dir) == DeviceFileState::Loaded(stored()),
                "the previous document must stay loadable"
            );
            remove_dir(&dir);
        }

        {
            use std::sync::Arc;
            use std::sync::atomic::{AtomicBool, Ordering};

            let dir = Arc::new(scratch_dir("concurrent"));
            let stop = Arc::new(AtomicBool::new(false));
            let mut writers = Vec::new();
            for seed in 0..3_u128 {
                let dir = Arc::clone(&dir);
                writers.push(std::thread::spawn(move || {
                    for round in 0..20_u128 {
                        let device = StoredDevice::new(
                            DeviceWireId(uuid::Uuid::from_u128(0x1000 + seed * 100 + round)),
                            format!("secret-{seed}-{round}"),
                        );
                        let written = store_device(&dir, &device);
                        assert!(written.is_ok(), "concurrent store must succeed");
                    }
                }));
            }
            let reader_dir = Arc::clone(&dir);
            let mut observed = 0_u32;
            while !stop.load(Ordering::Relaxed) {
                match load_stored_device(&reader_dir) {
                    DeviceFileState::Loaded(_) | DeviceFileState::Missing => {}
                    other => panic!("a partial document was observed: {other:?}"),
                }
                observed += 1;
                if observed > 200 {
                    break;
                }
            }
            for writer in writers {
                writer.join().expect("writer must not panic");
            }
            stop.store(true, Ordering::Relaxed);
            assert!(
                matches!(load_stored_device(&dir), DeviceFileState::Loaded(_)),
                "the final document must be loadable"
            );
            remove_dir(&dir);
        }
    }
}
