//! Targeted Deletion A3a: the Companion and Learning local-erasure
//! participants over the canonical store.
//!
//! Each test drives a participant directly through bounded demands, so the
//! sweep/continuation contract is observed without the Host fan-out, and
//! checks the mechanical remainder probe (shared with the production column
//! list) instead of re-stating which columns carry content.

use super::*;

use ene_companion::{ActivityId, RecordResumeActivityCommand};
use ene_preservation::{
    DeletionPurpose, DeletionSearchMaterial, DemandLocalErasureCommand, ErasureConditionRef,
    MechanicalDeletionTarget, ParticipantCompletionFact, ParticipantCompletionStatus,
    ParticipantErasureScope, ParticipantOwnerRef, PreservationRepository as _,
    StartTargetedDeletionCommand, StartTargetedDeletionOutcome, TargetedDeletionTarget,
};
use ene_task::{TaskId, TaskPurposeRef, TaskRef, TaskRevision};

use crate::erasure::exact_remainder_probe;
use crate::{CompanionErasureParticipant, ERASURE_SCAN_ROWS, LearningErasureParticipant};

async fn admit_owner(store: &Store, text: &str, owner: ParticipantOwnerRef) -> ErasureConditionRef {
    admit_owner_with_sources(store, text, owner, Vec::new()).await
}

async fn admit_owner_with_sources(
    store: &Store,
    text: &str,
    owner: ParticipantOwnerRef,
    sources: Vec<RawId>,
) -> ErasureConditionRef {
    match store
        .start_targeted_deletion(
            StartTargetedDeletionCommand::new(
                TargetedDeletionTarget {
                    mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                        text.to_owned(),
                    )),
                    semantic_hints: Vec::new(),
                },
                DeletionPurpose::Privacy,
                fixture_clock(),
                sources,
                vec![owner],
            )
            .confirmed_for_tests(),
        )
        .await
        .expect("the sweep fixture must admit")
    {
        StartTargetedDeletionOutcome::Started(current) => current.condition(),
        other => panic!("the sweep fixture must start, got {other:?}"),
    }
}

fn local_command(
    condition: ErasureConditionRef,
    owner: ParticipantOwnerRef,
    text: &str,
    sources: Vec<RawId>,
) -> DemandLocalErasureCommand {
    DemandLocalErasureCommand::new(
        condition,
        owner,
        ParticipantErasureScope::local(
            TargetedDeletionTarget {
                mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                    text.to_owned(),
                )),
                semantic_hints: Vec::new(),
            },
            sources,
        ),
    )
}

/// Drives one participant until it reports verified or held, so a test never
/// depends on how many bounded pages one sweep needs.
async fn drive(
    participant: &impl ene_preservation::ErasureParticipant,
    condition: ErasureConditionRef,
    owner: ParticipantOwnerRef,
    text: &str,
) -> ParticipantCompletionFact {
    drive_with_sources(participant, condition, owner, text, Vec::new()).await
}

pub(super) async fn drive_with_sources(
    participant: &impl ene_preservation::ErasureParticipant,
    condition: ErasureConditionRef,
    owner: ParticipantOwnerRef,
    text: &str,
    sources: Vec<RawId>,
) -> ParticipantCompletionFact {
    for _ in 0..ERASURE_SCAN_ROWS * 8 {
        let fact = participant
            .demand_local_erasure(local_command(condition, owner, text, sources.clone()))
            .await;
        if matches!(
            fact.status(),
            ParticipantCompletionStatus::Verified | ParticipantCompletionStatus::Held(_)
        ) {
            return fact;
        }
    }
    panic!("a bounded sweep must finish within the drive budget");
}

fn remainder(store: &Store, text: &str) -> u64 {
    let guard = crate::codec::lock_shared(&store.conn);
    exact_remainder_probe(&guard, text).unwrap()
}

async fn append_role(
    store: &Store,
    companion: CompanionId,
    generation: PresenceGeneration,
    role: HistoryRole,
    text: &str,
) -> RawId {
    let mut cmd = history_command(companion, generation, text);
    cmd.role = role;
    match store.append_message(cmd).await.unwrap() {
        HistoryAppendOutcome::CommittedAs { message } => message,
        other => panic!("the append must commit: {other:?}"),
    }
}

