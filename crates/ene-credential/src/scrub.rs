//! Secret non-exposure boundary: the scrub port and its credential-set
//! revision premise.
//!
//! A scrubbed text is only useful together with the credential-set revision
//! it was produced under. Writers compare that premise inside their durable
//! transaction (or their send claim) so content prepared before a credential
//! value change can never be committed or sent afterwards. The revision has a
//! single linearization point per change: an explicit approval/update or the
//! Host startup sweep, both of which sweep the effective values and advance
//! the revision in one transaction before any use.
//!
//! Credential values are pinned by the credential store at that boundary and
//! are not re-read from a free-running external source during one Host run,
//! so the durable revision is the whole currentness premise: a value change
//! is either not active yet (old pinned value, old revision) or active with
//! its sweep and revision already committed.

use ene_primitive::RevisionInner;
use thiserror::Error;

/// Monotonic identity of the registered credential set.
///
/// Bumped atomically with a usable credential ref becoming registered, with a
/// successful approval/re-approval, and with the Host startup sweep of the
/// effective values. It is non-secret metadata: it names a state of the set
/// without naming or deriving any value, and is safe to persist, compare, and
/// log. Follows the [`RevisionInner`] discipline: the inner count travels
/// only inside this newtype and [`Self::checked_next`] reports exhaustion
/// instead of aliasing `u64::MAX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CredentialSetRevision(RevisionInner);

impl CredentialSetRevision {
    /// The revision of an environment with no registered credential yet.
    #[must_use]
    pub fn initial() -> Self {
        Self(RevisionInner::from_u64(0))
    }

    #[must_use]
    pub fn from_u64(value: u64) -> Self {
        Self(RevisionInner::from_u64(value))
    }

    #[must_use]
    pub fn as_u64(&self) -> u64 {
        self.0.as_u64()
    }

    /// Callers must treat [`None`] as exhaustion and refuse the bump rather
    /// than reusing `u64::MAX` for a new set state.
    #[must_use]
    pub fn checked_next(&self) -> Option<Self> {
        self.0.checked_next().map(Self)
    }
}

/// Text together with the credential-set premise it was scrubbed under.
///
/// The premise is the revision read before the scrub consumed registered
/// refs and bearers. Writers require the set to still be exactly this
/// revision at commit or send time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrubbedText {
    /// Scrubbed copy; the raw input is no longer referenced after this.
    pub text: String,
    /// Credential-set revision the scrub was produced under.
    pub credential_set: CredentialSetRevision,
}

impl ScrubbedText {
    /// The oldest (most conservative) premise of a set of scrubbed pieces.
    ///
    /// Writers that combine several scrubbed pieces take the minimum, so the
    /// commit is accepted only when every piece was scrubbed under the same
    /// current revision.
    #[must_use]
    pub fn oldest_premise<'a>(
        pieces: impl IntoIterator<Item = &'a Self>,
    ) -> Option<CredentialSetRevision> {
        pieces.into_iter().map(|piece| piece.credential_set).min()
    }
}

/// Failure to prove that registered secret values are absent from text.
///
/// Scrubbing fails closed: when the registry or a bearer cannot be read, the
/// text must not reach a model prompt, durable storage, or a provider send.
/// This is never "no secret was found".
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecretScrubError {
    /// The credential registry or its revision could not be read.
    #[error("credential registry unavailable")]
    RegistryUnavailable,
    /// A registered credential's value could not be read.
    #[error("registered credential value unavailable")]
    SecretUnavailable,
}

/// Redaction of registered secret values with a currentness premise.
///
/// Implementations must return text with secret occurrences removed and must
/// not expose the secret itself. The returned [`ScrubbedText::credential_set`]
/// must be read before the refs and bearers it covers, so an approval or
/// startup sweep that follows leaves the premise stale.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract style uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait SecretScrubber: Send + Sync {
    /// Returns `text` with every registered secret occurrence replaced.
    ///
    /// # Errors
    ///
    /// [`SecretScrubError`] when absence cannot be proven for every
    /// registered credential; the caller must not use the original text.
    async fn scrub(&self, text: &str) -> Result<ScrubbedText, SecretScrubError>;
}

#[cfg(test)]
mod tests {
    use super::CredentialSetRevision;

    #[test]
    fn revision_exhaustion_reports_none_instead_of_aliasing() {
        assert_eq!(
            CredentialSetRevision::from_u64(0).checked_next(),
            Some(CredentialSetRevision::from_u64(1))
        );
        assert_eq!(
            CredentialSetRevision::from_u64(u64::MAX).checked_next(),
            None
        );
    }
}
