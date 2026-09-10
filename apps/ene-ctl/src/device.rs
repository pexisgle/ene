//! Client device identity at rest: the device file and the bootstrap secret.
//!
//! The client persists its pairing identity under the data directory as
//! [`DEVICE_FILE_NAME`] (`<data_dir>/client-device.json`):
//! `{device_id: <uuid>, pairing_secret: <hex>}`. The Host issues both halves
//! at different times — the device key arrives in the pairing answer, the
//! secret is shown once on the Host-local console by `approve-device` — so
//! the file is written after pairing whenever this process holds a secret,
//! and read back on the next connect.
//!
//! The approve-time secret reaches this process through exactly one inlet:
//! the [`BOOTSTRAP_SECRET_ENV`] (`ENE_PAIRING_SECRET`) process environment
//! variable. Resolution is explicit (see [`resolve_device_secret`]): a set,
//! non-blank environment value rotates — it wins over a differing or absent
//! file secret and overwrites the `0600` file at the first opportunity
//! (one-shot rotation for re-provisioning); with no environment value the
//! stored file secret wins unchanged; with neither side holding a secret the
//! caller takes the approve-and-provision path. The tradeoff is documented,
//! not hidden: process
//! environment is visible to the same user (for example via `/proc`), so a
//! co-user secret there is weaker than the file. This is accepted as a
//! one-shot bootstrap because the pairing threat model is same-machine
//! single-user console (the Host enforces same-user peers at the socket),
//! and a keyring integration is deferred follow-up. The secret is never
//! logged, never rendered in `Debug`, and never sent over the wire — only
//! ownership proofs derived from it leave the device.
//!
//! Tests never touch the process environment (workspace `unsafe` ban keeps
//! even `std::env::set_var` out of reach): [`read_bootstrap_secret`] is the
//! single environment reader and has no unit test; the pure
//! [`resolve_device_secret`] carries the rotation matrix instead.

use std::path::{Path, PathBuf};

use ene_api::v1::refs::DeviceWireId;
use serde::{Deserialize, Serialize};

use crate::errors::CliError;

/// File name of the client device identity inside the data directory.
pub const DEVICE_FILE_NAME: &str = "client-device.json";

/// Process environment variable carrying the one-shot approve-time secret.
///
/// Read only by [`read_bootstrap_secret`]. A set, non-blank value rotates:
/// it wins over a differing or absent file secret and overwrites the file
/// (see [`resolve_device_secret`]); with no value set the file wins.
pub const BOOTSTRAP_SECRET_ENV: &str = "ENE_PAIRING_SECRET";

/// Stored client device identity: the Host-issued device key plus the
/// approve-time pairing secret, serialized as
/// `{device_id: <uuid>, pairing_secret: <hex>}`.
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
    /// Device key the Host issued for this device's descriptor.
    pub device_id: DeviceWireId,
    /// Approve-time pairing secret (hex), proof key material.
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
    /// Bundles an issued device key with its pairing secret.
    #[must_use]
    pub fn new(device_id: DeviceWireId, pairing_secret: String) -> Self {
        Self {
            device_id,
            pairing_secret,
        }
    }

    /// Returns the secret when it can prove anything: a blank secret is
    /// treated as absent (a blank key proves nothing, so loading it would
    /// only postpone the provisioning guidance to proof time).
    #[must_use]
    pub fn secret(&self) -> Option<&str> {
        if self.pairing_secret.is_empty() {
            None
        } else {
            Some(self.pairing_secret.as_str())
        }
    }
}

/// Resolves the device file path for `data_dir`: `<data_dir>/client-device.json`.
///
/// Pure and side-effect free; absence surfaces as [`None`] from
/// [`load_stored_device`], never as an error.
#[must_use]
pub fn device_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DEVICE_FILE_NAME)
}

/// Loads the stored device identity, if any.
///
/// Missing files, unreadable files, corrupt JSON, and blank secrets all
/// yield [`None`]: every one of those means the same operational thing (take
/// the fresh-pairing path and, when a bootstrap secret is available, store
/// it). No error is ever raised here, so callers cannot confuse a first run
/// with a failure.
#[must_use]
pub fn load_stored_device(data_dir: &Path) -> Option<StoredDevice> {
    let bytes = std::fs::read(device_file_path(data_dir)).ok()?;
    let stored: StoredDevice = serde_json::from_slice(&bytes).ok()?;
    stored.secret()?;
    Some(stored)
}

