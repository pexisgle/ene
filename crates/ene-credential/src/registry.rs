//! Non-secret credential registry: the validated [`CredentialRef`], the
//! persistence boundary for refs, and resolution of the usable ones.
//!
//! Usable refs are written through the credential-registration intent plus
//! the Host-local approval path; this module owns the read side.

use crate::CredentialTechnicalError;
use crate::scrub::CredentialSetRevision;
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

/// Persistence boundary for the usable credential refs.
///
/// The trait is read-only: refs become usable through the credential
/// registration intent and the Host-local approval write, not here.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialRefRepository: Send + Sync {
    async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError>;
}

/// Durable identity of the registered credential set.
///
/// Separate from [`CredentialRefRepository`] because readers that only need
/// the set's currentness (secret scrubbers, writers validating a scrub
/// premise) must not gain the ref-management surface. The revision bumps
/// atomically with a usable ref becoming registered, with a successful
/// approval/re-approval, and with the Host startup sweep.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialSetRepository: Send + Sync {
    /// Loads the current credential-set revision.
    async fn current_set_revision(&self)
    -> Result<CredentialSetRevision, CredentialTechnicalError>;
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
