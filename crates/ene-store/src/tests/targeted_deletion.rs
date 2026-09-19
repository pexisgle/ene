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
    DeletionLifecycleChange, DeletionLifecycleOutcome, DeletionOperationId, DeletionPurpose,
    DeletionSearchMaterial, DeletionSweepGeneration, DemandLocalErasureCommand,
    ErasureConditionRef, ErasureParticipant as _, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantCompletionStatus, ParticipantErasureScope,
    ParticipantHoldClass, ParticipantOwnerRef, PreservationRepository as _,
    StartTargetedDeletionCommand, StartTargetedDeletionOutcome, TargetedDeletionTarget,
};
use ene_task::{TaskId, TaskPurposeRef, TaskRef, TaskRevision};

use crate::erasure::exact_remainder_probe;
use crate::{CompanionErasureParticipant, ERASURE_SCAN_ROWS, LearningErasureParticipant};

fn condition(sweep: u64) -> ErasureConditionRef {
    ErasureConditionRef {
        operation: DeletionOperationId::from_raw(RawId::new()),
        sweep: DeletionSweepGeneration::from_u64(sweep),
    }
}

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

async fn next_sweep(store: &Store, condition: ErasureConditionRef) -> ErasureConditionRef {
    let current = ene_preservation::DeletionOperationRef {
        operation: condition.operation,
        sweep: condition.sweep,
    };
    match store
        .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
        .await
        .expect("the next sweep must apply")
    {
        DeletionLifecycleOutcome::Applied(next) => next.condition(),
        other => panic!("the next sweep must apply, got {other:?}"),
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

/// Inserts one History row without the production append gate. A current
/// condition would otherwise hold a target-bearing Owner append; NextSweep
/// cursor tests still need a late row to exist.
fn insert_history_bypassing_erasure(
    store: &Store,
    companion: CompanionId,
    generation: PresenceGeneration,
    body: &str,
) -> RawId {
    let message = RawId::new();
    let round = RawId::new();
    let at = fixture_clock();
    let guard = store.conn.lock().expect("store lock");
    guard
        .execute(
            "INSERT INTO history_message (message_id, companion_id, round_id, role, body, lang, at, at_utc, presence_generation, command_id, local_id, round_wire, round_intent, round_intent_ref, client_counter, client_random) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL, ?10, NULL, NULL, NULL, NULL)",
            rusqlite::params![
                crate::codec::encode_id(message),
                crate::codec::encode_id(companion.as_raw()),
                crate::codec::encode_id(round),
                "owner",
                body,
                "en",
                at.to_rfc3339(),
                at.to_rfc3339_utc(),
                i64::try_from(generation.as_u64()).expect("generation fits"),
                round.as_uuid().as_hyphenated().to_string(),
            ],
        )
        .expect("the unguarded History insert must commit");
    message
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

#[tokio::test]
async fn a_memory_grounded_only_on_erased_evidence_is_erased_with_it() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "swordfish-ground";
    let source = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        &format!("the code is {target}"),
    )
    .await;

    // The Summary carries the exact target; the Memory text is a paraphrase
    // without it, so only the recorded correspondence can reach the Memory.
    let tainted_summary = summary(
        companion.as_raw(),
        &format!("The owner keeps a private launch code ({target})."),
        source,
        source,
    );
    let memory = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &tainted_summary,
                MemoryTarget::New { id: memory },
                "The owner keeps a private launch credential.",
                ChangeKind::Initial,
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));

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
        store
            .list_memory_revisions(memory, None, 100)
            .await
            .unwrap()
            .is_empty(),
        "a revision whose evidence Summary is erased cannot remain"
    );
    assert!(
        store
            .list_current_memories(companion.as_raw(), None, 100)
            .await
            .unwrap()
            .is_empty(),
        "the current recognition is grounded only in erased evidence"
    );
    // Erasing the History source is the Companion participant's scope; the
    // two participants together leave no exact remainder. Reuse the current
    // condition: a second admission of the same text is HeldByOperation.
    drive(
        &CompanionErasureParticipant::new(store.clone()),
        condition,
        ParticipantOwnerRef::Companion,
        target,
    )
    .await;
    assert_eq!(remainder(&store, target), 0);
}

