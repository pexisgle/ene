//! Explicit resume (AU17): the single-transaction revision forward, the
//! refusal priority, and the first-party activity source.
//!
//! Every commit here runs against a real store: a successful resume moves
//! the same Task to `r+1` with the purpose carried over, adopts only the
//! resume instruction (no body is copied), creates one new delegation with
//! its scope frozen from the current workspace association, advances
//! `Started → InProgress`, and registers the revision and delegation
//! notifications in the same transaction. Every refusal answers its
//! `Ok`-side outcome with zero writes anywhere; malformed rows, foreign
//! references, and forged activity premises fail closed as technical
//! errors.

use super::*;

use ene_action::{
    ActionAttemptId, ActionAttemptRepository as _, ActionCertainty, ActionStartOutcome,
    AttemptCommitPremise, CertaintyUpdateOutcome, EffectGrounds, OperationKind, RealTargetRef,
};
use ene_companion::{ManagementActivity, RecordResumeActivityCommand};
use ene_task::{
    OwnerMessageCurrentness, ResumeInstructionSource, ResumeTaskCommand, SteeringPremiseRef,
    TaskResumeCommitPremise, TaskResumeOutcome, TaskResumeReadiness,
};

async fn open_store() -> Store {
    open_memory().await.unwrap()
}

fn ready() -> TaskResumeReadiness {
    TaskResumeReadiness {
        permission_available: true,
        execution_free: true,
        launch_possible: true,
    }
}

async fn seed_task(store: &Store, companion: CompanionId) -> (TaskRef, WorkspaceAssocId) {
    let workspace = task_workspace("/srv/workspace/ene", None);
    let assoc = workspace.assoc;
    let created = store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("write the report"),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            acquired_at: fixture_clock(),
            assignee: AssigneeRef {
                companion: companion.as_raw(),
            },
            workspace: Some(workspace),
        })
        .await
        .expect("the AU2 task must commit");
    (created, assoc)
}

async fn seed_delegation(store: &Store, task: TaskRef, assoc: WorkspaceAssocId) -> DelegationId {
    let delegation = DelegationId::generate();
    let outcome = store
        .create_delegation(delegation_premise(
            delegation,
            task,
            TaskAgentEphemeralId::generate(),
            delegation_scope(Some(delegated_workspace(assoc, "/srv/workspace/ene", None))),
        ))
        .await
        .expect("the AU3 delegation must answer");
    assert!(
        matches!(outcome, DelegationOutcome::Delegated(_)),
        "the delegation must commit, got {outcome:?}"
    );
    delegation
}

async fn append_owner(
    store: &Store,
    companion: CompanionId,
    generation: PresenceGeneration,
    text: &str,
) -> RawId {
    match store
        .append_message(history_command(companion, generation, text))
        .await
        .expect("the Owner append must answer")
    {
        HistoryAppendOutcome::CommittedAs { message } => message,
        other => panic!("the Owner message must commit, got {other:?}"),
    }
}

fn history_instruction(message: RawId, companion: CompanionId) -> ResumeInstructionSource {
    ResumeInstructionSource::OwnerHistory {
        message,
        currentness: OwnerMessageCurrentness {
            companion: companion.as_raw(),
            message,
        },
    }
}

fn resume_premise(
    store_record: &ene_task::TaskRecord,
    instruction: ResumeInstructionSource,
    readiness: TaskResumeReadiness,
) -> TaskResumeCommitPremise {
    TaskResumeCommitPremise {
        command: ResumeTaskCommand {
            premise: SteeringPremiseRef {
                expected: store_record.task.reference,
                purpose: store_record.task.purpose,
            },
            instruction,
        },
        readiness,
        adopted_purpose_entry: TaskContextEntryId::generate(),
        adopted_instruction_entry: TaskContextEntryId::generate(),
        delegation: DelegationId::generate(),
        agent: TaskAgentEphemeralId::generate(),
        accepted_at: fixture_clock(),
    }
}

async fn commit(
    store: &Store,
    premise: TaskResumeCommitPremise,
) -> Result<TaskResumeOutcome, TaskTechnicalError> {
    store.commit_task_resume(premise).await
}

/// Row counts of every table the resume commit may touch, in one fixed
/// order, so a refusal can prove zero writes anywhere.
fn table_counts(store: &Store) -> Vec<i64> {
    let guard = match store.conn.lock() {
        Ok(locked) => locked,
        Err(poisoned) => poisoned.into_inner(),
    };
    [
        "task",
        "task_revision",
        "task_context_entry",
        "delegation",
        "task_result",
        "task_result_attempt",
        "undelivered",
    ]
    .iter()
    .map(|table| {
        guard
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), (), |row| {
                row.get(0)
            })
            .expect("the count must run")
    })
    .collect()
}

