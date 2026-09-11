//! Non-secret credential registry: the validated [`CredentialRef`], the
//! persistence boundary for refs, and the register and availability
//! operations over them.

use crate::CredentialTechnicalError;
use crate::scrub::{CredentialSetRevision, CredentialSetState};
use crate::secret::CredentialStore;

/// Non-secret handle naming one stored credential.
///
/// Safe to clone, log, and persist: it carries provider and label only, never
/// key material. Fields are private so every ref passes [`CredentialRef::new`],
/// which enforces the same grammar as the management wire target
/// (`credential:{provider}:{label}`): the provider is non-blank and contains
/// no `:` separator; the label is non-empty and may contain `:`. The advisory
/// id is derived from those parts, so provider, label, and id can never
/// disagree, and two distinct `(provider, label)` pairs can never share an id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialRef {
    provider: String,
    label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialRefError {
    #[error("credential provider must be non-blank and contain no ':'")]
    InvalidProvider,
    #[error("credential label must be non-empty")]
    InvalidLabel,
}

impl CredentialRef {
    /// # Errors
    ///
    /// Returns [`CredentialRefError::InvalidProvider`] for a blank provider or
    /// one containing `:`, and [`CredentialRefError::InvalidLabel`] for an
    /// empty label. A label may contain `:`, matching the wire parser.
    pub fn new(
        provider: impl Into<String>,
        label: impl Into<String>,
    ) -> Result<Self, CredentialRefError> {
        let provider = provider.into();
        let label = label.into();
        if provider.trim().is_empty() || provider.contains(':') {
            return Err(CredentialRefError::InvalidProvider);
        }
        if label.is_empty() {
            return Err(CredentialRefError::InvalidLabel);
        }
        Ok(Self { provider, label })
    }

    /// Provider name; matched exactly.
    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Stable composite id of `provider:label`, not a secret; derived, never
    /// stored separately.
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}:{}", self.provider, self.label)
    }
}

/// Command registering a credential ref in the registry.
///
/// There is intentionally no secret field: the bearer is provisioned through
/// the Host-local protected path directly into the [`CredentialStore`], never
/// through this command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterCredentialCommand {
    pub provider: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterOutcome {
    Registered(CredentialRef),
    InvalidProvider,
    InvalidLabel,
    /// A ref already exists; the stored ref was left untouched (no overwrite).
    AlreadyExists(CredentialRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialAvailability {
    /// True only when the ref is known to the registry and the store holds
    /// its bearer.
    pub present: bool,
    /// The known ref, if the registry knows it.
    pub credential: Option<CredentialRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialNotify {
    /// The bearer no longer works; the owner must reauthenticate.
    NeedsReauthentication(CredentialRef),
    /// The credential was revoked and its ref removed.
    Revoked(CredentialRef),
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialRefRepository: Send + Sync {
    async fn save_ref(&self, cred: CredentialRef) -> Result<(), CredentialTechnicalError>;

    async fn load_ref(
        &self,
        provider: &str,
        label: &str,
    ) -> Result<Option<CredentialRef>, CredentialTechnicalError>;

    async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError>;
}

/// Durable identity of the registered credential set.
///
/// Separate from [`CredentialRefRepository`] because readers that only need
/// the set's currentness (secret scrubbers, writers validating a scrub
/// premise) must not gain the ref-management surface. The revision bumps
/// atomically with a usable ref becoming registered and whenever an observed
/// value change is reconciled.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialSetRepository: Send + Sync {
    /// Loads the current credential-set state.
    async fn credential_set_state(&self) -> Result<CredentialSetState, CredentialTechnicalError>;

    /// Reconciles the effective values with the durable set atomically.
    ///
    /// One short transaction: reads each registered value through `values`,
    /// replaces any plaintext occurrence in durable content, records the
    /// observed fingerprint, and advances the revision. Called only when an
    /// observation found the values different from the recorded fingerprint;
    /// it performs no I/O beyond the value store and the database.
    fn reconcile_values<S: CredentialStore>(
        &self,
        refs: &[CredentialRef],
        values: &S,
    ) -> Result<CredentialSetRevision, CredentialTechnicalError>;
}

/// Registers a credential ref, never overwriting an existing one.
///
/// The provider and label must satisfy [`CredentialRef::new`]: a provider that
/// is blank or contains `:` yields [`RegisterOutcome::InvalidProvider`], and an
/// empty label yields [`RegisterOutcome::InvalidLabel`], both without touching
/// the repository. When a ref already exists for `(provider, label)`, the
/// stored ref is returned in [`RegisterOutcome::AlreadyExists`] and no write
/// occurs.
pub async fn register(
    cmd: RegisterCredentialCommand,
    repo: &impl CredentialRefRepository,
) -> Result<RegisterOutcome, CredentialTechnicalError> {
    let cred = match CredentialRef::new(cmd.provider, cmd.label) {
        Ok(cred) => cred,
        Err(CredentialRefError::InvalidProvider) => {
            return Ok(RegisterOutcome::InvalidProvider);
        }
        Err(CredentialRefError::InvalidLabel) => return Ok(RegisterOutcome::InvalidLabel),
    };
    if let Some(existing) = repo.load_ref(cred.provider(), cred.label()).await? {
        return Ok(RegisterOutcome::AlreadyExists(existing));
    }
    repo.save_ref(cred.clone()).await?;
    Ok(RegisterOutcome::Registered(cred))
}

/// Combines registry knowledge with store presence into one availability
/// fact: [`CredentialStore::contains`] supplies store presence, and the
/// credential is available only when both agree.
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

/// Resolves a credential id to a registered, bearer-backed ref for `provider`.
///
/// The Host uses this premise for setup gating and views: `Ok(None)` means
/// the pair is absent, unprovisioned, or names a credential of a different
/// provider (the route could not bill it), while `Err` is an infrastructure
/// failure. The two are never collapsed.
pub async fn available_credential(
    provider: &str,
    credential_id: &str,
    refs: &impl CredentialRefRepository,
    store: &impl CredentialStore,
) -> Result<Option<CredentialRef>, CredentialTechnicalError> {
    let known = refs.list_refs().await?;
    Ok(known.into_iter().find(|cred| {
        cred.provider() == provider && cred.id() == credential_id && store.contains(cred)
    }))
}
