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

use crate::{
    CredentialRefRepository, CredentialSetRepository, CredentialStore, REDACTED_CREDENTIAL,
};
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
/// revision at commit or send time. Only the credential-owned scrubber can
/// construct it; consumers cannot supply a body and a readable revision.
///
/// ```compile_fail
/// use ene_credential::{CredentialSetRevision, ScrubbedText};
/// let forged = ScrubbedText {
///     text: String::from("unscrubbed"),
///     credential_set: CredentialSetRevision::initial(),
/// };
/// ```
///
/// Neither the body nor the revision can be replaced on an existing proof.
///
/// ```compile_fail
/// use ene_credential::ScrubbedText;
/// fn replace(proof: &mut ScrubbedText) {
///     proof.text = String::from("unscrubbed");
/// }
/// ```
///
/// ```compile_fail
/// use ene_credential::{CredentialSetRevision, ScrubbedText};
/// fn refresh(proof: &mut ScrubbedText) {
///     proof.credential_set = CredentialSetRevision::from_u64(99);
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrubbedText {
    /// Scrubbed copy; the raw input is no longer referenced after this.
    text: String,
    /// Credential-set revision the scrub was produced under.
    credential_set: CredentialSetRevision,
}

impl ScrubbedText {
    /// Borrows the scrubbed text without permitting mutation of the proof.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn credential_set(&self) -> CredentialSetRevision {
        self.credential_set
    }

    /// Consumes the proof; the returned string is not itself a scrub proof.
    #[must_use]
    pub fn into_text(self) -> String {
        self.text
    }

    /// Keeps a prior preparation premise when already-scrubbed pieces were
    /// used to assemble this text before its final scrub. This can only make
    /// currentness more conservative, never replace text or promote a revision.
    #[must_use]
    pub fn with_oldest_premise(mut self, prior: CredentialSetRevision) -> Self {
        self.credential_set = self.credential_set.min(prior);
        self
    }

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

/// Redacts registered credential values from text on its way to a model
/// prompt or durable content.
///
/// Uses the existing credential boundary: bearer values are borrowed inside
/// `with_bearer` and only redacted copies escape. The credential store pins
/// its values for the whole Host run, so the revision read here names exactly
/// the values being applied; an explicit approval or the startup sweep
/// advances that revision with its own durable sweep. Every value is applied
/// longest-first so a shorter registered value cannot split an occurrence of
/// a longer one. An unreadable registry or bearer fails closed: absence
/// cannot be proven, so the caller must not use the original text.
pub struct CredentialScrubber<'a, R, S> {
    pub refs: &'a R,
    pub store: &'a S,
}

impl<R, S> SecretScrubber for CredentialScrubber<'_, R, S>
where
    R: CredentialRefRepository + CredentialSetRepository,
    S: CredentialStore,
{
    async fn scrub(&self, text: &str) -> Result<ScrubbedText, SecretScrubError> {
        let credential_set = self
            .refs
            .current_set_revision()
            .await
            .map_err(|_| SecretScrubError::RegistryUnavailable)?;
        let refs = self
            .refs
            .list_refs()
            .await
            .map_err(|_| SecretScrubError::RegistryUnavailable)?;
        let mut known: Vec<(usize, crate::CredentialRef)> = Vec::with_capacity(refs.len());
        for credential in refs {
            let length = self
                .store
                .with_bearer(&credential, |bearer| bearer.len())
                .map_err(|_| SecretScrubError::SecretUnavailable)?;
            if length == 0 {
                // An empty value matches every position; treating it as
                // unprovable keeps the raw text out of prompts and storage.
                return Err(SecretScrubError::SecretUnavailable);
            }
            known.push((length, credential));
        }
        known.sort_by_key(|(length, _)| std::cmp::Reverse(*length));
        let mut scrubbed = text.to_owned();
        for (_, credential) in known {
            let replaced = self.store.with_bearer(&credential, |bearer| {
                scrubbed.replace(bearer, REDACTED_CREDENTIAL)
            });
            let Ok(next) = replaced else {
                // A registered credential exists but its bearer cannot be
                // read, so absence of the value cannot be proven. Fail closed
                // rather than risk putting the raw text in a prompt or a
                // durable Learning row.
                return Err(SecretScrubError::SecretUnavailable);
            };
            scrubbed = next;
        }
        Ok(ScrubbedText {
            text: scrubbed,
            credential_set,
        })
    }
}