async fn start_attempt(
    store: &Store,
    delegation: DelegationId,
    task: TaskRef,
    assoc: WorkspaceAssocId,
    name: &str,
) -> ActionAttemptId {
    let attempt = ActionAttemptId::generate();
    let outcome = store
        .insert_attempt_if_current(AttemptCommitPremise {
            attempt,
            delegation: delegation.as_raw(),
            task: task.task.as_raw(),
            task_revision: RevisionInner::from_u64(task.revision.as_u64()),
            workspace: assoc.as_raw(),
            real_target: RealTargetRef::from_canonical_path(
                std::env::temp_dir()
                    .join(name)
                    .to_string_lossy()
                    .into_owned(),
            ),
            operation: OperationKind::Create,
            relied_evaluation: RawId::new(),
        })
        .await
        .expect("the AU5 insert must answer");
    assert_eq!(outcome, ActionStartOutcome::Started, "start {name}");
    attempt
}

async fn settle_success(store: &Store, attempt: ActionAttemptId) {
    let outcome = store
        .compare_and_set_certainty(
            attempt,
            ActionCertainty::Unknown,
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget,
        )
        .await
        .expect("the certainty CAS must answer");
    assert_eq!(outcome, CertaintyUpdateOutcome::Updated);
}

#[tokio::test]
async fn resume_commits_r_plus_one_with_the_purpose_carried_over() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, assoc) = seed_task(&store, companion).await;
    let old_delegation = seed_delegation(&store, created, assoc).await;
    let message = append_owner(&store, companion, generation, "keep going").await;
    let before = table_counts(&store);

    let record = store
        .load_task(created.task)
        .await
        .unwrap()
        .expect("the task must load");
    let premise = resume_premise(&record, history_instruction(message, companion), ready());
    let new_delegation = premise.delegation;
    let outcome = commit(&store, premise).await.expect("resume must answer");

    let TaskResumeOutcome::Resumed { task, delegation } = outcome else {
        panic!("a clean resume must commit, got {outcome:?}");
    };
    assert_eq!(task.task, created.task);
    assert_eq!(task.revision.as_u64(), 2);
    assert_eq!(delegation.delegation, new_delegation);
    assert_eq!(delegation.task, task);

    let record = store
        .load_task(created.task)
        .await
        .unwrap()
        .expect("the task must load");
    assert_eq!(record.task.reference, task);
    assert_eq!(
        record.task.purpose, record.revision.purpose,
        "the purpose identity is carried over, not changed"
    );
    assert_eq!(record.revision.purpose_text.text, "write the report");
    assert_eq!(record.task.progress, ene_task::TaskProgress::InProgress);
    // The carried purpose entry plus the one new resume instruction entry.
    let instructions = record
        .context
        .iter()
        .filter(|entry| matches!(entry.item, ene_task::TaskContextItem::AdoptedInstruction))
        .collect::<Vec<_>>();
    assert_eq!(instructions.len(), 1);
    assert_eq!(
        instructions[0].origin.kind,
        TaskContextOriginKind::OwnerConversation
    );
    assert_eq!(instructions[0].origin.source, message);
    assert_eq!(instructions[0].reference, task);
    // The new delegation freezes the current association; the old one still
    // names its own revision.
    let stored = store
        .load_delegation(new_delegation)
        .await
        .unwrap()
        .expect("the new delegation must load");
    assert_eq!(stored.task, task);
    let old = store
        .load_delegation(old_delegation)
        .await
        .unwrap()
        .expect("the old delegation is untouched");
    assert_eq!(old.task, created);

    let after = table_counts(&store);
    for (index, (before, after)) in before.iter().zip(after.iter()).enumerate() {
        let grown = match index {
            // task_revision, adopted-purpose entry, instruction entry.
            1 => 1,
            2 => 2,
            // The new delegation row.
            3 => 1,
            // The revision and delegation notifications.
            6 => 2,
            _ => 0,
        };
        assert_eq!(
            *after,
            *before + grown,
            "table index {index} must grow by exactly {grown}"
        );
    }
}