#[tokio::test]
async fn a_summary_pinning_a_covered_source_is_erased_with_it() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "swordfish-pin";
    // The captured window the formation was pinned to.
    let pinned = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        &format!("the code is {target}"),
    )
    .await;
    let clean_pin = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        "unrelated turn",
    )
    .await;

    // Both Summary texts paraphrase the target; only the recorded source pin
    // distinguishes them.
    let pinned_summary = summary(
        companion.as_raw(),
        "The owner keeps a private launch code.",
        pinned,
        pinned,
    );
    let clean_summary = summary(
        companion.as_raw(),
        "The owner keeps a private garden.",
        clean_pin,
        clean_pin,
    );
    let pinned_memory = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &pinned_summary,
                MemoryTarget::New { id: pinned_memory },
                "The owner keeps a private launch credential.",
                ChangeKind::Initial,
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));
    let clean_memory = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &clean_summary,
                MemoryTarget::New { id: clean_memory },
                "The owner keeps a private garden.",
                ChangeKind::Initial,
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));

    let participant = LearningErasureParticipant::new(store.clone());
    // The operation's covered sources name the pinned History turn: the
    // durable correlation A4 populates. The mechanical target text does not
    // occur in either Summary. Membership is the indexed table, not a
    // command-side copy of the sweep.
    let condition =
        admit_owner_with_sources(&store, target, ParticipantOwnerRef::Learning, vec![pinned]).await;
    let fact = drive(
        &participant,
        condition,
        ParticipantOwnerRef::Learning,
        target,
    )
    .await;
    assert_eq!(fact.status(), ParticipantCompletionStatus::Verified);

    let memories = store
        .list_current_memories(companion.as_raw(), None, 100)
        .await
        .unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].id, clean_memory);
    // Erasing the pinned History turn is the Companion participant's scope.
    drive(
        &CompanionErasureParticipant::new(store.clone()),
        condition,
        ParticipantOwnerRef::Companion,
        target,
    )
    .await;
    assert_eq!(remainder(&store, target), 0);
}

#[tokio::test]
async fn a_covered_source_correlates_a_summary_after_the_history_row_is_erased() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "swordfish-covered-after";
    let pinned = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        &format!("the code is {target}"),
    )
    .await;
    let clean_pin = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        "unrelated turn",
    )
    .await;
    let pinned_summary = summary(
        companion.as_raw(),
        "The owner keeps a private launch code.",
        pinned,
        pinned,
    );
    let clean_summary = summary(
        companion.as_raw(),
        "The owner keeps a private garden.",
        clean_pin,
        clean_pin,
    );
    let pinned_memory = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &pinned_summary,
                MemoryTarget::New { id: pinned_memory },
                "The owner keeps a private launch credential.",
                ChangeKind::Initial,
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));
    let clean_memory = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &clean_summary,
                MemoryTarget::New { id: clean_memory },
                "The owner keeps a private garden.",
                ChangeKind::Initial,
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));

    // The fan-out drives Companion before Learning, so the pinned turn is
    // already gone when the Learning sweep correlates. The operation's
    // covered source correlation is the durable link that still resolves.
    let sweep =
        admit_owner_with_sources(&store, target, ParticipantOwnerRef::Companion, vec![pinned])
            .await;
    let history = drive(
        &CompanionErasureParticipant::new(store.clone()),
        sweep,
        ParticipantOwnerRef::Companion,
        target,
    )
    .await;
    assert_eq!(history.status(), ParticipantCompletionStatus::Verified);
    assert!(store.load_message(pinned).await.unwrap().is_none());

    let learning = LearningErasureParticipant::new(store.clone());
    let fact = drive(&learning, sweep, ParticipantOwnerRef::Learning, target).await;
    assert_eq!(fact.status(), ParticipantCompletionStatus::Verified);
    let memories = store
        .list_current_memories(companion.as_raw(), None, 100)
        .await
        .unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].id, clean_memory);
    assert_eq!(remainder(&store, target), 0);
}

#[tokio::test]
async fn a_summary_pinning_a_target_bearing_message_is_erased_with_it() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "swordfish-live-pin";
    let pinned = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        &format!("the code is {target}"),
    )
    .await;
    let clean_pin = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        "unrelated turn",
    )
    .await;
    let pinned_summary = summary(
        companion.as_raw(),
        "The owner keeps a private launch code.",
        pinned,
        pinned,
    );
    let clean_summary = summary(
        companion.as_raw(),
        "The owner keeps a private garden.",
        clean_pin,
        clean_pin,
    );
    let pinned_memory = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &pinned_summary,
                MemoryTarget::New { id: pinned_memory },
                "The owner keeps a private launch credential.",
                ChangeKind::Initial,
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));
    let clean_memory = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &clean_summary,
                MemoryTarget::New { id: clean_memory },
                "The owner keeps a private garden.",
                ChangeKind::Initial,
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));

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
    let memories = store
        .list_current_memories(companion.as_raw(), None, 100)
        .await
        .unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].id, clean_memory);
}

