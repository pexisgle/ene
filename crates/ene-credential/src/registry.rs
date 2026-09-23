use crate::CredentialTechnicalError;
use crate::scrub::CredentialSetRevision;
use crate::secret::CredentialStore;

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

    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub fn id(&self) -> String {
        format!("{}:{}", self.provider, self.label)
    }
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialRefRepository: Send + Sync {
    async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError>;
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialSetRepository: Send + Sync {
    async fn current_set_revision(&self)
    -> Result<CredentialSetRevision, CredentialTechnicalError>;
}

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
