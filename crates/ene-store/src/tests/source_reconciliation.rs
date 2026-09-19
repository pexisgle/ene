//! Stage 6 M3: exhaustive, bounded covered-source reconciliation and the
//! already-claimed in-flight-use correspondence (lifecycle §4.1 point 4,
//! §11 R2, §12, §18).
//!
//! The admission page is a work bound only. These tests pin the correctness
//! side of the split: the durable cursor walks every covered identity however
//! many there are, an already-claimed use whose source falls past the first
//! page is still associated, entering `Finalizing` refuses while the walk is
//! incomplete, a new sweep resets the walk, a crash resumes from the durable
//! cursor, a retried page adds no second effect, and a fresh origin after
//! completion is never covered by the closed operation.

use super::*;

use ene_learning::LearningClaimRef;
use ene_preservation::{
    ConfirmTargetedDeletionOutcome, DELETION_RECONCILIATION_PAGE_SIZE, DeletionFinalizationOutcome,
    DeletionLifecycleChange, DeletionLifecycleOutcome, DeletionOperationRef, DeletionPurpose,
    DeletionReconciliationOutcome, DeletionSearchMaterial, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantCompletionOutcome, ParticipantCompletionStatus,
    ParticipantOwnerRef, PreservationRepository as _, StageTargetedDeletionRequestCommand,
    StageTargetedDeletionRequestOutcome, TargetedDeletionTarget,
};

use crate::CompanionErasureParticipant;
use crate::tests::targeted_deletion::drive_with_sources;

/// One covered identity per fixture row, strictly more than one
/// reconciliation page so the admission page cannot be the whole set.
const FIXTURE_SOURCES: usize = 70;

fn target_identity_body(target: &str) -> String {
    format!("fixture body carrying {target}")
}

/// Seeds [`FIXTURE_SOURCES`] target-bearing History identities through
/// fixture SQL and returns their encoded primary keys in canonical order.
///
/// The rows are durable identity rows, exactly what the production
/// enumeration walks; only the writer is a fixture because these tests pin
/// the walk, not the append path.
fn seed_covered_identities(store: &Store, target: &str) -> Vec<String> {
    let guard = store.conn.lock().unwrap();
    let mut keys = Vec::with_capacity(FIXTURE_SOURCES);
    for _ in 0..FIXTURE_SOURCES {
        let message = RawId::new();
        guard
            .execute(
                "INSERT INTO history_message (message_id,companion_id,round_id,role,body,lang,at,presence_generation) VALUES (?1,?2,?3,'owner',?4,'en',?5,1)",
                params![
                    crate::codec::encode_id(message),
                    crate::codec::encode_id(RawId::new()),
                    crate::codec::encode_id(RawId::new()),
                    target_identity_body(target),
                    fixture_clock().to_rfc3339()
                ],
            )
            .unwrap();
        keys.push(crate::codec::encode_id(message));
    }
    keys.sort();
    keys
}

fn decode(encoded: &str) -> RawId {
    crate::codec::decode_id(encoded).expect("the fixture key decodes")
}

/// Stages and confirms one first-party deletion operation through the
/// production request/confirmation path.
async fn admit_first_party(
    store: &Store,
    target: &str,
    participants: Vec<ParticipantOwnerRef>,
) -> DeletionOperationRef {
    let staged = store
        .stage_targeted_deletion(StageTargetedDeletionRequestCommand::new(
            TargetedDeletionTarget {
                mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                    target.to_owned(),
                )),
                semantic_hints: Vec::new(),
            },
            DeletionPurpose::Privacy,
            WallClockWithTz::now(),
        ))
        .await
        .expect("staging must answer");
    let request = match staged {
        StageTargetedDeletionRequestOutcome::Staged(request) => request,
        other => panic!("the scope must stage, got {other:?}"),
    };
    match store
        .confirm_targeted_deletion(request, participants)
        .await
        .expect("the confirmation must answer")
    {
        ConfirmTargetedDeletionOutcome::Started(current) => current,
        other => panic!("the confirmation must start, got {other:?}"),
    }
}

