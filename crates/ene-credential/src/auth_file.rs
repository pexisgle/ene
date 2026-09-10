//! File-backed custody for device-auth verification material: one protected
//! JSON file keeps a secret entry per paired device across process boundaries
//! and Host restarts.

use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use ene_primitive::{RawId, WallClockWithTz};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::CredentialTechnicalError;
use crate::pairing::{DeviceId, decode_hex_lower, encode_hex_lower, verify_pairing_proof};
use crate::secret::SecretValue;

/// File-backed custody for device-auth verification material.
///
/// Pairing secrets minted at approval must survive both process boundaries
/// (a separate `approve-device` process persists them while the serving
/// process verifies proofs) and Host restarts, so verification material
/// cannot live only in the serving process's memory. This store keeps one
/// entry per paired device in a protected file shared across processes and
/// restarts. Reads are read-through on every call: nothing is cached, so a
/// verifier always observes the latest persisted rotation or revocation.
/// Authentication stays per-connection-once, so the extra file read costs
/// correctness nothing it cannot afford.
///
/// The whole file is one JSON document mapping canonical device UUID text to
/// an entry holding the secret (lowercase hex), the owner-visible
/// descriptor, and the entry write time as RFC 3339:
///
/// ```json
/// {"devices": {"123e4567-e89b-12d3-a456-426614174000": {"secret_hex": "00ab",
/// "descriptor": "phone", "paired_at": "2026-09-08T12:00:00+09:00"}}}
/// ```
///
/// Secret custody: generation stays with the caller (the pairing repository
/// approve path mints the secret); this store only persists and returns
/// custody. [`load_secret`](FileDeviceAuthStore::load_secret) hands back an
/// owned [`SecretValue`], which is zeroized on drop and has no `Debug`
/// rendering. Secrets and descriptors are never logged and never appear in
/// this type's `Debug` output, which shows the path and the entry count
/// only.
///
/// File protection: on Unix the file at rest must be mode `0600`. Opening an
/// existing file with any other mode attempts to tighten it to `0600` and
/// fails when tightening does not stick; newly written files (including the
/// staging temp) are created `0600`. On non-Unix platforms there is no mode
/// check: the OS-specific protection story is documented at the call site
/// instead, and the file must still live in a directory only the owner can
/// read.
///
/// Caller-owned directory: the caller creates the parent directory. Opening
/// fails when the parent directory is missing, so a misconfigured data
/// directory can never silently redirect the store. A missing file is not an
/// error: opening succeeds empty and the file is created lazily on the first
/// save. A malformed file is always an error, never a silent default.
///
/// Atomicity story: every mutation rewrites the whole file by staging the
/// new bytes to a temp file in the same directory (created `0600` on Unix,
/// flushed with `sync_all`) and renaming it over the target. The rename is
/// the atomic replace: concurrent readers observe the old or the new
/// document whole, so torn reads are impossible. Read-modify-write cycles
/// still race across processes: concurrent approves of different devices are
/// last-writer-wins and can drop an entry, and concurrent approves of one
/// device are a rotation race. Approval is therefore an owner-serialized
/// operation; this store provides durability, not mutual exclusion.
///
/// Backup-exclusion contract: this file holds Group K verification material
/// with E classification. It must never enter backups or exports and must
/// never live inside `app.db`: a future backup stage walks the data
/// directory and must exclude it by name. The file name convention is
/// `device-auth.json` directly under the caller's data directory; restore
/// must not replace it, reset wipes it only on full-data reset, and a Host
/// without this file authenticates nothing until fresh pairing mints new
/// material.
pub struct FileDeviceAuthStore {
    /// Location of the protected JSON file; the parent directory is owned by
    /// the caller.
    path: PathBuf,
}

impl core::fmt::Debug for FileDeviceAuthStore {
    /// Renders the path and the entry count only.
    ///
    /// The read is best-effort: when the file cannot be read or parsed, the
    /// count renders as `"unreadable"` instead of failing. Secrets and
    /// descriptors never appear here.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.read_entries() {
            Ok(entries) => f
                .debug_struct("FileDeviceAuthStore")
                .field("path", &self.path)
                .field("entries", &entries.len())
                .finish(),
            Err(_) => f
                .debug_struct("FileDeviceAuthStore")
                .field("path", &self.path)
                .field("entries", &"unreadable")
                .finish(),
        }
    }
}

