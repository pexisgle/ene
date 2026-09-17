//! Stage 6 A3b: Task / Action / Inference owner-local Targeted Deletion
//! participants.
//!
//! The fixtures seed the canonical producers (Task creation and steering,
//! delegation, Action attempts, the inference claim and usage settlement, and
//! the production deletion admission) and then drive the real participants
//! through their own contract, so the tests pin the mechanical erase, the
//! bounded/restartable sweep, and the fact preservation on the same rows
//! production writes.

use super::*;
use crate::erasure::{
    ActionErasureParticipant, ERASED_LOCATOR, InferenceErasureParticipant, ROWS_PER_DEMAND,
    TaskErasureParticipant,
};
use ene_action::ActionAttemptRepository as _;
use ene_action::{ActionAttemptId, ActionCertainty, OperationKind};
use ene_companion::{TaskFact, UndeliveredSource};
use ene_preservation::*;
use ene_task::{TaskReportSourceRef, TaskResultId, orchestrate_result_arrival};

const TARGET: &str = "probe-target-1587";
const MARKER: &str = "[erased]";

fn target_path(name: &str) -> String {
    std::env::temp_dir()
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn target_participants() -> Vec<ParticipantOwnerRef> {
    vec![
        ParticipantOwnerRef::Task,
        ParticipantOwnerRef::Action,
        ParticipantOwnerRef::Inference,
    ]
}

async fn admit_target(store: &Store, text: &str, sources: Vec<RawId>) -> DeletionOperationRef {
    admit_target_with(store, text, sources, target_participants()).await
}

async fn admit_target_with(
    store: &Store,
    text: &str,
    sources: Vec<RawId>,
    participants: Vec<ParticipantOwnerRef>,
) -> DeletionOperationRef {
    let command = StartTargetedDeletionCommand::new(
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                text.into(),
            )),
            semantic_hints: vec![],
        },
        DeletionPurpose::Privacy,
        WallClockWithTz::now(),
        sources,
        participants,
    )
    .confirmed_for_tests();
    match store.start_targeted_deletion(command).await.unwrap() {
        StartTargetedDeletionOutcome::Started(current) => current,
        other => panic!("unexpected admission: {other:?}"),
    }
}

/// One demand built exactly the way the Host fan-out builds it: from the
/// protected operation material read, with the owner-local scope.
async fn demand(
    participant: &impl ErasureParticipant,
    store: &Store,
    current: DeletionOperationRef,
    owner: ParticipantOwnerRef,
) -> ParticipantCompletionFact {
    let material = match store
        .deletion_operation_material(current.operation)
        .await
        .unwrap()
    {
        DeletionMaterialOutcome::Material(material) => material,
        other => panic!("the protected material must read, got {other:?}"),
    };
    let scope =
        ParticipantErasureScope::local(material.target().clone(), material.sources().to_vec());
    participant
        .demand_local_erasure(DemandLocalErasureCommand::new(
            current.condition(),
            owner,
            scope,
        ))
        .await
}

/// Drives one participant until it verifies for the current sweep. Every
/// intermediate fact must be bounded work, never a hold.
async fn sweep_to_verified(
    participant: &impl ErasureParticipant,
    store: &Store,
    current: DeletionOperationRef,
    owner: ParticipantOwnerRef,
) -> ParticipantCompletionFact {
    for _ in 0..64 {
        let fact = demand(participant, store, current, owner).await;
        match fact.status() {
            ParticipantCompletionStatus::Verified => return fact,
            ParticipantCompletionStatus::MoreWork | ParticipantCompletionStatus::LocalComplete => {
                continue;
            }
            ParticipantCompletionStatus::Held(reason) => {
                panic!("the sweep must not hold: {reason:?}")
            }
        }
    }
    panic!("the bounded sweep must converge");
}

