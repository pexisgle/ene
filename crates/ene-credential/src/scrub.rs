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
use sha2::{Digest, Sha256};
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

/// Durable currentness of the registered credential value set.
///
/// `values: None` means a ref or approval change invalidated the last
/// observation; the next scrub reconciles the effective values (sweeps them
/// and advances the revision atomically) before issuing a premise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialSetState {
    pub revision: CredentialSetRevision,
    pub values: Option<CredentialValuesDigest>,
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

/// Order-independent fingerprint of the effective registered credential
/// values one observation saw.
///
/// It is a change detector, not a password hash: each `(ref, value)` pair is
/// hashed with a domain separator and the per-pair digests are XOR-folded, so
/// the same set yields the same fingerprint regardless of iteration order and
/// a value change alters the fingerprint with negligible collision risk. It
/// stays inside the credential owner and the store as an opaque token and
/// carries no part of any value.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CredentialValuesDigest([u8; 32]);

impl CredentialValuesDigest {
    /// All-zero fingerprint of the empty value set.
    #[must_use]
    pub fn empty() -> Self {
        Self([0_u8; 32])
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        use core::fmt::Write as _;
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            write!(out, "{byte:02x}").ok();
        }
        out
    }

    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        if text.len() != 64 {
            return None;
        }
        let (chunks, remainder) = text.as_bytes().as_chunks::<2>();
        if !remainder.is_empty() {
            return None;
        }
        let mut bytes = [0_u8; 32];
        for (position, chunk) in chunks.iter().enumerate() {
            let high = (chunk[0] as char).to_digit(16)?;
            let low = (chunk[1] as char).to_digit(16)?;
            bytes[position] = u8::try_from(high * 16 + low).ok()?;
        }
        Some(Self(bytes))
    }
}

impl core::fmt::Debug for CredentialValuesDigest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("CredentialValuesDigest(<redacted>)")
    }
}

/// Accumulates [`CredentialValuesDigest`] pairs in any order.
#[derive(Default)]
pub struct CredentialValuesDigestBuilder {
    accumulator: [u8; 32],
}

impl CredentialValuesDigestBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one `(ref, value)` pair into the fingerprint.
    ///
    /// Callers must fail closed before adding an empty value: an empty
    /// pattern matches every position.
    pub fn add(&mut self, cred: &crate::CredentialRef, bearer: &str) {
        let mut hasher = Sha256::new();
        hasher.update(b"ene.credential-values.v1");
        let id = cred.id();
        hasher.update(u64::try_from(id.len()).unwrap_or(u64::MAX).to_be_bytes());
        hasher.update(id.as_bytes());
        hasher.update(
            u64::try_from(bearer.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        hasher.update(bearer.as_bytes());
        let pair: [u8; 32] = hasher.finalize().into();
        for (slot, byte) in self.accumulator.iter_mut().zip(pair) {
            *slot ^= byte;
        }
    }

    #[must_use]
    pub fn finish(self) -> CredentialValuesDigest {
        CredentialValuesDigest(self.accumulator)
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
    /// The effective credential values moved past the observed set.
    ///
    /// The implementation reconciled the change (swept the new values and
    /// advanced the credential-set revision); the caller must re-scrub before
    /// using content prepared under the old set.
    #[error("registered credential values changed")]
    ValuesChanged,
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

    /// Confirms the effective value set still matches the last observation.
    ///
    /// Called at durable-write and send boundaries. When the values moved,
    /// the implementation reconciles them (sweeps the new values into durable
    /// content and advances the credential-set revision atomically) and
    /// returns [`SecretScrubError::ValuesChanged`]; the caller fails closed
    /// and re-scrubs instead of writing or sending content prepared under the
    /// old set.
    async fn verify_current(&self) -> Result<(), SecretScrubError>;
}

#[cfg(test)]
mod tests {
    use super::{CredentialSetRevision, CredentialValuesDigest, CredentialValuesDigestBuilder};
    use crate::CredentialRef;

    fn credential(label: &str) -> CredentialRef {
        CredentialRef::new("openai", label).expect("valid test fixture")
    }

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

    #[test]
    fn value_digest_is_order_independent_and_detects_a_change() {
        let first = credential("first");
        let second = credential("second");
        let mut left = CredentialValuesDigestBuilder::new();
        left.add(&first, "sk-a");
        left.add(&second, "sk-b");
        let mut right = CredentialValuesDigestBuilder::new();
        right.add(&second, "sk-b");
        right.add(&first, "sk-a");
        let left_digest = left.finish();
        let right_digest = right.finish();
        assert_eq!(left_digest, right_digest);

        let mut changed = CredentialValuesDigestBuilder::new();
        changed.add(&first, "sk-a");
        changed.add(&second, "sk-c");
        let changed_digest = changed.finish();
        assert_ne!(left_digest, changed_digest);
        assert_ne!(CredentialValuesDigest::empty(), changed_digest);

        let hex = changed_digest.to_hex();
        assert_eq!(CredentialValuesDigest::from_hex(&hex), Some(changed_digest));
        assert_eq!(CredentialValuesDigest::from_hex("not-hex"), None);
    }
}