#[tokio::test]
async fn a_multi_page_sweep_restarts_from_the_head_after_a_lost_cursor() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "swordfish-pages";
    let rows = ERASURE_SCAN_ROWS + 17;
    for index in 0..rows {
        append_role(
            &store,
            companion,
            generation,
            HistoryRole::Owner,
            &format!("{target} note {index}"),
        )
        .await;
    }
    append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        "an unrelated note",
    )
    .await;

    let participant = CompanionErasureParticipant::new(store.clone());
    let sweep = admit_owner(&store, target, ParticipantOwnerRef::Companion).await;
    let first = participant
        .demand_local_erasure(local_command(
            sweep,
            ParticipantOwnerRef::Companion,
            target,
            Vec::new(),
        ))
        .await;
    assert_eq!(first.status(), ParticipantCompletionStatus::MoreWork);

    // A restart loses the continuation cursor: the new participant re-walks
    // the sweep from its head and must not miss the rows the first page did
    // not reach.
    let restarted = CompanionErasureParticipant::new(store.clone());
    let fact = drive(&restarted, sweep, ParticipantOwnerRef::Companion, target).await;
    assert_eq!(fact.status(), ParticipantCompletionStatus::Verified);
    assert_eq!(remainder(&store, target), 0);
    assert_eq!(history_row_count(&store, companion).unwrap(), 1);
}

#[tokio::test]
async fn a_new_sweep_re_walks_instead_of_inheriting_a_verified_claim() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "swordfish-sweeps";

    // Sweep 1 verifies the empty store.
    let participant = CompanionErasureParticipant::new(store.clone());
    let first_condition = admit_owner(&store, target, ParticipantOwnerRef::Companion).await;
    let first = drive(
        &participant,
        first_condition,
        ParticipantOwnerRef::Companion,
        target,
    )
    .await;
    assert_eq!(first.status(), ParticipantCompletionStatus::Verified);

    // A delayed target-bearing row arrives after the sweep verified; a demand
    // for the next generation must walk from the head and erase it instead of
    // reporting the previous sweep's verified state. Production append would
    // hold while the condition is current, so the fixture writes the row
    // through the table the participant actually sweeps.
    insert_history_bypassing_erasure(
        &store,
        companion,
        generation,
        &format!("late arrival {target}"),
    );
    let second = drive(
        &participant,
        next_sweep(&store, first_condition).await,
        ParticipantOwnerRef::Companion,
        target,
    )
    .await;
    assert_eq!(second.status(), ParticipantCompletionStatus::Verified);
    assert!(second.erased_count() >= 1);
    assert_eq!(remainder(&store, target), 0);
}

#[tokio::test]
async fn a_repeat_demand_of_a_verified_sweep_erases_nothing_new() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "swordfish-repeat";
    append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        &format!("code {target}"),
    )
    .await;

    let participant = CompanionErasureParticipant::new(store.clone());
    let sweep = admit_owner(&store, target, ParticipantOwnerRef::Companion).await;
    let first = drive(&participant, sweep, ParticipantOwnerRef::Companion, target).await;
    assert_eq!(first.status(), ParticipantCompletionStatus::Verified);
    let erased = first.erased_count();

    // Idempotent repeat on the same sweep: the verified state is terminal for
    // the sweep and reports the same observed count.
    let repeat = participant
        .demand_local_erasure(local_command(
            sweep,
            ParticipantOwnerRef::Companion,
            target,
            Vec::new(),
        ))
        .await;
    assert_eq!(repeat.status(), ParticipantCompletionStatus::Verified);
    assert_eq!(repeat.erased_count(), erased);

    // A fresh participant (post-restart) re-scans the same range and finds
    // nothing left to erase.
    let restarted = CompanionErasureParticipant::new(store.clone());
    let rechecked = drive(&restarted, sweep, ParticipantOwnerRef::Companion, target).await;
    assert_eq!(rechecked.status(), ParticipantCompletionStatus::Verified);
    assert_eq!(rechecked.erased_count(), 0);
    assert_eq!(remainder(&store, target), 0);
}

