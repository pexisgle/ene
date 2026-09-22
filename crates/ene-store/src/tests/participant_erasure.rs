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
    ActionErasureParticipant, InferenceErasureParticipant, TaskErasureParticipant,
};
use ene_action::ActionAttemptRepository as _;
use ene_action::{ActionAttemptId, ActionCertainty, OperationKind};
use ene_preservation::*;
use ene_task::{TaskResultArrivalOutcome, TaskResultId};

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
    let result = match orchestrate_result_arrival(
        store,
        delegation,
        scrubbed_result(store, &format!("final report mentions {TARGET}")).await,
    )
    .await
    .expect("the result arrival must commit")
    {
        TaskResultArrivalOutcome::Recorded(result) => result,
        TaskResultArrivalOutcome::StaleCredentialSet { .. } => {
            panic!("the fixture scrubbed at the current revision")
        }
    };
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
