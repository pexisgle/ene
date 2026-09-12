//! Client device identity at rest: the device file and the bootstrap secret.
//!
//! The client persists its pairing identity under the data directory as
//! [`DEVICE_FILE_NAME`]. The Host issues the two halves at different times —
//! the device key arrives in the pairing answer, the secret is shown once on
//! the Host-local console by `approve-device` — so the file is written after
//! pairing whenever this process holds a secret and read back on the next
//! connect.
//!
//! The approve-time secret has exactly one inlet here: the
//! [`BOOTSTRAP_SECRET_ENV`] (`ENE_PAIRING_SECRET`) process environment
//! variable; see [`resolve_device_secret`] for the rotation decision.
//! Accepting it from the process environment is a deliberate tradeoff:
//! environment is visible to the same user (for example via `/proc`), so a
//! co-user secret there is weaker than the file; that is accepted as a
//! one-shot bootstrap because the pairing threat model is same-machine
//! single-user console (the Host enforces same-user peers at the socket), and
//! a keyring integration is deferred follow-up. The secret is never logged,
//! never rendered in `Debug`, and never sent over the wire — only ownership
//! proofs derived from it leave the device.
//!
//! [`read_bootstrap_secret`] is the single environment reader and has no unit
//! test: the workspace `unsafe` ban keeps even `std::env::set_var` out of
//! reach, and the pure [`resolve_device_secret`] carries the rotation matrix
//! instead.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use ene_api::v1::refs::DeviceWireId;
use serde::{Deserialize, Serialize};

use crate::errors::CliError;

pub const DEVICE_FILE_NAME: &str = "client-device.json";

/// Per-process staging counter: a temp name must never collide with another
/// write in this process, so concurrent stores each stage their own file.
static STAGE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Process environment variable carrying the one-shot approve-time secret,
/// read only by [`read_bootstrap_secret`]. A set, non-blank value rotates
/// (see [`resolve_device_secret`]).
pub const BOOTSTRAP_SECRET_ENV: &str = "ENE_PAIRING_SECRET";

/// The Host-issued device key plus the approve-time pairing secret, serialized
/// as `{device_id: <uuid>, pairing_secret: <hex>}`.
///
/// The secret is deliberately a plain string in memory (session-lifetime
/// only, see `SessionState` in [`crate::client`]): it must be usable for
/// proof derivation on demand, and the file permission (`0600` on Unix) is
/// the at-rest protection. `Debug` is custom and redacts the secret (the
/// device key stays visible for operator correlation): never log or format
/// this value beyond its `Debug`, and never render that `Debug` where the
/// redaction marker itself would mislead.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredDevice {
    pub device_id: DeviceWireId,
    pub pairing_secret: String,
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
            pairing_secret,
        }
    }

    /// A blank secret is treated as absent: it can prove nothing, so returning
    /// it would only postpone the provisioning guidance to proof time.
    #[must_use]
    pub fn secret(&self) -> Option<&str> {
        if self.pairing_secret.is_empty() {
            None
        } else {
            Some(self.pairing_secret.as_str())
        }
    }
}

#[must_use]
pub fn device_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DEVICE_FILE_NAME)
}

/// State of the client device file on disk.
///
/// [`Missing`](DeviceFileState::Missing) is a first run; the other failure
/// kinds are degraded state that must not silently take the first-run path.
/// Recovery may still overwrite the file after ownership proof, so callers
/// keep the distinction only for guidance, never for trust.
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

impl DeviceFileState {
    /// The stored device, when the file held a usable one.
    #[must_use]
    pub fn stored(&self) -> Option<&StoredDevice> {
        match self {
            Self::Loaded(stored) => Some(stored),
            Self::Missing | Self::Unreadable | Self::Malformed => None,
        }
    }
}

