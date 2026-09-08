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

use ene_primitive::{RawId, WallClockWithTz};
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

/// Opaque device identity for device pairing (Group K, Stage 2 thin scope).
///
/// Wraps a [`RawId`] with no `From` implementations to or from any other
/// type: a `DeviceId` names one logical device in this crate's pairing
/// records only. It is distinct from the wire `DeviceWireId` carried in
/// `ene-api` envelopes: mapping between wire and domain identities happens in
/// Host composition at the call boundary, never in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(
    /// Wrapped opaque identity; meaningful only as a device name in this crate.
    pub RawId,
);

/// One paired device: its minted identity, display string, and pairing time.
///
/// `descriptor` is an owner-supplied display string (for example `"phone"`);
/// it carries no secret material, so derived [`core::fmt::Debug`] is safe.
/// `paired_at` records when pairing completed, for display and audit only.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceRecord {
    /// Device identity minted at approval; never reused.
    pub id: DeviceId,
    /// Owner-visible display string naming the device.
    pub descriptor: String,
    /// Wall-clock time with its creation offset recording when pairing completed.
    pub paired_at: WallClockWithTz,
}

/// One requested-but-not-yet-approved pairing.
///
/// `descriptor` is the owner-supplied display string from the request; it
/// carries no secret material, so derived [`core::fmt::Debug`] is safe.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PendingPairing {
    /// Owner-visible display string naming the requesting device.
    pub descriptor: String,
    /// Wall-clock time with its creation offset recording when requested.
    pub requested_at: WallClockWithTz,
}

/// Outcome of a pairing request.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DevicePairingStatus {
    /// The descriptor is already paired; carries the existing record.
    Paired {
        /// Existing paired-device record, unchanged.
        device: DeviceRecord,
    },
    /// The descriptor is not yet paired; carries the pending request.
    Pending {
        /// Pending request, newly recorded or previously stored.
        pending: PendingPairing,
    },
}

/// Persistence boundary for device pairing requests and approvals.
///
/// Revocation is explicitly deferred: Stage 2 thin scope provides no
/// remove/revoke method, so paired records only accumulate.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait DevicePairingRepository: Send + Sync {
    /// Records a pairing request for `descriptor`.
    ///
    /// Returns [`DevicePairingStatus::Paired`] only when `descriptor` is
    /// already paired (idempotent re-request leaves the stored record
    /// untouched). Otherwise records a pending request — or returns the
    /// existing pending entry when one is already stored — and returns
    /// [`DevicePairingStatus::Pending`].
    ///
    /// Blank-descriptor contract: Host ingress validates that the descriptor
    /// is non-blank before calling. Implementations perform no blank check
    /// themselves: a blank `descriptor` is recorded as pending like any other
    /// string, never rejected here, so callers must not rely on this method
    /// to catch blank input.
    async fn request_pairing(
        &self,
        descriptor: String,
    ) -> Result<DevicePairingStatus, CredentialTechnicalError>;

    /// Approves the pending request for `descriptor`, pairing the device.
    ///
    /// On a known pending descriptor this mints a fresh identity via
    /// [`RawId::new`], moves the entry from pending to paired, and returns
    /// the new record. An unknown descriptor yields `Ok(None)` — not an
    /// error; the caller maps that outcome to a clarification request.
    /// Re-approving an already-paired descriptor is idempotent: the existing
    /// record is returned unchanged and no fresh identity is minted.
    ///
    /// Approval records an Owner decision transported from a trusted inlet;
    /// the repository never decides whether pairing is allowed, it records
    /// the decision it was given.
    async fn approve_pending(
        &self,
        descriptor: &str,
    ) -> Result<Option<DeviceRecord>, CredentialTechnicalError>;

    /// Loads the paired record for `id`, if any.
    async fn find_device(
        &self,
        id: &DeviceId,
    ) -> Result<Option<DeviceRecord>, CredentialTechnicalError>;

    /// Lists all currently pending pairing requests.
    async fn list_pending(&self) -> Result<Vec<PendingPairing>, CredentialTechnicalError>;
}

#[cfg(test)]
mod tests {
    use super::{
        CredentialRef, CredentialStore, CredentialTechnicalError, MemoryCredentialStore,
        RegisterCredentialCommand, RegisterOutcome, credential_availability, register,
    };
    use super::{
        DeviceId, DevicePairingRepository, DevicePairingStatus, DeviceRecord, PendingPairing,
    };
    use ene_primitive::{RawId, WallClockWithTz};
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

    struct FakePairingRepo {
        pending: Mutex<HashMap<String, PendingPairing>>,
        paired: Mutex<HashMap<String, DeviceRecord>>,
    }

    impl FakePairingRepo {
        fn new() -> Self {
            Self {
                pending: Mutex::new(HashMap::new()),
                paired: Mutex::new(HashMap::new()),
            }
        }
    }

    impl DevicePairingRepository for FakePairingRepo {
        async fn request_pairing(
            &self,
            descriptor: String,
        ) -> Result<DevicePairingStatus, CredentialTechnicalError> {
            if let Some(device) = self.paired.lock().await.get(&descriptor).cloned() {
                return Ok(DevicePairingStatus::Paired { device });
            }
            if let Some(pending) = self.pending.lock().await.get(&descriptor).cloned() {
                return Ok(DevicePairingStatus::Pending { pending });
            }
            let pending = PendingPairing {
                descriptor: descriptor.clone(),
                requested_at: WallClockWithTz::now(),
            };
            self.pending
                .lock()
                .await
                .insert(descriptor, pending.clone());
            Ok(DevicePairingStatus::Pending { pending })
        }