/// Counts stored values of one column that still contain the exact target.
fn matching_cells(store: &Store, table: &str, column: &str, target: &str) -> i64 {
    let guard = store.conn.lock().unwrap();
    guard
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM {table} WHERE {column} IS NOT NULL AND instr({column}, ?1) > 0"
            ),
            params![target],
            |row| row.get(0),
        )
        .unwrap()
}

fn attempt_premise(
    attempt: ActionAttemptId,
    delegation: DelegationId,
    task: TaskRef,
    workspace: WorkspaceAssocId,
    target: &str,
    operation: OperationKind,
) -> ene_action::AttemptCommitPremise {
    ene_action::AttemptCommitPremise {
        attempt,
        delegation: delegation.as_raw(),
        task: task.task.as_raw(),
        task_revision: RevisionInner::from_u64(task.revision.as_u64()),
        workspace: workspace.as_raw(),
        real_target: ene_action::RealTargetRef::from_canonical_path(target.to_owned()),
        operation,
        relied_evaluation: RawId::new(),
    }
}

/// Seeds one Task-owned target surface: the current purpose, a historical
/// revision purpose, the recorded result body, one workspace association with
/// the target in both paths, and the delegation's copied scope.
async fn seed_task_surface(store: &Store) -> (TaskRef, DelegationId, WorkspaceAssocId, RawId) {
    let source = RawId::new();
    let workspace = task_workspace(
        &format!("/srv/workspace/{TARGET}/work"),
        Some(&format!("/srv/workspace/{TARGET}/work/out")),
    );
    let assoc = workspace.assoc;
    let created = store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: format!("keep {TARGET} private"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source,
            },
            acquired_at: fixture_clock(),
            assignee: AssigneeRef {
                companion: RawId::new(),
            },
            workspace: Some(workspace),
        })
        .await
        .expect("the AU2 task must commit");
    let advanced = store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(TaskPurposeAdoptionPremise {
                purpose: TaskPurpose {
                    text: format!("now {TARGET} and {TARGET} again"),
                },
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source: RawId::new(),
                },
                acquired_at: fixture_clock(),
            }),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .expect("the steering must answer");
    let TaskCommitOutcome::CommittedAs(current) = advanced else {
        panic!("the purpose update must commit, got {advanced:?}");
    };
    let delegation = DelegationId::generate();
    assert!(matches!(
        store
            .create_delegation(DelegationCreationPremise {
                delegation,
                task: current,
                agent: TaskAgentEphemeralId::generate(),
                scope_copy: delegation_scope(Some(delegated_workspace(
                    assoc,
                    &format!("/srv/workspace/{TARGET}/work"),
                    Some(&format!("/srv/workspace/{TARGET}/work/out")),
                ))),
            })
            .await
            .expect("the delegation must commit"),
        DelegationOutcome::Delegated(_)
    ));
    (current, delegation, assoc, source)
}

/// Seeds one already-completed and one still-`Unknown` Action attempt under
/// the delegation, both targeting a path that carries the target.
async fn seed_action_surface(
    store: &Store,
    task: TaskRef,
    delegation: DelegationId,
    workspace: WorkspaceAssocId,
) -> (ActionAttemptId, ActionAttemptId) {
    let done = ActionAttemptId::generate();
    assert_eq!(
        store
            .insert_attempt_if_current(attempt_premise(
                done,
                delegation,
                task,
                workspace,
                &target_path(&format!("{TARGET}.md")),
                OperationKind::Create,
            ))
            .await
            .unwrap(),
        ene_action::ActionStartOutcome::Started
    );
    assert_eq!(
        store
            .compare_and_set_certainty(
                done,
                ActionCertainty::Unknown,
                ActionCertainty::ConfirmedSuccess,
                ene_action::EffectGrounds::ObservedAtTarget,
            )
            .await
            .unwrap(),
        ene_action::CertaintyUpdateOutcome::Updated
    );
    let unknown = ActionAttemptId::generate();
    assert_eq!(
        store
            .insert_attempt_if_current(attempt_premise(
                unknown,
                delegation,
                task,
                workspace,
                &target_path(&format!("{TARGET}-open.md")),
                OperationKind::Read,
            ))
            .await
            .unwrap(),
        ene_action::ActionStartOutcome::Started
    );
    (done, unknown)
}

