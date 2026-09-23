use crate::CredentialTechnicalError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RegistrationFingerprint {
    pub intent_id: String,
    pub kind: String,
    pub target: String,
    pub base: String,
    pub rationale_origin: String,
    pub rationale_quote: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegistrationState {
    AppliedAsOneTime,
    HeldByOperation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegistrationApply {
    Decided(RegistrationState),
    AlreadyDecided,
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialIntentRepository: Send + Sync {
    async fn request_registration_with_intent(
        &self,
        provider: String,
        label: String,
        fingerprint: RegistrationFingerprint,
    ) -> Result<RegistrationApply, CredentialTechnicalError>;
}