        async fn approve_pending(
            &self,
            descriptor: &str,
        ) -> Result<Option<DeviceRecord>, CredentialTechnicalError> {
            if let Some(device) = self.paired.lock().await.get(descriptor).cloned() {
                return Ok(Some(device));
            }
            let pending = self.pending.lock().await.remove(descriptor);
            let Some(stored) = pending else {
                return Ok(None);
            };
            let device = DeviceRecord {
                id: DeviceId(RawId::new()),
                descriptor: stored.descriptor,
                paired_at: WallClockWithTz::now(),
            };
            self.paired
                .lock()
                .await
                .insert(descriptor.to_owned(), device.clone());
            Ok(Some(device))
        }

        async fn find_device(
            &self,
            id: &DeviceId,
        ) -> Result<Option<DeviceRecord>, CredentialTechnicalError> {
            let paired = self.paired.lock().await;
            Ok(paired.values().find(|device| device.id == *id).cloned())
        }

        async fn list_pending(&self) -> Result<Vec<PendingPairing>, CredentialTechnicalError> {
            let pending = self.pending.lock().await;
            Ok(pending.values().cloned().collect())
        }
    }

    #[tokio::test]
    async fn first_request_yields_pending() {
        let repo = FakePairingRepo::new();
        let status = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(status, Ok(DevicePairingStatus::Pending { .. })));
        let Ok(DevicePairingStatus::Pending { pending }) = status else {
            return;
        };
        assert_eq!(pending.descriptor.as_str(), "phone");
        let listed = repo.list_pending().await;
        let Ok(items) = listed else {
            return;
        };
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn second_request_for_same_descriptor_stays_pending_without_duplicate() {
        let repo = FakePairingRepo::new();
        let first = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(first, Ok(DevicePairingStatus::Pending { .. })));
        let second = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(second, Ok(DevicePairingStatus::Pending { .. })));
        let Ok(DevicePairingStatus::Pending {
            pending: first_pending,
        }) = first
        else {
            return;
        };
        let Ok(DevicePairingStatus::Pending {
            pending: second_pending,
        }) = second
        else {
            return;
        };
        assert_eq!(first_pending.descriptor, second_pending.descriptor);
        let listed = repo.list_pending().await;
        let Ok(items) = listed else {
            return;
        };
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn approve_unknown_descriptor_returns_none() {
        let repo = FakePairingRepo::new();
        let approved = repo.approve_pending("unknown").await;
        assert!(matches!(approved, Ok(None)));
        let found = repo.find_device(&DeviceId(RawId::new())).await;
        assert!(matches!(found, Ok(None)));
    }

    #[tokio::test]
    async fn approve_moves_pending_to_paired() {
        let repo = FakePairingRepo::new();
        let requested = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(requested, Ok(DevicePairingStatus::Pending { .. })));
        let approved = repo.approve_pending("phone").await;
        assert!(matches!(approved, Ok(Some(_))));
        let Ok(Some(device)) = approved else {
            return;
        };
        assert_eq!(device.descriptor.as_str(), "phone");
        let listed = repo.list_pending().await;
        let Ok(items) = listed else {
            return;
        };
        assert!(items.is_empty());
        let found = repo.find_device(&device.id).await;
        assert!(matches!(found, Ok(Some(_))));
        let Ok(Some(stored)) = found else {
            return;
        };
        assert_eq!(stored, device);
    }

    #[tokio::test]
    async fn re_request_after_paired_returns_paired_with_same_id() {
        let repo = FakePairingRepo::new();
        let requested = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(requested, Ok(DevicePairingStatus::Pending { .. })));
        let approved = repo.approve_pending("phone").await;
        let Ok(Some(device)) = approved else {
            return;
        };
        let again = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(again, Ok(DevicePairingStatus::Paired { .. })));
        let Ok(DevicePairingStatus::Paired { device: existing }) = again else {
            return;
        };
        assert_eq!(existing.id, device.id);
        assert_eq!(existing, device);
    }

    #[tokio::test]
    async fn re_approve_returns_the_existing_record() {
        let repo = FakePairingRepo::new();
        let requested = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(requested, Ok(DevicePairingStatus::Pending { .. })));
        let approved = repo.approve_pending("phone").await;
        let Ok(Some(first)) = approved else {
            return;
        };
        let reapproved = repo.approve_pending("phone").await;
        assert!(matches!(reapproved, Ok(Some(_))));
        let Ok(Some(second)) = reapproved else {
            return;
        };
        assert_eq!(second.id, first.id);
        assert_eq!(second, first);
    }

    #[tokio::test]
    async fn list_pending_reports_each_descriptor_once() {
        let repo = FakePairingRepo::new();
        let first = repo.request_pairing("phone".to_owned()).await;
        assert!(matches!(first, Ok(DevicePairingStatus::Pending { .. })));
        let second = repo.request_pairing("tablet".to_owned()).await;
        assert!(matches!(second, Ok(DevicePairingStatus::Pending { .. })));
        let listed = repo.list_pending().await;
        let Ok(items) = listed else {
            return;
        };
        let descriptors: Vec<&str> = items.iter().map(|item| item.descriptor.as_str()).collect();
        assert_eq!(descriptors.len(), 2);
        assert!(descriptors.contains(&"phone"));
        assert!(descriptors.contains(&"tablet"));
    }
}
