//! Bearer secret confinement: the zeroizing [`SecretValue`], the
//! [`CredentialStore`] request-builder boundary, and the in-memory and
//! environment-backed store implementations.

use std::collections::HashMap;
use std::sync::Mutex;

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::CredentialTechnicalError;
use crate::registry::CredentialRef;

/// Secret key material, confined to this crate.
///
/// The bytes are `pub(crate)` so only in-crate store implementations can
/// touch them. There is deliberately no [`core::fmt::Debug`] implementation:
/// deriving or hand-writing one would risk logging bearer material. Memory is
/// zeroized on drop via [`ZeroizeOnDrop`].
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
/// Implementations hold [`SecretValue`] internally and expose the bearer only
/// as a `&str` borrowed into the caller's closure `f`. The caller must build
/// an owned request (headers, body) inside the closure and perform I/O after
/// it returns: the borrow cannot escape, so there is deliberately no getter
/// returning an owned secret. Existence checks via [`CredentialStore::contains`]
/// are non-secret and safe to branch on.
pub trait CredentialStore: Send + Sync {
    /// Runs `f` with the bearer for `cred`.
    ///
    /// Errors when the credential is unknown, the secret is not valid UTF-8,
    /// or the backend fails; the error never carries secret material.
    fn with_bearer<R>(
        &self,
        cred: &CredentialRef,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError>;

    /// Existence is non-secret metadata.
    fn contains(&self, cred: &CredentialRef) -> bool;

    /// Serving-time intake: stores `secret` for `cred`.
    ///
    /// Product durable storage is an OS protected store; this method is the
    /// port. [`EnvCredentialStore`] is test/dev only and fail-closes.
    /// `secret` is never returned, logged, or included in the error.
    ///
    /// # Errors
    ///
    /// [`CredentialTechnicalError::StorageUnavailable`] when the backend
    /// cannot accept the value (env store, locked OS store, or similar).
    fn put(&self, cred: &CredentialRef, secret: &str) -> Result<(), CredentialTechnicalError>;
}

/// Candidate value fully loaded before the publication write guard is taken.
///
/// The raw value has no public accessor and is zeroized on drop. Moving this
/// object into [`VersionedCredentialStore::activate`] publishes the already
/// prepared value without another OS-store read.
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

    /// Whether the prepared candidate is the value the confirmed operation
    /// supplied. The value itself never leaves this owner type.
    #[must_use]
    pub fn matches(&self, expected: &str) -> bool {
        self.secret.bytes() == expected.as_bytes()
    }
}

/// Version-aware backend for the publication protocol.
///
/// The product OS store implements this alongside [`CredentialStore`]; the
/// in-memory store implements it for tests and local development. A backend
/// that cannot hold versions (the environment store) deliberately does not:
/// the Host then reports that registration is unavailable instead of writing a
/// value nothing can activate.
pub trait VersionedCredentialStore: Send + Sync {
    /// Publishes one value as a new version item.
    ///
    /// # Errors
    ///
    /// [`CredentialTechnicalError::StorageUnavailable`] when the item cannot
    /// be created. The error never carries the value.
    fn put_version(
        &self,
        cred: &CredentialRef,
        version: u64,
        secret: &str,
    ) -> Result<(), CredentialTechnicalError>;

    /// Runs `f` with one version's value.
    ///
    /// # Errors
    ///
    /// [`CredentialTechnicalError::StorageUnavailable`] when the version is
    /// absent or unreadable. The error never carries the value.
    fn with_version<R>(
        &self,
        cred: &CredentialRef,
        version: u64,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError>;

    /// Loads an immutable candidate snapshot before the publication guard.
    ///
    /// # Errors
    ///
    /// [`CredentialTechnicalError::StorageUnavailable`] when the candidate
    /// version cannot be loaded into the snapshot.
    fn prepare_snapshot(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<PreparedCredentialSnapshot, CredentialTechnicalError>;

    /// Publishes a prepared snapshot without OS I/O.
    fn activate(&self, snapshot: PreparedCredentialSnapshot);

    /// Removes an active snapshot after its durable version becomes unusable.
    fn deactivate(&self, cred: &CredentialRef);

    /// Removes one version item.
    ///
    /// # Errors
    ///
    /// [`CredentialTechnicalError::StorageUnavailable`] when the backend
    /// refuses the removal.
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

/// Versioned in-memory backend for tests and local development.
///
/// It exists so the publication protocol can be exercised end to end without
/// an OS store. It is never a product source of truth: the Host wires the OS
/// backend for the product path and reports registration as unavailable when
/// no version-capable backend is configured.
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

    /// Publishes and activates one value directly, for fixtures that need a
    /// usable credential without driving the registration protocol. Product
    /// code never calls this: the Host publishes through the owner.
    pub fn provision(&self, cred: CredentialRef, secret: &str) {
        let version = 1;
        let mut versions = match self.versions.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        versions.insert(
            (cred.clone(), version),
            SecretValue::new(secret.as_bytes().to_vec()),
        );
        drop(versions);
        let mut active = match self.active.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        active.insert(
            cred,
            (version, SecretValue::new(secret.as_bytes().to_vec())),
        );
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

    /// A bare `put` is refused: only the owner's publish-then-activate path
    /// makes a value usable, in tests exactly as in the OS store.
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
/// Holds [`SecretValue`] entries keyed by `(provider, label)` behind a
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

    /// Test/dev provisioning path standing in for the Host-local protected
    /// path; production backends must not accept secrets this casually.
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

/// The only environment input [`EnvCredentialStore`] reads, and only once at
/// construction.
pub const ENV_API_KEY: &str = "ENE_OPENAI_API_KEY";

/// Environment-backed bearer store for the `OpenAI` provider.
///
/// The bearer is read from [`ENV_API_KEY`] exactly once when the store is
/// constructed (Host startup) and held as a zeroizing [`SecretValue`] for the
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
    /// Reads [`ENV_API_KEY`] once and pins it for this run.
    #[must_use]
    pub fn new() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Constructs from an injected provisioning lookup, read exactly once.
    #[must_use]
    pub fn from_lookup(lookup: impl FnOnce(&str) -> Option<String>) -> Self {
        Self {
            bearer: resolve_for("openai", lookup).map(|value| SecretValue::new(value.into_bytes())),
        }
    }

    /// The pinned bearer for one provider, gated to the closed world.
    fn pinned(&self, provider: &str) -> Option<&SecretValue> {
        if provider != "openai" {
            return None;
        }
        self.bearer.as_ref()
    }
}

// Shared by the constructor and the tests, so provider gating and emptiness
// stay in one place. An empty value counts as absent, matching an unset
// variable; values arrive as `String`, so the bearer is already valid UTF-8.
pub(crate) fn resolve_for(
    provider: &str,
    lookup: impl FnOnce(&str) -> Option<String>,
) -> Option<String> {
    if provider != "openai" {
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
