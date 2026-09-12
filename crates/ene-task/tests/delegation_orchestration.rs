//! H-A delegation orchestration boundary checks (IB §4 / §13.2, AU3).
//!
//! The Task repository is faked so each test can script `load_task` and
//! `create_delegation` and inspect the exact `DelegationCreationPremise` the
//! orchestration composed. The checks pin the precheck outcomes that must not
//! create anything (missing Task, stale revision), the identity minting for
//! the delegation and its ephemeral Task Agent, the workspace scope copy that
//! crosses unchanged, and the outcome mapping: domain outcomes stay on the
//! `Ok` side while repository technical failures stay `Err`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures and helpers live outside #[test] functions, where clippy.toml's test allowances do not apply"
)]

use std::collections::VecDeque;
use std::sync::Mutex;

use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, CreateDelegationCommand, DelegatedWorkspace, DelegationCreationPremise,
    DelegationId, DelegationOutcome, DelegationRef, DelegationScope, Task, TaskCommitOutcome,
    TaskCommitPremise, TaskContextEntry, TaskContextEntryId, TaskContextItem, TaskContextOrigin,
    TaskContextOriginKind, TaskCreationPremise, TaskId, TaskPurpose, TaskPurposeRef, TaskRecord,
    TaskRef, TaskRepository, TaskRevision, TaskRevisionRecord, TaskTechnicalError,
    WorkspaceAssocId, WorkspaceFolderRef, orchestrate_delegation,
};

/// The repository-side script for one `create_delegation` call.
///
/// The success reply carries no identity: the fixture echoes the identities
/// the orchestration minted into the committed [`DelegationRef`], exactly as
/// the durable repository would.
enum FakeDelegationReply {
    Delegated,
    Stale(TaskRef),
    Missing(TaskId),
}

/// Scripted `TaskRepository`: one `load_task` fixture, a queue of
/// `create_delegation` replies, a capture of every
/// `DelegationCreationPremise` handed to `create_delegation`, and the
/// delegator the fixture stamps on a successful commit.
struct FakeTaskRepository {
    load: Mutex<Result<Option<TaskRecord>, TaskTechnicalError>>,
    delegation_replies: Mutex<VecDeque<Result<FakeDelegationReply, TaskTechnicalError>>>,
    delegated: Mutex<Vec<DelegationCreationPremise>>,
    delegator: AssigneeRef,
}

impl FakeTaskRepository {
    fn new(load: Result<Option<TaskRecord>, TaskTechnicalError>, delegator: AssigneeRef) -> Self {
        Self {
            load: Mutex::new(load),
            delegation_replies: Mutex::new(VecDeque::new()),
            delegated: Mutex::new(Vec::new()),
            delegator,
        }
    }

    fn script_delegation(&self, reply: Result<FakeDelegationReply, TaskTechnicalError>) {
        self.delegation_replies
            .lock()
            .expect("fixture script is never poisoned")
            .push_back(reply);
    }