/// Seeds one claimed Task Agent attempt with its settled usage fact.
async fn seed_inference_surface(
    store: &Store,
    task: TaskRef,
    delegation: DelegationId,
    source: RawId,
) -> InferenceTicketId {
    let saved = save_consent(
        store,
        None,
        ConsentRecord {
            capability: CapabilityKind::Dialogue,
            id: String::from("consent-1"),
            rev: ConsentRevision::from_u64(1),
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            credential_id: String::from("openai:main"),
        },
    )
    .await;
    assert!(matches!(saved, ConsentCommitOutcome::Committed { .. }));
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(InferenceAttempt {
                ticket,
                consumer: ConsumerKind::TaskAgent,
                capability: CapabilityKind::Dialogue,
                purpose: PurposeKind::TaskAgentTurn,
                expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
                expected_credential_set: CredentialSetRevision::initial(),
                provider: String::from("openai"),
                model: String::from("dialogue-1"),
                data_use: vec![source],
                task_agent: Some(ene_inference::TaskAgentAttemptPremise {
                    delegation: delegation.as_raw(),
                    task: task.task.as_raw(),
                    task_revision: RevisionInner::from_u64(task.revision.as_u64()),
                    data_use: vec![source],
                }),
                pricing: None,
                usage_estimate: None,
            })
            .await,
        Ok(AttemptBeginOutcome::Started)
    );
    store
        .record_usage(UsageFact {
            ticket,
            provider: String::from("openai"),
            model: String::from("dialogue-1"),
            input_tokens: Some(120),
            cached_input_tokens: Some(20),
            output_tokens: Some(30),
            source: UsageSource::Reported,
        })
        .await
        .expect("the usage settlement must commit");
    ticket
}

struct Fixture {
    current: DeletionOperationRef,
    task: TaskRef,
    delegation: DelegationId,
    result: TaskResultId,
    done: ActionAttemptId,
    unknown: ActionAttemptId,
    ticket: InferenceTicketId,
}

/// Seeds the whole current Task/Action/Inference production surface and
/// admits one operation over the target. Attempts are recorded before the
/// result arrival seals the delegation, exactly like a real execution.
async fn seed_fixture(store: &Store) -> Fixture {
    let (task, delegation, workspace, source) = seed_task_surface(store).await;
    let (done, unknown) = seed_action_surface(store, task, delegation, workspace).await;
    let ticket = seed_inference_surface(store, task, delegation, source).await;
    let result = orchestrate_result_arrival(
        store,
        delegation,
        ene_task::TaskAgentOutput::new(format!("final report mentions {TARGET}")),
    )
    .await
    .expect("the result arrival must commit");
    let current = admit_target(store, TARGET, vec![source]).await;
    Fixture {
        current,
        task,
        delegation,
        result: result.result,
        done,
        unknown,
        ticket,
    }
}

