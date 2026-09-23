use crate::{
    CredentialRefRepository, CredentialSetRepository, CredentialStore, REDACTED_CREDENTIAL,
};
use ene_primitive::RevisionInner;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CredentialSetRevision(RevisionInner);

impl CredentialSetRevision {
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrubbedText {
    text: String,
    credential_set: CredentialSetRevision,
}

impl ScrubbedText {
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn credential_set(&self) -> CredentialSetRevision {
        self.credential_set
    }

    #[must_use]
    pub fn into_text(self) -> String {
        self.text
    }

    #[must_use]
    pub fn with_oldest_premise(mut self, prior: CredentialSetRevision) -> Self {
        self.credential_set = self.credential_set.min(prior);
        self
    }

    #[must_use]
    pub fn oldest_premise<'a>(
        pieces: impl IntoIterator<Item = &'a Self>,
    ) -> Option<CredentialSetRevision> {
        pieces.into_iter().map(|piece| piece.credential_set).min()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecretScrubError {
    #[error("credential registry unavailable")]
    RegistryUnavailable,
    #[error("registered credential value unavailable")]
    SecretUnavailable,
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract style uses native async fn; Send bounds settle with the Host adapter"
)]
pub trait SecretScrubber: Send + Sync {
    async fn scrub(&self, text: &str) -> Result<ScrubbedText, SecretScrubError>;
}

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
                return Err(SecretScrubError::SecretUnavailable);
            }
            known.push((length, credential));
        }
        known.sort_by_key(|(length, _)| std::cmp::Reverse(*length));
        let mut scrubbed = text.to_owned();
        for (_, credential) in &known {
            let replaced = self.store.with_bearer(credential, |bearer| {
                scrubbed.replace(bearer, REDACTED_CREDENTIAL)
            });
            let Ok(next) = replaced else {
                return Err(SecretScrubError::SecretUnavailable);
            };
            scrubbed = next;
        }
        let marker_len = REDACTED_CREDENTIAL.len();
        let marker_starts: Vec<usize> = scrubbed
            .match_indices(REDACTED_CREDENTIAL)
            .map(|(start, _)| start)
            .collect();
        for (_, credential) in &known {
            let still_present = self
                .store
                .with_bearer(credential, |bearer| {
                    scrubbed.char_indices().any(|(start, _)| {
                        if !scrubbed[start..].starts_with(bearer) {
                            return false;
                        }
                        let end = start + bearer.len();
                        !marker_starts
                            .iter()
                            .any(|&marker| start >= marker && end <= marker + marker_len)
                    })
                })
                .map_err(|_| SecretScrubError::SecretUnavailable)?;
            if still_present {
                return Err(SecretScrubError::SecretUnavailable);
            }
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
    async fn a_value_inside_the_marker_is_still_proven_absent() {
        let (refs, store) = registry(&["cred"], CredentialSetRevision::from_u64(2));
        let proof = CredentialScrubber {
            refs: &refs,
            store: &store,
        }
        .scrub("token cred tail")
        .await
        .expect("a marker-substring value must not defeat the absence proof");
        assert_eq!(proof.text(), "token [credential] tail");
    }

    #[tokio::test]
    async fn a_value_reconstructed_at_a_marker_boundary_fails_closed() {
        assert_eq!(
            error_of(&["c", "]x"], "cx").await,
            SecretScrubError::SecretUnavailable
        );
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
