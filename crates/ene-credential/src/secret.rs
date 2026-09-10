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
    /// Raw bearer bytes; never logged, never rendered.
    pub(crate) bytes: Vec<u8>,
}

impl SecretValue {
    /// Wraps raw bearer bytes for an in-crate store.
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// Borrows the bearer bytes for an in-crate store.
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

    /// Deletes the bearer for `cred`.
    fn delete(&self, cred: &CredentialRef) -> Result<(), CredentialTechnicalError>;

    /// Reports whether the store holds a bearer for `cred`.
    ///
    /// Existence is non-secret metadata.
    fn contains(&self, cred: &CredentialRef) -> bool;
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
    /// Entries keyed by the validated ref itself; values hold the confined
    /// secret.
    entries: Mutex<HashMap<CredentialRef, SecretValue>>,
}

impl core::fmt::Debug for MemoryCredentialStore {
    /// Renders the entry count and public refs; secrets are never rendered.
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
    /// Creates an empty store holding no bearers.
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryCredentialStore {
    /// Creates an empty store holding no bearers.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Inserts or replaces the bearer for `cred`.
    ///
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

    fn delete(&self, cred: &CredentialRef) -> Result<(), CredentialTechnicalError> {
        let mut entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.remove(cred);
        Ok(())
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        let entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.contains_key(cred)
    }
}

/// Name of the process environment variable carrying the `OpenAI` bearer.
///
/// The only environment input [`EnvCredentialStore`] ever reads, and only
/// inside [`CredentialStore::with_bearer`] and [`CredentialStore::contains`],
/// at call time.
pub const ENV_API_KEY: &str = "ENE_OPENAI_API_KEY";

/// Environment-backed bearer store for the `OpenAI` provider.
///
/// The struct is fieldless by design: the bearer is read from
/// [`ENV_API_KEY`] on every call and never cached in memory, so backup
/// exclusion holds trivially (there is nothing to back up) and key rotation
/// takes effect on the next call without a restart. Only the `"openai"`
/// provider is served (closed world until real OS stores arrive); every other
/// provider reports absent.
///
/// Request-builder discipline (shared with every [`CredentialStore`]): the
/// bearer is lent as `&str` into the caller's closure, which must build its
/// owned request (headers, body) there and perform I/O after it returns.
/// Nothing borrows the key out, and errors carry a status class only, never
/// key material.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnvCredentialStore;

impl EnvCredentialStore {
    /// Creates the store. Performs no I/O; the environment is read per call.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

// Pure-over-closure lookup shared by `contains` and `with_bearer`, so both
// agree on provider gating and emptiness. Production passes
// `|name| std::env::var(name).ok()` (a safe function; no `unsafe` involved)
// as `lookup`; tests inject closures, which keeps them hermetic: only the
// one-line wiring at each call site touches the real process environment. An
// empty value counts as absent, matching an unset variable. Values arrive as
// `String`, so the bearer is already valid UTF-8; no revalidation exists.
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
        // Single live read of the process environment per call: never cached,
        // rotation-friendly. `std::env::var` is a safe function.
        let Some(bearer) = resolve_for(cred.provider(), |name| std::env::var(name).ok()) else {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: "env credential missing".to_owned(),
            });
        };
        // The borrow of `bearer` cannot escape: `f` must build its owned
        // request inside this closure.
        Ok(f(&bearer))
    }

    fn delete(&self, _cred: &CredentialRef) -> Result<(), CredentialTechnicalError> {
        // The bearer lives in the process environment, which this store
        // cannot mutate. Reporting success would claim a deletion that did
        // not happen; revocation is ref-side, by removing the
        // `CredentialRef` from the `CredentialRefRepository`.
        Err(CredentialTechnicalError::StorageUnavailable {
            reason: "environment credentials cannot be deleted; remove the credential ref"
                .to_owned(),
        })
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        // Same predicate as `with_bearer`'s gate, so availability and access
        // never disagree. Existence is non-secret metadata.
        resolve_for(cred.provider(), |name| std::env::var(name).ok()).is_some()
    }
}
