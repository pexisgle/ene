//! Credential registry contracts: non-secret refs, secret hygiene, and the
//! request-builder store pattern.
//!
//! [`CredentialRef`] is the only credential value that may leave this crate
//! freely: it names a credential without carrying any secret material. Key
//! material lives solely in [`SecretValue`], which has no [`core::fmt::Debug`]
//! implementation and is zeroized on drop.
//!
//! Secrets enter only through the Host-local protected path (a future
//! behaviors-stage store): [`RegisterCredentialCommand`] deliberately carries
//! no secret field, so registration can never smuggle key material through
//! the registry. [`CredentialStore::with_bearer`] exposes the bearer only
//! inside a caller closure; the caller must build an owned request there and
//! send it after the closure returns, because the borrowed bearer never
//! escapes the closure's lifetime.

use std::collections::HashMap;
use std::sync::Mutex;

use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Non-secret handle naming one stored credential.
///
/// Safe to clone, log, and persist: it carries provider and label only, never
/// key material.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialRef {
    /// Stable composite key of `provider:label`, not a secret.
    pub id: String,
    /// Provider name; matched exactly.
    pub provider: String,
    /// Owner-chosen label distinguishing credentials of one provider.
    pub label: String,
}

/// Command registering a credential ref in the registry.
///
/// There is intentionally no secret field: the bearer is provisioned through
/// the Host-local protected path directly into the [`CredentialStore`], never
/// through this command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterCredentialCommand {
    /// Provider the credential belongs to; must be non-blank.
    pub provider: String,
    /// Owner-chosen label for the credential.
    pub label: String,
}

/// Outcome of a registry `register` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterOutcome {
    /// A new ref was persisted.
    Registered(CredentialRef),
    /// The provider name was blank.
    InvalidProvider,
    /// A ref already exists; the stored ref was left untouched (no overwrite).
    AlreadyExists(CredentialRef),
}

/// Availability of one credential across registry and store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialAvailability {
    /// True only when the ref is known to the registry and the store holds
    /// its bearer.
    pub present: bool,
    /// The known ref, if the registry knows it.
    pub credential: Option<CredentialRef>,
}

/// Host-facing notification about a credential state change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialNotify {
    /// The bearer no longer works; the owner must reauthenticate.
    NeedsReauthentication(CredentialRef),
    /// The credential was revoked and its ref removed.
    Revoked(CredentialRef),
}

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

/// Technical failures of credential storage.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CredentialTechnicalError {
    /// The credential store was unreachable or rejected the operation.
    #[error("credential storage unavailable: {reason}")]
    StorageUnavailable {
        /// Backend-supplied cause, without secret material.
        reason: String,
    },
}