/// Reads the device file, distinguishing a genuinely missing file from
/// unreadable or malformed state. Failure messages carry no path or content.
#[must_use]
pub fn load_stored_device(data_dir: &Path) -> DeviceFileState {
    let bytes = match std::fs::read(device_file_path(data_dir)) {
        Ok(bytes) => bytes,
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

/// Whether `connect` must persist the effective secret after the ownership
/// proof was accepted.
///
/// A `Rotated` secret is new and must replace the file once proven; a
/// `Stored` secret only needs a write when the paired device identity
/// changed (the Host forgot the device and issued a fresh key), so a normal
/// reconnect never rewrites the file. `Missing` has nothing to persist.
#[must_use]
pub(crate) fn must_persist_after_acceptance(
    source: SecretSource,
    stored_device: Option<DeviceWireId>,
    paired_device: DeviceWireId,
) -> bool {
    match source {
        SecretSource::Rotated => true,
        SecretSource::Stored => stored_device != Some(paired_device),
        SecretSource::Missing => false,
    }
}

/// Atomically replaces the device file: the new document is staged to an
/// owner-only temp in the same directory, synced, and renamed over the
/// target, so a crash or write failure leaves either the old or the new
/// document whole, never a torn or empty file. Failure messages carry the
/// operation and I/O kind only, never the secret, the device key, or the
/// path.
///
/// Callers persist only after the Host accepted the ownership proof, so a
/// failed bootstrap attempt never replaces a working file.
pub fn store_device(data_dir: &Path, device: &StoredDevice) -> Result<(), CliError> {
    let path = device_file_path(data_dir);
    let bytes = serde_json::to_vec(device)
        .map_err(|error| CliError::Transport(format!("client device encode failed: {error}")))?;
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
fn stage_and_replace(staged: &Path, target: &Path, bytes: &[u8]) -> Result<(), CliError> {
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

fn store_error(error: &std::io::Error) -> CliError {
    CliError::Transport(format!("client device store failed: {}", error.kind()))
}

/// How [`resolve_device_secret`] derived the effective pairing secret; the
/// tri-state exists so callers know whether the file must be overwritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretSource {
    /// The stored file secret is effective (no environment bootstrap set).
    Stored,
    /// The environment bootstrap is effective (it differs from or the file
    /// lacks a secret); the caller must persist it over the file.
    Rotated,
    /// Neither side holds a secret; provisioning guidance applies.
    Missing,
}

/// A set, non-blank environment value rotates: it wins over a differing or
/// absent file secret (`Rotated`, and the caller overwrites the file); an
/// environment value equal to the file secret is not a rotation (`Stored`);
/// with no environment value the file secret wins (`Stored`); with neither
/// side holding a secret there is nothing to use (`Missing`).
///
/// Blank inputs count as absent on both sides (a blank key proves nothing).
/// The caller reads the real environment through [`read_bootstrap_secret`]
/// and passes the value in, keeping this decision pure and testable without
/// environment mutation.
#[must_use]
pub fn resolve_device_secret(
    file_secret: Option<String>,
    env_secret: Option<String>,
) -> (Option<String>, SecretSource) {
    let file = file_secret.filter(|secret| !secret.is_empty());
    let env = env_secret.filter(|secret| !secret.is_empty());
    match (file, env) {
        (Some(file_value), Some(env_value)) => {
            if env_value == file_value {
                (Some(file_value), SecretSource::Stored)
            } else {
                (Some(env_value), SecretSource::Rotated)
            }
        }
        (Some(file_value), None) => (Some(file_value), SecretSource::Stored),
        (None, Some(env_value)) => (Some(env_value), SecretSource::Rotated),
        (None, None) => (None, SecretSource::Missing),
    }
}

/// The single environment reader in the crate; callers pass the result into
/// [`resolve_device_secret`]. Blank values count as absent, and the value is
/// never logged.
#[must_use]
pub fn read_bootstrap_secret() -> Option<String> {
    std::env::var(BOOTSTRAP_SECRET_ENV)
        .ok()
        .filter(|secret| !secret.is_empty())
}

#[cfg(test)]
mod tests {
    use ene_api::v1::refs::DeviceWireId;

    use super::{
        DeviceFileState, SecretSource, StoredDevice, device_file_path, load_stored_device,
        must_persist_after_acceptance, resolve_device_secret, store_device,
    };

    /// Unique per process and test, so parallel tests never share a device
    /// file.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ene-ctl-device-{}-{}-{name}",
            std::process::id(),
            crate::client::platform_display(),
        ));
        let created = std::fs::create_dir_all(&dir);
        assert!(created.is_ok(), "scratch dir must create: {created:?}");
        dir
    }

    fn remove_dir(dir: &std::path::Path) {
        if std::fs::remove_dir_all(dir).is_err() {
            // Scratch cleanup is best effort; a leftover temp dir never
            // affects the test verdict.
        }
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
    fn missing_file_loads_as_missing() {
        let dir = scratch_dir("missing");
        assert!(
            !device_file_path(&dir).exists(),
            "the scratch file must start absent"
        );
        assert!(
            load_stored_device(&dir) == DeviceFileState::Missing,
            "a missing file is the first-run state"
        );
        remove_dir(&dir);
    }

    #[test]
    fn stored_device_roundtrips_through_the_file() {
        let dir = scratch_dir("roundtrip");
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
        remove_dir(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn stored_device_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = scratch_dir("perms");
        let stored_result = store_device(&dir, &stored());
        assert!(
            stored_result.is_ok(),
            "storing must succeed: {stored_result:?}"
        );
        let metadata = std::fs::metadata(device_file_path(&dir));
        assert!(metadata.is_ok(), "the device file must exist");
        let Ok(metadata) = metadata else {
            remove_dir(&dir);
            return;
        };
        assert!(
            metadata.permissions().mode() & 0o777 == 0o600,
            "the device file must be owner-only, got {:o}",
            metadata.permissions().mode() & 0o777
        );
        remove_dir(&dir);
    }

    #[test]
    fn corrupt_file_is_malformed_not_missing() {
        let dir = scratch_dir("corrupt");
        let written = std::fs::write(device_file_path(&dir), b"{not json");
        assert!(written.is_ok(), "the corrupt fixture must write");
        assert!(
            load_stored_device(&dir) == DeviceFileState::Malformed,
            "a corrupt file is a degraded state, never a first run"
        );
        remove_dir(&dir);
    }

    #[test]
    fn blank_secret_file_is_malformed() {
        let dir = scratch_dir("blank");
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

    #[test]
    fn rotations_and_identity_changes_need_a_post_acceptance_write() {
        let paired = stored().device_id;
        let other = DeviceWireId(uuid::Uuid::from_u128(2));
        assert!(
            must_persist_after_acceptance(SecretSource::Rotated, Some(paired), paired),
            "a rotated secret must be persisted once proven"
        );
        assert!(
            must_persist_after_acceptance(SecretSource::Rotated, None, paired),
            "a first provision must be persisted once proven"
        );
        assert!(
            !must_persist_after_acceptance(SecretSource::Stored, Some(paired), paired),
            "a normal reconnect must not rewrite the file"
        );
        assert!(
            must_persist_after_acceptance(SecretSource::Stored, Some(other), paired),
            "a changed device identity must be persisted"
        );
        assert!(
            !must_persist_after_acceptance(SecretSource::Missing, None, paired),
            "nothing is persisted when no secret exists"
        );
    }

    /// A staging failure (here: the parent directory refuses creation) must
    /// leave the previous document byte-identical. Skipped when permissions
    /// are not enforced (for example a privileged runner).
    #[cfg(unix)]
    #[test]
    fn staging_failure_keeps_the_previous_document() {
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

        // A privileged environment can still write into a 0500 directory;
        // the failure mode under test cannot be reached there.
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

    /// Concurrent writers never publish a partial document: every observed
    /// file state is either absent (first creation) or a complete, loadable
    /// identity written by one of the writers.
    #[test]
    fn concurrent_stores_publish_only_whole_documents() {
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

    #[test]
    fn stored_device_secret_rejects_blanks() {
        assert!(
            stored().secret() == Some("abcdef0123456789"),
            "a real secret is usable"
        );
        let blank = StoredDevice::new(DeviceWireId(uuid::Uuid::from_u128(1)), String::new());
        assert!(
            blank.secret().is_none(),
            "a blank secret is absent: {blank:?}"
        );
    }

    #[test]
    fn stored_device_debug_redacts_the_secret() {
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
    }

    #[test]
    fn resolver_keeps_the_file_without_env() {
        assert!(
            resolve_device_secret(Some(String::from("file-secret")), None)
                == (Some(String::from("file-secret")), SecretSource::Stored),
            "the file alone wins unchanged"
        );
        assert!(
            resolve_device_secret(Some(String::from("file-secret")), Some(String::new()))
                == (Some(String::from("file-secret")), SecretSource::Stored),
            "a blank env never displaces the file"
        );
    }

    #[test]
    fn resolver_rotates_on_a_differing_env_secret() {
        assert!(
            resolve_device_secret(
                Some(String::from("file-secret")),
                Some(String::from("env-secret")),
            ) == (Some(String::from("env-secret")), SecretSource::Rotated),
            "a differing env value rotates over the file"
        );
        assert!(
            resolve_device_secret(None, Some(String::from("env-secret")))
                == (Some(String::from("env-secret")), SecretSource::Rotated),
            "the env bootstrap applies when the file lacks a secret"
        );
        assert!(
            resolve_device_secret(Some(String::new()), Some(String::from("env-secret")))
                == (Some(String::from("env-secret")), SecretSource::Rotated),
            "a blank file secret counts as lacking, so env rotates"
        );
    }

    #[test]
    fn resolver_treats_equal_secrets_as_stored() {
        assert!(
            resolve_device_secret(
                Some(String::from("same-secret")),
                Some(String::from("same-secret")),
            ) == (Some(String::from("same-secret")), SecretSource::Stored),
            "an equal env value is not a rotation"
        );
    }

    #[test]
    fn resolver_reports_missing_when_neither_side_holds_a_secret() {
        assert!(
            resolve_device_secret(None, None) == (None, SecretSource::Missing),
            "no secret anywhere means the approve-and-provision path"
        );
        assert!(
            resolve_device_secret(None, Some(String::new())) == (None, SecretSource::Missing),
            "a blank env secret counts as absent"
        );
        assert!(
            resolve_device_secret(Some(String::new()), None) == (None, SecretSource::Missing),
            "a blank file secret counts as absent"
        );
        assert!(
            resolve_device_secret(Some(String::new()), Some(String::new()))
                == (None, SecretSource::Missing),
            "blank on both sides is still missing"
        );
    }
}
