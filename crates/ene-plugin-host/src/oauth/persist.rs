//! Credential persistence: a replaceable trait with a 0600 file backend.
//!
//! The file backend writes `Map<storage_key, CredentialData>` as JSON to
//! `<app_data_dir>/credentials.json` — deliberately separate from
//! `settings.json`, which never carries stored secrets. The trait exists so
//! an OS keychain can replace the plaintext file without touching the vault,
//! the flow manager, or the refresher.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ene_connector::CredentialData;
use thiserror::Error;

/// Errors produced by a [`CredentialPersister`] backend.
#[derive(Debug, Error)]
pub enum PersistError {
    /// The underlying file (or keychain) operation failed.
    #[error("credential store I/O error: {0}")]
    Io(#[source] std::io::Error),
    /// The stored data could not be serialized.
    #[error("credential store serialization error: {0}")]
    Serialize(String),
}

/// Storage backend for credential entries keyed by vault storage key.
///
/// All methods are synchronous: the payload is at most a few kilobytes of
/// JSON and the calls are infrequent (flow completion, token rotation,
/// revocation), so blocking a task briefly is cheaper than plumbing async
/// through every call site.
pub trait CredentialPersister: Send + Sync {
    /// Loads every entry. Never fails: a missing file is an empty store, an
    /// unreadable or corrupt file is backed up and treated as empty, and an
    /// entry that fails to parse is skipped — one bad entry must not brick
    /// the vault (or the whole host).
    fn load(&self) -> HashMap<String, CredentialData>;
    /// Atomically replaces the whole store with `entries`.
    fn save(&self, entries: &HashMap<String, CredentialData>) -> Result<(), PersistError>;
    /// Removes `ids` from the store, returning how many entries were removed
    /// (no-op when an id is absent).
    fn remove(&self, ids: &[String]) -> Result<usize, PersistError>;
}

/// Plaintext JSON credential store with owner-only permissions on Unix.
pub struct FileCredentialPersister {
    path: PathBuf,
}

impl FileCredentialPersister {
    /// Creates a persister writing to `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The target file path (for diagnostics).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl CredentialPersister for FileCredentialPersister {
    fn load(&self) -> HashMap<String, CredentialData> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
            Err(e) => {
                tracing::warn!(
                    component = "CredentialStore",
                    path = %self.path.display(),
                    error = %e,
                    "Reading the credential store failed; starting from an empty store"
                );
                return HashMap::new();
            }
        };
        let entries: serde_json::Map<String, serde_json::Value> = match serde_json::from_slice(
            &bytes,
        ) {
            Ok(entries) => entries,
            Err(e) => {
                // An interrupted write (crash between truncate and rename) is
                // the likely cause; keep the evidence under a backup name and
                // start fresh so the vault can still serve.
                let backup = self.path.with_extension("json.bak");
                if let Err(bak_error) = std::fs::rename(&self.path, &backup) {
                    tracing::warn!(
                        component = "CredentialStore",
                        path = %self.path.display(),
                        backup = %backup.display(),
                        error = %bak_error,
                        "Could not back up the corrupt credential store"
                    );
                }
                tracing::warn!(
                    component = "CredentialStore",
                    path = %self.path.display(),
                    backup = %backup.display(),
                    error = %e,
                    "The credential store was corrupt; it was moved aside and the store starts empty"
                );
                return HashMap::new();
            }
        };
        let mut loaded = HashMap::new();
        for (key, value) in entries {
            match serde_json::from_value::<CredentialData>(value) {
                Ok(data) => {
                    loaded.insert(key, data);
                }
                Err(e) => {
                    // A forward-format entry (e.g. from a newer host) must
                    // not take down the whole store.
                    tracing::warn!(
                        component = "CredentialStore",
                        key = %key,
                        error = %e,
                        "Skipping an unreadable credential store entry"
                    );
                }
            }
        }
        loaded
    }

    fn save(&self, entries: &HashMap<String, CredentialData>) -> Result<(), PersistError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(PersistError::Io)?;
        }
        let json = serde_json::to_vec_pretty(entries).map_err(|e| {
            PersistError::Serialize(format!("serializing the credential store: {e}"))
        })?;
        let tmp = self.path.with_extension("json.tmp");
        {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                // 0600 from creation so the tmp file never exists with looser
                // perms; set_permissions below additionally forces it when
                // the tmp path was left behind by an interrupted save.
                options.mode(0o600);
            }
            let mut file = options.open(&tmp).map_err(PersistError::Io)?;
            use std::io::Write;
            file.write_all(&json).map_err(PersistError::Io)?;
            file.sync_all().map_err(PersistError::Io)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(PersistError::Io)?;
        }
        // Rename is atomic on the same filesystem: readers either see the
        // old or the new file, never a torn write.
        #[cfg(not(windows))]
        std::fs::rename(&tmp, &self.path).map_err(PersistError::Io)?;
        #[cfg(windows)]
        {
            // `rename` fails when the destination exists on Windows; the
            // replace is best-effort non-atomic there.
            drop(std::fs::remove_file(&self.path));
            std::fs::rename(&tmp, &self.path).map_err(PersistError::Io)?;
        }
        Ok(())
    }

    fn remove(&self, ids: &[String]) -> Result<usize, PersistError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let mut entries = self.load();
        let mut removed = 0;
        for id in ids {
            if entries.remove(id).is_some() {
                removed += 1;
            }
        }
        if removed > 0 {
            self.save(&entries)?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "tests use unwrap for concise failure messages"
)]
mod tests {
    use super::*;
    use ene_connector::CredentialStore;