#[tokio::test]
async fn task_participant_erases_task_owned_bodies_and_path_copies() {
    let store = open_memory().await.unwrap();
    let fixture = seed_fixture(&store).await;
    let before = store
        .load_task(fixture.task.task)
        .await
        .unwrap()
        .expect("the task must load");
    let before_inference = store.load_inference_attempt(fixture.ticket).await.unwrap();
    let before_usage = store.load_usage_cost(fixture.ticket).await.unwrap();

    let participant = TaskErasureParticipant::new(store.clone());
    let first = demand(
        &participant,
        &store,
        fixture.current,
        ParticipantOwnerRef::Task,
    )
    .await;
    assert_eq!(
        first.status(),
        ParticipantCompletionStatus::LocalComplete,
        "the bounded erase pass finishes before the remainder check"
    );
    assert!(
        !format!("{first:?}").contains(TARGET),
        "a completion fact never carries the target body"
    );
    let verified = sweep_to_verified(
        &participant,
        &store,
        fixture.current,
        ParticipantOwnerRef::Task,
    )
    .await;
    assert_eq!(verified.remainder_count(), 0);

    // No Task-owned body or path copy carries the target any more.
    let after = store
        .load_task(fixture.task.task)
        .await
        .unwrap()
        .expect("the task still loads");
    assert!(!after.revision.purpose_text.text.contains(TARGET));
    assert!(after.revision.purpose_text.text.contains(MARKER));
    let historical = store
        .load_report_source_bounded(
            ene_task::TaskReportSourceRef::RevisionPurpose {
                task: fixture.task.task,
                revision: ene_task::TaskRevision::initial(),
            },
            0,
            4096,
        )
        .await
        .unwrap()
        .expect("the historical revision is retained");
    assert!(!historical.text.contains(TARGET), "got {}", historical.text);
    assert!(historical.text.contains(MARKER));
    let result = store
        .load_task_result(fixture.result)
        .await
        .unwrap()
        .expect("the result row is retained");
    assert!(
        !result.body.text().contains(TARGET),
        "got {:?}",
        result.body
    );
    assert!(result.body.text().contains(MARKER));
    let workspace = after
        .workspace
        .as_ref()
        .expect("the association is retained to keep the Task readable");
    assert!(!workspace.folder.path.contains(TARGET));
    assert!(
        !workspace
            .save_target
            .as_ref()
            .expect("the save target was set")
            .path
            .contains(TARGET)
    );
    let delegation = store
        .load_delegation(fixture.delegation)
        .await
        .unwrap()
        .expect("the delegation correspondence is retained");
    let scope = delegation
        .scope
        .workspace
        .expect("the copied scope is retained");
    assert!(!scope.folder.path.contains(TARGET));
    assert!(
        !scope
            .save_target
            .expect("the copy had one")
            .path
            .contains(TARGET)
    );

    // Objective Task facts are untouched: identity, revision, adopted-purpose
    // pointer, progress, context, and the workspace association identity.
    assert_eq!(after.task.reference, before.task.reference);
    assert_eq!(after.task.purpose, before.task.purpose);
    assert_eq!(after.task.progress, before.task.progress);
    assert_eq!(after.revision.purpose, before.revision.purpose);
    assert_eq!(after.context.len(), before.context.len());
    assert_eq!(
        after.workspace.as_ref().unwrap().assoc,
        before.workspace.as_ref().unwrap().assoc
    );
    assert_eq!(after.task.adopted_result, before.task.adopted_result);
    assert_eq!(
        store.load_inference_attempt(fixture.ticket).await.unwrap(),
        before_inference,
        "another owner's sweep never touches the inference correlation"
    );
    assert_eq!(
        store.load_usage_cost(fixture.ticket).await.unwrap(),
        before_usage,
        "another owner's sweep never touches the settled usage fact"
    );
}

