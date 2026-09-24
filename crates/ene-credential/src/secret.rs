use std::collections::HashMap;
use std::sync::Mutex;

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::CredentialTechnicalError;
use crate::registry::CredentialRef;

pub(crate) fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SecretValue {
    pub(crate) value: String,
}

impl SecretValue {
    pub(crate) fn new(value: String) -> Self {
        Self { value }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }
}

pub trait CredentialStore: Send + Sync {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError>;

    fn contains(&self, cred: &CredentialRef) -> bool;
}

pub struct PreparedCredentialSnapshot {
    pub(crate) credential: CredentialRef,
    pub(crate) version: u64,
    pub(crate) secret: SecretValue,
}

impl PreparedCredentialSnapshot {
    #[must_use]
    pub(crate) fn new(credential: CredentialRef, version: u64, secret: SecretValue) -> Self {
        Self {
            credential,
            version,
            secret,
        }
    }

    #[must_use]
    pub fn matches(&self, expected: &str) -> bool {
        self.secret.as_str() == expected
    }
}

pub(crate) struct ActiveVersions(Mutex<HashMap<CredentialRef, (u64, SecretValue)>>);

impl ActiveVersions {
    pub(crate) fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }

    pub(crate) fn publish(&self, snapshot: PreparedCredentialSnapshot) {
        let PreparedCredentialSnapshot {
            credential,
            version,
            secret,
        } = snapshot;
        lock_or_recover(&self.0).insert(credential, (version, secret));
    }

    pub(crate) fn deactivate(&self, cred: &CredentialRef) {
        lock_or_recover(&self.0).remove(cred);
    }

    pub(crate) fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        let active = lock_or_recover(&self.0);
        let Some((_, secret)) = active.get(cred) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: no published version is active", cred.id()),
            });
        };
        Ok(f(secret.as_str()))
    }

    pub(crate) fn contains(&self, cred: &CredentialRef) -> bool {
        lock_or_recover(&self.0).contains_key(cred)
    }

    pub(crate) fn keys(&self) -> Vec<CredentialRef> {
        lock_or_recover(&self.0).keys().cloned().collect()
    }
}

pub trait VersionedCredentialStore: Send + Sync {
    fn put_version(
        &self,
        cred: &CredentialRef,
        version: u64,
        secret: &str,
    ) -> Result<(), CredentialTechnicalError>;

    fn with_version<R>(
        &self,
        cred: &CredentialRef,
        version: u64,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError>;

    fn prepare_snapshot(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<PreparedCredentialSnapshot, CredentialTechnicalError>;

    fn activate(&self, snapshot: PreparedCredentialSnapshot);

    fn deactivate(&self, cred: &CredentialRef);

    fn delete_version(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<(), CredentialTechnicalError>;
}

pub struct MemoryVersionedStore {
    versions: Mutex<HashMap<(CredentialRef, u64), SecretValue>>,
    active: ActiveVersions,
}

impl core::fmt::Debug for MemoryVersionedStore {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("MemoryVersionedStore")
            .field("versions", &lock_or_recover(&self.versions).len())
            .finish()
    }
}

impl Default for MemoryVersionedStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryVersionedStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            versions: Mutex::new(HashMap::new()),
            active: ActiveVersions::new(),
        }
    }
}

impl VersionedCredentialStore for MemoryVersionedStore {
    fn put_version(
        &self,
        cred: &CredentialRef,
        version: u64,
        secret: &str,
    ) -> Result<(), CredentialTechnicalError> {
        let mut versions = lock_or_recover(&self.versions);
        let key = (cred.clone(), version);
        if versions.contains_key(&key) {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: version {version} is already published", cred.id()),
            });
        }
        versions.insert(key, SecretValue::new(secret.to_owned()));
        Ok(())
    }

    fn with_version<R>(
        &self,
        cred: &CredentialRef,
        version: u64,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        let versions = lock_or_recover(&self.versions);
        let Some(secret) = versions.get(&(cred.clone(), version)) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: version {version} is absent", cred.id()),
            });
        };
        Ok(f(secret.as_str()))
    }

    fn prepare_snapshot(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<PreparedCredentialSnapshot, CredentialTechnicalError> {
        let versions = lock_or_recover(&self.versions);
        let Some(secret) = versions.get(&(cred.clone(), version)) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: version {version} is absent", cred.id()),
            });
        };
        Ok(PreparedCredentialSnapshot::new(
            cred.clone(),
            version,
            SecretValue::new(secret.as_str().to_owned()),
        ))
    }

    fn activate(&self, snapshot: PreparedCredentialSnapshot) {
        self.active.publish(snapshot);
    }

    fn deactivate(&self, cred: &CredentialRef) {
        self.active.deactivate(cred);
    }

    fn delete_version(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<(), CredentialTechnicalError> {
        lock_or_recover(&self.versions).remove(&(cred.clone(), version));
        Ok(())
    }
}

impl CredentialStore for MemoryVersionedStore {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        self.active.with_bearer(cred, f)
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        self.active.contains(cred)
    }
}

pub struct MemoryCredentialStore {
    entries: std::sync::Arc<Mutex<HashMap<CredentialRef, SecretValue>>>,
}

/// Cloning shares the entries, so a restarted Host keeps reading the same
/// backing state that the persistent OS store provides in production.
impl Clone for MemoryCredentialStore {
    fn clone(&self) -> Self {
        Self {
            entries: std::sync::Arc::clone(&self.entries),
        }
    }
}

impl core::fmt::Debug for MemoryCredentialStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let entries = lock_or_recover(&self.entries);
        let refs: Vec<&CredentialRef> = entries.keys().collect();
        f.debug_struct("MemoryCredentialStore")
            .field("len", &refs.len())
            .field("refs", &refs)
            .finish()
    }
}

impl Default for MemoryCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryCredentialStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: std::sync::Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn insert(&self, cred: CredentialRef, secret: &str) {
        lock_or_recover(&self.entries).insert(cred, SecretValue::new(secret.to_owned()));
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        let entries = lock_or_recover(&self.entries);
        let Some(secret) = entries.get(cred) else {
            let id = cred.id();
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("unknown credential {id}"),
            });
        };
        Ok(f(secret.as_str()))
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        lock_or_recover(&self.entries).contains_key(cred)
    }
}

pub const ENV_API_KEY: &str = "ENE_OPENAI_API_KEY";

const SUPPORTED_PROVIDER: &str = "openai";

pub struct EnvCredentialStore {
    bearer: Option<SecretValue>,
}

impl core::fmt::Debug for EnvCredentialStore {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("EnvCredentialStore")
            .field("present", &self.bearer.is_some())
            .finish()
    }
}

impl Default for EnvCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvCredentialStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            bearer: std::env::var(ENV_API_KEY)
                .ok()
                .filter(|value| !value.is_empty())
                .map(SecretValue::new),
        }
    }

    fn pinned(&self, provider: &str) -> Option<&SecretValue> {
        if provider != SUPPORTED_PROVIDER {
            return None;
        }
        self.bearer.as_ref()
    }
}

impl CredentialStore for EnvCredentialStore {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        let Some(secret) = self.pinned(cred.provider()) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: "env credential missing".to_owned(),
            });
        };
        Ok(f(secret.as_str()))
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        self.pinned(cred.provider()).is_some()
    }
}
