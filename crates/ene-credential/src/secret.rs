//! Bearer secret confinement: the zeroizing `SecretValue`, the
//! [`CredentialStore`] request-builder boundary, and the in-memory and
//! environment-backed store implementations.

use std::collections::HashMap;
use std::sync::Mutex;

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::CredentialTechnicalError;
use crate::registry::CredentialRef;

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SecretValue {
    pub(crate) bytes: Vec<u8>,
}

impl SecretValue {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Bearer store with a request-builder access pattern.
///
/// Implementations hold `SecretValue` internally and expose the bearer only
/// as a `&str` borrowed into the caller's closure `f`. The caller must build
/// an owned request (headers, body) inside the closure and perform I/O after
/// it returns: the borrow cannot escape, so there is deliberately no getter
/// returning an owned secret. Existence checks via [`CredentialStore::contains`]
/// are non-secret and safe to branch on.
pub trait CredentialStore: Send + Sync {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError>;

    fn contains(&self, cred: &CredentialRef) -> bool;

    fn put(&self, cred: &CredentialRef, secret: &str) -> Result<(), CredentialTechnicalError>;
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
        self.secret.bytes() == expected.as_bytes()
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

impl VersionedCredentialStore for crate::OsCredentialStore {
    fn put_version(
        &self,
        cred: &CredentialRef,
        version: u64,
        secret: &str,
    ) -> Result<(), CredentialTechnicalError> {
        crate::OsCredentialStore::put_version(self, cred, version, secret)
    }

    fn with_version<R>(
        &self,
        cred: &CredentialRef,
        version: u64,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        crate::OsCredentialStore::with_version(self, cred, version, f)
    }

    fn prepare_snapshot(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<PreparedCredentialSnapshot, CredentialTechnicalError> {
        crate::OsCredentialStore::prepare_snapshot(self, cred, version)
    }

    fn activate(&self, snapshot: PreparedCredentialSnapshot) {
        crate::OsCredentialStore::activate(self, snapshot);
    }

    fn deactivate(&self, cred: &CredentialRef) {
        crate::OsCredentialStore::deactivate(self, cred);
    }

    fn delete_version(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<(), CredentialTechnicalError> {
        crate::OsCredentialStore::delete_version(self, cred, version)
    }
}

pub struct MemoryVersionedStore {
    versions: Mutex<HashMap<(CredentialRef, u64), SecretValue>>,
    active: Mutex<HashMap<CredentialRef, (u64, SecretValue)>>,
}

impl core::fmt::Debug for MemoryVersionedStore {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let versions = match self.versions.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        formatter
            .debug_struct("MemoryVersionedStore")
            .field("versions", &versions.len())
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
            active: Mutex::new(HashMap::new()),
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
        let mut versions = match self.versions.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let key = (cred.clone(), version);
        if versions.contains_key(&key) {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: version {version} is already published", cred.id()),
            });
        }
        versions.insert(key, SecretValue::new(secret.as_bytes().to_vec()));
        Ok(())
    }

    fn with_version<R>(
        &self,
        cred: &CredentialRef,
        version: u64,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        let versions = match self.versions.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(secret) = versions.get(&(cred.clone(), version)) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: version {version} is absent", cred.id()),
            });
        };
        let Ok(bearer) = core::str::from_utf8(secret.bytes()) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: the stored value is not valid UTF-8", cred.id()),
            });
        };
        Ok(f(bearer))
    }

    fn prepare_snapshot(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<PreparedCredentialSnapshot, CredentialTechnicalError> {
        let versions = match self.versions.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(secret) = versions.get(&(cred.clone(), version)) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("{}: version {version} is absent", cred.id()),
            });
        };
        Ok(PreparedCredentialSnapshot::new(
            cred.clone(),
            version,
            SecretValue::new(secret.bytes().to_vec()),
        ))
    }

    fn activate(&self, snapshot: PreparedCredentialSnapshot) {
        let PreparedCredentialSnapshot {
            credential,
            version,
            secret,
        } = snapshot;
        let mut active = match self.active.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        active.insert(credential, (version, secret));
    }

    fn deactivate(&self, cred: &CredentialRef) {
        let mut active = match self.active.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        active.remove(cred);
    }

    fn delete_version(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<(), CredentialTechnicalError> {
        let mut versions = match self.versions.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        versions.remove(&(cred.clone(), version));
        Ok(())
    }
}