    fn delegated(&self) -> Vec<DelegationCreationPremise> {
        self.delegated
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
        _premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("forward_steering is outside this fixture's scope"),
        })
    }

    async fn load_task(&self, _task: TaskId) -> Result<Option<TaskRecord>, TaskTechnicalError> {
        self.load
            .lock()
            .expect("fixture script is never poisoned")
            .clone()
    }

    async fn create_delegation(
        &self,
        premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError> {
        self.delegated
            .lock()
            .expect("fixture capture is never poisoned")
            .push(premise.clone());
        let reply = self
            .delegation_replies
            .lock()
            .expect("fixture script is never poisoned")
            .pop_front()
            .expect("every exercised create_delegation call has a scripted reply");
        Ok(match reply? {
            FakeDelegationReply::Delegated => DelegationOutcome::Delegated(DelegationRef {
                delegation: premise.delegation,
                task: premise.task,
                delegator: self.delegator,
                agent: premise.agent,
                scope: premise.scope_copy,
            }),
            FakeDelegationReply::Stale(current) => DelegationOutcome::StaleTaskRevision { current },
            FakeDelegationReply::Missing(task) => DelegationOutcome::MissingTask { task },
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

fn assignee() -> AssigneeRef {
    AssigneeRef {
        companion: RawId::new(),
    }
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

fn command(task: TaskRef, scope_copy: DelegationScope) -> CreateDelegationCommand {
    CreateDelegationCommand { task, scope_copy }
}

#[tokio::test]
async fn missing_task_is_reported_without_creating_a_delegation() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let repository = FakeTaskRepository::new(Ok(None), assignee());

    let outcome = orchestrate_delegation(
        &repository,
        command(expected, DelegationScope { workspace: None }),
    )
    .await
    .expect("a missing Task is a domain outcome, not a technical error");

    match outcome {
        DelegationOutcome::MissingTask { task: found } => assert_eq!(found, task),
        other => panic!("expected MissingTask, got {other:?}"),
    }
    assert!(
        repository.delegated().is_empty(),
        "a missing Task must not create a delegation"
    );
}

#[tokio::test]
async fn stale_revision_reports_current_without_creating_a_delegation() {
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
        assignee(),
    );
    repository.script_delegation(Ok(FakeDelegationReply::Delegated));

    let outcome = orchestrate_delegation(
        &repository,
        command(expected, DelegationScope { workspace: None }),
    )
    .await
    .expect("a stale premise is a domain outcome, not a technical error");

    match outcome {
        DelegationOutcome::StaleTaskRevision { current: found } => assert_eq!(found, current),
        other => panic!("expected StaleTaskRevision, got {other:?}"),
    }
    assert!(
        repository.delegated().is_empty(),
        "a stale revision must not create a delegation"
    );
}

#[tokio::test]
async fn delegated_outcome_echoes_the_minted_identities_and_the_command() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(1),
    };
    let delegator = assignee();
    let scope_copy = DelegationScope { workspace: None };
    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(1),
            purpose,
            TaskContextEntryId::generate(),
        ))),
        delegator,
    );
    repository.script_delegation(Ok(FakeDelegationReply::Delegated));

    // Compile-level shape check: exhaustively destructuring the command proves
    // it carries exactly the boundary task token and the scope copy. A
    // caller-minted delegation or agent identity, or a placeholder field for a
    // producer that does not exist yet, would not compile here.
    let CreateDelegationCommand {
        task: probed_task,
        scope_copy: probed_scope,
    } = command(expected, scope_copy.clone());
    assert_eq!(probed_task, expected);
    assert_eq!(probed_scope, scope_copy);

    let outcome = orchestrate_delegation(&repository, command(expected, scope_copy.clone()))
        .await
        .expect("a delegation decision is a domain outcome, not a technical error");

    let captured = repository.delegated();
    assert_eq!(
        captured.len(),
        1,
        "one command creates at most one delegation"
    );
    let premise = captured
        .first()
        .expect("the delegation premise was captured");

    match outcome {
        DelegationOutcome::Delegated(found) => {
            assert_eq!(
                found.delegation, premise.delegation,
                "the minted delegation identity is the one the repository committed"
            );
            assert_eq!(
                found.agent, premise.agent,
                "the minted ephemeral agent identity is the one the repository committed"
            );
            assert_eq!(
                found.task, expected,
                "the committed delegation relies on the command's revision"
            );
            assert_eq!(
                found.delegator, delegator,
                "the repository stamps the delegator; the orchestration never passes one"
            );
            assert_eq!(
                found.scope, scope_copy,
                "the command's scope copy crosses unchanged"
            );
        }
        other => panic!("expected Delegated, got {other:?}"),
    }
    assert_eq!(
        premise.task, expected,
        "the precheck's task token crosses unchanged"
    );
    assert_eq!(
        premise.scope_copy, scope_copy,
        "the precheck's scope copy crosses unchanged"
    );
}

