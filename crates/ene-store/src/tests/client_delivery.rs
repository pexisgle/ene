//! Durable Client body-delivery evidence: the create/advance, the
//! compare-and-delete against a concurrently advanced sequence, the bounded
//! identity page, and persistence across a real reopen (lifecycle §8.1).
//!
//! The fixture uses two bare `RawId` identities, the same Host-minted
//! `(counter, random)` projection the Host composition derives, so the tests
//! exercise exactly the stored form without depending on `ene-core`.

use super::*;

/// The Host composition projects the boot pair through
/// `Uuid::from_u64_pair`; the stored form is the canonical UUID text, so a
/// deterministic text fixture exercises exactly the same durable shape.
fn incarnation(counter: u64, random: u64) -> RawId {
    let text = format!("{counter:08x}-{random:04x}-0000-0000-000000000000");
    RawId::from_uuid(text.parse().expect("the fixture identity parses"))
}

#[tokio::test]
async fn delivery_creates_advances_and_persists_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("client-delivery.db");
    let store = Store::open(&path).await.unwrap();
    let first = incarnation(7, 11);
    let second = incarnation(8, 12);
    assert_eq!(
        store.client_delivery_evidence_seq(first).await.unwrap(),
        None,
        "an unknown incarnation has no evidence"
    );
    store.note_client_delivery_evidence(first).await.unwrap();
    store.note_client_delivery_evidence(second).await.unwrap();
    store.note_client_delivery_evidence(first).await.unwrap();
    assert_eq!(
        store.client_delivery_evidence_seq(first).await.unwrap(),
        Some(2),
        "a repeat delivery advances the sequence instead of minting a row"
    );
    assert_eq!(
        store.client_delivery_evidence_seq(second).await.unwrap(),
        Some(1)
    );
    assert_eq!(
        store
            .client_delivery_evidence_incarnations(None, 100)
            .await
            .unwrap(),
        vec![first, second],
        "the page is ordered by canonical identity"
    );
    drop(store);

    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(
        reopened.client_delivery_evidence_seq(first).await.unwrap(),
        Some(2),
        "a reopen (Host restart) never clears the evidence"
    );
    assert_eq!(
        reopened.client_delivery_evidence_seq(second).await.unwrap(),
        Some(1)
    );
}

#[tokio::test]
async fn clear_matches_the_observed_sequence_exactly() {
    let store = open_memory().await.unwrap();
    let identity = incarnation(21, 31);
    store.note_client_delivery_evidence(identity).await.unwrap();
    assert!(
        !store
            .clear_client_delivery_evidence(identity, 2)
            .await
            .unwrap(),
        "a stale expected sequence never clears the row"
    );
    assert_eq!(
        store.client_delivery_evidence_seq(identity).await.unwrap(),
        Some(1)
    );
    assert!(
        store
            .clear_client_delivery_evidence(identity, 1)
            .await
            .unwrap(),
        "the exact observed sequence clears the row"
    );
    assert_eq!(
        store.client_delivery_evidence_seq(identity).await.unwrap(),
        None
    );
    assert!(
        !store
            .clear_client_delivery_evidence(identity, 1)
            .await
            .unwrap(),
        "clearing an absent row changes nothing"
    );
}

#[tokio::test]
async fn a_delivery_between_read_and_clear_survives_the_clear() {
    let store = open_memory().await.unwrap();
    let identity = incarnation(22, 32);
    store.note_client_delivery_evidence(identity).await.unwrap();
    let observed = store
        .client_delivery_evidence_seq(identity)
        .await
        .unwrap()
        .expect("the delivery created evidence");
    // A body delivered after the wipe went on the wire advances the sequence.
    store.note_client_delivery_evidence(identity).await.unwrap();
    assert!(
        !store
            .clear_client_delivery_evidence(identity, observed)
            .await
            .unwrap(),
        "a racing delivery prevents the clear"
    );
    assert_eq!(
        store.client_delivery_evidence_seq(identity).await.unwrap(),
        Some(observed + 1),
        "the surviving row names the later delivery"
    );
}

#[tokio::test]
async fn admission_unions_durable_delivery_evidence_into_the_snapshot() {
    use ene_preservation::{
        DeletionPurpose, DeletionSearchMaterial, MechanicalDeletionTarget, ParticipantOwnerRef,
        PreservationRepository as _, StartTargetedDeletionCommand, StartTargetedDeletionOutcome,
        TargetedDeletionTarget,
    };

    let store = open_memory().await.unwrap();
    let delivered = incarnation(31, 41);
    let unrelated = ParticipantOwnerRef::HostTransient;
    // The caller's participant list was built before this delivery: the
    // authoritative snapshot read happens inside the admission transaction and
    // must still union the durable evidence (lifecycle §8/§8.1). Otherwise a
    // body handed over in that window could read as erased after completion.
    store
        .note_client_delivery_evidence(delivered)
        .await
        .unwrap();
    let outcome = store
        .start_targeted_deletion(
            StartTargetedDeletionCommand::new(
                TargetedDeletionTarget {
                    mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                        String::from("private target"),
                    )),
                    semantic_hints: Vec::new(),
                },
                DeletionPurpose::Privacy,
                WallClockWithTz::now(),
                Vec::new(),
                vec![unrelated],
            )
            .confirmed_for_tests(),
        )
        .await
        .unwrap();
    let StartTargetedDeletionOutcome::Started(current) = outcome else {
        panic!("admission must start, got {outcome:?}");
    };
    let owners: Vec<String> = {
        let conn = store.conn.lock().unwrap();
        let mut statement = conn
            .prepare("SELECT participant_owner FROM deletion_participant WHERE operation_id=?1")
            .unwrap();
        statement
            .query_map(
                [crate::codec::encode_id(current.operation.as_raw())],
                |row| row.get(0),
            )
            .unwrap()
            .collect::<Result<Vec<String>, _>>()
            .unwrap()
    };
    assert!(
        owners.contains(&ParticipantOwnerRef::ClientIncarnation(delivered).storage_name()),
        "the durable incarnation joins the snapshot even though the caller did not name it: {owners:?}"
    );
    assert!(owners.contains(&unrelated.storage_name()));
}

#[tokio::test]
async fn the_identity_page_is_bounded_and_validated() {
    let store = open_memory().await.unwrap();
    for counter in 0..3 {
        store
            .note_client_delivery_evidence(incarnation(counter, 5))
            .await
            .unwrap();
    }
    let page = store
        .client_delivery_evidence_incarnations(None, 2)
        .await
        .unwrap();
    assert_eq!(page.len(), 2);
    let next = store
        .client_delivery_evidence_incarnations(Some(page[1]), 2)
        .await
        .unwrap();
    assert_eq!(next.len(), 1, "the page continues after the last identity");
    assert_eq!(
        store
            .client_delivery_evidence_incarnations(None, 0)
            .await
            .unwrap_err(),
        ene_preservation::PreservationTechnicalError::InvalidLimit
    );
    assert_eq!(
        store
            .client_delivery_evidence_incarnations(None, 101)
            .await
            .unwrap_err(),
        ene_preservation::PreservationTechnicalError::InvalidLimit
    );
}