/// Seeds the Learning consent the formation claim needs; one durable record.
async fn seed_learning_consent(store: &Store) {
    let saved = save_consent(
        store,
        None,
        ConsentRecord {
            capability: CapabilityKind::Learning,
            id: String::from("consent-learning"),
            rev: ConsentRevision::from_u64(1),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            credential_id: String::from("cred-1"),
        },
    )
    .await;
    assert!(
        matches!(saved, ConsentCommitOutcome::Committed { .. }),
        "the Learning consent must seed, got {saved:?}"
    );
}

/// One Learning formation claim committed through the production inference
/// boundary, carrying the ordered source correlation the formation read.
async fn claim_formation(store: &Store, data_use: Vec<RawId>) -> InferenceTicketId {
    let ticket = InferenceTicketId(RawId::new());
    let outcome = store
        .begin_inference_attempt(InferenceAttempt {
            ticket,
            consumer: ConsumerKind::CompanionLearning,
            capability: CapabilityKind::Learning,
            purpose: PurposeKind::MemoryFormation,
            expected_consent: (
                String::from("consent-learning"),
                ConsentRevision::from_u64(1),
            ),
            expected_credential_set: CredentialSetRevision::initial(),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            task_agent: None,
            data_use,
            pricing: None,
            usage_estimate: None,
        })
        .await
        .expect("the formation claim must answer");
    assert_eq!(
        outcome,
        AttemptBeginOutcome::Started,
        "the pre-condition claim must start"
    );
    ticket
}

/// One delayed Learning formation carrying the claim handle, with clean
/// evidence and content so only the claim correspondence can refuse it.
fn delayed_formation(companion: RawId, claim: InferenceTicketId) -> MemoryChangeCommit {
    let source = RawId::new();
    let summary = SummaryRecord {
        id: SummaryId::generate(),
        scope: LearningScope::companion(companion),
        content: String::from("a clean paraphrase"),
        source: SourceRangeRef {
            kind: ExperienceSourceKind::Dialogue,
            start: source,
            end: source,
        },
        formed_at: fixture_clock(),
    };
    MemoryChangeCommit {
        summary: Some(summary),
        secret_premise: None,
        claim: Some(LearningClaimRef::from_raw(claim.0)),
        change: MemoryChange {
            target: MemoryTarget::New {
                id: MemoryId::generate(),
            },
            scope: LearningScope::companion(companion),
            content: String::from("a clean recall"),
            importance: Importance::default(),
            temporal: TemporalMeaning::Enduring,
            change: ChangeKind::Initial,
            recall_suppressed: false,
            at: fixture_clock(),
        },
    }
}

/// Drives the bounded reconciliation of one operation's current sweep to
/// completion through the canonical API.
async fn reconcile_to_complete(store: &Store, current: DeletionOperationRef) -> u32 {
    let mut steps = 0;
    loop {
        steps += 1;
        assert!(steps < 64, "the walk must finish inside the fixture budget");
        match store
            .reconcile_deletion_sources(current, DELETION_RECONCILIATION_PAGE_SIZE)
            .await
            .expect("a reconciliation step must answer")
        {
            DeletionReconciliationOutcome::Advanced => {}
            DeletionReconciliationOutcome::Complete => return steps,
            other => panic!("unexpected reconciliation step: {other:?}"),
        }
    }
}

/// Records every required participant `Verified` for the current sweep
/// without completing the operation.
async fn verify_all(store: &Store, current: DeletionOperationRef) {
    let mut after = None;
    loop {
        let page = store
            .deletion_participants(current.operation, after, 100)
            .await
            .expect("the required snapshot must read");
        if page.is_empty() {
            break;
        }
        let page_len = page.len();
        for record in page {
            after = Some(record.participant.owner);
            let outcome = store
                .record_participant_completion(ParticipantCompletionFact::verified(
                    current.condition(),
                    record.participant.owner,
                    0,
                    WallClockWithTz::now(),
                ))
                .await
                .expect("a verified fact must record");
            assert!(
                matches!(outcome, ParticipantCompletionOutcome::Recorded(_)),
                "the fixture fact must apply: {outcome:?}"
            );
        }
        if page_len < 100 {
            break;
        }
    }
}