#[cfg(test)]
mod scrub_tests {
    use super::{CredentialScrubber, CredentialSetRevision, SecretScrubError, SecretScrubber as _};
    use crate::CredentialTechnicalError;
    use crate::registry::CredentialRef;
    use crate::secret::MemoryCredentialStore;

    struct Registry {
        refs: Vec<CredentialRef>,
        revision: CredentialSetRevision,
    }

    impl crate::registry::CredentialRefRepository for Registry {
        async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
            Ok(self.refs.clone())
        }
    }

    impl crate::registry::CredentialSetRepository for Registry {
        async fn current_set_revision(
            &self,
        ) -> Result<CredentialSetRevision, CredentialTechnicalError> {
            Ok(self.revision)
        }
    }

    fn registry(
        values: &[&str],
        revision: CredentialSetRevision,
    ) -> (Registry, MemoryCredentialStore) {
        let store = MemoryCredentialStore::new();
        let refs = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let cred = CredentialRef::new("acme", format!("key-{index}")).expect("valid ref");
                store.insert(cred.clone(), value);
                cred
            })
            .collect();
        (Registry { refs, revision }, store)
    }

    /// A registry whose revision read fails while its refs stay readable,
    /// modelling the two failure ports separately.
    struct BrokenRevisionRegistry(Vec<CredentialRef>);

    impl crate::registry::CredentialRefRepository for BrokenRevisionRegistry {
        async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
            Ok(self.0.clone())
        }
    }

    impl crate::registry::CredentialSetRepository for BrokenRevisionRegistry {
        async fn current_set_revision(
            &self,
        ) -> Result<CredentialSetRevision, CredentialTechnicalError> {
            Err(CredentialTechnicalError::StorageUnavailable {
                reason: String::from("fixture"),
            })
        }
    }

    async fn error_of(values: &[&str], text: &str) -> SecretScrubError {
        let (refs, store) = registry(values, CredentialSetRevision::from_u64(3));
        let scrubber = CredentialScrubber {
            refs: &refs,
            store: &store,
        };
        match scrubber.scrub(text).await {
            Ok(_) => panic!("scrub must fail closed"),
            Err(error) => error,
        }
    }

    #[tokio::test]
    async fn registered_values_are_redacted_and_revision_names_the_set() {
        let (refs, store) = registry(&["sk-abc"], CredentialSetRevision::from_u64(5));
        let proof = CredentialScrubber {
            refs: &refs,
            store: &store,
        }
        .scrub("token sk-abc tail")
        .await
        .expect("readable registry");
        assert_eq!(proof.text(), "token [credential] tail");
        assert_eq!(proof.credential_set(), CredentialSetRevision::from_u64(5));
    }

    #[tokio::test]
    async fn the_longer_value_is_removed_whole() {
        let (refs, store) = registry(&["sk-abc", "sk-abcdef"], CredentialSetRevision::from_u64(1));
        let proof = CredentialScrubber {
            refs: &refs,
            store: &store,
        }
        .scrub("sk-abcdef")
        .await
        .expect("readable registry");
        assert_eq!(proof.text(), "[credential]");
    }

    #[tokio::test]
    async fn an_unreadable_registry_fails_closed() {
        let registry = BrokenRevisionRegistry(Vec::new());
        let scrubber = CredentialScrubber {
            refs: &registry,
            store: &MemoryCredentialStore::new(),
        };
        match scrubber.scrub("unrelated").await {
            Ok(_) => panic!("an unreadable registry cannot prove absence"),
            Err(error) => assert_eq!(error, SecretScrubError::RegistryUnavailable),
        }
    }

    #[tokio::test]
    async fn an_empty_bearer_cannot_prove_absence() {
        assert_eq!(
            error_of(&[""], "any text").await,
            SecretScrubError::SecretUnavailable
        );
    }

    #[tokio::test]
    async fn proof_construction_is_conservative_only() {
        let (refs, store) = registry(&[], CredentialSetRevision::from_u64(9));
        let proof = CredentialScrubber {
            refs: &refs,
            store: &store,
        }
        .scrub("body")
        .await
        .expect("readable registry");
        assert_eq!(
            proof
                .clone()
                .with_oldest_premise(CredentialSetRevision::from_u64(4))
                .credential_set(),
            CredentialSetRevision::from_u64(4)
        );
        assert_eq!(
            proof
                .with_oldest_premise(CredentialSetRevision::from_u64(12))
                .credential_set(),
            CredentialSetRevision::from_u64(9),
            "a prior premise never raises the revision"
        );
    }
}

#[cfg(test)]
mod revision_tests {
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
