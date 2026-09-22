use crate::CredentialTechnicalError;
use crate::registry::CredentialRef;
use crate::secret::{CredentialStore, PreparedCredentialSnapshot, SecretValue};

/// Prefix of the installation namespace for the OS protected store.
///
/// The composition root appends a per-data-directory identity to this prefix,
/// so two data directories never share an OS item even under one OS user whose
/// OS keyring is shared, and an unrelated application's item is never read.
pub const DEFAULT_NAMESPACE: &str = "ene";

#[must_use]
pub fn service_name(namespace: &str, cred: &CredentialRef, version: u64) -> String {
    format!(
        "{namespace}/{provider}/{label}/v{version}",
        provider = cred.provider(),
        label = cred.label()
    )
}

pub struct OsCredentialStore {
    namespace: String,
    active: std::sync::Mutex<std::collections::HashMap<CredentialRef, (u64, SecretValue)>>,
}

impl core::fmt::Debug for OsCredentialStore {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        formatter
            .debug_struct("OsCredentialStore")
            .field("namespace", &self.namespace)
            .field("active", &active.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl OsCredentialStore {
    #[must_use]
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            active: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn put_version(
        &self,
        cred: &CredentialRef,
        version: u64,
        secret: &str,
    ) -> Result<(), CredentialTechnicalError> {
        let entry = self.entry(cred, version)?;
        match entry.get_password() {
            Ok(_) => {
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("{}: version {version} is already published", cred.id()),
                });
            }
            Err(keyring::Error::NoEntry) => {}
            Err(error) => {
                return Err(CredentialTechnicalError::StorageUnavailable {
                    reason: format!("{}: {}", cred.id(), error),
                });
            }
        }
        entry
            .set_password(secret)
            .map_err(|error| CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: {}", cred.id(), error),
            })
    }

    pub fn prepare_snapshot(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<PreparedCredentialSnapshot, CredentialTechnicalError> {
        let snapshot = self.with_version(cred, version, |bearer| bearer.as_bytes().to_vec())?;
        Ok(PreparedCredentialSnapshot::new(
            cred.clone(),
            version,
            SecretValue::new(snapshot),
        ))
    }

    pub fn activate(&self, snapshot: PreparedCredentialSnapshot) {
        let PreparedCredentialSnapshot {
            credential,
            version,
            secret,
        } = snapshot;
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        active.insert(credential, (version, secret));
    }

    pub fn deactivate(&self, cred: &CredentialRef) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        active.remove(cred);
    }

    pub fn delete_version(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<(), CredentialTechnicalError> {
        let entry = self.entry(cred, version)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: {}", cred.id(), error),
            }),
        }
    }

    pub fn with_version<R>(
        &self,
        cred: &CredentialRef,
        version: u64,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        let entry = self.entry(cred, version)?;
        let value =
            entry
                .get_password()
                .map_err(|error| CredentialTechnicalError::StorageUnavailable {
                    reason: format!("{}: {}", cred.id(), error),
                })?;
        let secret = SecretValue::new(value.into_bytes());
        let Ok(bearer) = core::str::from_utf8(secret.bytes()) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: the stored value is not valid UTF-8", cred.id()),
            });
        };
        Ok(f(bearer))
    }

    fn entry(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<keyring::Entry, CredentialTechnicalError> {
        keyring::Entry::new(
            &service_name(&self.namespace, cred, version),
            cred.provider(),
        )
        .map_err(|error| CredentialTechnicalError::StorageUnavailable {
            reason: format!("{}: {}", cred.id(), error),
        })
    }
}

impl CredentialStore for OsCredentialStore {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        let active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((_, secret)) = active.get(cred) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: no published version is active", cred.id()),
            });
        };
        let bearer = core::str::from_utf8(secret.bytes()).map_err(|_| {
            CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: the active value is not valid UTF-8", cred.id()),
            }
        })?;
        Ok(f(bearer))
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(cred)
    }

    fn put(&self, cred: &CredentialRef, _secret: &str) -> Result<(), CredentialTechnicalError> {
        Err(CredentialTechnicalError::StorageUnavailable {
            reason: format!(
                "{}: the OS store publishes versions through the credential owner, not a bare put",
                cred.id()
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::service_name;
    use crate::registry::CredentialRef;

    #[test]
    fn the_item_name_carries_the_version() {
        let cred = CredentialRef::new("openai", "main").expect("valid ref");
        assert_eq!(service_name("ene", &cred, 1), "ene/openai/main/v1");
        assert_ne!(service_name("ene", &cred, 1), service_name("ene", &cred, 2));
    }

    #[test]
    fn the_namespace_separates_installations() {
        let cred = CredentialRef::new("openai", "main").expect("valid ref");
        assert_ne!(
            service_name("ene", &cred, 1),
            service_name("ene-other", &cred, 1)
        );
    }

    #[test]
    fn a_bare_put_is_refused_without_touching_the_store() {
        use super::OsCredentialStore;
        use crate::secret::CredentialStore as _;

        let store = OsCredentialStore::new("ene-test-never-written");
        let cred = CredentialRef::new("openai", "main").expect("valid ref");
        let refused = store.put(&cred, "sk-never-stored");
        assert!(refused.is_err(), "a bare put must fail closed");
        assert!(!store.contains(&cred), "nothing may be published by a put");
        let rendered = format!("{refused:?}");
        assert!(
            !rendered.contains("sk-never-stored"),
            "the refusal must not echo the value: {rendered}"
        );
    }

    #[test]
    fn a_read_without_an_active_version_is_unavailable() {
        use super::OsCredentialStore;
        use crate::secret::CredentialStore as _;

        let store = OsCredentialStore::new("ene-test-never-written");
        let cred = CredentialRef::new("openai", "main").expect("valid ref");
        let read = store.with_bearer(&cred, |bearer| bearer.to_string());
        assert!(read.is_err(), "no active version means no bearer");
    }

    #[test]
    fn the_real_os_store_round_trips_when_available() {
        use super::OsCredentialStore;
        use crate::secret::CredentialStore as _;

        let namespace = format!("ene-probe-{}", ene_primitive::RawId::new().as_uuid());
        let store = OsCredentialStore::new(namespace);
        let cred = CredentialRef::new("probe", "round-trip").expect("valid ref");
        if store.put_version(&cred, 1, "probe-value-one").is_err() {
            eprintln!("os-store probe: 未実施 (the platform store is unavailable here)");
            return;
        }
        eprintln!("os-store probe: the platform store accepted a version");
        let read = store.with_version(&cred, 1, |bearer| bearer.to_string());
        assert_eq!(
            read.expect("a published version must read back"),
            "probe-value-one"
        );
        store
            .put_version(&cred, 2, "probe-value-two")
            .expect("a second version is its own item");
        assert_eq!(
            store
                .with_version(&cred, 2, |bearer| bearer.to_string())
                .expect("the second version must read back"),
            "probe-value-two"
        );
        let snapshot = store
            .prepare_snapshot(&cred, 2)
            .expect("prepare second version");
        store.activate(snapshot);
        assert_eq!(
            store
                .with_bearer(&cred, |bearer| bearer.to_string())
                .expect("activation must point reads at the new version"),
            "probe-value-two"
        );
        assert!(
            store.put_version(&cred, 2, "probe-value-two").is_err(),
            "a published version is immutable"
        );
        store
            .delete_version(&cred, 1)
            .expect("a retired version must be removable");
        store
            .delete_version(&cred, 2)
            .expect("the active version must be removable by cleanup");
        assert!(
            store.with_version(&cred, 2, |_| ()).is_err(),
            "the removed version must be gone"
        );
    }
}
