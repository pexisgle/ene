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

/// A stale premise abandons the candidate without adopting it, and the refused
/// attempt is durable: a retry observes the same decision.
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
