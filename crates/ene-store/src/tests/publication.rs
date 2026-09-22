use ene_credential::{
    ActivationOutcome, CredentialPublicationRepository as _, MutationKind, MutationOutcome,
    MutationPhase, SecretVersionId, UncommittedMutationOutcome,
};

use crate::Store;

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
        active.active,
        Some(candidate),
        "the active pointer names the candidate version"
    );
    assert_eq!(active.cleanup, None, "no version was retired");
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

#[tokio::test]
async fn a_rotation_retires_the_previous_version_for_cleanup() {
    let store = Store::open_in_memory().await.unwrap();
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
        "the replaced version stays addressable for cleanup"
    );
    let active = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(active.active, Some(second));
    assert_eq!(active.cleanup, Some(first));
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
    store
        .mark_credential_cleaned("m-2", "acme", "main", first)
        .await
        .unwrap();
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
        cleaned.active,
        Some(second),
        "cleanup never moves the active pointer"
    );
    assert_eq!(
        cleaned.cleanup, None,
        "the removed version is no longer pending"
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
    assert_eq!(active.active, None, "a refused candidate is never active");
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
    assert_eq!(active.active, None);
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
        "the invalidated version stays addressable for cleanup"
    );
    let active = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(
        active.active, None,
        "revocation clears the active reference"
    );
    assert_eq!(active.cleanup, Some(first));
    let stored = store
        .credential_mutation("m-revoke")
        .await
        .unwrap()
        .expect("the revocation mutation must be readable");
    assert_eq!(stored.phase, MutationPhase::CleanupPending);
    assert_eq!(stored.outcome, Some(MutationOutcome::Revoked { revision }));
    assert_eq!(stored.decided_revision, Some(revision));
    // Cleanup completes the mutation without moving the (cleared) reference.
    store
        .mark_credential_cleaned("m-revoke", "acme", "main", first)
        .await
        .unwrap();
    let cleaned = store
        .active_credential_version("acme", "main")
        .await
        .unwrap();
    assert_eq!(cleaned.active, None);
    assert_eq!(cleaned.cleanup, None);
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
    assert_eq!(active.active, None);
    assert_eq!(active.cleanup, None);
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
        active.active,
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
    assert_eq!(active.active, Some(first));
}
