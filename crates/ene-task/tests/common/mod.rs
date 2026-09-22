//! Shared fixtures for `ene-task` integration tests.
//!
//! A Task result body can only enter through the credential-owned scrub
//! boundary. These helpers run the real `CredentialScrubber` over an empty
//! readable registry, so the premise a test passes is minted exactly like a
//! production one; no test fake constructs a body/revision pair.

use ene_credential::{
    CredentialRef, CredentialRefRepository, CredentialScrubber, CredentialSetRepository,
    CredentialSetRevision, CredentialTechnicalError, MemoryCredentialStore,
};
use ene_task::{
    DelegationId, TaskRepository, TaskResultArrivalOutcome, TaskResultRecord,
    TaskResultScrubPremise, orchestrate_result_arrival,
};

/// A readable registry with no refs, pinned at one revision.
struct EmptyRegistry(CredentialSetRevision);

impl CredentialRefRepository for EmptyRegistry {
    #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
    async fn list_refs(&self) -> Result<Vec<CredentialRef>, CredentialTechnicalError> {
        Ok(Vec::new())
    }
}

impl CredentialSetRepository for EmptyRegistry {
    #[expect(clippy::unused_async_trait_impl, reason = "fixture repository port")]
    async fn current_set_revision(
        &self,
    ) -> Result<CredentialSetRevision, CredentialTechnicalError> {
        Ok(self.0)
    }
}

/// Scrubs `text` through the credential-owned boundary under `revision`.
pub async fn scrubbed_at(revision: CredentialSetRevision, text: &str) -> TaskResultScrubPremise {
    use ene_credential::SecretScrubber as _;

    let registry = EmptyRegistry(revision);
    let values = MemoryCredentialStore::new();
    TaskResultScrubPremise::from_scrubbed(
        CredentialScrubber {
            refs: &registry,
            store: &values,
        }
        .scrub(text)
        .await
        .expect("the empty fixture registry is readable"),
    )
}

/// Scrubs `text` at the initial revision; an empty registry leaves it as is.
pub async fn scrubbed(text: &str) -> TaskResultScrubPremise {
    scrubbed_at(CredentialSetRevision::initial(), text).await
}

/// Runs one result arrival over a fake repository and returns the recorded
/// result. Fake repositories never refuse a premise, so a stale refusal here
/// would be a fixture error.
pub async fn record(
    repository: &impl TaskRepository,
    delegation: DelegationId,
    text: &str,
) -> TaskResultRecord {
    match orchestrate_result_arrival(repository, delegation, scrubbed(text).await)
        .await
        .expect("the fake repository must answer the arrival")
    {
        TaskResultArrivalOutcome::Recorded(record) => record,
        TaskResultArrivalOutcome::StaleCredentialSet { .. } => {
            panic!("a fake repository cannot refuse a current premise")
        }
    }
}
