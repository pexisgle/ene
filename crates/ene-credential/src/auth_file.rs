use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

use ene_primitive::{RawId, WallClockWithTz};
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::CredentialTechnicalError;
use crate::pairing::{DeviceId, decode_hex_lower, encode_hex_lower, verify_pairing_proof};
use crate::secret::SecretValue;

#[derive(Clone)]
pub struct FileDeviceAuthStore {
    path: PathBuf,
}

impl core::fmt::Debug for FileDeviceAuthStore {
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

    pub fn save_secret(
        &self,
        device: &DeviceId,
        descriptor: &str,
        secret: &str,
    ) -> Result<(), CredentialTechnicalError> {
        self.with_mutation_lock(|| {
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
        })
    }

    fn lock_path(&self) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(std::ffi::OsString::from)
            .unwrap_or_else(|| std::ffi::OsString::from("device-auth.json"));
        name.push(".lock");
        self.path.with_file_name(name)
    }

    pub(crate) fn with_mutation_lock<R>(
        &self,
        mutate: impl FnOnce() -> Result<R, CredentialTechnicalError>,
    ) -> Result<R, CredentialTechnicalError> {
        let shown = self.path.display();
        let lock_path = self.lock_path();
        let lock_error = |err: &std::io::Error| CredentialTechnicalError::StorageUnavailable {
            reason: format!("device-auth lock for {shown} is unavailable: {err}"),
        };
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|err| lock_error(&err))?;
        lock.lock().map_err(|err| lock_error(&err))?;
        let result = mutate();
        drop(lock);
        result
    }

    pub(crate) fn load_secret(
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
        let text =
            String::from_utf8(bytes).map_err(|_| CredentialTechnicalError::StorageUnavailable {
                reason: format!("device-auth entry for {key} holds malformed secret material"),
            })?;
        Ok(Some(SecretValue::new(text)))
    }

    pub fn has_secret(&self, device: &DeviceId) -> Result<bool, CredentialTechnicalError> {
        let entries = self.read_entries()?;
        Ok(entries.contains_key(&device_key(device)))
    }

    pub fn verify_device_proof(
        &self,
        device: &DeviceId,
        nonce: &str,
        proof_hex: &str,
    ) -> Result<bool, CredentialTechnicalError> {
        let Some(secret) = self.load_secret(device)? else {
            return Ok(false);
        };
        Ok(verify_pairing_proof(secret.as_str(), nonce, proof_hex))
    }

    pub fn erase_target_text(&self, target: &str) -> Result<u64, CredentialTechnicalError> {
        if target.is_empty() {
            return Ok(0);
        }
        self.with_mutation_lock(|| {
            let entries = self.read_entries()?;
            let mut retained = BTreeMap::new();
            let mut removed = 0_u64;
            for (key, entry) in entries {
                if entry_matches_target(&key, &entry, target)? {
                    removed = removed.saturating_add(1);
                } else {
                    retained.insert(key, entry);
                }
            }
            if removed > 0 {
                self.write_entries(&retained)?;
            }
            Ok(removed)
        })
    }

    pub fn count_target_text(&self, target: &str) -> Result<u64, CredentialTechnicalError> {
        if target.is_empty() {
            return Ok(0);
        }
        let entries = self.read_entries()?;
        entries.iter().try_fold(0_u64, |count, (key, entry)| {
            if entry_matches_target(key, entry, target)? {
                Ok(count.saturating_add(1))
            } else {
                Ok(count)
            }
        })
    }

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

    fn write_entries(
        &self,
        entries: &BTreeMap<String, StoredDeviceAuth>,
    ) -> Result<(), CredentialTechnicalError> {
        let shown = self.path.display();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("device-auth.json");
        let tmp = self
            .path
            .with_file_name(format!("{file_name}.tmp.{pid}.{nanos}"));
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

fn entry_matches_target(
    key: &str,
    entry: &StoredDeviceAuth,
    target: &str,
) -> Result<bool, CredentialTechnicalError> {
    if key.contains(target) || entry.descriptor.contains(target) || entry.secret_hex == target {
        return Ok(true);
    }
    let bytes = decode_hex_lower(&entry.secret_hex).ok_or_else(|| {
        CredentialTechnicalError::StorageUnavailable {
            reason: String::from("device-auth entry holds malformed secret material"),
        }
    })?;
    let secret = Zeroizing::new(bytes);
    Ok(secret.as_slice() == target.as_bytes())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredDeviceAuth {
    secret_hex: String,
    descriptor: String,
    paired_at: String,
}

fn device_key(device: &DeviceId) -> String {
    device.0.as_uuid().to_string()
}

pub(crate) fn parse_device_key(text: &str) -> Option<DeviceId> {
    text.parse().ok().map(RawId::from_uuid).map(DeviceId)
}

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

#[cfg(not(unix))]
fn enforce_owner_only(_path: &Path) -> Result<(), CredentialTechnicalError> {
    Ok(())
}

fn stage_file(tmp: &Path, rendered: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut staged = options.open(tmp)?;
    std::io::Write::write_all(&mut staged, rendered)?;
    staged.sync_all()
}

fn remove_best_effort(tmp: &Path) {
    if std::fs::remove_file(tmp).is_err() {}
}

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

fn parse_device_auth_file(bytes: &[u8]) -> Result<BTreeMap<String, StoredDeviceAuth>, String> {
    serde_json::from_slice::<DeviceAuthFile>(bytes)
        .map(|file| file.devices)
        .map_err(|_| "file is not a valid device-auth document".to_owned())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceAuthFile {
    #[serde(deserialize_with = "devices_without_duplicates")]
    devices: BTreeMap<String, StoredDeviceAuth>,
}

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
