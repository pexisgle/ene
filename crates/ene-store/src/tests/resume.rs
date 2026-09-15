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
use ene_companion::{ActivityRepository as _, ManagementActivity, RecordResumeActivityCommand};
use ene_task::{
    ConversationTaskRepository as _, OwnerMessageCurrentness, ResumeInstructionSource,
    ResumeTaskCommand, SteeringPremiseRef, TaskAgentOutput, TaskResumeCommitPremise,
    TaskResumeHold, TaskResumeOutcome, TaskResumeReadiness, orchestrate_result_arrival,
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

async fn commit_guarded(
    store: &Store,
    premise: TaskResumeCommitPremise,
    currentness: OwnerMessageCurrentness,
) -> Result<TaskResumeOutcome, TaskTechnicalError> {
    store
        .commit_task_resume_from_conversation(premise, currentness)
        .await
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
async fn resume_advances_a_started_task_to_in_progress() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, _) = seed_task(&store, companion).await;
    let message = append_owner(&store, companion, generation, "start it").await;
    let record = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(record.task.progress, ene_task::TaskProgress::Started);

    let outcome = commit(
        &store,
        resume_premise(&record, history_instruction(message, companion), ready()),
    )
    .await
    .expect("resume must answer");
    assert!(
        matches!(outcome, TaskResumeOutcome::Resumed { .. }),
        "a delegation-less Started task resumes, got {outcome:?}"
    );
    let record = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(record.task.progress, ene_task::TaskProgress::InProgress);
}

#[tokio::test]
async fn missing_task_answers_without_writes() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let message = append_owner(&store, companion, generation, "keep going").await;
    let missing = TaskId::generate();
    let before = table_counts(&store);
    let outcome = commit(
        &store,
        TaskResumeCommitPremise {
            command: ResumeTaskCommand {
                premise: SteeringPremiseRef {
                    expected: TaskRef {
                        task: missing,
                        revision: TaskRevision::from_u64(1),
                    },
                    purpose: TaskPurposeRef {
                        task: missing,
                        adopted_revision: TaskRevision::from_u64(1),
                    },
                },
                instruction: history_instruction(message, companion),
            },
            readiness: ready(),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction_entry: TaskContextEntryId::generate(),
            delegation: DelegationId::generate(),
            agent: TaskAgentEphemeralId::generate(),
            accepted_at: fixture_clock(),
        },
    )
    .await
    .expect("resume must answer");
    assert_eq!(outcome, TaskResumeOutcome::MissingTask { task: missing });
    assert_eq!(table_counts(&store), before);
}

#[tokio::test]
async fn terminal_task_answers_without_writes() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, _) = seed_task(&store, companion).await;
    let message = append_owner(&store, companion, generation, "keep going").await;
    store
        .cancel_task(created.task)
        .await
        .expect("cancel must commit");
    let before = table_counts(&store);

    let record = store.load_task(created.task).await.unwrap().unwrap();
    let outcome = commit(
        &store,
        resume_premise(&record, history_instruction(message, companion), ready()),
    )
    .await
    .expect("resume must answer");
    assert_eq!(
        outcome,
        TaskResumeOutcome::TaskTerminal {
            task: created.task,
            progress: ene_task::TaskProgress::Cancelled,
        }
    );
    assert_eq!(table_counts(&store), before);
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
async fn already_running_wins_over_unknown_effects() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, assoc) = seed_task(&store, companion).await;
    let delegation = seed_delegation(&store, created, assoc).await;
    let message = append_owner(&store, companion, generation, "keep going").await;
    // An unsettled attempt would hold the resume on its own.
    start_attempt(&store, delegation, created, assoc, "pending.txt").await;

    let record = store.load_task(created.task).await.unwrap().unwrap();
    let before = table_counts(&store);
    let mut readiness = ready();
    readiness.execution_free = false;
    let outcome = commit(
        &store,
        resume_premise(&record, history_instruction(message, companion), readiness),
    )
    .await
    .expect("resume must answer");
    assert_eq!(
        outcome,
        TaskResumeOutcome::AlreadyRunning { task: created.task }
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
async fn adoptable_sealed_result_answers_before_any_new_work() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, assoc) = seed_task(&store, companion).await;
    let delegation = seed_delegation(&store, created, assoc).await;
    let message = append_owner(&store, companion, generation, "keep going").await;
    orchestrate_result_arrival(
        &store,
        delegation,
        TaskAgentOutput::new(String::from("the recorded answer")),
    )
    .await
    .expect("the arrival must record");

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
        TaskResumeOutcome::ResultAvailable { task: created.task }
    );
    assert_eq!(table_counts(&store), before);
}