async fn record_activity(store: &Store, companion: CompanionId, body: &str) -> ActivityId {
    let task = TaskId::generate();
    record_activity_id(
        store,
        RecordResumeActivityCommand {
            companion,
            task: TaskRef {
                task,
                revision: TaskRevision::initial(),
            },
            purpose: TaskPurposeRef {
                task,
                adopted_revision: TaskRevision::initial(),
            },
            body: body.to_owned(),
            command: RawId::new(),
        },
    )
    .await
    .unwrap()
}

fn summary(companion: RawId, content: &str, start: RawId, end: RawId) -> SummaryRecord {
    SummaryRecord {
        id: SummaryId::generate(),
        scope: LearningScope::companion(companion),
        content: content.to_owned(),
        source: SourceRangeRef {
            kind: ExperienceSourceKind::Dialogue,
            start,
            end,
        },
        formed_at: fixture_clock(),
    }
}

fn change(
    summary: &SummaryRecord,
    target: MemoryTarget,
    content: &str,
    kind: ChangeKind,
) -> MemoryChangeCommit {
    MemoryChangeCommit {
        summary: Some(summary.clone()),
        secret_premise: None,
        claim: None,
        change: MemoryChange {
            target,
            scope: LearningScope::companion(summary.scope.companion_id()),
            content: content.to_owned(),
            importance: Importance::default(),
            temporal: TemporalMeaning::Enduring,
            change: kind,
            recall_suppressed: false,
            at: fixture_clock(),
        },
    }
}

async fn commit(store: &Store, commit: MemoryChangeCommit) -> MemoryChangeOutcome {
    store.commit_memory_change(commit).await.unwrap()
}

#[tokio::test]
async fn companion_erasure_removes_exact_bodies_and_dangling_references() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "swordfish-c0de";

    let tainted_owner = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        &format!("my launch code is {target}"),
    )
    .await;
    let clean_owner = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        "the weather is nice today",
    )
    .await;
    // A target-bearing reply that also registered an undelivered reference:
    // the reference must not survive its erased source.
    let mut reply = history_command(companion, generation, &format!("noted: {target}"));
    reply.role = HistoryRole::Companion;
    let (outcome, registered) = store
        .append_reply_with_undelivered(reply, true, None)
        .await
        .unwrap();
    let HistoryAppendOutcome::CommittedAs {
        message: tainted_reply,
    } = outcome
    else {
        panic!("the reply must commit: {outcome:?}");
    };
    assert!(
        registered.is_some(),
        "the reply registers an undelivered row"
    );
    let tainted_activity = record_activity(&store, companion, &format!("remember {target}")).await;
    let clean_activity = record_activity(&store, companion, "unrelated activity note").await;

    // Positive control: the probe sees exactly the target-bearing content
    // before the sweep.
    assert_eq!(remainder(&store, target), 3);

    let participant = CompanionErasureParticipant::new(store.clone());
    let condition = admit_owner(&store, target, ParticipantOwnerRef::Companion).await;
    let fact = drive(
        &participant,
        condition,
        ParticipantOwnerRef::Companion,
        target,
    )
    .await;
    assert_eq!(fact.status(), ParticipantCompletionStatus::Verified);
    assert!(
        fact.erased_count() >= 4,
        "history, reply, activity, and the dangling reference"
    );
    assert_eq!(fact.remainder_count(), 0);

    assert!(store.load_message(tainted_owner).await.unwrap().is_none());
    assert!(store.load_message(tainted_reply).await.unwrap().is_none());
    assert!(
        store
            .load_activity(tainted_activity)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .list_unpresented(companion, None, 50)
            .await
            .unwrap()
            .entries
            .is_empty(),
        "a reference whose source is erased is erased with it"
    );
    // Unrelated rows survive the sweep.
    assert!(store.load_message(clean_owner).await.unwrap().is_some());
    assert!(store.load_activity(clean_activity).await.unwrap().is_some());
    assert_eq!(remainder(&store, target), 0);
}

