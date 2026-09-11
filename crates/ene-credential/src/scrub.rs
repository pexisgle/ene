//! Secret non-exposure boundary: the scrub port and its credential-set
//! revision premise.
//!
//! A scrubred text is only useful together with the credential-set revision
//! it was produced under. Writers compare that premise inside their durable
//! transaction (or their send claim) so content scrubbed before a credential
//! became registered can never be committed or sent afterwards: either the
//! write lands first and the approval's sweep redacts it, or the approval
//! lands first and the stale premise refuses the write.

use ene_primitive::RevisionInner;
use thiserror::Error;

/// Monotonic identity of the registered credential set.
///
/// Bumped atomically with a usable credential ref becoming registered (and
/// with the approval sweep). It is non-secret metadata: it names a state of
/// the set without naming any value, and is safe to compare, persist, and
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
/// The premise is the revision read *before* the scrub consumed registered
/// refs and bearers, so it is never newer than the set the scrub actually
/// saw. Writers require the set to still be exactly this revision at commit
/// or send time.
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
/// must be read before the refs and bearers it covers, so a later approval
/// always leaves the premise older than the new set.
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