#[tokio::test]
async fn result_blocked_only_by_confirmed_failure_does_not_hold_resume() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, assoc) = seed_task(&store, companion).await;
    let delegation = seed_delegation(&store, created, assoc).await;
    let message = append_owner(&store, companion, generation, "keep going").await;
    let attempt = start_attempt(&store, delegation, created, assoc, "refused.txt").await;
    // A confirmed failure blocks adoption but never a resume: no Unknown
    // remains, and the sealed result cannot complete anyway.
    let settled = store
        .compare_and_set_certainty(
            attempt,
            ActionCertainty::Unknown,
            ActionCertainty::ConfirmedFailure,
            EffectGrounds::RefusedBeforeEffect,
        )
        .await
        .expect("the certainty CAS must answer");
    assert_eq!(settled, CertaintyUpdateOutcome::Updated);
    orchestrate_result_arrival(
        &store,
        delegation,
        TaskAgentOutput::new(String::from("the recorded answer")),
    )
    .await
    .expect("the arrival must record");

    let record = store.load_task(created.task).await.unwrap().unwrap();
    let outcome = commit(
        &store,
        resume_premise(&record, history_instruction(message, companion), ready()),
    )
    .await
    .expect("resume must answer");
    assert!(
        matches!(outcome, TaskResumeOutcome::Resumed { .. }),
        "a failure-blocked result is not availability, got {outcome:?}"
    );
}

#[tokio::test]
async fn revalidation_holds_answer_in_order_without_writes() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();

    // WorkspaceUnavailable: a task with no confirmed association.
    let bare = store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("workspace-less work"),
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
            workspace: None,
        })
        .await
        .expect("creation must commit");
    let message = append_owner(&store, companion, generation, "keep going").await;
    let record = store.load_task(bare.task).await.unwrap().unwrap();
    let before = table_counts(&store);
    let outcome = commit(
        &store,
        resume_premise(&record, history_instruction(message, companion), ready()),
    )
    .await
    .expect("resume must answer");
    assert_eq!(
        outcome,
        TaskResumeOutcome::NeedsRevalidation(TaskResumeHold::WorkspaceUnavailable)
    );
    assert_eq!(table_counts(&store), before);

    // InstructionUnavailable: the relied message does not exist.
    let (created, _) = seed_task(&store, companion).await;
    let record = store.load_task(created.task).await.unwrap().unwrap();
    let before = table_counts(&store);
    let outcome = commit(
        &store,
        resume_premise(
            &record,
            history_instruction(RawId::new(), companion),
            ready(),
        ),
    )
    .await
    .expect("resume must answer");
    assert_eq!(
        outcome,
        TaskResumeOutcome::NeedsRevalidation(TaskResumeHold::InstructionUnavailable)
    );
    assert_eq!(table_counts(&store), before);

    // PermissionUnavailable: the Host has no fresh judgement to offer.
    let mut readiness = ready();
    readiness.permission_available = false;
    let outcome = commit(
        &store,
        resume_premise(&record, history_instruction(message, companion), readiness),
    )
    .await
    .expect("resume must answer");
    assert_eq!(
        outcome,
        TaskResumeOutcome::NeedsRevalidation(TaskResumeHold::PermissionUnavailable)
    );

    // DataUseHeld: the instruction source is covered by a durable erasure
    // condition.
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "INSERT INTO erasure_condition (operation_id, sweep) VALUES (?1, ?2)",
                params![crate::codec::encode_id(RawId::new()), 1_i64,],
            )
            .expect("the condition must insert");
        guard
            .execute(
                "INSERT INTO erasure_condition_source (operation_id, sweep, source) VALUES (?1, ?2, ?3)",
                params![
                    crate::codec::encode_id(RawId::new()),
                    1_i64,
                    crate::codec::encode_id(message),
                ],
            )
            .expect("the coverage must insert");
    }
    let outcome = commit(
        &store,
        resume_premise(&record, history_instruction(message, companion), ready()),
    )
    .await
    .expect("resume must answer");
    assert_eq!(
        outcome,
        TaskResumeOutcome::NeedsRevalidation(TaskResumeHold::DataUseHeld)
    );
    assert_eq!(table_counts(&store), before);

    // ExecutionUnavailable: the Host cannot launch the new delegation.
    let mut readiness = ready();
    readiness.launch_possible = false;
    // The erasure coverage above would win first; clear it for this case.
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute("DELETE FROM erasure_condition_source", ())
            .expect("the coverage must clear");
    }
    let outcome = commit(
        &store,
        resume_premise(&record, history_instruction(message, companion), readiness),
    )
    .await
    .expect("resume must answer");
    assert_eq!(
        outcome,
        TaskResumeOutcome::NeedsRevalidation(TaskResumeHold::ExecutionUnavailable)
    );
    assert_eq!(table_counts(&store), before);
}

