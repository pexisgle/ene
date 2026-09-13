//! H-A steering orchestration boundary checks (IB §4 / §13.2).
//!
//! The Task repository is faked so each test can script `load_task` and
//! `forward_steering` and inspect the exact `TaskCommitPremise` the
//! orchestration composed. The checks pin the boundary-token comparison
//! (expected revision plus adopted-purpose identity), the identity minting
//! for the new revision's adopted-purpose and adopted-instruction entries,
//! and the outcome mapping: domain outcomes stay on the `Ok` side, while
//! repository technical failures stay `Err`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures and helpers live outside #[test] functions, where clippy.toml's test allowances do not apply"
)]

use std::sync::Mutex;

use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationRef,
    SteeringPremiseRef, SteeringProposalPremise, Task, TaskAgentResultArrival, TaskCommitOutcome,
    TaskCommitPremise, TaskContextEntry, TaskContextEntryId, TaskContextItem, TaskContextOrigin,
    TaskContextOriginKind, TaskCreationPremise, TaskId, TaskProgress, TaskProposalOutcome,
    TaskPurpose, TaskPurposeRef, TaskRecord, TaskRef, TaskRepository, TaskResultAcceptance,
    TaskResultAdoptionClaim, TaskResultId, TaskResultRecord, TaskRevision, TaskRevisionRecord,
    TaskTechnicalError, orchestrate_steering,
};

/// Scripted `TaskRepository`: one load fixture, one forward result, and a
/// capture of every `TaskCommitPremise` handed to `forward_steering`.
struct FakeTaskRepository {
    load: Mutex<Result<Option<TaskRecord>, TaskTechnicalError>>,
    forward: Mutex<Result<TaskCommitOutcome, TaskTechnicalError>>,
    forwarded: Mutex<Vec<TaskCommitPremise>>,
}

impl FakeTaskRepository {
    fn new(
        load: Result<Option<TaskRecord>, TaskTechnicalError>,
        forward: Result<TaskCommitOutcome, TaskTechnicalError>,
    ) -> Self {
        Self {
            load: Mutex::new(load),
            forward: Mutex::new(forward),
            forwarded: Mutex::new(Vec::new()),
        }
    }

    fn forwarded(&self) -> Vec<TaskCommitPremise> {
        self.forwarded
            .lock()
            .expect("fixture capture is never poisoned")
            .clone()
    }
}

impl TaskRepository for FakeTaskRepository {
    async fn create_task(
        &self,
        _premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("create_task is outside this fixture's scope"),
        })
    }

    async fn forward_steering(
        &self,
        premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        self.forwarded
            .lock()
            .expect("fixture capture is never poisoned")
            .push(premise);
        self.forward
            .lock()
            .expect("fixture script is never poisoned")
            .clone()
    }

    async fn load_task(&self, _task: TaskId) -> Result<Option<TaskRecord>, TaskTechnicalError> {
        self.load
            .lock()
            .expect("fixture script is never poisoned")
            .clone()
    }

    async fn create_delegation(
        &self,
        _premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("create_delegation is outside this fixture's scope"),
        })
    }

    async fn load_delegation(
        &self,
        _delegation: DelegationId,
    ) -> Result<Option<DelegationRef>, TaskTechnicalError> {
        Ok(None)
    }

    async fn record_task_result_arrival(
        &self,
        _arrival: TaskAgentResultArrival,
    ) -> Result<TaskResultRecord, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("record_task_result_arrival is outside this fixture's scope"),
        })
    }

    async fn load_task_result(
        &self,
        _result: TaskResultId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("load_task_result is outside this fixture's scope"),
        })
    }

    async fn load_delegation_result(
        &self,
        _delegation: DelegationId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        Ok(None)
    }

    async fn adopt_result(
        &self,
        _claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("adopt_result is outside this fixture's scope"),
        })
    }
}

fn clock() -> WallClockWithTz {
    WallClockWithTz::parse_rfc3339("2026-09-08T12:00:00+09:00").expect("fixture timestamp parses")
}

fn revision(value: u64) -> TaskRevision {
    TaskRevision::from_u64(value)
}