    fn oauth_data() -> CredentialData {
        CredentialStore::oauth2("access", Some("refresh"), None).expose_for_persistence()
    }

    #[test]
    fn save_load_roundtrip_preserves_entries() {
        let dir = tempfile::tempdir().unwrap();
        let persister = FileCredentialPersister::new(dir.path().join("credentials.json"));
        let mut entries = HashMap::new();
        entries.insert("google.calendar".to_string(), oauth_data());
        entries.insert("anthropic".to_string(), {
            CredentialStore::from_api_key("sk-test").expose_for_persistence()
        });
        persister.save(&entries).unwrap();
        let mut loaded = persister.load();
        assert_eq!(loaded.len(), 2);
        let google = CredentialStore::from_exported(loaded.remove("google.calendar").unwrap());
        let anthropic = CredentialStore::from_exported(loaded.remove("anthropic").unwrap());
        assert_eq!(google.access_token(), Some("access"));
        assert_eq!(anthropic.api_key(), Some("sk-test"));
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let persister = FileCredentialPersister::new(dir.path().join("absent.json"));
        assert!(persister.load().is_empty());
    }

    #[test]
    fn save_is_atomic_and_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let persister = FileCredentialPersister::new(path.clone());
        let mut entries = HashMap::new();
        entries.insert("a".to_string(), oauth_data());
        persister.save(&entries).unwrap();
        assert!(!path.with_extension("json.tmp").exists());
        assert!(path.exists());
    }

    #[test]
    fn remove_drops_only_requested_ids() {
        let dir = tempfile::tempdir().unwrap();
        let persister = FileCredentialPersister::new(dir.path().join("credentials.json"));
        let mut entries = HashMap::new();
        entries.insert("a".to_string(), oauth_data());
        entries.insert("b".to_string(), oauth_data());
        persister.save(&entries).unwrap();
        persister.remove(&["a".to_string()]).unwrap();
        let loaded = persister.load();
        assert!(!loaded.contains_key("a"));
        assert!(loaded.contains_key("b"));
        // Removing an absent id is a silent no-op.
        persister.remove(&["nope".to_string()]).unwrap();
        assert_eq!(persister.load().len(), 1);
    }

    #[test]
    fn corrupt_file_is_backed_up_and_treated_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let persister = FileCredentialPersister::new(path.clone());
        assert!(persister.load().is_empty());
        assert!(path.with_extension("json.bak").exists());
    }

    #[test]
    fn unreadable_entry_is_skipped_while_others_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(
            &path,
            br#"{"good":{"type":"o_auth2","access_token":"a","refresh_token":null,"expires_at":null},"bad":42}"#,
        )
        .unwrap();
        let persister = FileCredentialPersister::new(path);
        let loaded = persister.load();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key("good"));
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let persister = FileCredentialPersister::new(path.clone());
        let mut entries = HashMap::new();
        entries.insert("a".to_string(), oauth_data());
        persister.save(&entries).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn save_tightens_loose_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let persister = FileCredentialPersister::new(path.clone());
        let mut entries = HashMap::new();
        entries.insert("a".to_string(), oauth_data());
        persister.save(&entries).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
