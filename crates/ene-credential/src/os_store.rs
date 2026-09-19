//! Real OS protected store adapter (Stage 7 A1c).
//!
//! The port is [`CredentialStore`](crate::CredentialStore); this adapter is the
//! provisional backend named in [First-party desktop] §7.4: Windows Credential
//! Manager and the Linux Secret Service, through the `keyring` crate. The
//! adapter is deliberately thin: it owns one OS item per
//! `(installation namespace, provider, label, version)` and never reuses a
//! published item's slot for a new value, because a published item is the
//! durable copy of an active or retired version and overwriting it would make
//! a rotation indistinguishable from a re-write of the same version.
//!
//! [First-party desktop]: ../../../../docs/design/concrete/first-party-desktop.md

use crate::CredentialTechnicalError;
use crate::registry::CredentialRef;
use crate::secret::{CredentialStore, SecretValue};

/// Installation namespace: one prefix for every item this installation owns,
/// so two data directories or two installations never collide and an unrelated
/// application's item is never read.
pub const DEFAULT_NAMESPACE: &str = "ene";

/// The OS service name of one credential version item.
///
/// The version is part of the item name, never a value inside it: publishing a
/// new version writes a new item and leaves the retired one addressable for the
/// secret-removal leases that still need it.
#[must_use]
pub fn service_name(namespace: &str, cred: &CredentialRef, version: u64) -> String {
    format!(
        "{namespace}/{provider}/{label}/v{version}",
        provider = cred.provider(),
        label = cred.label()
    )
}

/// The OS protected store of one installation.
///
/// Secrets are read through [`CredentialStore::with_bearer`] and written
/// through [`OsCredentialStore::put_version`]. The adapter keeps no cache: the
/// credential owner's immutable snapshot is the only in-memory copy, and a
/// stale OS read never silently becomes a newer value.
pub struct OsCredentialStore {
    namespace: String,
    /// Version the adapter reads for each ref. The credential owner moves this
    /// pointer only after the activation transaction commits.
    active: std::sync::Mutex<std::collections::HashMap<CredentialRef, u64>>,
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
    /// Opens the adapter for one installation namespace.
    #[must_use]
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            active: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Publishes one value as a new version item and returns the version id.
    ///
    /// Writing an item that already exists is refused: a version is immutable
    /// once published, so a repeated write cannot make two different values
    /// look like the same version.
    ///
    /// # Errors
    ///
    /// [`CredentialTechnicalError::StorageUnavailable`] when the OS store
    /// refuses the write or the item already exists. The message never carries
    /// the value.
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

    /// Points this adapter at the version the credential owner published.
    ///
    /// Called only after the activation transaction commits, so a read before
    /// the commit still resolves the previous version.
    pub fn activate(&self, cred: &CredentialRef, version: u64) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        active.insert(cred.clone(), version);
    }

    /// Removes one version item. Used by cleanup for retired versions.
    ///
    /// # Errors
    ///
    /// [`CredentialTechnicalError::StorageUnavailable`] when the OS store
    /// refuses the delete. A missing item is success: the goal is absence.
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

    /// Reads one version's value into the request-builder closure.
    ///
    /// # Errors
    ///
    /// [`CredentialTechnicalError::StorageUnavailable`] when the version is
    /// not published, the OS store refuses the read, or the value is not valid
    /// UTF-8. The error never carries the value.
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
        let version = {
            let active = self
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            active.get(cred).copied()
        };
        let Some(version) = version else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: no published version is active", cred.id()),
            });
        };
        self.with_version(cred, version, f)
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        let version = {
            let active = self
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            active.get(cred).copied()
        };
        let Some(version) = version else {
            return false;
        };
        match self.entry(cred, version) {
            Ok(entry) => entry.get_password().is_ok(),
            Err(_) => false,
        }
    }

    /// Serving-time intake through the OS store alone is refused: the product
    /// path publishes a version and activates it in one owner boundary
    /// ([Credential publication] §3). A bare `put` would write a value nothing
    /// has activated, so it fails closed instead of looking like registration.
    ///
    /// [Credential publication]: ../../../../docs/design/concrete/credential-publication.md
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

    /// The item name carries the version, so publishing a new version never
    /// reuses a retired item's slot.
    #[test]
    fn the_item_name_carries_the_version() {
        let cred = CredentialRef::new("openai", "main").expect("valid ref");
        assert_eq!(service_name("ene", &cred, 1), "ene/openai/main/v1");
        assert_ne!(service_name("ene", &cred, 1), service_name("ene", &cred, 2));
    }

    /// Two installations never share an item. Or an unrelated application's.
    #[test]
    fn the_namespace_separates_installations() {
        let cred = CredentialRef::new("openai", "main").expect("valid ref");
        assert_ne!(
            service_name("ene", &cred, 1),
            service_name("ene-other", &cred, 1)
        );
    }

    /// A bare `put` never looks like a registration: only the owner's
    /// publish-then-activate path makes a value usable.
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

    /// Reading without an active version is unavailable, never a guess at
    /// "the newest item".
    #[test]
    fn a_read_without_an_active_version_is_unavailable() {
        use super::OsCredentialStore;
        use crate::secret::CredentialStore as _;

        let store = OsCredentialStore::new("ene-test-never-written");
        let cred = CredentialRef::new("openai", "main").expect("valid ref");
        let read = store.with_bearer(&cred, |bearer| bearer.to_string());
        assert!(read.is_err(), "no active version means no bearer");
    }

    /// The real backend probe: publishes one version, reads it back, proves a
    /// second version is a separate item, and removes both.
    ///
    /// The probe records 未実施 instead of failing when the platform store is
    /// unavailable (a Linux session with no Secret Service, a locked keyring):
    /// an unrun probe is not a pass, and it is not a product defect either.
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
        store.activate(&cred, 2);
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