#[tokio::test]
async fn stopped_companion_holds_before_workspace_and_instruction() {
    let store = open_store().await;
    let (companion, _generation) = running_companion(&store).await.unwrap();
    // A workspace-less task would hold on the workspace; the stopped
    // companion wins first.
    let bare = store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("workspace-less work"),
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
            workspace: None,
        })
        .await
        .expect("creation must commit");
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE companion SET lifecycle = 'stopped' WHERE companion_id = ?1",
                params![crate::codec::encode_id(companion.as_raw())],
            )
            .expect("the lifecycle must update");
    }
    let record = store.load_task(bare.task).await.unwrap().unwrap();
    let before = table_counts(&store);
    let outcome = commit(
        &store,
        resume_premise(
            &record,
            history_instruction(RawId::new(), companion),
            ready(),
        ),
    )
    .await
    .expect("resume must answer");
    assert_eq!(
        outcome,
        TaskResumeOutcome::NeedsRevalidation(TaskResumeHold::CompanionUnavailable)
    );
    assert_eq!(table_counts(&store), before);
}

#[tokio::test]
async fn exhausted_revision_answers_without_writes() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, assoc) = seed_task(&store, companion).await;
    let message = append_owner(&store, companion, generation, "keep going").await;
    // Pin the durable sequence at its signed-integer ceiling: the next
    // distinct revision has no representation, so the resume exhausts.
    const CEILING: i64 = i64::MAX;
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE task SET revision = ?1, purpose_adopted_revision = ?1 WHERE task_id = ?2",
                params![CEILING, crate::codec::encode_id(created.task.as_raw())],
            )
            .expect("the task must pin");
        guard
            .execute(
                "INSERT INTO task_revision (task_id, revision, purpose_adopted_revision, purpose_text, assignee) VALUES (?1, ?2, ?2, ?3, ?4)",
                params![
                    crate::codec::encode_id(created.task.as_raw()),
                    CEILING,
                    "write the report",
                    crate::codec::encode_id(companion.as_raw()),
                ],
            )
            .expect("the snapshot must insert");
        guard
            .execute(
                "INSERT INTO task_context_entry (entry_id, task_id, revision, item_kind, purpose_adopted_revision, origin_kind, origin_source, acquired_at) VALUES (?1, ?2, ?3, 'adopted_purpose', ?3, 'owner_conversation', ?4, ?5)",
                params![
                    crate::codec::encode_id(RawId::new()),
                    crate::codec::encode_id(created.task.as_raw()),
                    CEILING,
                    crate::codec::encode_id(message),
                    fixture_clock().to_rfc3339(),
                ],
            )
            .expect("the purpose entry must insert");
    }
    let before = table_counts(&store);
    let ceiling = TaskRevision::from_u64(CEILING as u64);
    let outcome = commit(
        &store,
        TaskResumeCommitPremise {
            command: ResumeTaskCommand {
                premise: SteeringPremiseRef {
                    expected: TaskRef {
                        task: created.task,
                        revision: ceiling,
                    },
                    purpose: TaskPurposeRef {
                        task: created.task,
                        adopted_revision: ceiling,
                    },
                },
                instruction: history_instruction(message, companion),
            },
            readiness: ready(),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction_entry: TaskContextEntryId::generate(),
            delegation: DelegationId::generate(),
            agent: TaskAgentEphemeralId::generate(),
            accepted_at: fixture_clock(),
        },
    )
    .await
    .expect("resume must answer");
    assert_eq!(
        outcome,
        TaskResumeOutcome::RevisionExhausted { task: created.task }
    );
    assert_eq!(table_counts(&store), before);
    let _ = assoc;
}