#[tokio::test]
async fn task_participant_is_bounded_idempotent_and_resumes_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("participant-bounded.db");
    let (current, task) = {
        let store = Store::open(&path).await.unwrap();
        let task = store
            .create_task(TaskCreationPremise {
                task: TaskId::generate(),
                purpose: TaskPurpose {
                    text: format!("{TARGET} only"),
                },
                entry: TaskContextEntryId::generate(),
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source: RawId::new(),
                },
                acquired_at: fixture_clock(),
                assignee: AssigneeRef {
                    companion: RawId::new(),
                },
                workspace: None,
            })
            .await
            .unwrap();
        // More tasks than one demand's row budget proves the bound.
        for index in 0..ROWS_PER_DEMAND + 16 {
            let created = store
                .create_task(TaskCreationPremise {
                    task: TaskId::generate(),
                    purpose: TaskPurpose {
                        text: format!("{TARGET} {index}"),
                    },
                    entry: TaskContextEntryId::generate(),
                    origin: TaskContextOrigin {
                        kind: TaskContextOriginKind::OwnerConversation,
                        source: RawId::new(),
                    },
                    acquired_at: fixture_clock(),
                    assignee: AssigneeRef {
                        companion: RawId::new(),
                    },
                    workspace: None,
                })
                .await
                .unwrap();
            assert_eq!(created.revision, TaskRevision::initial());
        }
        let current = admit_target(&store, TARGET, vec![]).await;
        let participant = TaskErasureParticipant::new(store.clone());
        let first = demand(&participant, &store, current, ParticipantOwnerRef::Task).await;
        assert_eq!(
            first.status(),
            ParticipantCompletionStatus::MoreWork,
            "one demand never sweeps more rows than its budget"
        );
        assert_eq!(
            first.erased_count(),
            u64::from(ROWS_PER_DEMAND),
            "the bound is on the rows examined, and every examined match was erased"
        );
        // The rest of the surface is untouched by that one bounded demand.
        assert!(
            matching_cells(&store, "task", "purpose_text", TARGET) > 0,
            "a bounded pass leaves the remainder for the next demand"
        );
        (current, task)
    };
    // Crash: the participant's in-memory cursor is gone; the durable rows are
    // not. The reopened handle re-drives the same sweep idempotently.
    let reopened = Store::open(&path).await.unwrap();
    let participant = TaskErasureParticipant::new(reopened.clone());
    let remaining = matching_cells(&reopened, "task", "purpose_text", TARGET)
        + matching_cells(&reopened, "task_revision", "purpose_text", TARGET);
    assert_eq!(
        remaining,
        i64::from(ROWS_PER_DEMAND) + 34,
        "the first demand redacted its bounded page; the rest stayed for the resume"
    );
    let verified =
        sweep_to_verified(&participant, &reopened, current, ParticipantOwnerRef::Task).await;
    assert_eq!(verified.remainder_count(), 0);
    assert_eq!(matching_cells(&reopened, "task", "purpose_text", TARGET), 0);
    assert_eq!(
        matching_cells(&reopened, "task_revision", "purpose_text", TARGET),
        0
    );
    assert_eq!(
        verified.erased_count(),
        u64::try_from(remaining).unwrap(),
        "the resumed sweep redacts exactly the remaining occurrences"
    );
    // Every Task still exists: erasure redacts values, never owner rows.
    assert_eq!(
        task_table_count(&reopened, "task"),
        i64::from(ROWS_PER_DEMAND) + 17
    );
    assert!(
        reopened
            .load_task(task.task)
            .await
            .unwrap()
            .expect("the task still exists")
            .revision
            .purpose_text
            .text
            .contains(MARKER)
    );

    // Idempotency: a fresh sweep over the clean surface erases nothing and
    // still verifies.
    let again = TaskErasureParticipant::new(reopened.clone());
    let verified_again =
        sweep_to_verified(&again, &reopened, current, ParticipantOwnerRef::Task).await;
    assert_eq!(
        verified_again.erased_count(),
        0,
        "re-driving a clean sweep has no second semantic effect"
    );
    assert_eq!(verified_again.remainder_count(), 0);
}

