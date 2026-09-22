use ene_credential::{
    ActivationOutcome, CredentialPublicationRepository as _, CredentialRef,
    CredentialRefRepository as _, CredentialTechnicalError, MemoryVersionedStore, MutationKind,
    MutationOutcome, MutationPhase, RetiredCredentialVersion, SecretVersionId,
    UncommittedMutationOutcome, VersionedCredentialStore,
};

use crate::Store;

fn credential() -> CredentialRef {
    CredentialRef::new("acme", "main").expect("valid test fixture")
}

/// Versioned store whose removal fails for one version, so a test can hold a
/// retirement pending across passes without failing the whole pass.
struct FailingDelete {
    inner: MemoryVersionedStore,
    fail: std::sync::atomic::AtomicU64,
}

impl FailingDelete {
    fn new(fail: u64) -> Self {
        Self {
            inner: MemoryVersionedStore::new(),
            fail: std::sync::atomic::AtomicU64::new(fail),
        }
    }

    fn allow_removal(&self) {
        self.fail.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

impl VersionedCredentialStore for FailingDelete {
    fn put_version(
        &self,
        cred: &CredentialRef,
        version: u64,
        secret: &str,
    ) -> Result<(), CredentialTechnicalError> {
        self.inner.put_version(cred, version, secret)
    }

    fn with_version<R>(
        &self,
        cred: &CredentialRef,
        version: u64,
        f: impl FnOnce(&str) -> R,
    ) -> Result<R, CredentialTechnicalError> {
        self.inner.with_version(cred, version, f)
    }

    fn prepare_snapshot(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<ene_credential::PreparedCredentialSnapshot, CredentialTechnicalError> {
        self.inner.prepare_snapshot(cred, version)
    }

    fn activate(&self, snapshot: ene_credential::PreparedCredentialSnapshot) {
        self.inner.activate(snapshot);
    }

    fn deactivate(&self, cred: &CredentialRef) {
        self.inner.deactivate(cred);
    }

    fn delete_version(
        &self,
        cred: &CredentialRef,
        version: u64,
    ) -> Result<(), CredentialTechnicalError> {
        if version == self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(CredentialTechnicalError::StorageUnavailable {
                reason: String::from("injected removal failure"),
            });
        }
        self.inner.delete_version(cred, version)
    }
}

fn retired_row(version: SecretVersionId, mutation_id: &str) -> RetiredCredentialVersion {
    RetiredCredentialVersion {
        provider: String::from("acme"),
        label: String::from("main"),
        version,
        mutation_id: mutation_id.to_owned(),
    }
}

async fn staged(store: &Store, mutation_id: &str, version: u64) -> SecretVersionId {
    let candidate = SecretVersionId::from_u64(version);
    let mutation = store
        .begin_credential_mutation(
            mutation_id.to_owned(),
            MutationKind::Register,
            String::from("acme"),
            String::from("main"),
            None,
            Some(candidate),
        )
        .await
        .expect("the mutation must begin");
    assert_eq!(
        mutation.phase,
        MutationPhase::Prepared,
        "a fresh mutation is recorded before any OS write"
    );
    assert_eq!(
        mutation.candidate_version,
        Some(candidate),
        "Prepared must durably name the OS item before it is written"
    );
    store
        .mark_credential_staged(mutation_id, candidate)
        .await
        .expect("the candidate must stage");
    candidate
}

#[tokio::test]
async fn activation_commits_the_reference_version_and_revision_together() {
    let store = Store::open_in_memory().await.unwrap();
    let candidate = staged(&store, "m-1", 1).await;
    let outcome = store
        .activate_credential("m-1", "sk-first", None)
        .await
        .expect("activation must run");
    let ActivationOutcome::Activated { revision, retired } = outcome else {
        panic!("the first activation must commit, got {outcome:?}");
    };
    assert_eq!(retired, None, "nothing was active before");
    let active = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(
        active,
        Some(candidate),
        "the active pointer names the candidate version"
    );
    assert!(
        store
            .pending_credential_retirements(10)
            .await
            .unwrap()
            .is_empty(),
        "nothing was retired"
    );
    let stored = store
        .credential_mutation("m-1")
        .await
        .unwrap()
        .expect("the mutation must be readable");
    assert_eq!(
        stored.phase,
        MutationPhase::Activated,
        "nothing was retired, so activation needs no cleanup"
    );
    assert_eq!(
        stored.outcome,
        Some(MutationOutcome::Activated { revision }),
        "the decided outcome is stored independently of the phase"
    );
    assert_eq!(stored.decided_revision, Some(revision));
}

/// A rotation retires the previous version into the durable set and leaves its
/// item addressable until a cleanup pass records that it was removed.
#[tokio::test]
async fn a_rotation_retires_the_previous_version_for_cleanup() {
    let store = Store::open_in_memory().await.unwrap();
    let values = MemoryVersionedStore::new();
    values.put_version(&credential(), 1, "sk-first").unwrap();
    values.put_version(&credential(), 2, "sk-second").unwrap();
    let first = staged(&store, "m-1", 1).await;
    store
        .activate_credential("m-1", "sk-first", None)
        .await
        .unwrap();
    let second = staged(&store, "m-2", 2).await;
    let outcome = store
        .activate_credential("m-2", "sk-second", Some("sk-first"))
        .await
        .unwrap();
    let ActivationOutcome::Activated { retired, .. } = outcome else {
        panic!("the rotation must commit, got {outcome:?}");
    };
    assert_eq!(
        retired,
        Some(first),
        "the replaced version is retired by this commit"
    );
    let active = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(active, Some(second));
    assert_eq!(
        store.pending_credential_retirements(10).await.unwrap(),
        vec![retired_row(first, "m-2")],
        "the retired version is a durable pending row"
    );
    let rotated = store
        .credential_mutation("m-2")
        .await
        .unwrap()
        .expect("the rotation mutation must be readable");
    assert_eq!(
        rotated.phase,
        MutationPhase::CleanupPending,
        "an active value with a retired version still needs cleanup"
    );
    // One bounded pass removes the OS item and records the removal together
    // with the completing phase.
    assert_eq!(store.sweep_retired_credentials(&values, 10).unwrap(), 1);
    assert!(
        values.with_version(&credential(), 1, |_| ()).is_err(),
        "the retired OS item is gone"
    );
    let cleansed = store
        .credential_mutation("m-2")
        .await
        .unwrap()
        .expect("the rotation mutation must stay readable");
    assert_eq!(
        cleansed.phase,
        MutationPhase::Completed,
        "a cleaned retirement completes the mutation that recorded it"
    );
    let cleaned = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(
        cleaned,
        Some(second),
        "cleanup never moves the active pointer"
    );
    assert!(
        store
            .pending_credential_retirements(10)
            .await
            .unwrap()
            .is_empty(),
        "the removed version is no longer pending"
    );
    assert_eq!(
        store.sweep_retired_credentials(&values, 10).unwrap(),
        0,
        "a repeated pass has nothing left to sweep"
    );
}

/// Two rotations in a row both stay tracked while the earlier cleanup is
/// pending, and bounded passes sweep both versions.
#[tokio::test]
async fn two_rotations_keep_both_retired_versions_and_sweep_them() {
    let store = Store::open_in_memory().await.unwrap();
    let values = MemoryVersionedStore::new();
    for (version, secret) in [(1, "sk-1"), (2, "sk-2"), (3, "sk-3")] {
        values.put_version(&credential(), version, secret).unwrap();
    }
    let first = staged(&store, "m-1", 1).await;
    store
        .activate_credential("m-1", "sk-1", None)
        .await
        .unwrap();
    let second = staged(&store, "m-2", 2).await;
    store
        .activate_credential("m-2", "sk-2", Some("sk-1"))
        .await
        .unwrap();
    // The second rotation happens before the first retirement was cleaned.
    let third = staged(&store, "m-3", 3).await;
    store
        .activate_credential("m-3", "sk-3", Some("sk-2"))
        .await
        .unwrap();
    assert_eq!(
        store.pending_credential_retirements(10).await.unwrap(),
        vec![retired_row(first, "m-2"), retired_row(second, "m-3")],
        "neither retirement was overwritten by the later rotation"
    );
    assert_eq!(
        store
            .credential_mutation("m-2")
            .await
            .unwrap()
            .unwrap()
            .phase,
        MutationPhase::CleanupPending
    );
    assert_eq!(
        store
            .credential_mutation("m-3")
            .await
            .unwrap()
            .unwrap()
            .phase,
        MutationPhase::CleanupPending
    );
    // One bounded pass at a time: the oldest pending version is swept first,
    // and only its mutation completes.
    assert_eq!(store.sweep_retired_credentials(&values, 1).unwrap(), 1);
    assert!(values.with_version(&credential(), 1, |_| ()).is_err());
    assert!(values.with_version(&credential(), 2, |_| ()).is_ok());
    assert_eq!(
        store
            .credential_mutation("m-2")
            .await
            .unwrap()
            .unwrap()
            .phase,
        MutationPhase::Completed
    );
    assert_eq!(
        store
            .credential_mutation("m-3")
            .await
            .unwrap()
            .unwrap()
            .phase,
        MutationPhase::CleanupPending
    );
    assert_eq!(
        store.pending_credential_retirements(10).await.unwrap(),
        vec![retired_row(second, "m-3")]
    );
    // The next pass drains the second retirement.
    assert_eq!(store.sweep_retired_credentials(&values, 1).unwrap(), 1);
    assert!(values.with_version(&credential(), 2, |_| ()).is_err());
    assert_eq!(
        store
            .credential_mutation("m-3")
            .await
            .unwrap()
            .unwrap()
            .phase,
        MutationPhase::Completed
    );
    assert!(
        store
            .pending_credential_retirements(10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .active_credential_version("acme", "main")
            .await
            .unwrap(),
        Some(third)
    );
}

/// A revocation while a rotation's cleanup is pending keeps both versions
/// tracked and sweeps both.
#[tokio::test]
async fn a_revocation_while_a_cleanup_is_pending_keeps_both_versions() {
    let store = Store::open_in_memory().await.unwrap();
    let values = MemoryVersionedStore::new();
    values.put_version(&credential(), 1, "sk-1").unwrap();
    values.put_version(&credential(), 2, "sk-2").unwrap();
    let first = staged(&store, "m-1", 1).await;
    store
        .activate_credential("m-1", "sk-1", None)
        .await
        .unwrap();
    let second = staged(&store, "m-2", 2).await;
    store
        .activate_credential("m-2", "sk-2", Some("sk-1"))
        .await
        .unwrap();
    let current = ene_credential::CredentialSetRepository::current_set_revision(&store)
        .await
        .unwrap()
        .as_u64();
    store
        .begin_credential_mutation(
            String::from("m-revoke"),
            MutationKind::Revoke,
            String::from("acme"),
            String::from("main"),
            Some(current),
            None,
        )
        .await
        .unwrap();
    store
        .revoke_credential("m-revoke", Some("sk-2"))
        .await
        .unwrap();
    assert_eq!(
        store.pending_credential_retirements(10).await.unwrap(),
        vec![retired_row(first, "m-2"), retired_row(second, "m-revoke")],
        "the revocation kept the rotation's pending version and added its own"
    );
    assert_eq!(
        store
            .credential_mutation("m-revoke")
            .await
            .unwrap()
            .unwrap()
            .phase,
        MutationPhase::CleanupPending
    );
    assert_eq!(store.sweep_retired_credentials(&values, 10).unwrap(), 2);
    assert!(values.with_version(&credential(), 1, |_| ()).is_err());
    assert!(values.with_version(&credential(), 2, |_| ()).is_err());
    for mutation_id in ["m-2", "m-revoke"] {
        assert_eq!(
            store
                .credential_mutation(mutation_id)
                .await
                .unwrap()
                .unwrap()
                .phase,
            MutationPhase::Completed,
            "{mutation_id} completes only after its version is gone"
        );
    }
    assert!(
        store
            .pending_credential_retirements(10)
            .await
            .unwrap()
            .is_empty()
    );
}

/// Bounded cleanup makes progress across passes, stays within its batch, and
/// never reports a version swept whose removal failed.
#[tokio::test]
async fn bounded_cleanup_makes_progress_and_never_completes_early() {
    let store = Store::open_in_memory().await.unwrap();
    let values = FailingDelete::new(1);
    for (version, secret) in [(1, "sk-1"), (2, "sk-2"), (3, "sk-3"), (4, "sk-4")] {
        values.put_version(&credential(), version, secret).unwrap();
    }
    let mut previous: Option<u64> = None;
    for version in 1_u64..=4 {
        let mutation_id = format!("m-{version}");
        staged(&store, &mutation_id, version).await;
        let replaced = previous.map(|previous| format!("sk-{previous}"));
        store
            .activate_credential(&mutation_id, &format!("sk-{version}"), replaced.as_deref())
            .await
            .unwrap();
        previous = Some(version);
    }
    assert_eq!(
        store
            .pending_credential_retirements(10)
            .await
            .unwrap()
            .len(),
        3
    );
    // One pass examines at most the requested bound and leaves the version
    // whose removal was not confirmed pending.
    assert_eq!(store.sweep_retired_credentials(&values, 2).unwrap(), 1);
    assert_eq!(
        store.pending_credential_retirements(10).await.unwrap(),
        vec![
            retired_row(SecretVersionId::from_u64(1), "m-2"),
            retired_row(SecretVersionId::from_u64(3), "m-4"),
        ],
        "the failed removal and the version beyond the batch bound both stay"
    );
    assert_eq!(
        store
            .credential_mutation("m-2")
            .await
            .unwrap()
            .unwrap()
            .phase,
        MutationPhase::CleanupPending,
        "an unconfirmed removal must not complete its mutation"
    );
    assert!(values.with_version(&credential(), 1, |_| ()).is_ok());
    assert!(values.with_version(&credential(), 2, |_| ()).is_err());
    assert!(values.with_version(&credential(), 3, |_| ()).is_ok());
    // Once the removal succeeds, the remaining versions drain.
    values.allow_removal();
    assert_eq!(store.sweep_retired_credentials(&values, 2).unwrap(), 2);
    assert!(
        store
            .pending_credential_retirements(10)
            .await
            .unwrap()
            .is_empty()
    );
    for mutation_id in ["m-2", "m-4"] {
        assert_eq!(
            store
                .credential_mutation(mutation_id)
                .await
                .unwrap()
                .unwrap()
                .phase,
            MutationPhase::Completed
        );
    }
}

/// A crash after the OS erase but before the state write leaves the row
/// pending, and the next pass re-attempts the idempotent removal.
#[tokio::test]
async fn a_crash_after_erase_re_attempts_the_removal() {
    let store = Store::open_in_memory().await.unwrap();
    let values = MemoryVersionedStore::new();
    values.put_version(&credential(), 1, "sk-1").unwrap();
    values.put_version(&credential(), 2, "sk-2").unwrap();
    staged(&store, "m-1", 1).await;
    store
        .activate_credential("m-1", "sk-1", None)
        .await
        .unwrap();
    staged(&store, "m-2", 2).await;
    store
        .activate_credential("m-2", "sk-2", Some("sk-1"))
        .await
        .unwrap();
    // The erase happened, the state write did not.
    values.delete_version(&credential(), 1).unwrap();
    assert_eq!(
        store.pending_credential_retirements(10).await.unwrap(),
        vec![retired_row(SecretVersionId::from_u64(1), "m-2")],
        "an erased but unrecorded version is still pending, never reported swept"
    );
    assert_eq!(
        store
            .credential_mutation("m-2")
            .await
            .unwrap()
            .unwrap()
            .phase,
        MutationPhase::CleanupPending
    );
    // The next pass re-attempts the removal and records it.
    assert_eq!(store.sweep_retired_credentials(&values, 10).unwrap(), 1);
    assert_eq!(
        store
            .credential_mutation("m-2")
            .await
            .unwrap()
            .unwrap()
            .phase,
        MutationPhase::Completed
    );
    assert!(
        store
            .pending_credential_retirements(10)
            .await
            .unwrap()
            .is_empty()
    );
}

/// A restart resumes a retirement whose removal never ran, from the durable
/// row alone.
#[tokio::test]
async fn a_restart_resumes_pending_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("publication.db");
    let values = MemoryVersionedStore::new();
    values.put_version(&credential(), 1, "sk-1").unwrap();
    values.put_version(&credential(), 2, "sk-2").unwrap();
    {
        let store = Store::open(&path).await.unwrap();
        staged(&store, "m-1", 1).await;
        store
            .activate_credential("m-1", "sk-1", None)
            .await
            .unwrap();
        staged(&store, "m-2", 2).await;
        store
            .activate_credential("m-2", "sk-2", Some("sk-1"))
            .await
            .unwrap();
        assert_eq!(
            store.pending_credential_retirements(10).await.unwrap(),
            vec![retired_row(SecretVersionId::from_u64(1), "m-2")]
        );
    }
    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(
        reopened.pending_credential_retirements(10).await.unwrap(),
        vec![retired_row(SecretVersionId::from_u64(1), "m-2")],
        "the pending retirement survives the restart"
    );
    assert_eq!(
        reopened
            .credential_mutation("m-2")
            .await
            .unwrap()
            .unwrap()
            .phase,
        MutationPhase::CleanupPending
    );
    assert_eq!(reopened.sweep_retired_credentials(&values, 10).unwrap(), 1);
    assert!(values.with_version(&credential(), 1, |_| ()).is_err());
    assert_eq!(
        reopened
            .credential_mutation("m-2")
            .await
            .unwrap()
            .unwrap()
            .phase,
        MutationPhase::Completed
    );
    assert!(
        reopened
            .pending_credential_retirements(10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_stale_premise_abandons_the_candidate_and_stays_decided() {
    let store = Store::open_in_memory().await.unwrap();
    let mutation = store
        .begin_credential_mutation(
            String::from("m-stale"),
            MutationKind::Register,
            String::from("acme"),
            String::from("main"),
            Some(7),
            Some(SecretVersionId::from_u64(3)),
        )
        .await
        .unwrap();
    assert_eq!(mutation.expected_revision, Some(7));
    store
        .mark_credential_staged("m-stale", SecretVersionId::from_u64(3))
        .await
        .unwrap();
    let outcome = store
        .activate_credential("m-stale", "sk-late", None)
        .await
        .unwrap();
    let ActivationOutcome::Stale { current_revision } = outcome else {
        panic!("a moved premise must not adopt the candidate, got {outcome:?}");
    };
    assert_ne!(current_revision, 7);
    let active = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(active, None, "a refused candidate is never active");
    let stored = store.credential_mutation("m-stale").await.unwrap().unwrap();
    assert_eq!(stored.phase, MutationPhase::Abandoned);
    assert_eq!(stored.outcome, Some(MutationOutcome::Stale));
    let retried = store
        .activate_credential("m-stale", "sk-late", None)
        .await
        .unwrap();
    assert_eq!(
        retried,
        ActivationOutcome::AlreadyDecided(MutationOutcome::Stale)
    );
}

#[tokio::test]
async fn mutation_ids_are_write_once_and_unknown_ids_are_refused() {
    let store = Store::open_in_memory().await.unwrap();
    let first = store
        .begin_credential_mutation(
            String::from("m-dup"),
            MutationKind::Register,
            String::from("acme"),
            String::from("main"),
            None,
            Some(SecretVersionId::from_u64(4)),
        )
        .await
        .unwrap();
    let second = store
        .begin_credential_mutation(
            String::from("m-dup"),
            MutationKind::Register,
            String::from("acme"),
            String::from("main"),
            None,
            Some(SecretVersionId::from_u64(4)),
        )
        .await
        .unwrap();
    assert_eq!(first, second, "a retry observes the original attempt");
    let rebound = store
        .begin_credential_mutation(
            String::from("m-dup"),
            MutationKind::Register,
            String::from("acme"),
            String::from("other"),
            None,
            Some(SecretVersionId::from_u64(9)),
        )
        .await;
    assert!(
        rebound.is_err(),
        "one mutation id must not be rebound to another premise"
    );
    assert_eq!(
        store
            .activate_credential("m-unknown", "sk-never", None)
            .await
            .unwrap(),
        ActivationOutcome::Missing,
        "an unknown mutation id must not adopt anything"
    );
}

/// A refusal recorded without an activation leaves nothing active, and the
/// outcome survives a later read.
#[tokio::test]
async fn a_refused_mutation_records_its_outcome_without_activating() {
    let store = Store::open_in_memory().await.unwrap();
    store
        .begin_credential_mutation(
            String::from("m-refused"),
            MutationKind::Register,
            String::from("acme"),
            String::from("main"),
            None,
            Some(SecretVersionId::from_u64(5)),
        )
        .await
        .unwrap();
    store
        .record_credential_mutation_outcome("m-refused", UncommittedMutationOutcome::Refused)
        .await
        .unwrap();
    let stored = store
        .credential_mutation("m-refused")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.outcome, Some(MutationOutcome::Refused));
    assert_eq!(stored.phase, MutationPhase::Abandoned);
    let active = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(active, None);
}

/// A revocation is recorded as its own mutation kind and stages no candidate value.
#[tokio::test]
async fn revocation_is_its_own_mutation_kind() {
    let store = Store::open_in_memory().await.unwrap();
    let mutation = store
        .begin_credential_mutation(
            String::from("m-revoke"),
            MutationKind::Revoke,
            String::from("acme"),
            String::from("main"),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(mutation.kind, MutationKind::Revoke);
    assert_eq!(
        mutation.candidate_version, None,
        "revocation stages no value"
    );
}

/// A revocation clears the active reference, advances the revision, records the
/// `Revoked` outcome, and leaves the invalidated version addressable until its
/// item is removed.
#[tokio::test]
async fn a_revocation_invalidates_the_reference_and_retires_the_version() {
    let store = Store::open_in_memory().await.unwrap();
    let first = staged(&store, "m-1", 1).await;
    store
        .activate_credential("m-1", "sk-first", None)
        .await
        .unwrap();
    let current = ene_credential::CredentialSetRepository::current_set_revision(&store)
        .await
        .unwrap()
        .as_u64();
    store
        .begin_credential_mutation(
            String::from("m-revoke"),
            MutationKind::Revoke,
            String::from("acme"),
            String::from("main"),
            Some(current),
            None,
        )
        .await
        .unwrap();
    let outcome = store
        .revoke_credential("m-revoke", Some("sk-first"))
        .await
        .expect("the revocation must run");
    let ActivationOutcome::Activated { revision, retired } = outcome else {
        panic!("the revocation must commit, got {outcome:?}");
    };
    assert_eq!(
        retired,
        Some(first),
        "the invalidated version is retired by this commit"
    );
    let active = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(active, None, "revocation clears the active reference");
    assert_eq!(
        store.pending_credential_retirements(10).await.unwrap(),
        vec![retired_row(first, "m-revoke")]
    );
    // Revocation deletes the usable ref, so a later re-registration must be
    // approved again instead of being answered as already applied.
    assert!(
        store.list_refs().await.unwrap().is_empty(),
        "the revoked pair must no longer be a usable credential ref"
    );
    let stored = store
        .credential_mutation("m-revoke")
        .await
        .unwrap()
        .expect("the revocation mutation must be readable");
    assert_eq!(stored.phase, MutationPhase::CleanupPending);
    assert_eq!(stored.outcome, Some(MutationOutcome::Revoked { revision }));
    assert_eq!(stored.decided_revision, Some(revision));
    // Cleanup completes the mutation without moving the (cleared) reference.
    let values = MemoryVersionedStore::new();
    values.put_version(&credential(), 1, "sk-first").unwrap();
    assert_eq!(store.sweep_retired_credentials(&values, 10).unwrap(), 1);
    let cleaned = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(cleaned, None);
    assert!(
        store
            .pending_credential_retirements(10)
            .await
            .unwrap()
            .is_empty()
    );
    let completed = store
        .credential_mutation("m-revoke")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.phase, MutationPhase::Completed);
    // A retry answers from the stored decision and never invalidates again.
    assert_eq!(
        store
            .revoke_credential("m-revoke", Some("sk-first"))
            .await
            .unwrap(),
        ActivationOutcome::AlreadyDecided(MutationOutcome::Revoked { revision })
    );
}

/// A revocation with no active reference still commits its revision and
/// completes immediately, and a decided revocation is never re-applied.
#[tokio::test]
async fn a_revocation_without_an_active_value_completes() {
    let store = Store::open_in_memory().await.unwrap();
    store
        .begin_credential_mutation(
            String::from("m-revoke"),
            MutationKind::Revoke,
            String::from("acme"),
            String::from("main"),
            None,
            None,
        )
        .await
        .unwrap();
    let outcome = store
        .revoke_credential("m-revoke", None)
        .await
        .expect("the revocation must run");
    let ActivationOutcome::Activated { revision, retired } = outcome else {
        panic!("the revocation must commit, got {outcome:?}");
    };
    assert_eq!(retired, None, "nothing was active to retire");
    let active = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(active, None);
    assert!(
        store
            .pending_credential_retirements(10)
            .await
            .unwrap()
            .is_empty()
    );
    let stored = store
        .credential_mutation("m-revoke")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.phase,
        MutationPhase::Completed,
        "a revocation with nothing to remove is complete"
    );
    assert_eq!(stored.outcome, Some(MutationOutcome::Revoked { revision }));
    assert_eq!(
        store.revoke_credential("m-revoke", None).await.unwrap(),
        ActivationOutcome::AlreadyDecided(MutationOutcome::Revoked { revision })
    );
}

/// A moved premise refuses the revocation without invalidating the current
/// reference, and a registration mutation is never revoked.
#[tokio::test]
async fn a_stale_or_non_revoke_mutation_changes_nothing() {
    let store = Store::open_in_memory().await.unwrap();
    let first = staged(&store, "m-1", 1).await;
    store
        .activate_credential("m-1", "sk-first", None)
        .await
        .unwrap();
    store
        .begin_credential_mutation(
            String::from("m-stale"),
            MutationKind::Revoke,
            String::from("acme"),
            String::from("main"),
            Some(7),
            None,
        )
        .await
        .unwrap();
    let stale = store
        .revoke_credential("m-stale", Some("sk-first"))
        .await
        .unwrap();
    assert!(matches!(stale, ActivationOutcome::Stale { .. }));
    let active = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(
        active,
        Some(first),
        "a stale revocation never clears the live reference"
    );
    let stored = store.credential_mutation("m-stale").await.unwrap().unwrap();
    assert_eq!(stored.phase, MutationPhase::Abandoned);
    assert_eq!(stored.outcome, Some(MutationOutcome::Stale));
    // A registration mutation is refused even before it stages a candidate.
    store
        .begin_credential_mutation(
            String::from("m-register"),
            MutationKind::Register,
            String::from("acme"),
            String::from("main"),
            None,
            Some(SecretVersionId::from_u64(9)),
        )
        .await
        .unwrap();
    assert_eq!(
        store.revoke_credential("m-register", None).await.unwrap(),
        ActivationOutcome::Missing,
        "a registration mutation is not a revocation"
    );
    let active = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(active, Some(first));
}