#[tokio::test]
async fn learning_erasure_removes_every_revision_and_the_derived_index() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "swordfish-memory";
    let source_start = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        &format!("the code is {target}"),
    )
    .await;
    let source_end = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Companion,
        "understood",
    )
    .await;

    let tainted_summary = summary(
        companion.as_raw(),
        &format!("The owner shared a launch code: {target}."),
        source_start,
        source_end,
    );
    let memory = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &tainted_summary,
                MemoryTarget::New { id: memory },
                &format!("owner code is {target}"),
                ChangeKind::Initial,
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));
    assert!(matches!(
        commit(
            &store,
            change(
                &tainted_summary,
                MemoryTarget::Existing {
                    id: memory,
                    expected_revision: MemoryRevision::initial(),
                },
                &format!("owner code remains {target}"),
                ChangeKind::Refined,
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));

    // Positive control: an unrelated formation stays readable and recallable.
    let clean_summary = summary(
        companion.as_raw(),
        "The owner likes jasmine tea.",
        RawId::new(),
        RawId::new(),
    );
    let clean_memory = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &clean_summary,
                MemoryTarget::New { id: clean_memory },
                "The owner likes jasmine tea.",
                ChangeKind::Initial,
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));

    assert!(remainder(&store, target) >= 4);

    let participant = LearningErasureParticipant::new(store.clone());
    let condition = admit_owner(&store, target, ParticipantOwnerRef::Learning).await;
    let fact = drive(
        &participant,
        condition,
        ParticipantOwnerRef::Learning,
        target,
    )
    .await;
    assert_eq!(fact.status(), ParticipantCompletionStatus::Verified);
    assert!(
        fact.erased_count() >= 4,
        "summary, memory, and both revisions"
    );
    assert_eq!(fact.remainder_count(), 0);

    assert!(
        store
            .list_current_memories(companion.as_raw(), None, 100)
            .await
            .unwrap()
            .iter()
            .all(|memory| memory.id == clean_memory),
        "only the unrelated memory stays reachable"
    );
    assert!(
        store
            .list_memory_revisions(memory, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    let summaries = store
        .load_summaries(&[tainted_summary.id, clean_summary.id])
        .await
        .unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, clean_summary.id);
    let recalled = store
        .recall_candidates(companion.as_raw(), &[String::from("swordfish")], 50)
        .await
        .unwrap();
    assert!(
        recalled
            .iter()
            .all(|memory| !memory.content.contains(target)),
        "the derived index must not answer with an erased recognition"
    );
    // The History source carrying the target is the Companion participant's
    // scope; both participants together must leave no exact remainder. The
    // same current condition is reused: a second admission of the same text
    // would be HeldByOperation while the first is unfinished.
    drive(
        &CompanionErasureParticipant::new(store.clone()),
        condition,
        ParticipantOwnerRef::Companion,
        target,
    )
    .await;
    assert_eq!(remainder(&store, target), 0);
}

/// Reads the raw database file and reports whether `needle` appears
/// verbatim. SQLite stores TEXT as UTF-8 bytes, so an erased body that only
/// survives in freed pages is visible here even when every SQL read is clean.
fn raw_file_contains(path: &std::path::Path, needle: &str) -> bool {
    let bytes = std::fs::read(path).expect("the database file must be readable");
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[tokio::test]
async fn erased_body_bytes_do_not_survive_in_the_raw_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secure-delete.db");
    let store = Store::open(&path).await.unwrap();
    let companion = store.ensure_running_companion().await.unwrap();
    let generation = store
        .load_attribution(companion.as_raw())
        .await
        .unwrap()
        .unwrap()
        .generation;
    let canary = "raw-canary-secure-delete-0f9c1";
    append_role(&store, companion, generation, HistoryRole::Owner, canary).await;
    assert!(
        raw_file_contains(&path, canary),
        "positive control: the committed body must be in the file before erasure"
    );
    {
        let guard = crate::codec::lock_shared(&store.conn);
        let enabled: i64 = guard
            .query_row("PRAGMA secure_delete", (), |row| row.get(0))
            .unwrap();
        assert_eq!(enabled, 1, "every store connection raises secure_delete");
    }

    let participant = CompanionErasureParticipant::new(store.clone());
    let condition = admit_owner(&store, canary, ParticipantOwnerRef::Companion).await;
    let fact = drive(
        &participant,
        condition,
        ParticipantOwnerRef::Companion,
        canary,
    )
    .await;
    assert_eq!(
        fact.status(),
        ParticipantCompletionStatus::Verified,
        "the bounded erase pass must verify the canary is gone"
    );
    super::preservation::complete_via_a5(
        &store,
        ene_preservation::DeletionOperationRef {
            operation: condition.operation,
            sweep: condition.sweep,
        },
    )
    .await;
    assert!(
        !raw_file_contains(&path, canary),
        "an erased body must not survive in the raw database file"
    );
}