impl CredentialStore for MemoryVersionedStore {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        let active = match self.active.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
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
        self.with_bearer(cred, |_| ()).is_ok()
    }

    fn put(&self, cred: &CredentialRef, _secret: &str) -> Result<(), CredentialTechnicalError> {
        Err(CredentialTechnicalError::StorageUnavailable {
            reason: format!(
                "{}: versions are published through the credential owner, not a bare put",
                cred.id()
            ),
        })
    }
}

/// In-memory bearer store for tests and local development only.
///
/// Holds `SecretValue` entries keyed by `(provider, label)` behind a
/// mutex. Not a production backend: contents live in process memory and
/// vanish on restart.
///
/// [`core::fmt::Debug`] lists only the public refs and the entry count, never
/// secret material.
pub struct MemoryCredentialStore {
    entries: Mutex<HashMap<CredentialRef, SecretValue>>,
}

impl core::fmt::Debug for MemoryCredentialStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
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
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn insert(&self, cred: CredentialRef, secret: &str) {
        let mut entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.insert(cred, SecretValue::new(secret.as_bytes().to_vec()));
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        let entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(secret) = entries.get(cred) else {
            let id = cred.id();
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("unknown credential {id}"),
            });
        };
        let Ok(bearer) = core::str::from_utf8(secret.bytes()) else {
            let id = cred.id();
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("stored secret for {id} is not valid UTF-8"),
            });
        };
        Ok(f(bearer))
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        let entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.contains_key(cred)
    }

    fn put(&self, cred: &CredentialRef, secret: &str) -> Result<(), CredentialTechnicalError> {
        self.insert(cred.clone(), secret);
        Ok(())
    }
}

pub const ENV_API_KEY: &str = "ENE_OPENAI_API_KEY";

/// The single closed-world provider allow-list, consulted both when the bearer
/// is pinned at construction and when it is served.
const SUPPORTED_PROVIDER: &str = "openai";

/// Environment-backed bearer store for the `OpenAI` provider.
///
/// The bearer is read from [`ENV_API_KEY`] exactly once when the store is
/// constructed (Host startup) and held as a zeroizing `SecretValue` for the
/// rest of the run. It is deliberately not re-read per call: a running Host
/// must not silently adopt a different value than the one its current
/// credential-set revision was swept and advanced for. Rotation therefore
/// takes effect on the next Host start, where the startup sweep and revision
/// advance complete before any use. Only the `"openai"` provider is served
/// (closed world until real OS stores arrive); every other provider reports
/// absent. The value is in memory only, so backup exclusion still holds.
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
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    #[must_use]
    pub fn from_lookup(lookup: impl FnOnce(&str) -> Option<String>) -> Self {
        Self {
            bearer: resolve_for(SUPPORTED_PROVIDER, lookup)
                .map(|value| SecretValue::new(value.into_bytes())),
        }
    }

    fn pinned(&self, provider: &str) -> Option<&SecretValue> {
        if provider != SUPPORTED_PROVIDER {
            return None;
        }
        self.bearer.as_ref()
    }
}

// `SUPPORTED_PROVIDER` is the single allow-list: the construction-time gate in
// `resolve_for` and the serving-time gate in `pinned` both consult it, so the
// two can never drift. An empty value counts as absent, matching an unset
// variable; values arrive as `String`, so the bearer is already valid UTF-8.
pub(crate) fn resolve_for(
    provider: &str,
    lookup: impl FnOnce(&str) -> Option<String>,
) -> Option<String> {
    if provider != SUPPORTED_PROVIDER {
        return None;
    }
    let raw = lookup(ENV_API_KEY)?;
    if raw.is_empty() {
        return None;
    }
    Some(raw)
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
        let Ok(bearer) = core::str::from_utf8(secret.bytes()) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: "pinned env credential is not valid UTF-8".to_owned(),
            });
        };
        Ok(f(bearer))
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        self.pinned(cred.provider()).is_some()
    }

    fn put(&self, cred: &CredentialRef, _secret: &str) -> Result<(), CredentialTechnicalError> {
        let id = cred.id();
        Err(CredentialTechnicalError::StorageUnavailable {
            reason: format!("{id}: env credential store is not a product source of truth"),
        })
    }
}