/// One operation's stored source correlations in canonical order.
fn source_rows(store: &Store, current: DeletionOperationRef) -> Vec<String> {
    let guard = store.conn.lock().unwrap();
    let mut statement = guard
        .prepare(
            "SELECT source FROM erasure_condition_source WHERE operation_id=?1 AND sweep=?2 ORDER BY source",
        )
        .unwrap();
    statement
        .query_map(
            params![
                crate::codec::encode_id(current.operation.as_raw()),
                current.sweep.as_u64() as i64
            ],
            |row| row.get(0),
        )
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

/// The durable already-claimed-use correspondence of one operation, in key
/// order.
fn hold_rows(store: &Store, current: DeletionOperationRef) -> Vec<String> {
    let guard = store.conn.lock().unwrap();
    let mut statement = guard
        .prepare(
            "SELECT use_kind || ':' || use_id FROM erasure_use_hold WHERE operation_id=?1 ORDER BY use_kind, use_id",
        )
        .unwrap();
    statement
        .query_map(
            [crate::codec::encode_id(current.operation.as_raw())],
            |row| row.get(0),
        )
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn expected_hold(claim: InferenceTicketId) -> String {
    format!("inference_attempt:{}", crate::codec::encode_id(claim.0))
}

/// The incomplete reconciliation rows of one operation's current sweep.
fn incomplete_tables(store: &Store, current: DeletionOperationRef) -> Vec<String> {
    let guard = store.conn.lock().unwrap();
    let mut statement = guard
        .prepare(
            "SELECT identity_table FROM deletion_reconciliation WHERE operation_id=?1 AND sweep=?2 AND complete=0 ORDER BY identity_table",
        )
        .unwrap();
    statement
        .query_map(
            params![
                crate::codec::encode_id(current.operation.as_raw()),
                current.sweep.as_u64() as i64
            ],
            |row| row.get(0),
        )
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

/// The regression this slice exists for: 70 covered identities, the last one
/// used by an already-committed formation claim. Admission publishes one
/// bounded page; entering `Finalizing` is refused until the durable cursor
/// walks the rest; the walk associates the claim; the delayed formation
/// arriving after completion is refused, and a fresh origin after completion
/// is accepted.
#[tokio::test]
async fn the_last_of_many_covered_sources_still_holds_its_claimed_use_across_completion() {
    let store = open_memory().await.unwrap();
    let target = "m3-canary-beyond-the-admission-page";
    let companion = RawId::new();
    let keys = seed_covered_identities(&store, target);
    assert_eq!(keys.len(), FIXTURE_SOURCES);
    let page = DELETION_RECONCILIATION_PAGE_SIZE as usize;
    assert!(FIXTURE_SOURCES > page, "the fixture must exceed one page");
    let last = keys.last().cloned().expect("the fixture has a last key");

    // R2 use-first: the formation's provider claim (with its ordered source
    // correlation) commits before the deletion condition exists.
    seed_learning_consent(&store).await;
    let claim = claim_formation(&store, vec![decode(&last)]).await;

    let current = admit_first_party(&store, target, vec![ParticipantOwnerRef::Companion]).await;
    // Admission published exactly one bounded page, and the last identity is
    // beyond it; the publication is not the correctness set.
    let published = source_rows(&store, current);
    assert_eq!(published, keys[..page].to_vec());
    assert!(!published.contains(&last));
    assert!(
        crate::preservation::covering_condition(&store.conn.lock().unwrap(), &last)
            .unwrap()
            .is_some(),
        "an unpublished covered identity is still mechanically covered"
    );

    // The completion premise is explicit and fail-closed: participant
    // verification alone never enters Finalizing while the walk is
    // incomplete.
    verify_all(&store, current).await;
    assert_eq!(
        store
            .begin_deletion_finalizing(current)
            .await
            .expect("the finalizing transition must answer"),
        DeletionFinalizationOutcome::ReconciliationIncomplete,
        "the completion candidate must refuse while the walk is incomplete"
    );

    // The durable cursor finishes the walk; the claim is associated with the
    // operation when its source's page commits.
    let steps = reconcile_to_complete(&store, current).await;
    assert!(steps >= 1, "the walk took at least one continuation page");
    assert_eq!(source_rows(&store, current), keys);
    assert_eq!(
        hold_rows(&store, current),
        vec![expected_hold(claim)],
        "the claim whose source fell past the admission page is durably held"
    );

    // The owner sweep collects the 70 identities, then the operation
    // completes through the sealed boundary.
    let participant = CompanionErasureParticipant::new(store.clone());
    let fact = drive_with_sources(
        &participant,
        current.condition(),
        ParticipantOwnerRef::Companion,
        target,
        Vec::new(),
    )
    .await;
    assert!(matches!(
        fact.status(),
        ParticipantCompletionStatus::Verified
    ));
    assert_eq!(
        store
            .begin_deletion_finalizing(current)
            .await
            .expect("the finalizing transition must answer"),
        DeletionFinalizationOutcome::Finalizing
    );
    assert_eq!(
        store
            .complete_deletion_finalizing(current)
            .await
            .expect("the completion commit must answer"),
        DeletionFinalizationOutcome::Completed
    );
    assert_eq!(
        crate::erasure::exact_remainder_probe(&crate::codec::lock_shared(&store.conn), target)
            .unwrap(),
        0,
        "no durable surface keeps the exact target"
    );

    // The delayed result arrives after completion: the durable correspondence
    // refuses it even though no current condition is readable.
    assert_eq!(
        store
            .commit_memory_change(delayed_formation(companion, claim))
            .await
            .expect("the delayed commit must answer"),
        MemoryChangeOutcome::HeldForErasure,
        "a formation claimed before the interval stays stale after completion"
    );

    // The completed operation is not a permanent ban: a fresh claim from a
    // fresh source after completion is a new origin.
    let fresh = claim_formation(&store, vec![RawId::new()]).await;
    assert!(
        matches!(
            store
                .commit_memory_change(delayed_formation(companion, fresh))
                .await
                .expect("the fresh commit must answer"),
            MemoryChangeOutcome::Committed { .. }
        ),
        "a post-completion origin is accepted"
    );
}

/// A delayed arrival that reaches its acceptance boundary while the walk is
/// still incomplete is associated directly from its own correlation, so it
/// cannot escape the correspondence.
#[tokio::test]
async fn an_arrival_during_the_walk_is_held_by_its_own_correlation() {
    let store = open_memory().await.unwrap();
    let target = "m3-window-canary";
    let companion = RawId::new();
    let keys = seed_covered_identities(&store, target);
    let last = keys.last().cloned().expect("the fixture has a last key");
    seed_learning_consent(&store).await;
    let claim = claim_formation(&store, vec![decode(&last)]).await;
    let current = admit_first_party(&store, target, vec![ParticipantOwnerRef::Companion]).await;
    assert_eq!(incomplete_tables(&store, current).len(), 1);

    // The arrival commits while reconciliation is incomplete: no hold row
    // exists yet, so the claim's own ordered correlation is compared directly
    // against the operation's protected target.
    assert_eq!(
        store
            .commit_memory_change(delayed_formation(companion, claim))
            .await
            .expect("the arrival must answer"),
        MemoryChangeOutcome::HeldForErasure,
        "the arrival window is closed by the direct mechanical check"
    );
    assert_eq!(
        hold_rows(&store, current),
        vec![expected_hold(claim)],
        "the direct check writes the same durable correspondence"
    );
}

/// A crash between pages resumes from the durable cursor: no page is replayed
/// from memory and no generation is invented.
#[tokio::test]
async fn reconciliation_resumes_from_the_durable_cursor_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m3-restart.db");
    let store = Store::open(&path).await.unwrap();
    let target = "m3-restart-canary";
    let keys = seed_covered_identities(&store, target);
    let last = keys.last().cloned().expect("the fixture has a last key");
    seed_learning_consent(&store).await;
    let claim = claim_formation(&store, vec![decode(&last)]).await;
    let current = admit_first_party(&store, target, vec![ParticipantOwnerRef::Companion]).await;
    let page = DELETION_RECONCILIATION_PAGE_SIZE as usize;
    assert_eq!(source_rows(&store, current), keys[..page].to_vec());
    assert_eq!(incomplete_tables(&store, current).len(), 1);
    drop(store);

    let reopened = Store::open(&path).await.unwrap();
    assert_eq!(
        source_rows(&reopened, current),
        keys[..page].to_vec(),
        "the admission page is durable"
    );
    let steps = reconcile_to_complete(&reopened, current).await;
    assert!(steps >= 1);
    assert_eq!(source_rows(&reopened, current), keys);
    assert_eq!(
        hold_rows(&reopened, current),
        vec![expected_hold(claim)],
        "the resumed walk associates the claim"
    );
}

/// A retried page is idempotent: rewinding the durable cursor and re-running
/// the same page adds no source row and no second hold.
#[tokio::test]
async fn a_retried_reconciliation_page_is_idempotent() {
    let store = open_memory().await.unwrap();
    let target = "m3-idempotent-canary";
    let keys = seed_covered_identities(&store, target);
    let last = keys.last().cloned().expect("the fixture has a last key");
    seed_learning_consent(&store).await;
    let claim = claim_formation(&store, vec![decode(&last)]).await;
    let current = admit_first_party(&store, target, vec![ParticipantOwnerRef::Companion]).await;
    let id = crate::codec::encode_id(current.operation.as_raw());
    let rewind = |store: &Store| {
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE deletion_reconciliation SET cursor='',complete=0 WHERE operation_id=?1 AND sweep=?2 AND identity_table='history_message'",
                params![id, current.sweep.as_u64() as i64],
            )
            .unwrap();
    };
    let steps = reconcile_to_complete(&store, current).await;
    assert!(steps >= 1);
    assert_eq!(source_rows(&store, current), keys);
    assert_eq!(hold_rows(&store, current), vec![expected_hold(claim)]);
    // Rewinding and replaying both pages changes nothing: the source rows are
    // keyed and the holds are keyed.
    rewind(&store);
    let _ = reconcile_to_complete(&store, current).await;
    assert_eq!(source_rows(&store, current), keys);
    assert_eq!(hold_rows(&store, current), vec![expected_hold(claim)]);
}

/// A new sweep generation resets the walk: the completion premise is refused
/// again until the reset walk finishes, and the reset walk re-publishes the
/// inherited correlations.
#[tokio::test]
async fn a_new_sweep_resets_reconciliation_and_refuses_completion() {
    let store = open_memory().await.unwrap();
    let target = "m3-reset-canary";
    let keys = seed_covered_identities(&store, target);
    let current = admit_first_party(&store, target, vec![ParticipantOwnerRef::Companion]).await;
    reconcile_to_complete(&store, current).await;
    assert!(incomplete_tables(&store, current).is_empty());

    let DeletionLifecycleOutcome::Applied(next) = store
        .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
        .await
        .expect("the sweep advance must answer")
    else {
        panic!("the generation must advance");
    };
    assert_eq!(
        incomplete_tables(&store, next).len(),
        crate::preservation::KNOWN_SOURCE_IDENTITIES.len(),
        "every identity table restarts its walk for the new sweep"
    );
    assert_eq!(
        source_rows(&store, next),
        keys,
        "NextSweep copies the cumulative correlations forward"
    );
    verify_all(&store, next).await;
    assert_eq!(
        store
            .begin_deletion_finalizing(next)
            .await
            .expect("the finalizing transition must answer"),
        DeletionFinalizationOutcome::ReconciliationIncomplete,
        "the reset walk must complete before the new sweep can finalize"
    );
    reconcile_to_complete(&store, next).await;
    assert!(
        matches!(
            store
                .begin_deletion_finalizing(next)
                .await
                .expect("the finalizing transition must answer"),
            DeletionFinalizationOutcome::RemainderCollected(_)
        ),
        "the unerased bodies are a remainder, not a completion"
    );
}