#[tokio::test]
async fn a_delayed_arrival_is_found_by_the_remainder_pass_not_verified_silently() {
    let store = open_memory().await.unwrap();
    // The surface is clean from the start, so the erase pass completes with
    // zero matches.
    let current = admit_target(&store, TARGET, vec![]).await;
    let participant = TaskErasureParticipant::new(store.clone());
    let first = demand(&participant, &store, current, ParticipantOwnerRef::Task).await;
    assert_eq!(first.status(), ParticipantCompletionStatus::LocalComplete);
    assert_eq!(first.erased_count(), 0);

    // A target-bearing row arrives inside the deletion interval. A4's
    // acceptance boundaries collect or refuse covered bodies, so a copy that
    // still reaches durable storage is exactly the case the remainder pass
    // must catch: it is simulated here with a direct row write (a
    // crash-restored or otherwise ungated copy) so the participant's
    // positive-control property stays covered independently of the boundary
    // gates.
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO task (task_id,revision,purpose_adopted_revision,purpose_text,assignee,progress) VALUES (?1,1,1,?2,?3,'started')",
            rusqlite::params![
                crate::codec::encode_id(RawId::new()),
                format!("late arrival with {TARGET}"),
                crate::codec::encode_id(RawId::new()),
            ],
        )
        .expect("the ungated late row must land");
    }

    let found = demand(&participant, &store, current, ParticipantOwnerRef::Task).await;
    assert_eq!(
        found.status(),
        ParticipantCompletionStatus::MoreWork,
        "the remainder pass must detect the late occurrence"
    );
    assert!(
        found.remainder_count() > 0,
        "the positive control: the check reports the occurrence it found"
    );
    assert!(found.erased_count() > 0);
    assert_eq!(
        matching_cells(&store, "task", "purpose_text", TARGET),
        0,
        "the found occurrence is redacted, not merely reported"
    );
    let verified =
        sweep_to_verified(&participant, &store, current, ParticipantOwnerRef::Task).await;
    assert_eq!(verified.remainder_count(), 0);
}

#[tokio::test]
async fn action_participant_erases_the_target_and_preserves_certainty() {
    let store = open_memory().await.unwrap();
    let fixture = seed_fixture(&store).await;
    let participant = ActionErasureParticipant::new(store.clone());
    let verified = sweep_to_verified(
        &participant,
        &store,
        fixture.current,
        ParticipantOwnerRef::Action,
    )
    .await;
    assert_eq!(verified.remainder_count(), 0);
    assert!(verified.erased_count() > 0);

    let done = store
        .load_attempt(fixture.done)
        .await
        .expect("the completed attempt must still read")
        .expect("the completed attempt row is retained");
    assert_eq!(
        done.certainty,
        ActionCertainty::ConfirmedSuccess,
        "deletion never rewrites an observed external effect"
    );
    assert_eq!(
        done.grounds,
        Some(ene_action::EffectGrounds::ObservedAtTarget)
    );
    assert_eq!(done.operation, OperationKind::Create);
    assert!(!done.real_target.as_path().contains(TARGET));
    assert!(
        std::path::Path::new(done.real_target.as_path()).is_absolute(),
        "the erased locator stays a readable canonical path"
    );
    let unknown = store
        .load_attempt(fixture.unknown)
        .await
        .expect("the unknown attempt must still read")
        .expect("the unknown attempt row is retained");
    assert_eq!(
        unknown.certainty,
        ActionCertainty::Unknown,
        "an unconfirmed effect stays unknown, never 'not executed'"
    );
    assert_eq!(unknown.grounds, None);
    assert!(!unknown.real_target.as_path().contains(TARGET));
    assert_eq!(
        task_table_count(&store, "action_attempt"),
        2,
        "erasure redacts the stored target, never the execution fact"
    );
    assert_eq!(
        matching_cells(&store, "action_attempt", "real_target", TARGET),
        0
    );
}

#[tokio::test]
async fn inference_participant_verifies_without_touching_attribution() {
    let store = open_memory().await.unwrap();
    let fixture = seed_fixture(&store).await;
    let before_attempt = store
        .load_inference_attempt(fixture.ticket)
        .await
        .unwrap()
        .expect("the claimed attempt exists");
    let before_usage = store
        .load_usage_cost(fixture.ticket)
        .await
        .unwrap()
        .expect("the settled usage exists");

    let participant = InferenceErasureParticipant::new(store.clone());
    let verified = sweep_to_verified(
        &participant,
        &store,
        fixture.current,
        ParticipantOwnerRef::Inference,
    )
    .await;
    assert_eq!(
        verified.erased_count(),
        0,
        "the inference surface has no body copy to erase"
    );
    assert_eq!(verified.remainder_count(), 0);
    assert_eq!(
        store.load_inference_attempt(fixture.ticket).await.unwrap(),
        Some(before_attempt),
        "the ticket correlation survives unchanged"
    );
    assert_eq!(
        store.load_usage_cost(fixture.ticket).await.unwrap(),
        Some(before_usage),
        "settled usage is neither zeroed nor double-counted"
    );
    assert_eq!(
        task_table_count(&store, "usage_fact"),
        1,
        "one settlement stays one row"
    );
}

