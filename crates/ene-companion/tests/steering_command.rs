//! H-A command-level steering mapping checks (IB §4 / §13.2).
//!
//! `propose_steering` is the companion-side entry point: it maps the Owner's
//! command onto the Task-side value premise and delegates to the Task
//! orchestration. The faked repository pins what crossed the boundary — the
//! boundary token, the proposed purpose text, and the instruction source —
//! and that no caller-minted context-entry identity or future revision can
//! appear in the command type.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures and helpers live outside #[test] functions, where clippy.toml's test allowances do not apply"
)]

use std::sync::Mutex;

use ene_companion::dialogue::{ProposeSteeringCommand, propose_steering};
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationRef,
    SteeringPremiseRef, Task, TaskCommitOutcome, TaskCommitPremise, TaskContextEntry,
    TaskContextEntryId, TaskContextItem, TaskContextOrigin, TaskContextOriginKind,
    TaskCreationPremise, TaskId, TaskProposalOutcome, TaskPurpose, TaskPurposeRef, TaskRecord,
    TaskRef, TaskRepository, TaskRevision, TaskRevisionRecord, TaskTechnicalError,
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

fn steer_command(
    expected: TaskRef,
    purpose: TaskPurposeRef,
    new_purpose: Option<TaskPurpose>,
    instruction_source: RawId,
) -> ProposeSteeringCommand {
    ProposeSteeringCommand {
        premise: SteeringPremiseRef { expected, purpose },
        new_purpose,
        instruction_source,
    }
}

#[tokio::test]
async fn propose_steering_maps_the_command_onto_the_task_premise() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(2),
    };
    let purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(2),
    };
    let existing_purpose_entry = TaskContextEntryId::generate();
    let instruction_source = RawId::new();
    let new_purpose = TaskPurpose {
        text: String::from("also cover the review findings"),
    };
    let accepted = TaskRef {
        task,
        revision: revision(3),
    };

    // Compile-level shape check: exhaustively destructuring the command proves
    // it carries exactly the boundary token, the proposed purpose text, and
    // the instruction source. A caller-minted entry identity or a future
    // revision would not compile here.
    let probe = steer_command(
        expected,
        purpose,
        Some(new_purpose.clone()),
        instruction_source,
    );
    let ProposeSteeringCommand {
        premise,
        new_purpose: probed_purpose,
        instruction_source: probed_source,
    } = probe;
    assert_eq!(premise, SteeringPremiseRef { expected, purpose });
    assert_eq!(probed_purpose, Some(new_purpose.clone()));
    assert_eq!(probed_source, instruction_source);

    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(2),
            purpose,
            existing_purpose_entry,
        ))),
        Ok(TaskCommitOutcome::CommittedAs(accepted)),
    );

    let outcome = propose_steering(
        steer_command(
            expected,
            purpose,
            Some(new_purpose.clone()),
            instruction_source,
        ),
        &repository,
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
        "one command commits at most one forward"
    );
    let premise = forwarded.first().expect("the forward premise was captured");
    assert_eq!(
        premise.expected, expected,
        "the command's boundary token crosses unchanged"
    );
    assert_eq!(
        premise
            .new_purpose
            .as_ref()
            .map(|adoption| &adoption.purpose),
        Some(&new_purpose),
        "the command's purpose text is what the Task owner adopts"
    );
    let instruction = premise
        .adopted_instruction
        .as_ref()
        .expect("the instruction is adopted");
    assert_eq!(
        instruction.origin,
        TaskContextOrigin {
            kind: TaskContextOriginKind::OwnerConversation,
            source: instruction_source,
        },
        "instruction_source crosses as the origin record, not as the adoption identity"
    );
    assert_ne!(
        premise.adopted_purpose_entry, existing_purpose_entry,
        "the Task owner mints a fresh purpose entry"
    );
    assert_ne!(
        instruction.entry, existing_purpose_entry,
        "the Task owner mints a fresh instruction entry"
    );
    assert_ne!(
        premise.adopted_purpose_entry, instruction.entry,
        "purpose and instruction entries are distinct identities"
    );
}