#[tokio::test]
async fn technical_failures_stay_errors() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };

    let load_error = TaskTechnicalError::StorageUnavailable {
        reason: String::from("load unavailable"),
    };
    let repository = FakeTaskRepository::new(Err(load_error.clone()), assignee());
    let outcome = orchestrate_delegation(
        &repository,
        command(expected, DelegationScope { workspace: None }),
    )
    .await;
    match outcome {
        Err(error) => assert_eq!(
            error, load_error,
            "the load failure is not a stale revision"
        ),
        Ok(other) => panic!("expected a technical error, got {other:?}"),
    }
    assert!(repository.delegated().is_empty());

    let create_error = TaskTechnicalError::StorageUnavailable {
        reason: String::from("create_delegation unavailable"),
    };
    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(1),
            TaskPurposeRef {
                task,
                adopted_revision: revision(1),
            },
            TaskContextEntryId::generate(),
        ))),
        assignee(),
    );
    repository.script_delegation(Err(create_error.clone()));
    let outcome = orchestrate_delegation(
        &repository,
        command(expected, DelegationScope { workspace: None }),
    )
    .await;
    match outcome {
        Err(error) => assert_eq!(error, create_error),
        Ok(other) => panic!("expected a technical error, got {other:?}"),
    }
    assert_eq!(
        repository.delegated().len(),
        1,
        "the commit was attempted before the technical failure"
    );
}

#[tokio::test]
async fn delegated_workspace_scope_copy_crosses_unchanged() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let purpose = TaskPurposeRef {
        task,
        adopted_revision: revision(1),
    };
    let folder = WorkspaceFolderRef {
        path: String::from("/workspace/inbox/"),
    };
    let save_target = WorkspaceFolderRef {
        path: String::from("/workspace/outbox/"),
    };
    let scope_copy = DelegationScope {
        workspace: Some(DelegatedWorkspace {
            assoc: WorkspaceAssocId::generate(),
            folder: folder.clone(),
            save_target: Some(save_target.clone()),
        }),
    };
    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(1),
            purpose,
            TaskContextEntryId::generate(),
        ))),
        assignee(),
    );
    repository.script_delegation(Ok(FakeDelegationReply::Delegated));

    let outcome = orchestrate_delegation(&repository, command(expected, scope_copy.clone()))
        .await
        .expect("a delegation decision is a domain outcome, not a technical error");

    let captured = repository.delegated();
    let premise = captured
        .first()
        .expect("the delegation premise was captured");
    assert_eq!(
        premise.scope_copy, scope_copy,
        "the Some(DelegatedWorkspace) copy must cross byte-identically, never normalized"
    );
    // Pin the original field values directly: the copy is the creation-time
    // projection, so folder and save target must survive whole-struct equality.
    let workspace = premise
        .scope_copy
        .workspace
        .as_ref()
        .expect("the workspace copy crosses");
    assert_eq!(workspace.folder, folder);
    assert_eq!(workspace.save_target, Some(save_target));

    match outcome {
        DelegationOutcome::Delegated(found) => {
            assert_eq!(
                found.scope, scope_copy,
                "the committed copy is the command's copy"
            );
        }
        other => panic!("expected Delegated, got {other:?}"),
    }
}

#[tokio::test]
async fn repository_stale_and_missing_outcomes_pass_through() {
    let task = TaskId::generate();
    let expected = TaskRef {
        task,
        revision: revision(1),
    };
    let purpose = TaskPurposeRef {
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
            purpose,
            TaskContextEntryId::generate(),
        ))),
        assignee(),
    );
    repository.script_delegation(Ok(FakeDelegationReply::Stale(raced)));
    let outcome = orchestrate_delegation(
        &repository,
        command(expected, DelegationScope { workspace: None }),
    )
    .await
    .expect("a lost compare is a domain outcome, not a technical error");
    match outcome {
        DelegationOutcome::StaleTaskRevision { current: found } => assert_eq!(found, raced),
        other => panic!("expected StaleTaskRevision, got {other:?}"),
    }

    let repository = FakeTaskRepository::new(
        Ok(Some(record(
            task,
            revision(1),
            purpose,
            TaskContextEntryId::generate(),
        ))),
        assignee(),
    );
    repository.script_delegation(Ok(FakeDelegationReply::Missing(task)));
    let outcome = orchestrate_delegation(
        &repository,
        command(expected, DelegationScope { workspace: None }),
    )
    .await
    .expect("a missing Task from the commit is a domain outcome");
    match outcome {
        DelegationOutcome::MissingTask { task: found } => assert_eq!(found, task),
        other => panic!("expected MissingTask, got {other:?}"),
    }
}