#[tokio::test]
async fn malformed_durable_rows_fail_closed() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, _) = seed_task(&store, companion).await;
    let message = append_owner(&store, companion, generation, "keep going").await;
    let record = store.load_task(created.task).await.unwrap().unwrap();

    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE task SET progress = 'bogus' WHERE task_id = ?1",
                params![crate::codec::encode_id(created.task.as_raw())],
            )
            .expect("the corruption must write");
    }
    let outcome = commit(
        &store,
        resume_premise(&record, history_instruction(message, companion), ready()),
    )
    .await;
    assert!(
        matches!(outcome, Err(TaskTechnicalError::StorageUnavailable { .. })),
        "an unknown progress value is unreadable, got {outcome:?}"
    );
}

#[tokio::test]
async fn guarded_currentness_supersedes_without_writes() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, _) = seed_task(&store, companion).await;
    let first = append_owner(&store, companion, generation, "first").await;
    let second = append_owner(&store, companion, generation, "second").await;
    let record = store.load_task(created.task).await.unwrap().unwrap();
    let before = table_counts(&store);

    // The turn relied on the first message, but the second overtook it.
    let stale = commit_guarded(
        &store,
        resume_premise(&record, history_instruction(first, companion), ready()),
        OwnerMessageCurrentness {
            companion: companion.as_raw(),
            message: first,
        },
    )
    .await
    .expect("resume must answer");
    assert_eq!(stale, TaskResumeOutcome::Superseded);
    assert_eq!(table_counts(&store), before);

    // The current message commits.
    let current = commit_guarded(
        &store,
        resume_premise(&record, history_instruction(second, companion), ready()),
        OwnerMessageCurrentness {
            companion: companion.as_raw(),
            message: second,
        },
    )
    .await
    .expect("resume must answer");
    assert!(
        matches!(current, TaskResumeOutcome::Resumed { .. }),
        "the current turn commits, got {current:?}"
    );
}