impl FileDeviceAuthStore {
    /// Opens the protected device-auth file at `path`.
    ///
    /// The caller owns directory creation: opening fails when the parent
    /// directory is missing. A missing file opens as an empty store and is
    /// created lazily on the first save. An existing file keeps its bytes
    /// untouched, but on Unix its mode is verified (and tightened to `0600`
    /// when lax; see the type-level contract). Paths naming no file, and
    /// paths naming a directory, are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the path
    /// names no file, the parent directory is missing, the path is a
    /// directory, file metadata cannot be read, or Unix permissions cannot
    /// be tightened to owner-only. Error reasons carry the path only, never
    /// file content.
    pub fn open(path: &Path) -> Result<Self, CredentialTechnicalError> {
        let shown = path.display();
        if path.file_name().is_none() {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth path {shown} names no file"),
            });
        }
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            Some(_) | None => Path::new("."),
        };
        if !parent.is_dir() {
            let parent_shown = parent.display();
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth directory {parent_shown} is missing"),
            });
        }
        if path.exists() {
            if path.is_dir() {
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("device-auth path {shown} is a directory"),
                });
            }
            enforce_owner_only(path)?;
        }
        Ok(Self {
            path: path.to_owned(),
        })
    }

    /// Persists `secret` for `device`, creating or rotating its entry.
    ///
    /// The caller mints the secret; this method performs no strength
    /// validation on it, it only takes custody. The entry's `descriptor` is
    /// the owner-visible display string and `paired_at` is stamped with the
    /// write time for display and audit only (it is not the pairing record's
    /// pairing time). The write goes through the atomic temp-plus-rename
    /// path; concurrent approves must be owner-serialized by the caller.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the
    /// file cannot be read (including a malformed existing file), the
    /// staging temp cannot be written, or the atomic replace fails.
    pub fn save_secret(
        &self,
        device: &DeviceId,
        descriptor: &str,
        secret: &str,
    ) -> Result<(), CredentialTechnicalError> {
        let mut entries = self.read_entries()?;
        entries.insert(
            device_key(device),
            StoredDeviceAuth {
                secret_hex: encode_hex_lower(secret.as_bytes()),
                descriptor: descriptor.to_owned(),
                paired_at: WallClockWithTz::now().to_rfc3339(),
            },
        );
        self.write_entries(&entries)
    }

    /// Loads the persisted secret for `device`, if any.
    ///
    /// Every call reads the file through: there is no cache, so a rotation
    /// or revocation persisted by another process is observed immediately.
    /// An unknown device (or a missing file) yields `Ok(None)`; only an
    /// unreadable or malformed file yields an error, never a silent default.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the
    /// file cannot be read or fails validation.
    pub fn load_secret(
        &self,
        device: &DeviceId,
    ) -> Result<Option<SecretValue>, CredentialTechnicalError> {
        let entries = self.read_entries()?;
        let key = device_key(device);
        let Some(entry) = entries.get(&key) else {
            return Ok(None);
        };
        let Some(bytes) = decode_hex_lower(&entry.secret_hex) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth entry for {key} holds malformed secret material"),
            });
        };
        Ok(Some(SecretValue::new(bytes)))
    }

    /// Verifies one pairing ownership proof against the persisted secret.
    ///
    /// Loads through [`load_secret`](FileDeviceAuthStore::load_secret) on
    /// every call: there is no cache, so verification always observes the
    /// latest persisted rotation or revocation. Authentication stays
    /// per-connection-once, so the extra file read costs correctness nothing
    /// it cannot afford. The secret bytes never leave this crate: they are
    /// borrowed into the constant-time comparison inside
    /// [`verify_pairing_proof`] and zeroized on drop with the [`SecretValue`].
    /// An unknown device yields `Ok(false)`; a stored secret that is not
    /// valid UTF-8 (never minted by the approve path, which stores UUID text)
    /// likewise yields `Ok(false)`. Both are fail-closed without
    /// distinguishing the reason to the caller.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the
    /// file cannot be read or fails validation.
    pub fn verify_device_proof(
        &self,
        device: &DeviceId,
        nonce: &str,
        proof_hex: &str,
    ) -> Result<bool, CredentialTechnicalError> {
        let Some(secret) = self.load_secret(device)? else {
            return Ok(false);
        };
        let Ok(text) = core::str::from_utf8(secret.bytes()) else {
            return Ok(false);
        };
        Ok(verify_pairing_proof(text, nonce, proof_hex))
    }

    /// Revokes `device` by deleting its entry from the protected file.
    ///
    /// This is the durable half of device revocation: once the atomic
    /// rewrite completes, no process loading through this store will verify
    /// proofs for the device again. Deleting an unknown device — or deleting
    /// while the file is missing — succeeds without writing anything.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the
    /// file cannot be read (including a malformed existing file) or the
    /// atomic rewrite fails.
    pub fn delete_for(&self, device: &DeviceId) -> Result<(), CredentialTechnicalError> {
        let mut entries = self.read_entries()?;
        if entries.remove(&device_key(device)).is_none() {
            return Ok(());
        }
        self.write_entries(&entries)
    }

    // Reads and validates the whole file; a missing file reads as empty.
    fn read_entries(&self) -> Result<BTreeMap<String, StoredDeviceAuth>, CredentialTechnicalError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(err) => {
                let shown = self.path.display();
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("device-auth file {shown} is unreadable: {err}"),
                });
            }
        };
        let parsed = parse_device_auth_file(&bytes).map_err(|detail| {
            let shown = self.path.display();
            CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth file {shown} is malformed: {detail}"),
            }
        })?;
        let mut entries = BTreeMap::new();
        for (device_text, entry) in parsed {
            let Some(device) = parse_device_key(&device_text) else {
                let shown = self.path.display();
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("device-auth file {shown} holds an invalid device key"),
                });
            };
            if decode_hex_lower(&entry.secret_hex).is_none() {
                let shown = self.path.display();
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("device-auth file {shown} holds malformed secret material"),
                });
            }
            if WallClockWithTz::parse_rfc3339(&entry.paired_at).is_err() {
                let shown = self.path.display();
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("device-auth file {shown} holds an invalid timestamp"),
                });
            }
            if entries.insert(device_key(&device), entry).is_some() {
                let shown = self.path.display();
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("device-auth file {shown} holds a duplicate device entry"),
                });
            }
        }
        Ok(entries)
    }

    // Renders the entries and replaces the file via temp-plus-rename in the
    // same directory, so readers never observe a partial document.
    fn write_entries(
        &self,
        entries: &BTreeMap<String, StoredDeviceAuth>,
    ) -> Result<(), CredentialTechnicalError> {
        let shown = self.path.display();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let tmp = self.path.with_extension(format!("tmp.{pid}.{nanos}"));
        let rendered = render_device_auth_file(entries)?;
        if let Err(err) = stage_file(&tmp, rendered.as_bytes()) {
            remove_best_effort(&tmp);
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth write to {shown} failed: {err}"),
            });
        }
        if let Err(err) = std::fs::rename(&tmp, &self.path) {
            remove_best_effort(&tmp);
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth write to {shown} failed: {err}"),
            });
        }
        enforce_owner_only(&self.path)
    }
}

