//! Non-secret credential registry: the validated [`CredentialRef`], the
//! persistence boundary for refs, and the register and availability
//! operations over them.

use crate::CredentialTechnicalError;
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
    /// Provider name; matched exactly.
    provider: String,
    /// Owner-chosen label distinguishing credentials of one provider.
    label: String,
}

/// Why a [`CredentialRef`] could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialRefError {
    /// Provider was blank or contained the `:` separator.
    #[error("credential provider must be non-blank and contain no ':'")]
    InvalidProvider,
    /// Label was empty.
    #[error("credential label must be non-empty")]
    InvalidLabel,
}

impl CredentialRef {
    /// Builds a ref from its parts, enforcing the credential grammar.
    ///
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

    /// Owner-chosen label distinguishing credentials of one provider.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Stable composite id of `provider:label`, not a secret.
    ///
    /// Derived, never stored separately: the id always reflects the validated
    /// parts above.
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
    /// The provider name was blank or contained `:`.
    InvalidProvider,
    /// The label was empty.
    InvalidLabel,
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

/// Resolves a credential id to a registered, bearer-backed ref.
///
/// The Host uses this premise for setup gating and views: `Ok(None)` means
/// the pair is absent or unprovisioned (setup incomplete), while `Err` is an
/// infrastructure failure. The two are never collapsed.
pub async fn available_credential(
    credential_id: &str,
    refs: &impl CredentialRefRepository,
    store: &impl CredentialStore,
) -> Result<Option<CredentialRef>, CredentialTechnicalError> {
    let known = refs.list_refs().await?;
    Ok(known
        .into_iter()
        .find(|cred| cred.id() == credential_id && store.contains(cred)))
}