fn record(
    task: TaskId,
    current_revision: TaskRevision,
    purpose: TaskPurposeRef,
    purpose_entry: TaskContextEntryId,
) -> TaskRecord {
    let reference = TaskRef {
        task,
        revision: current_revision,
    };
    let assignee = AssigneeRef {
        companion: RawId::new(),
    };
    TaskRecord {
        task: Task {
            reference,
            purpose,
            assignee,
            progress: TaskProgress::InProgress,
            adopted_result: None,
        },
        revision: TaskRevisionRecord {
            reference,
            purpose,
            purpose_text: TaskPurpose {
                text: String::from("loaded purpose body"),
            },
            assignee,
        },
        context: vec![TaskContextEntry {
            entry: purpose_entry,
            reference,
            item: TaskContextItem::AdoptedPurpose(purpose),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            acquired_at: clock(),
        }],
        workspace: None,
    }
}

fn proposal(
    expected: TaskRef,
    purpose: TaskPurposeRef,
    new_purpose: Option<TaskPurpose>,
    instruction_source: RawId,
) -> SteeringProposalPremise {
    SteeringProposalPremise {
        premise: SteeringPremiseRef { expected, purpose },
        new_purpose,
        instruction_source,
    }
}

#[tokio::test]
async fn accepted_steering_mints_fresh_entries_and_keeps_expected_unchanged() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let current_purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(1),
    };
    let existing_purpose_entry = TaskContextEntryId::generate();
    let instruction_source = RawId::new();
    let accepted = TaskRef {
        task,
        revision: revision(2),
    };
    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(1),
            current_purpose,
            existing_purpose_entry,
        ))),
        Ok(TaskCommitOutcome::CommittedAs(accepted)),
    );

    let outcome = orchestrate_steering(
        &repository,
        proposal(
            expected,
            current_purpose,
            Some(TaskPurpose {
                text: String::from("steer toward the new goal"),
            }),
            instruction_source,
        ),
    )
    .await
    .expect("a steering decision is a domain outcome, not a technical error");

    match outcome {
        TaskProposalOutcome::AcceptedAsSteering(found) => assert_eq!(found, accepted),
        other => panic!("expected AcceptedAsSteering, got {other:?}"),
    }

    let forwarded = repository.forwarded();
    assert_eq!(
        forwarded.len(),
        1,
        "one proposal commits at most one forward"
    );
    let premise = forwarded.first().expect("the forward premise was captured");
    assert_eq!(
        premise.expected, expected,
        "the caller's boundary token travels unchanged"
    );
    // Compile-level: `TaskCommitPremise` has no successor-revision field; the
    // repository derives `expected.revision + 1` only after the CAS succeeds.
    assert_eq!(
        accepted.revision,
        expected
            .revision
            .checked_next()
            .expect("revision 1 has a successor"),
        "the repository, not the caller, names the new revision"
    );

    let purpose_adoption = premise
        .new_purpose
        .as_ref()
        .expect("the new purpose is adopted");
    assert_eq!(
        purpose_adoption.purpose,
        TaskPurpose {
            text: String::from("steer toward the new goal"),
        }
    );
    let origin = TaskContextOrigin {
        kind: TaskContextOriginKind::OwnerConversation,
        source: instruction_source,
    };
    assert_eq!(
        purpose_adoption.origin, origin,
        "the new purpose entry is sourced from the steering utterance"
    );

    let instruction = premise
        .adopted_instruction
        .as_ref()
        .expect("this forward adopts the instruction");
    assert_eq!(
        instruction.origin, origin,
        "the adopted instruction is sourced from the same utterance"
    );
    assert_ne!(
        premise.adopted_purpose_entry, existing_purpose_entry,
        "the new revision records a fresh purpose entry"
    );
    assert_ne!(
        instruction.entry, existing_purpose_entry,
        "the instruction entry is not the predecessor's purpose entry"
    );
    assert_ne!(
        premise.adopted_purpose_entry, instruction.entry,
        "purpose and instruction entries are distinct identities"
    );
    assert_ne!(
        purpose_adoption.acquired_at,
        clock(),
        "the purpose adoption time is captured at adoption, not copied from the loaded record"
    );
    assert_ne!(
        instruction.acquired_at,
        clock(),
        "the instruction adoption time is captured at adoption"
    );
}

