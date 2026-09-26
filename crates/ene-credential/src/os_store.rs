use crate::CredentialTechnicalError;
use crate::registry::CredentialRef;
use crate::secret::{ActiveVersions, CredentialStore, PreparedCredentialSnapshot, SecretValue};

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
    active: ActiveVersions,
}

impl core::fmt::Debug for OsCredentialStore {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("OsCredentialStore")
            .field("namespace", &self.namespace)
            .field("active", &self.active.keys())
            .finish()
    }
}

impl OsCredentialStore {
    #[must_use]
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            active: ActiveVersions::new(),
        }
    }

    pub fn put_version(
        &self,
        cred: &CredentialRef,
        version: u64,
        secret: &str,
    ) -> Result<(), CredentialTechnicalError> {
        let entry = self.entry(cred, version)?;
        match entry.get_password().map(SecretValue::new) {
            Ok(existing) => {
                drop(existing);
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
        let snapshot = self.with_version(cred, version, str::to_owned)?;
        Ok(PreparedCredentialSnapshot::new(
            cred.clone(),
            version,
            SecretValue::new(snapshot),
        ))
    }

    pub fn activate(&self, snapshot: PreparedCredentialSnapshot) {
        self.active.publish(snapshot);
    }

    pub fn deactivate(&self, cred: &CredentialRef) {
        self.active.deactivate(cred);
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
        let secret = SecretValue::new(value);
        Ok(f(secret.as_str()))
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
        self.active.with_bearer(cred, f)
    }

    fn published_version(
        &self,
        cred: &CredentialRef,
    ) -> Result<Option<u64>, CredentialTechnicalError> {
        self.active.published_version(cred)
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        self.active.contains(cred)
    }
}

#[cfg(test)]
mod tests {
    use super::{OsCredentialStore, service_name};
    use crate::registry::CredentialRef;
    use crate::secret::CredentialStore as _;

    #[test]
    fn item_names_and_activation_are_scoped() {
        let cred = CredentialRef::new("openai", "main").expect("valid ref");
        assert_eq!(service_name("ene", &cred, 1), "ene/openai/main/v1");
        assert_ne!(service_name("ene", &cred, 1), service_name("ene", &cred, 2));
        assert_ne!(
            service_name("ene", &cred, 1),
            service_name("ene-other", &cred, 1)
        );

        let store = OsCredentialStore::new("ene-test-never-written");
        let read = store.with_bearer(&cred, |bearer| bearer.to_string());
        assert!(read.is_err(), "no active version means no bearer");
    }

    #[test]
    fn the_real_os_store_round_trips_when_available() {
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