#[tokio::test]
async fn a_stale_sweep_fact_never_updates_the_current_sweep() {
    let store = open_memory().await.unwrap();
    let current = admit_target_with(&store, TARGET, vec![], vec![ParticipantOwnerRef::Task]).await;
    let participant = TaskErasureParticipant::new(store.clone());
    let stale = demand(&participant, &store, current, ParticipantOwnerRef::Task).await;
    assert_ne!(
        stale.status(),
        ParticipantCompletionStatus::Held(ParticipantHoldClass::Unsupported)
    );

    assert!(matches!(
        store
            .change_deletion_lifecycle(current, DeletionLifecycleChange::NextSweep)
            .await
            .unwrap(),
        DeletionLifecycleOutcome::Applied(_)
    ));
    assert_eq!(
        store.record_participant_completion(stale).await.unwrap(),
        ParticipantCompletionOutcome::StaleSweep,
        "a completion minted against the superseded generation is refused"
    );
    let rows = store
        .deletion_participants(current.operation, None, 100)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].progress, ParticipantProgress::Pending);
    assert_eq!(rows[0].participant.owner, ParticipantOwnerRef::Task);
}

#[tokio::test]
async fn an_unrepresentable_target_is_held_and_erases_nothing() {
    let store = open_memory().await.unwrap();
    let (task, delegation, workspace, _source) = seed_task_surface(&store).await;
    // The target is the erased-locator marker itself: redacting this path
    // consumes its root, so the owner can neither keep the redaction absolute
    // nor store the marker without the target. Fail closed.
    let attempt = ActionAttemptId::generate();
    assert_eq!(
        store
            .insert_attempt_if_current(attempt_premise(
                attempt,
                delegation,
                task,
                workspace,
                &format!("{ERASED_LOCATOR}/file.md"),
                OperationKind::Read,
            ))
            .await
            .unwrap(),
        ene_action::ActionStartOutcome::Started
    );
    let current = admit_target(&store, ERASED_LOCATOR, vec![]).await;
    let participant = ActionErasureParticipant::new(store.clone());
    let fact = demand(&participant, &store, current, ParticipantOwnerRef::Action).await;
    assert_eq!(
        fact.status(),
        ParticipantCompletionStatus::Held(ParticipantHoldClass::Failed),
        "an unrepresentable value is an explicit hold, never a fake verification"
    );
    let stored = store
        .load_attempt(attempt)
        .await
        .unwrap()
        .expect("the row is untouched");
    assert_eq!(
        stored.real_target.as_path(),
        format!("{ERASED_LOCATOR}/file.md"),
        "the failed page rolled back instead of half-redacting"
    );
}