// One file entry: secret bytes as lowercase hex plus display and timing
// metadata. Keys live in the surrounding map, canonicalized to hyphenated
// UUID text. Unknown fields are rejected at decode (`deny_unknown_fields`)
// so a hand-edited file with stray keys fails closed instead of silently
// dropping them; duplicate and missing fields are rejected by the derived
// `Deserialize` impl itself.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDeviceAuth {
    secret_hex: String,
    descriptor: String,
    paired_at: String,
}

// Renders `device` as canonical hyphenated UUID text for use as a file key.
fn device_key(device: &DeviceId) -> String {
    device.0.as_uuid().to_string()
}

// Parses a file key back into a `DeviceId`; yields `None` for anything that
// is not UUID text. The backing UUID type is inferred through
// `RawId::from_uuid` and never named, so this stays on the existing
// dependency set.
pub(crate) fn parse_device_key(text: &str) -> Option<DeviceId> {
    text.parse().ok().map(RawId::from_uuid).map(DeviceId)
}

// Enforces owner-only mode on an existing file (Unix): already-`0600` passes
// through, anything else is tightened, and a tighten that does not stick is
// an error. Non-Unix has no mode to enforce; the call still succeeds so the
// documented directory-level protection applies instead.
#[cfg(unix)]
fn enforce_owner_only(path: &Path) -> Result<(), CredentialTechnicalError> {
    let shown = path.display();
    let current =
        std::fs::metadata(path).map_err(|err| CredentialTechnicalError::StorageUnavailable {
            reason: format!("device-auth file {shown} is unreadable: {err}"),
        })?;
    if current.permissions().mode() & 0o777 == 0o600 {
        return Ok(());
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|err| {
        CredentialTechnicalError::StorageUnavailable {
            reason: format!("device-auth file {shown} cannot be tightened to owner-only: {err}"),
        }
    })?;
    let tightened =
        std::fs::metadata(path).map_err(|err| CredentialTechnicalError::StorageUnavailable {
            reason: format!("device-auth file {shown} is unreadable: {err}"),
        })?;
    if tightened.permissions().mode() & 0o777 != 0o600 {
        return Err(CredentialTechnicalError::StorageUnavailable {
            reason: format!("device-auth file {shown} cannot be tightened to owner-only"),
        });
    }
    Ok(())
}