/// Persistence boundary for non-secret credential refs.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialRefRepository: Send + Sync {
    /// Persists a credential ref.
    async fn save_ref(&self, cred: CredentialRef) -> Result<(), CredentialTechnicalError>;

    /// Loads the ref for one `(provider, label)` pair, if any.
    async fn load_ref(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<Option<CredentialRef>, CredentialTechnicalError>;

    /// Lists all known refs.
    async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError>;
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

/// Registers a credential ref, never overwriting an existing one.
///
/// A blank provider (empty or whitespace-only) yields
/// [`RegisterOutcome::InvalidProvider`] without touching the repository. When
/// a ref already exists for `(provider, label)`, the stored ref is returned
/// in [`RegisterOutcome::AlreadyExists`] and no write occurs.
pub async fn register(
    cmd: RegisterCredentialCommand,
    repo: &impl CredentialRefRepository,
) -> Result<RegisterOutcome, CredentialTechnicalError> {
    if cmd.provider.trim().is_empty() {
        return Ok(RegisterOutcome::InvalidProvider);
    }
    if let Some(existing) = repo.load_ref(&cmd.provider, &cmd.label).await? {
        return Ok(RegisterOutcome::AlreadyExists(existing));
    }
    let cred = CredentialRef {
        id: format!("{}:{}", cmd.provider, cmd.label),
        provider: cmd.provider,
        label: cmd.label,
    };
    repo.save_ref(cred.clone()).await?;
    Ok(RegisterOutcome::Registered(cred))
}

/// Combines registry knowledge with store presence into one availability fact.
///
/// `repo_known` reports whether the registry holds the ref; store presence is
/// read via [`CredentialStore::contains`]. The credential is available only
/// when both agree.
pub fn credential_availability(
    cred: &CredentialRef,
    repo_known: bool,
    store: &impl CredentialStore,
) -> CredentialAvailability {
    let present = repo_known && store.contains(cred);
    CredentialAvailability {
        present,
        credential: repo_known.then(|| cred.clone()),
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
    /// Entries keyed by `(provider, label)`; values pair the public ref with
    /// its confined secret.
    entries: Mutex<HashMap<(String, String), (CredentialRef, SecretValue)>>,
}

impl core::fmt::Debug for MemoryCredentialStore {
    /// Renders the entry count and public refs; secrets are never rendered.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let refs: Vec<&CredentialRef> = entries.values().map(|(cred, _)| cred).collect();
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
        let key = (cred.provider.clone(), cred.label.clone());
        let mut entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.insert(key, (cred, SecretValue::new(secret.as_bytes().to_vec())));
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
        let Some((_, secret)) = entries.get(&(cred.provider.clone(), cred.label.clone())) else {
            let id = cred.id.as_str();
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: format!("unknown credential {id}"),
            });
        };
        let Ok(bearer) = core::str::from_utf8(secret.bytes()) else {
            let id = cred.id.as_str();
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
        entries.remove(&(cred.provider.clone(), cred.label.clone()));
        Ok(())
    }

    fn contains(&self, cred: &CredentialRef) -> bool {
        let entries = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        entries.contains_key(&(cred.provider.clone(), cred.label.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CredentialRef, CredentialStore, CredentialTechnicalError, MemoryCredentialStore,
        RegisterCredentialCommand, RegisterOutcome, credential_availability, register,
    };
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    struct FakeRepo {
        refs: Mutex<HashMap<(String, String), CredentialRef>>,
        saves: Mutex<u64>,
    }

    impl FakeRepo {
        fn new() -> Self {
            Self {
                refs: Mutex::new(HashMap::new()),
                saves: Mutex::new(0),
            }
        }

        async fn save_count(&self) -> u64 {
            *self.saves.lock().await
        }
    }

    impl super::CredentialRefRepository for FakeRepo {
        async fn save_ref(&self, cred: CredentialRef) -> Result<(), CredentialTechnicalError> {
            let mut refs = self.refs.lock().await;
            refs.insert((cred.provider.clone(), cred.label.clone()), cred);
            let mut saves = self.saves.lock().await;
            *saves += 1;
            Ok(())
        }

        async fn load_ref(
            &self,
            provider: &str,
            label: &str,
        ) -> Result<Option<CredentialRef>, CredentialTechnicalError> {
            let refs = self.refs.lock().await;
            Ok(refs.get(&(provider.to_owned(), label.to_owned())).cloned())
        }

        async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
            let refs = self.refs.lock().await;
            Ok(refs.values().cloned().collect())
        }
    }

    fn command() -> RegisterCredentialCommand {
        RegisterCredentialCommand {
            provider: "acme".to_owned(),
            label: "main".to_owned(),
        }
    }

    #[tokio::test]
    async fn register_persists_a_new_ref() {
        let repo = FakeRepo::new();
        let outcome = register(command(), &repo).await;
        assert!(matches!(outcome, Ok(RegisterOutcome::Registered(_))));
        assert_eq!(repo.save_count().await, 1);
    }

    #[tokio::test]
    async fn register_rejects_a_blank_provider() {
        let repo = FakeRepo::new();
        let cmd = RegisterCredentialCommand {
            provider: "   ".to_owned(),
            label: "main".to_owned(),
        };
        let outcome = register(cmd, &repo).await;
        assert!(matches!(outcome, Ok(RegisterOutcome::InvalidProvider)));
        assert_eq!(repo.save_count().await, 0);
    }

    #[tokio::test]
    async fn re_register_returns_the_existing_ref_without_overwriting() {
        let repo = FakeRepo::new();
        let first_outcome = register(command(), &repo).await;
        assert!(matches!(first_outcome, Ok(RegisterOutcome::Registered(_))));
        let Ok(RegisterOutcome::Registered(first)) = first_outcome else {
            return;
        };
        let second_outcome = register(command(), &repo).await;
        assert!(matches!(
            second_outcome,
            Ok(RegisterOutcome::AlreadyExists(_))
        ));
        let Ok(RegisterOutcome::AlreadyExists(existing)) = second_outcome else {
            return;
        };
        assert_eq!(existing, first);
        assert_eq!(repo.save_count().await, 1);
    }

    #[tokio::test]
    async fn ref_equality_ignores_nothing() {
        let repo = FakeRepo::new();
        let outcome = register(command(), &repo).await;
        assert!(matches!(outcome, Ok(RegisterOutcome::Registered(_))));
        let Ok(RegisterOutcome::Registered(cred)) = outcome else {
            return;
        };
        assert_eq!(
            cred,
            CredentialRef {
                id: "acme:main".to_owned(),
                provider: "acme".to_owned(),
                label: "main".to_owned(),
            }
        );
    }

    #[test]
    fn availability_requires_both_registry_and_store() {
        let store = MemoryCredentialStore::new();
        let cred = CredentialRef {
            id: "acme:main".to_owned(),
            provider: "acme".to_owned(),
            label: "main".to_owned(),
        };
        let missing = credential_availability(&cred, false, &store);
        assert!(!missing.present);
        assert_eq!(missing.credential, None);
        store.insert(cred.clone(), "bearer-token");
        let store_only = credential_availability(&cred, false, &store);
        assert!(!store_only.present);
        let both = credential_availability(&cred, true, &store);
        assert!(both.present);
        assert_eq!(both.credential, Some(cred.clone()));
        let removed = store.delete(&cred);
        assert!(removed.is_ok());
        let after_delete = credential_availability(&cred, true, &store);
        assert!(!after_delete.present);
        assert_eq!(after_delete.credential, Some(cred));
    }

    #[test]
    fn bearer_closure_receives_the_inserted_secret() {
        let store = MemoryCredentialStore::new();
        let cred = CredentialRef {
            id: "acme:main".to_owned(),
            provider: "acme".to_owned(),
            label: "main".to_owned(),
        };
        store.insert(cred.clone(), "bearer-token");
        let seen = store.with_bearer(&cred, str::len);
        assert_eq!(seen, Ok("bearer-token".len()));
    }

    #[test]
    fn public_debug_output_carries_no_secret() {
        let cred = CredentialRef {
            id: "acme:main".to_owned(),
            provider: "acme".to_owned(),
            label: "main".to_owned(),
        };
        let rendered = format!("{cred:?}");
        assert!(rendered.contains("acme"));
        assert!(!rendered.contains("bearer-token"));
    }
}