#[tokio::test]
async fn unchanged_purpose_carries_forward_while_the_instruction_is_adopted() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let current_purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(1),
    };
    let existing_purpose_entry = TaskContextEntryId::generate();
    let accepted = TaskRef {
        task,
        revision: revision(2),
    };
    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(1),
            current_purpose,
            existing_purpose_entry,
        ))),
        Ok(TaskCommitOutcome::CommittedAs(accepted)),
    );

    let outcome = orchestrate_steering(
        &repository,
        proposal(expected, current_purpose, None, RawId::new()),
    )
    .await
    .expect("a steering decision is a domain outcome, not a technical error");

    match outcome {
        TaskProposalOutcome::AcceptedAsSteering(found) => assert_eq!(found, accepted),
        other => panic!("expected AcceptedAsSteering, got {other:?}"),
    }

    let forwarded = repository.forwarded();
    assert_eq!(
        forwarded.len(),
        1,
        "an unchanged purpose is still one revision forward"
    );
    let premise = forwarded.first().expect("the forward premise was captured");
    assert!(
        premise.new_purpose.is_none(),
        "an unchanged purpose carries the adopted purpose identity forward"
    );
    assert!(
        premise.adopted_instruction.is_some(),
        "steering always adopts its instruction in this slice"
    );
    assert_ne!(
        premise.adopted_purpose_entry, existing_purpose_entry,
        "a carry-forward still records the new revision's own purpose entry"
    );
}

#[tokio::test]
async fn stale_revision_reports_current_without_forwarding() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let current = TaskRef {
        task,
        revision: revision(2),
    };
    let current_purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(2),
    };
    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(2),
            current_purpose,
            TaskContextEntryId::generate(),
        ))),
        Ok(TaskCommitOutcome::CommittedAs(TaskRef {
            task,
            revision: revision(3),
        })),
    );

    let outcome = orchestrate_steering(
        &repository,
        proposal(
            expected,
            TaskPurposeRef {
                task,
                adopted_revision: revision(1),
            },
            Some(TaskPurpose {
                text: String::from("stale proposal"),
            }),
            RawId::new(),
        ),
    )
    .await
    .expect("a stale premise is a domain outcome, not a technical error");

    match outcome {
        TaskProposalOutcome::StalePremise { current: found } => assert_eq!(found, current),
        other => panic!("expected StalePremise, got {other:?}"),
    }
    assert!(
        repository.forwarded().is_empty(),
        "a stale revision must not forward"
    );
}

#[tokio::test]
async fn stale_purpose_identity_at_the_same_revision_does_not_forward() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(2),
    };
    let current_purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(2),
    };
    let stale_purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(1),
    };
    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(2),
            current_purpose,
            TaskContextEntryId::generate(),
        ))),
        Ok(TaskCommitOutcome::CommittedAs(TaskRef {
            task,
            revision: revision(3),
        })),
    );

    let outcome = orchestrate_steering(
        &repository,
        proposal(expected, stale_purpose, None, RawId::new()),
    )
    .await
    .expect("a stale purpose is a domain outcome, not a technical error");

    match outcome {
        TaskProposalOutcome::StalePremise { current: found } => assert_eq!(found, expected),
        other => panic!("expected StalePremise, got {other:?}"),
    }
    assert!(
        repository.forwarded().is_empty(),
        "a purpose-token mismatch at the expected revision must not forward; \
         the comparison is the adopted identity, not the purpose text"
    );
}

#[tokio::test]
async fn missing_task_is_reported_without_forwarding() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(1),
    };
    let repository = FakeTaskRepository::new(
        Ok(None),
        Ok(TaskCommitOutcome::CommittedAs(TaskRef {
            task,
            revision: revision(2),
        })),
    );

    let outcome =
        orchestrate_steering(&repository, proposal(expected, purpose, None, RawId::new()))
            .await
            .expect("a missing Task is a domain outcome, not a technical error");

    match outcome {
        TaskProposalOutcome::MissingTask { task: found } => assert_eq!(found, task),
        other => panic!("expected MissingTask, got {other:?}"),
    }
    assert!(
        repository.forwarded().is_empty(),
        "a missing Task must not forward"
    );
}

#[tokio::test]
async fn terminal_task_is_reported_without_minting_or_forwarding() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(1),
    };
    let mut loaded = record(task, revision(1), purpose, TaskContextEntryId::generate());
    loaded.task.progress = TaskProgress::Completed;
    let repository = FakeTaskRepository::new(
        Ok(Some(loaded)),
        Ok(TaskCommitOutcome::CommittedAs(TaskRef {
            task,
            revision: revision(2),
        })),
    );

    let outcome =
        orchestrate_steering(&repository, proposal(expected, purpose, None, RawId::new()))
            .await
            .expect("terminal progress is a domain outcome, not a technical error");

    assert_eq!(
        outcome,
        TaskProposalOutcome::TaskTerminal {
            task,
            progress: TaskProgress::Completed,
        }
    );
    assert!(
        repository.forwarded().is_empty(),
        "a terminal Task must not receive a steering commit"
    );
}