#[tokio::test]
async fn activity_record_is_idempotent_by_command_and_resumes() {
    let store = open_store().await;
    let (companion, _) = running_companion(&store).await.unwrap();
    let (created, _) = seed_task(&store, companion).await;
    let record = store.load_task(created.task).await.unwrap().unwrap();
    let command_key = RawId::new();

    let first = store
        .record_resume_activity(RecordResumeActivityCommand {
            companion,
            task: record.task.reference,
            purpose: record.task.purpose,
            body: String::from("continue from the saved facts"),
            command: command_key,
        })
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
    let second = store
        .record_resume_activity(RecordResumeActivityCommand {
            companion,
            task: record.task.reference,
            purpose: record.task.purpose,
            body: String::from("continue from the saved facts"),
            command: command_key,
        })
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

#[tokio::test]
async fn activity_for_another_task_is_a_forged_reference() {
    let store = open_store().await;
    let (companion, _) = running_companion(&store).await.unwrap();
    let (first_task, _) = seed_task(&store, companion).await;
    let (second_task, _) = seed_task(&store, companion).await;
    let first = store.load_task(first_task.task).await.unwrap().unwrap();
    let second = store.load_task(second_task.task).await.unwrap().unwrap();

    let activity = store
        .record_resume_activity(RecordResumeActivityCommand {
            companion,
            task: first.task.reference,
            purpose: first.task.purpose,
            body: String::from("continue the first task"),
            command: RawId::new(),
        })
        .await
        .expect("the activity must record");

    // Resuming the second task from the first task's activity is forgery,
    // never a hold: it fails closed with zero writes.
    let before = table_counts(&store);
    let outcome = commit(
        &store,
        resume_premise(
            &second,
            ResumeInstructionSource::OwnerManagement {
                activity: activity.as_raw(),
            },
            ready(),
        ),
    )
    .await;
    assert!(
        matches!(outcome, Err(TaskTechnicalError::StorageUnavailable { .. })),
        "a forged activity reference must fail closed, got {outcome:?}"
    );
    assert_eq!(table_counts(&store), before);

    // A conflicting reuse of one command key fails closed as well.
    let conflict = store
        .record_resume_activity(RecordResumeActivityCommand {
            companion,
            task: second.task.reference,
            purpose: second.task.purpose,
            body: String::from("a different instruction"),
            command: RawId::new(),
        })
        .await;
    assert!(
        conflict.is_ok(),
        "a fresh key records independently of content"
    );
    let _ = first_task;
}

#[tokio::test]
async fn conflicting_command_key_reuse_fails_closed() {
    let store = open_store().await;
    let (companion, _) = running_companion(&store).await.unwrap();
    let (created, _) = seed_task(&store, companion).await;
    let record = store.load_task(created.task).await.unwrap().unwrap();
    let command_key = RawId::new();
    store
        .record_resume_activity(RecordResumeActivityCommand {
            companion,
            task: record.task.reference,
            purpose: record.task.purpose,
            body: String::from("continue from the saved facts"),
            command: command_key,
        })
        .await
        .expect("the activity must record");
    let conflict = store
        .record_resume_activity(RecordResumeActivityCommand {
            companion,
            task: record.task.reference,
            purpose: record.task.purpose,
            body: String::from("a different instruction"),
            command: command_key,
        })
        .await;
    assert!(
        matches!(
            conflict,
            Err(ene_companion::CompanionTechnicalError::StorageUnavailable { .. })
        ),
        "one key never names two instructions, got {conflict:?}"
    );
}

#[tokio::test]
async fn malformed_activity_row_fails_closed() {
    let store = open_store().await;
    let (companion, _) = running_companion(&store).await.unwrap();
    let (created, _) = seed_task(&store, companion).await;
    let record = store.load_task(created.task).await.unwrap().unwrap();
    let activity = store
        .record_resume_activity(RecordResumeActivityCommand {
            companion,
            task: record.task.reference,
            purpose: record.task.purpose,
            body: String::from("continue from the saved facts"),
            command: RawId::new(),
        })
        .await
        .expect("the activity must record");
    {
        let guard = match store.conn.lock() {
            Ok(locked) => locked,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .execute(
                "UPDATE activity_record SET kind = 'bogus' WHERE activity_id = ?1",
                params![crate::codec::encode_id(activity.as_raw())],
            )
            .expect("the corruption must write");
    }
    let loaded = store.load_activity(activity).await;
    assert!(
        matches!(
            loaded,
            Err(ene_companion::CompanionTechnicalError::StorageUnavailable { .. })
        ),
        "an unknown activity kind is unreadable, got {loaded:?}"
    );
    let outcome = commit(
        &store,
        resume_premise(
            &record,
            ResumeInstructionSource::OwnerManagement {
                activity: activity.as_raw(),
            },
            ready(),
        ),
    )
    .await;
    assert!(
        matches!(outcome, Err(TaskTechnicalError::StorageUnavailable { .. })),
        "the resume resolves no body from a malformed row, got {outcome:?}"
    );
    let _ = created;
}

#[tokio::test]
async fn late_old_delegation_result_records_to_the_original_only() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, assoc) = seed_task(&store, companion).await;
    let old_delegation = seed_delegation(&store, created, assoc).await;
    let message = append_owner(&store, companion, generation, "keep going").await;
    let record = store.load_task(created.task).await.unwrap().unwrap();
    let outcome = commit(
        &store,
        resume_premise(&record, history_instruction(message, companion), ready()),
    )
    .await
    .expect("resume must answer");
    let TaskResumeOutcome::Resumed { task: resumed, .. } = outcome else {
        panic!("the resume must commit, got {outcome:?}");
    };
    assert_eq!(resumed.revision.as_u64(), 2);

    // The old execution's late final result still records and seals under
    // its own identity, but adoption leaves the new revision alone.
    let arrival = orchestrate_result_arrival(
        &store,
        old_delegation,
        TaskAgentOutput::new(String::from("the late answer")),
    )
    .await
    .expect("the late arrival must record");
    let claim = store
        .load_result_adoption_claim(arrival.result)
        .await
        .expect("the claim must derive")
        .expect("the result must exist");
    let acceptance = store
        .adopt_result(claim)
        .await
        .expect("adoption must answer");
    assert_eq!(
        acceptance,
        ene_task::TaskResultAcceptance::RecordedToOriginalOnly
    );
    let record = store.load_task(created.task).await.unwrap().unwrap();
    assert_eq!(record.task.reference, resumed);
    assert_eq!(record.task.progress, ene_task::TaskProgress::InProgress);
    assert_eq!(record.task.adopted_result, None);
}