#[tokio::test]
async fn inference_attribution_columns_are_facts_not_body_copies() {
    let store = open_memory().await.unwrap();
    // A target that coincides with a configured route token: the attribution
    // columns are objective facts (lifecycle §11), not copies of any logical
    // input body, so the Inference owner verifies without rewriting them and
    // without touching the settled usage.
    let route = "probe-route-1587";
    let saved = save_consent(
        &store,
        None,
        ConsentRecord {
            capability: CapabilityKind::Dialogue,
            id: String::from("consent-1"),
            rev: ConsentRevision::from_u64(1),
            provider: String::from(route),
            model: String::from("dialogue-1"),
            credential_id: String::from("openai:main"),
        },
    )
    .await;
    assert!(matches!(saved, ConsentCommitOutcome::Committed { .. }));
    let ticket = InferenceTicketId(RawId::new());
    assert_eq!(
        store
            .begin_inference_attempt(InferenceAttempt {
                ticket,
                consumer: ConsumerKind::CompanionLearning,
                capability: CapabilityKind::Dialogue,
                purpose: PurposeKind::DialogueResponse,
                expected_consent: (String::from("consent-1"), ConsentRevision::from_u64(1)),
                expected_credential_set: CredentialSetRevision::initial(),
                provider: String::from(route),
                model: String::from("dialogue-1"),
                data_use: vec![RawId::new()],
                task_agent: None,
                pricing: None,
                usage_estimate: None,
            })
            .await,
        Ok(AttemptBeginOutcome::Started)
    );
    store
        .record_usage(UsageFact {
            ticket,
            provider: String::from(route),
            model: String::from("dialogue-1"),
            input_tokens: Some(7),
            cached_input_tokens: Some(0),
            output_tokens: Some(3),
            source: UsageSource::Reported,
        })
        .await
        .expect("the usage settlement must commit");
    let before_usage = store.load_usage_cost(ticket).await.unwrap();
    let before_attempt = store.load_inference_attempt(ticket).await.unwrap();

    let current = admit_target(&store, route, vec![]).await;
    let participant = InferenceErasureParticipant::new(store.clone());
    let verified = sweep_to_verified(
        &participant,
        &store,
        current,
        ParticipantOwnerRef::Inference,
    )
    .await;
    assert_eq!(verified.erased_count(), 0);
    assert_eq!(
        store.load_inference_attempt(ticket).await.unwrap(),
        before_attempt
    );
    assert_eq!(store.load_usage_cost(ticket).await.unwrap(), before_usage);
    assert_eq!(
        store
            .load_inference_attempt(ticket)
            .await
            .unwrap()
            .expect("the attempt stays")
            .provider,
        route,
        "the attribution fact is retained, never zeroed or rewritten"
    );
}

#[tokio::test]
async fn derived_presentation_reads_never_resurrect_an_erased_body() {
    let store = open_memory().await.unwrap();
    let fixture = seed_fixture(&store).await;
    let source = UndeliveredSource::TaskRecord {
        task: fixture.task.task.as_raw(),
        fact: TaskFact::ActionAttempt {
            attempt: fixture.done.as_raw(),
            certainty: ene_companion::ActionCertaintyWire::ConfirmedSuccess,
        },
    };
    let before = store
        .load_undelivered_excerpt(source, 4096)
        .await
        .unwrap()
        .expect("the attempt excerpt derives from the owner row");
    assert!(
        before.text.contains(TARGET),
        "the fixture's excerpt carries the target before erasure"
    );

    let participant = ActionErasureParticipant::new(store.clone());
    sweep_to_verified(
        &participant,
        &store,
        fixture.current,
        ParticipantOwnerRef::Action,
    )
    .await;
    let task_participant = TaskErasureParticipant::new(store.clone());
    sweep_to_verified(
        &task_participant,
        &store,
        fixture.current,
        ParticipantOwnerRef::Task,
    )
    .await;
    let after = store
        .load_undelivered_excerpt(source, 4096)
        .await
        .unwrap()
        .expect("the undelivered source itself is a fact and stays");
    assert!(
        !after.text.contains(TARGET),
        "the derived excerpt reads the erased owner row, never a stale copy: {}",
        after.text
    );
    let result_excerpt = store
        .load_undelivered_excerpt(
            UndeliveredSource::TaskRecord {
                task: fixture.task.task.as_raw(),
                fact: TaskFact::ResultRecorded(fixture.result.as_raw()),
            },
            4096,
        )
        .await
        .unwrap()
        .expect("the adopted result registers a derived excerpt source");
    assert!(
        !result_excerpt.text.contains(TARGET),
        "the result excerpt derives from the erased result body: {}",
        result_excerpt.text
    );
    let report = store
        .load_report_source_bounded(TaskReportSourceRef::ResultBody(fixture.result), 0, 4096)
        .await
        .unwrap()
        .expect("the result row is retained");
    assert!(!report.text.contains(TARGET));
}