#[tokio::test]
async fn repository_task_terminal_maps_through_with_the_progress_payload() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(1),
    };
    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(1),
            purpose,
            TaskContextEntryId::generate(),
        ))),
        Ok(TaskCommitOutcome::TaskTerminal {
            task,
            progress: TaskProgress::Failed,
        }),
    );

    let outcome =
        orchestrate_steering(&repository, proposal(expected, purpose, None, RawId::new()))
            .await
            .expect("a terminal CAS loser is a domain outcome, not a technical error");

    assert_eq!(
        outcome,
        TaskProposalOutcome::TaskTerminal {
            task,
            progress: TaskProgress::Failed,
        }
    );
    assert_eq!(repository.forwarded().len(), 1);
}

#[tokio::test]
async fn cas_race_stale_expected_maps_to_stale_premise() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let current_purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(1),
    };
    let raced = TaskRef {
        task,
        revision: revision(2),
    };
    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(1),
            current_purpose,
            TaskContextEntryId::generate(),
        ))),
        Ok(TaskCommitOutcome::StaleExpected { current: raced }),
    );

    let outcome = orchestrate_steering(
        &repository,
        proposal(expected, current_purpose, None, RawId::new()),
    )
    .await
    .expect("a lost compare is a domain outcome, not a technical error");

    match outcome {
        TaskProposalOutcome::StalePremise { current: found } => assert_eq!(found, raced),
        other => panic!("expected StalePremise, got {other:?}"),
    }
    assert_eq!(
        repository.forwarded().len(),
        1,
        "the pre-check passed; the race is decided inside forward_steering"
    );
}

#[tokio::test]
async fn revision_exhausted_passes_through_as_a_domain_outcome() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let current_purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(1),
    };
    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(1),
            current_purpose,
            TaskContextEntryId::generate(),
        ))),
        Ok(TaskCommitOutcome::RevisionExhausted { task }),
    );

    let outcome = orchestrate_steering(
        &repository,
        proposal(expected, current_purpose, None, RawId::new()),
    )
    .await
    .expect("revision exhaustion is a domain outcome, not a technical error");

    match outcome {
        TaskProposalOutcome::RevisionExhausted { task: found } => assert_eq!(found, task),
        other => panic!("expected RevisionExhausted, got {other:?}"),
    }
}

#[tokio::test]
async fn repository_missing_task_passes_through() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let current_purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(1),
    };
    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(1),
            current_purpose,
            TaskContextEntryId::generate(),
        ))),
        Ok(TaskCommitOutcome::MissingTask { task }),
    );

    let outcome = orchestrate_steering(
        &repository,
        proposal(expected, current_purpose, None, RawId::new()),
    )
    .await
    .expect("a missing Task from the commit is a domain outcome");

    match outcome {
        TaskProposalOutcome::MissingTask { task: found } => assert_eq!(found, task),
        other => panic!("expected MissingTask, got {other:?}"),
    }
}

#[tokio::test]
async fn technical_failures_stay_errors() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let current_purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(1),
    };

    let load_error = TaskTechnicalError::StorageUnavailable {
        reason: String::from("load unavailable"),
    };
    let repository = FakeTaskRepository::new(
        Err(load_error.clone()),
        Ok(TaskCommitOutcome::CommittedAs(TaskRef {
            task,
            revision: revision(2),
        })),
    );
    let outcome = orchestrate_steering(
        &repository,
        proposal(expected, current_purpose, None, RawId::new()),
    )
    .await;
    match outcome {
        Err(error) => assert_eq!(error, load_error, "the load failure is not a stale premise"),
        Ok(other) => panic!("expected a technical error, got {other:?}"),
    }
    assert!(repository.forwarded().is_empty());

    let forward_error = TaskTechnicalError::StorageUnavailable {
        reason: String::from("forward unavailable"),
    };
    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(1),
            current_purpose,
            TaskContextEntryId::generate(),
        ))),
        Err(forward_error.clone()),
    );
    let outcome = orchestrate_steering(
        &repository,
        proposal(expected, current_purpose, None, RawId::new()),
    )
    .await;
    match outcome {
        Err(error) => assert_eq!(error, forward_error),
        Ok(other) => panic!("expected a technical error, got {other:?}"),
    }
}