/// Persists the device identity to the `0600` device file.
///
/// Unix enforces owner-only permissions after the write (creation mode plus
/// an explicit permission set, so a pre-existing lax file is tightened, not
/// kept); other platforms use plain creation. The failure message carries
/// the operation only — never the secret, the device key, or the path.
///
/// # Errors
///
/// Returns [`CliError::Transport`] when the file cannot be encoded or
/// written.
pub fn store_device(data_dir: &Path, device: &StoredDevice) -> Result<(), CliError> {
    let path = device_file_path(data_dir);
    let bytes = serde_json::to_vec(device)
        .map_err(|error| CliError::Transport(format!("client device encode failed: {error}")))?;
    std::fs::write(&path, &bytes).map_err(|error| {
        CliError::Transport(format!("client device store failed: {}", error.kind()))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(
            |error| CliError::Transport(format!("client device store failed: {}", error.kind())),
        )?;
    }
    Ok(())
}

/// How [`resolve_device_secret`] derived the effective pairing secret.
///
/// The tri-state makes rotation explicit: `Stored` means the file won and
/// the file needs no secret overwrite (a fresh device key may still be
/// persisted alongside it); `Rotated` means the environment bootstrap won
/// and the caller must overwrite the file (one-shot rotation); `Missing`
/// means neither side holds a secret and the caller takes the
/// approve-and-provision path.
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

/// Resolves the effective pairing secret with an explicit rotation source.
///
/// A set, non-blank environment value rotates: it wins over a differing or
/// absent file secret (`Rotated`, and the caller overwrites the file). An
/// environment value equal to the file secret is not a rotation (`Stored`).
/// With no environment value the file secret wins (`Stored`). With neither
/// side holding a secret there is nothing to use (`Missing`).
///
/// Blank inputs count as absent on both sides (a blank key proves nothing,
/// so preferring it would only mask the side that actually holds a secret).
/// The caller reads the real environment once through
/// [`read_bootstrap_secret`] and passes the value in, which keeps this
/// decision pure and testable without environment mutation.
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

/// Reads the one-shot bootstrap secret from the process environment.
///
/// This is the single environment reader in the crate: callers pass its
/// result into [`resolve_device_secret`], which rotates over a differing or
/// absent file secret and otherwise keeps the file.
/// Blank values count as absent. The value is never logged; see the
/// module docs for the documented `/proc`-visibility tradeoff.
#[must_use]
pub fn read_bootstrap_secret() -> Option<String> {
    std::env::var(BOOTSTRAP_SECRET_ENV)
        .ok()
        .filter(|secret| !secret.is_empty())
}

#[cfg(test)]
mod tests {
    //! Device-file roundtrips in a scratch directory plus the pure resolver
    //! matrix. No sockets, no environment mutation, no network: the only
    //! environment read in the crate ([`read_bootstrap_secret`]) has no test
    //! by design (see the module docs).

    use ene_api::v1::refs::DeviceWireId;

    use super::{
        SecretSource, StoredDevice, device_file_path, load_stored_device, resolve_device_secret,
        store_device,
    };

    /// Creates a scratch directory unique to this process and test, so
    /// parallel tests never share a device file.
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

    /// Removes a scratch directory; failures are ignored (best effort).
    fn remove_dir(dir: &std::path::Path) {
        if std::fs::remove_dir_all(dir).is_err() {
            // Scratch cleanup is best effort; a leftover temp dir never
            // affects the test verdict.
        }
    }

    /// Builds a stored device with a fixed key and secret.
    fn stored() -> StoredDevice {
        StoredDevice::new(
            DeviceWireId(uuid::Uuid::from_u128(
                0x1234_5678_9abc_def0_1234_5678_9abc_def0,
            )),
            String::from("abcdef0123456789"),
        )
    }

    #[test]
    fn missing_file_loads_as_absent() {
        let dir = scratch_dir("missing");
        assert!(
            !device_file_path(&dir).exists(),
            "the scratch file must start absent"
        );
        assert!(
            load_stored_device(&dir).is_none(),
            "a missing file means the fresh-pairing path"
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
            loaded == Some(stored()),
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
    fn corrupt_file_loads_as_absent() {
        let dir = scratch_dir("corrupt");
        let written = std::fs::write(device_file_path(&dir), b"{not json");
        assert!(written.is_ok(), "the corrupt fixture must write");
        assert!(
            load_stored_device(&dir).is_none(),
            "a corrupt file means the fresh-pairing path, never an error"
        );
        remove_dir(&dir);
    }

    #[test]
    fn blank_secret_loads_as_absent() {
        let dir = scratch_dir("blank");
        let blank = StoredDevice::new(DeviceWireId(uuid::Uuid::from_u128(1)), String::new());
        let stored_result = store_device(&dir, &blank);
        assert!(
            stored_result.is_ok(),
            "storing must succeed: {stored_result:?}"
        );
        assert!(
            load_stored_device(&dir).is_none(),
            "a blank secret proves nothing, so it loads as absent"
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