// Non-Unix platforms have no Unix mode bits; protection rests on the
// caller-owned directory, as documented on the store.
#[cfg(not(unix))]
fn enforce_owner_only(_path: &Path) -> Result<(), CredentialTechnicalError> {
    Ok(())
}

// Stages the new document to a temp file in the same directory. On Unix the
// temp is created `0600` so secrets are never briefly world-readable;
// `sync_all` keeps a crash from leaving a truncated temp behind.
fn stage_file(tmp: &Path, rendered: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut staged = options.open(tmp)?;
    std::io::Write::write_all(&mut staged, rendered)?;
    staged.sync_all()
}

// Best-effort temp cleanup after a failed write; leftovers stay in the same
// directory for the owner to notice and remove.
fn remove_best_effort(tmp: &Path) {
    if std::fs::remove_file(tmp).is_err() {
        // Nothing to do: the temp carries no trust beyond the store file
        // itself, and reporting cleanup failure would mask the real error.
    }
}

// Renders the whole document in one canonical shape: entries ordered by
// device key (`BTreeMap` iteration), no whitespace, trailing newline.
// Struct field order fixes the entry key order, so the rendering the doc
// comment on [`FileDeviceAuthStore`] shows is exact. `serde_json` is the one
// JSON implementation for both directions; a render failure is reported,
// never defaulted.
fn render_device_auth_file(
    entries: &BTreeMap<String, StoredDeviceAuth>,
) -> Result<String, CredentialTechnicalError> {
    #[derive(Serialize)]
    struct Document<'a> {
        devices: &'a BTreeMap<String, StoredDeviceAuth>,
    }
    let mut rendered = serde_json::to_string(&Document { devices: entries }).map_err(|_| {
        CredentialTechnicalError::StorageUnavailable {
            reason: String::from("device-auth entries cannot be rendered"),
        }
    })?;
    rendered.push('\n');
    Ok(rendered)
}

// Parses the whole document with `serde_json`; every failure maps to one
// fixed content-free detail. `serde_json` error displays can echo the
// offending input (unexpected values, unknown field names), and this file
// holds secrets and descriptors — so nothing from the decoder output ever
// reaches an error string. Semantic checks (UUID keys, hex secrets,
// RFC 3339 timestamps) happen in `read_entries`.
fn parse_device_auth_file(bytes: &[u8]) -> Result<BTreeMap<String, StoredDeviceAuth>, String> {
    serde_json::from_slice::<DeviceAuthFile>(bytes)
        .map(|file| file.devices)
        .map_err(|_| "file is not a valid device-auth document".to_owned())
}

// Whole-file document: exactly one `devices` section mapping canonical
// device UUID text to entries. Unknown top-level fields are rejected, so a
// hand-edited file with stray keys fails closed instead of silently
// dropping them.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceAuthFile {
    /// Device entries keyed by canonical UUID text; duplicates rejected.
    #[serde(deserialize_with = "devices_without_duplicates")]
    devices: BTreeMap<String, StoredDeviceAuth>,
}

// Rejects duplicate device entries at decode: deserializing straight into
// a map would let a later entry silently overwrite an earlier one, but the
// custody contract fails closed on hand-edited files instead.
fn devices_without_duplicates<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, StoredDeviceAuth>, D::Error>
where
    D: Deserializer<'de>,
{
    struct WithoutDuplicates;
    impl<'de> Visitor<'de> for WithoutDuplicates {
        type Value = BTreeMap<String, StoredDeviceAuth>;
        fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            formatter.write_str("a devices object without duplicate entries")
        }
        fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut devices = BTreeMap::new();
            while let Some((key, entry)) = access.next_entry::<String, StoredDeviceAuth>()? {
                if devices.insert(key, entry).is_some() {
                    return Err(serde::de::Error::custom("duplicate device entry"));
                }
            }
            Ok(devices)
        }
    }
    deserializer.deserialize_map(WithoutDuplicates)
}