#[tokio::test]
async fn past_facts_page_carries_attribution_without_bodies() {
    let store = open_store().await;
    let (companion, generation) = running_companion(&store).await.unwrap();
    let (created, assoc) = seed_task(&store, companion).await;
    let delegation = seed_delegation(&store, created, assoc).await;
    let first = start_attempt(&store, delegation, created, assoc, "first.txt").await;
    settle_success(&store, first).await;
    let second = start_attempt(&store, delegation, created, assoc, "second.txt").await;
    let result = orchestrate_result_arrival(
        &store,
        delegation,
        TaskAgentOutput::new(String::from("the sealed answer body")),
    )
    .await
    .expect("the arrival must record");
    let _ = append_owner(&store, companion, generation, "keep going").await;

    let page = store
        .load_past_executed_facts(created.task)
        .await
        .expect("the facts must read");
    assert!(!page.has_more);
    assert_eq!(page.facts.len(), 3, "two attempts then the result");
    assert_eq!(page.facts[0].source, first.as_raw());
    assert!(page.facts[0].line.contains("create"));
    assert!(page.facts[0].line.contains("confirmed_success"));
    assert_eq!(page.facts[1].source, second.as_raw());
    assert!(page.facts[1].line.contains("unknown"));
    assert_eq!(page.facts[2].source, result.result.as_raw());
    assert!(page.facts[2].line.contains("adopted=none"));
    for fact in &page.facts {
        assert!(
            !fact.line.contains("the sealed answer body"),
            "no body travels in the facts block: {}",
            fact.line
        );
        assert!(!fact.line.contains('\n'), "one fact is one line");
    }
}
