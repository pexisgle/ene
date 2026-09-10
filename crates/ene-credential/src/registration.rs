//! Intent-aware credential registration: the management register path's
//! atomic pending-or-usable determination.
//!
//! The registration owner decides whether a requested pair is already
//! usable or now pends Owner approval; this module carries that decision's
//! durable fingerprint and the one-transaction boundary that writes the
//! pending row and the intent journal row together. The journal's outcome
//! vocabulary stays with the intents it snapshots; only the state decision
//! is credential-owned here.

use crate::CredentialTechnicalError;

/// Durable fingerprint of one credential-registration intent.
///
/// Mirrors the management-intent journal columns (intent id, kind, target,
/// base view, rationale) while staying a credential-owned type: the
/// registration decision must not borrow the consent intent vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RegistrationFingerprint {
    /// Intent key, as hyphenated UUID text.
    pub intent_id: String,
    /// Intent kind discriminator (`"register"`).
    pub kind: String,
    /// Intent target text.
    pub target: String,
    /// Base-view mark text the intent was built on.
    pub base: String,
    /// Rationale origin text.
    pub rationale_origin: String,
    /// Rationale quote, if the intent carried one.
    pub rationale_quote: Option<String>,
}

/// Decided registration state recorded by one atomic request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegistrationState {
    /// The pair is already usable as a one-time approval.
    AppliedAsOneTime,
    /// The pair is not usable yet: its request pends Owner approval.
    HeldByOperation,
}

/// Result of one atomic registration request.
///
/// [`Self::Decided`] is the fresh state this call recorded.
/// [`Self::AlreadyDecided`] means a concurrent or earlier call owns the
/// intent row: the caller resolves the stored snapshot through the intent
/// journal instead of guessing it here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegistrationApply {
    /// This call decided and recorded the state.
    Decided(RegistrationState),
    /// An intent row already existed; nothing changed here.
    AlreadyDecided,
}

/// Atomic credential-registration boundary for the management register path.
///
/// One transaction: the intent claim check first, then the pending insert
/// or usable recheck, then the journal row, so the decided state and the
/// row that describes it can never strand apart. An existing intent row
/// never rewrites; the caller answers from the journal.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract uses native async fn; Send bounds settle with the store impl"
)]
pub trait CredentialIntentRepository: Send + Sync {
    /// Requests registration of `(provider, label)` under `fingerprint`.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialTechnicalError::StorageUnavailable`] when the
    /// store fails or the pair is blank; a blank pair is a caller bug the
    /// wire target grammar already rejects.
    async fn request_registration_with_intent(
        &self,
        provider: String,
        label: String,
        fingerprint: RegistrationFingerprint,
    ) -> Result<RegistrationApply, CredentialTechnicalError>;
}