#[tokio::test]
async fn stale_revision_and_purpose_answer_without_writes() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, _) = seed_task(&store, companion).await;
    let message = append_owner(&store, companion, generation, "keep going").await;
    let record = store.load_task(created.task).await.unwrap().unwrap();
    let before = table_counts(&store);

    // A moved revision with the old purpose.
    let mut moved = resume_premise(&record, history_instruction(message, companion), ready());
    moved.command.premise.expected.revision = TaskRevision::from_u64(99);
    let outcome = commit(&store, moved).await.expect("resume must answer");
    assert_eq!(
        outcome,
        TaskResumeOutcome::StalePremise { current: created },
        "a moved revision is stale"
    );

    // The same revision with a foreign purpose identity.
    let mut foreign = resume_premise(&record, history_instruction(message, companion), ready());
    foreign.command.premise.purpose = TaskPurposeRef {
        task: created.task,
        adopted_revision: TaskRevision::from_u64(99),
    };
    let outcome = commit(&store, foreign).await.expect("resume must answer");
    assert_eq!(
        outcome,
        TaskResumeOutcome::StalePremise { current: created },
        "purpose text matches are never identity"
    );
    assert_eq!(table_counts(&store), before);
}

#[tokio::test]
async fn unknown_effects_hold_and_settlement_releases() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, assoc) = seed_task(&store, companion).await;
    let delegation = seed_delegation(&store, created, assoc).await;
    let message = append_owner(&store, companion, generation, "keep going").await;
    let attempt = start_attempt(&store, delegation, created, assoc, "pending.txt").await;

    let record = store.load_task(created.task).await.unwrap().unwrap();
    let before = table_counts(&store);
    let outcome = commit(
        &store,
        resume_premise(&record, history_instruction(message, companion), ready()),
    )
    .await
    .expect("resume must answer");
    assert_eq!(
        outcome,
        TaskResumeOutcome::HeldByUnknownEffects { task: created.task }
    );
    assert_eq!(table_counts(&store), before);

    // Objective evidence settles the attempt; the same resume then commits.
    settle_success(&store, attempt).await;
    let record = store.load_task(created.task).await.unwrap().unwrap();
    let outcome = commit(
        &store,
        resume_premise(&record, history_instruction(message, companion), ready()),
    )
    .await
    .expect("resume must answer");
    assert!(
        matches!(outcome, TaskResumeOutcome::Resumed { .. }),
        "settlement releases the hold, got {outcome:?}"
    );
}

#[tokio::test]
async fn activity_record_is_idempotent_by_command_and_resumes() {
    let store = open_store().await;
    let (companion, _) = running_companion(&store).await.unwrap();
    let (created, _) = seed_task(&store, companion).await;
    let record = store.load_task(created.task).await.unwrap().unwrap();
    let command_key = RawId::new();

    let first = record_activity_id(
        &store,
        RecordResumeActivityCommand {
            companion,
            task: record.task.reference,
            purpose: record.task.purpose,
            body: String::from("continue from the saved facts"),
            command: command_key,
        },
    )
    .await
    .expect("the activity must record");
    let loaded: Option<ManagementActivity> = store
        .load_activity(first)
        .await
        .expect("the activity must load");
    let loaded = loaded.expect("the activity must exist");
    assert_eq!(loaded.task, record.task.reference);
    assert_eq!(loaded.purpose, record.task.purpose);
    assert_eq!(loaded.body, "continue from the saved facts");

    // The same epoch key returns the same activity without a second row.
    let second = record_activity_id(
        &store,
        RecordResumeActivityCommand {
            companion,
            task: record.task.reference,
            purpose: record.task.purpose,
            body: String::from("continue from the saved facts"),
            command: command_key,
        },
    )
    .await
    .expect("the retry must answer");
    assert_eq!(first, second);

    let outcome = commit(
        &store,
        resume_premise(
            &record,
            ResumeInstructionSource::OwnerManagement {
                activity: first.as_raw(),
            },
            ready(),
        ),
    )
    .await
    .expect("resume must answer");
    let TaskResumeOutcome::Resumed { task, .. } = outcome else {
        panic!("an activity-sourced resume must commit, got {outcome:?}");
    };
    let record = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(record.task.reference, task);
    let instructions = record
        .context
        .iter()
        .filter(|entry| matches!(entry.item, ene_task::TaskContextItem::AdoptedInstruction))
        .collect::<Vec<_>>();
    assert_eq!(instructions.len(), 1);
    assert_eq!(
        instructions[0].origin.kind,
        TaskContextOriginKind::OwnerManagement
    );
    assert_eq!(instructions[0].origin.source, first.as_raw());
}