#[tokio::test]
async fn a_matching_current_memory_takes_its_history_and_index_with_it() {
    let store = open_memory().await.unwrap();
    let (companion, _) = running_companion(&store).await.unwrap();
    let target = "swordfish-current-only";
    let summary = summary(
        companion.as_raw(),
        "The owner shared a code.",
        RawId::new(),
        RawId::new(),
    );
    let memory = MemoryId::generate();
    assert!(matches!(
        commit(
            &store,
            change(
                &summary,
                MemoryTarget::New { id: memory },
                "The owner keeps a private credential.",
                ChangeKind::Initial,
            ),
        )
        .await,
        MemoryChangeOutcome::Committed { .. }
    ));
    // A divergent shape the required-memory invariant does not leave on its
    // own (the revision snapshot was rewritten while the current row kept the
    // text). The sweep must erase the recognition whole, not only the row the
    // page matched, or the change history would keep the erased body.
    {
        let guard = crate::codec::lock_shared(&store.conn);
        guard
            .execute(
                "UPDATE learning_memory SET content = ?2 WHERE memory_id = ?1",
                params![
                    crate::codec::encode_id(memory.as_raw()),
                    format!("owner secret {target}")
                ],
            )
            .unwrap();
    }

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
        store
            .list_current_memories(companion.as_raw(), None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_memory_revisions(memory, None, 100)
            .await
            .unwrap()
            .is_empty(),
        "the whole change history follows the erased recognition"
    );
    assert_eq!(remainder(&store, target), 0);
}

#[tokio::test]
async fn a_local_demand_without_material_is_held_not_a_fake_success() {
    let store = open_memory().await.unwrap();
    let participant = CompanionErasureParticipant::new(store.clone());
    let fact = participant
        .demand_local_erasure(DemandLocalErasureCommand::new(
            condition(1),
            ParticipantOwnerRef::Companion,
            ParticipantErasureScope::correlation_only(vec![RawId::new()]),
        ))
        .await;
    assert_eq!(
        fact.status(),
        ParticipantCompletionStatus::Held(ParticipantHoldClass::Failed),
        "a local owner cannot erase without the protected material"
    );

    let learner = LearningErasureParticipant::new(store);
    let fact = learner
        .demand_local_erasure(DemandLocalErasureCommand::new(
            condition(1),
            ParticipantOwnerRef::Learning,
            ParticipantErasureScope::correlation_only(Vec::new()),
        ))
        .await;
    assert_eq!(
        fact.status(),
        ParticipantCompletionStatus::Held(ParticipantHoldClass::Failed)
    );
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

/// Blocker 3: a demand that passed its pre-check can still race a completed
/// operation. The actual erase transaction re-reads canonical currentness and
/// must leave a fresh post-completion History body byte-identical.
#[tokio::test]
async fn a_stale_companion_erase_does_not_mutate_fresh_post_completion_history() {
    let store = open_memory().await.unwrap();
    let (companion, generation) = running_companion(&store).await.unwrap();
    let target = "stale-erase-fresh-origin";
    let old = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        &format!("old copy of {target}"),
    )
    .await;
    let condition = admit_owner(&store, target, ParticipantOwnerRef::Companion).await;
    let current = ene_preservation::DeletionOperationRef {
        operation: condition.operation,
        sweep: condition.sweep,
    };

    store.arm_erasure_mutation_park_for_tests();
    let parked = {
        let store = store.clone();
        tokio::spawn(async move {
            CompanionErasureParticipant::new(store)
                .demand_local_erasure(local_command(
                    condition,
                    ParticipantOwnerRef::Companion,
                    target,
                    Vec::new(),
                ))
                .await
        })
    };
    store.wait_erasure_mutation_park_for_tests().await;

    let verified = drive(
        &CompanionErasureParticipant::new(store.clone()),
        condition,
        ParticipantOwnerRef::Companion,
        target,
    )
    .await;
    assert_eq!(verified.status(), ParticipantCompletionStatus::Verified);
    super::preservation::complete_via_a5(&store, current).await;
    assert_eq!(remainder(&store, target), 0);

    let fresh_body = format!("fresh origin of {target}");
    let fresh = append_role(
        &store,
        companion,
        generation,
        HistoryRole::Owner,
        &fresh_body,
    )
    .await;

    store.release_erasure_mutation_park_for_tests();
    let stale = parked.await.expect("the parked demand joins");
    assert_eq!(
        stale.status(),
        ParticipantCompletionStatus::LocalComplete,
        "a completed condition is NotCurrent, never Verified"
    );
    assert_eq!(stale.erased_count(), 0);

    let timeline = store
        .load_timeline(companion, None, None, 100)
        .await
        .unwrap();
    let fresh_row = timeline
        .iter()
        .find(|item| item.id == fresh)
        .expect("the fresh origin must remain");
    assert_eq!(fresh_row.text, fresh_body);
    assert!(
        timeline.iter().all(|item| item.id != old),
        "the pre-completion copy is erased by the current sweep"
    );
    let status = store
        .deletion_status(None, 10)
        .await
        .expect("the status must read");
    let record = status
        .iter()
        .find(|record| record.current.operation == current.operation)
        .expect("the completed operation remains on the status view");
    assert_eq!(
        record.phase,
        ene_preservation::DeletionOperationPhase::Completed
    );
}
