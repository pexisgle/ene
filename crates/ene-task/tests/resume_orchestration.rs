//! Explicit resume orchestration checks (H-A.1 / AU17).
//!
//! The repository is faked so each test scripts the commit answer and
//! inspects the exact premise the orchestration composed: the minted
//! identities are fresh per call, the command and readiness travel
//! unchanged for both instruction sources, refusals pass through, and a
//! `ResultAvailable` answer routes the current revision's sealed results
//! through the existing adoption gate before answering.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures and helpers live outside #[test] functions, where clippy.toml's test allowances do not apply"
)]

mod fixture;

use std::collections::VecDeque;
use std::sync::Mutex;

use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, ConversationTaskRepository, DelegationCreationPremise, DelegationId,
    DelegationOutcome, DelegationRef, DelegationScope, OwnerMessageCurrentness,
    ResumeInstructionSource, ResumeTaskCommand, SteeringPremiseRef, Task, TaskAgentEphemeralId,
    TaskAgentOutput, TaskAgentResultArrival, TaskCancelOutcome, TaskCommitOutcome,
    TaskCommitPremise, TaskContextEntry, TaskContextEntryId, TaskContextItem, TaskContextOrigin,
    TaskContextOriginKind, TaskCreationPremise, TaskId, TaskProgress, TaskPurpose, TaskPurposeRef,
    TaskRecord, TaskRef, TaskReportRow, TaskReportRowCursor, TaskReportRowKind, TaskRepository,
    TaskResultAcceptance, TaskResultAdoptionClaim, TaskResultId, TaskResultRecord,
    TaskResumeCommitPremise, TaskResumeHold, TaskResumeOutcome, TaskResumeReadiness, TaskRevision,
    TaskRevisionRecord, TaskTechnicalError, orchestrate_resume, orchestrate_resume_current,
};

fn readiness() -> TaskResumeReadiness {
    TaskResumeReadiness {
        permission_available: true,
        execution_free: true,
        launch_possible: true,
    }
}

fn task_ref(task: TaskId, revision: u64) -> TaskRef {
    TaskRef {
        task,
        revision: TaskRevision::from_u64(revision),
    }
}

/// Scripted repository: one queued commit answer plus captures of the
/// premise, and the minimal reads the availability routing exercises.
struct FakeResumeRepository {
    commits: Mutex<VecDeque<Result<TaskResumeOutcome, TaskTechnicalError>>>,
    premises: Mutex<Vec<TaskResumeCommitPremise>>,
    record: Mutex<Option<TaskRecord>>,
    report_rows: Mutex<Vec<TaskReportRow>>,
    results: Mutex<Vec<TaskResultRecord>>,
    claims: Mutex<Vec<TaskResultAdoptionClaim>>,
}

impl FakeResumeRepository {
    fn new() -> Self {
        Self {
            commits: Mutex::new(VecDeque::new()),
            premises: Mutex::new(Vec::new()),
            record: Mutex::new(None),
            report_rows: Mutex::new(Vec::new()),
            results: Mutex::new(Vec::new()),
            claims: Mutex::new(Vec::new()),
        }
    }

    fn script_commit(&self, reply: Result<TaskResumeOutcome, TaskTechnicalError>) {
        self.commits
            .lock()
            .expect("fixture script is never poisoned")
            .push_back(reply);
    }

    fn premises(&self) -> Vec<TaskResumeCommitPremise> {
        self.premises
            .lock()
            .expect("fixture capture is never poisoned")
            .clone()
    }

    fn claims(&self) -> Vec<TaskResultAdoptionClaim> {
        self.claims
            .lock()
            .expect("fixture capture is never poisoned")
            .clone()
    }
}

impl TaskRepository for FakeResumeRepository {
    async fn record_task_agent_observation(
        &self,
        _premise: ene_task::TaskAgentObservationPremise,
    ) -> Result<ene_task::TaskAgentObservationId, ene_task::TaskTechnicalError> {
        Err(fixture::unsupported("record_task_agent_observation"))
    }
    async fn create_task(
        &self,
        _premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError> {
        Err(fixture::unsupported("create_task"))
    }

    async fn forward_steering(
        &self,
        _premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("forward_steering"))
    }

    async fn load_task(&self, _task: TaskId) -> Result<Option<TaskRecord>, TaskTechnicalError> {
        Ok(self
            .record
            .lock()
            .expect("fixture script is never poisoned")
            .clone())
    }

    async fn cancel_task(&self, _task: TaskId) -> Result<TaskCancelOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("cancel_task"))
    }

    async fn create_delegation(
        &self,
        _premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("create_delegation"))
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
    ) -> Result<ene_task::TaskResultArrivalOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("record_task_result_arrival"))
    }

    async fn load_task_result(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        Ok(self
            .results
            .lock()
            .expect("fixture script is never poisoned")
            .iter()
            .find(|stored| stored.result == result)
            .cloned())
    }

    async fn load_delegation_result(
        &self,
        _delegation: DelegationId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        Ok(None)
    }

    async fn delegation_has_started_work(
        &self,
        _delegation: DelegationId,
    ) -> Result<bool, TaskTechnicalError> {
        Ok(false)
    }

    async fn adopt_result(
        &self,
        claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError> {
        self.claims
            .lock()
            .expect("fixture capture is never poisoned")
            .push(claim.clone());
        Ok(TaskResultAcceptance::WithheldByEffectFacts {
            attempts: Vec::new(),
        })
    }

    async fn fail_task(
        &self,
        _premise: ene_task::TaskFailurePremise,
    ) -> Result<ene_task::TaskFailureOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("fail_task"))
    }

    async fn load_result_adoption_claim(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultAdoptionClaim>, TaskTechnicalError> {
        Ok(Some(TaskResultAdoptionClaim {
            result,
            attempt_refs: Vec::new(),
        }))
    }

    async fn list_unadopted_results_after(
        &self,
        _after: Option<ene_task::UnadoptedResultCursor>,
        _limit: u64,
    ) -> Result<Vec<ene_task::UnadoptedResultCursor>, TaskTechnicalError> {
        Ok(Vec::new())
    }

    async fn load_task_action_attempts(
        &self,
        _task: TaskId,
    ) -> Result<Vec<RawId>, TaskTechnicalError> {
        Ok(Vec::new())
    }

    async fn list_tasks_after(
        &self,
        _after: Option<TaskId>,
        _limit: u32,
    ) -> Result<Vec<ene_task::TaskHeadline>, TaskTechnicalError> {
        Err(fixture::unsupported("list_tasks_after"))
    }

    async fn list_task_report_rows_after(
        &self,
        _task: TaskId,
        after: Option<TaskReportRowCursor>,
        limit: u32,
    ) -> Result<Vec<TaskReportRow>, TaskTechnicalError> {
        let rows = self
            .report_rows
            .lock()
            .expect("fixture script is never poisoned")
            .clone();
        let start = match after {
            None => 0,
            Some(cursor) => rows
                .iter()
                .position(|row| row.kind == cursor.kind && row.id == cursor.id)
                .map_or(rows.len(), |index| index + 1),
        };
        Ok(rows.into_iter().skip(start).take(limit as usize).collect())
    }

    async fn load_report_source_bounded(
        &self,
        _source: ene_task::TaskReportSourceRef,
        _cursor_bytes: u64,
        _limit_bytes: u32,
    ) -> Result<Option<ene_task::TaskReportSourcePage>, TaskTechnicalError> {
        Err(fixture::unsupported("load_report_source_bounded"))
    }

    async fn commit_task_resume(
        &self,
        premise: TaskResumeCommitPremise,
    ) -> Result<TaskResumeOutcome, TaskTechnicalError> {
        self.premises
            .lock()
            .expect("fixture capture is never poisoned")
            .push(premise);
        self.commits
            .lock()
            .expect("fixture script is never poisoned")
            .pop_front()
            .expect("every exercised resume has a scripted reply")
    }

    async fn load_past_executed_facts(
        &self,
        _task: TaskId,
    ) -> Result<ene_task::PastExecutedFactsPage, TaskTechnicalError> {
        Ok(ene_task::PastExecutedFactsPage {
            facts: Vec::new(),
            has_more: false,
        })
    }
}

fn command(task: TaskRef, purpose: TaskPurposeRef) -> ResumeTaskCommand {
    ResumeTaskCommand {
        premise: SteeringPremiseRef {
            expected: task,
            purpose,
        },
        instruction: ResumeInstructionSource::OwnerHistory {
            message: RawId::new(),
            currentness: OwnerMessageCurrentness {
                companion: RawId::new(),
                message: RawId::new(),
            },
        },
    }
}

fn resumed(task: TaskRef) -> TaskResumeOutcome {
    TaskResumeOutcome::Resumed {
        task,
        delegation: DelegationRef {
            delegation: DelegationId::generate(),
            task,
            delegator: AssigneeRef {
                companion: RawId::new(),
            },
            agent: TaskAgentEphemeralId::generate(),
            scope: DelegationScope { workspace: None },
        },
    }
}

#[tokio::test]
async fn resume_mints_fresh_identities_and_passes_the_command_through() {
    let repository = FakeResumeRepository::new();
    let task = TaskId::generate();
    let relied = task_ref(task, 3);
    let purpose = TaskPurposeRef {
        task,
        adopted_revision: TaskRevision::from_u64(3),
    };
    let instruction = ResumeInstructionSource::OwnerManagement {
        activity: RawId::new(),
    };
    repository.script_commit(Ok(resumed(task_ref(task, 4))));

    let outcome = orchestrate_resume(
        &repository,
        ResumeTaskCommand {
            premise: SteeringPremiseRef {
                expected: relied,
                purpose,
            },
            instruction,
        },
        readiness(),
    )
    .await
    .expect("a committed resume is a domain outcome");

    assert!(matches!(outcome, TaskResumeOutcome::Resumed { .. }));
    let premises = repository.premises();
    assert_eq!(premises.len(), 1);
    let premise = premises.first().expect("the premise was captured");
    assert_eq!(premise.command.premise.expected, relied);
    assert_eq!(premise.command.premise.purpose, purpose);
    assert_eq!(premise.command.instruction, instruction);
    assert_eq!(premise.readiness, readiness());

    // A second resume mints entirely fresh identities: the caller never
    // names an entry, delegation, or agent.
    repository.script_commit(Ok(resumed(task_ref(task, 5))));
    orchestrate_resume(
        &repository,
        ResumeTaskCommand {
            premise: SteeringPremiseRef {
                expected: task_ref(task, 4),
                purpose,
            },
            instruction,
        },
        readiness(),
    )
    .await
    .expect("a second resume is a domain outcome");
    let premises = repository.premises();
    assert_eq!(premises.len(), 2);
    assert_ne!(
        premises[0].adopted_purpose_entry,
        premises[1].adopted_purpose_entry
    );
    assert_ne!(
        premises[0].adopted_instruction_entry,
        premises[1].adopted_instruction_entry
    );
    assert_ne!(premises[0].delegation, premises[1].delegation);
    assert_ne!(premises[0].agent, premises[1].agent);
}

#[tokio::test]
async fn both_instruction_sources_travel_unchanged() {
    for instruction in [
        ResumeInstructionSource::OwnerHistory {
            message: RawId::new(),
            currentness: OwnerMessageCurrentness {
                companion: RawId::new(),
                message: RawId::new(),
            },
        },
        ResumeInstructionSource::OwnerManagement {
            activity: RawId::new(),
        },
    ] {
        let repository = FakeResumeRepository::new();
        let task = TaskId::generate();
        let relied = task_ref(task, 1);
        let purpose = TaskPurposeRef {
            task,
            adopted_revision: TaskRevision::from_u64(1),
        };
        repository.script_commit(Ok(resumed(task_ref(task, 2))));
        let outcome = orchestrate_resume(
            &repository,
            ResumeTaskCommand {
                premise: SteeringPremiseRef {
                    expected: relied,
                    purpose,
                },
                instruction,
            },
            readiness(),
        )
        .await
        .expect("a committed resume is a domain outcome");
        assert!(matches!(outcome, TaskResumeOutcome::Resumed { .. }));
        let premises = repository.premises();
        assert_eq!(
            premises
                .first()
                .expect("the premise was captured")
                .command
                .instruction,
            instruction,
            "the source travels to the commit without reinterpretation"
        );
    }
}

#[tokio::test]
async fn refusals_pass_through_without_routing() {
    for outcome in [
        TaskResumeOutcome::StalePremise {
            current: task_ref(TaskId::generate(), 2),
        },
        TaskResumeOutcome::Superseded,
        TaskResumeOutcome::TaskTerminal {
            task: TaskId::generate(),
            progress: TaskProgress::Cancelled,
        },
        TaskResumeOutcome::AlreadyRunning {
            task: TaskId::generate(),
        },
        TaskResumeOutcome::HeldByUnknownEffects {
            task: TaskId::generate(),
        },
        TaskResumeOutcome::NeedsRevalidation(TaskResumeHold::WorkspaceUnavailable),
        TaskResumeOutcome::MissingTask {
            task: TaskId::generate(),
        },
        TaskResumeOutcome::RevisionExhausted {
            task: TaskId::generate(),
        },
    ] {
        let repository = FakeResumeRepository::new();
        let task = TaskId::generate();
        repository.script_commit(Ok(outcome.clone()));
        let answered = orchestrate_resume(
            &repository,
            command(
                task_ref(task, 1),
                TaskPurposeRef {
                    task,
                    adopted_revision: TaskRevision::from_u64(1),
                },
            ),
            readiness(),
        )
        .await
        .expect("a refusal is a domain outcome, not a technical error");
        assert_eq!(answered, outcome);
        assert!(
            repository.claims().is_empty(),
            "only ResultAvailable routes to adoption, got {outcome:?}"
        );
    }
}

#[tokio::test]
async fn result_available_routes_the_current_revision_through_adoption() {
    let repository = FakeResumeRepository::new();
    let task = TaskId::generate();
    let current = task_ref(task, 2);
    let result = TaskResultId::generate();
    let delegation = DelegationId::generate();
    repository.script_commit(Ok(TaskResumeOutcome::ResultAvailable { task }));
    *repository
        .record
        .lock()
        .expect("fixture script is never poisoned") = Some(TaskRecord {
        task: Task {
            reference: current,
            purpose: TaskPurposeRef {
                task,
                adopted_revision: TaskRevision::from_u64(2),
            },
            assignee: AssigneeRef {
                companion: RawId::new(),
            },
            progress: TaskProgress::InProgress,
            adopted_result: None,
        },
        revision: TaskRevisionRecord {
            reference: current,
            purpose: TaskPurposeRef {
                task,
                adopted_revision: TaskRevision::from_u64(2),
            },
            purpose_text: TaskPurpose {
                text: String::from("probe purpose"),
            },
            assignee: AssigneeRef {
                companion: RawId::new(),
            },
        },
        context: vec![TaskContextEntry {
            entry: TaskContextEntryId::generate(),
            reference: current,
            item: TaskContextItem::AdoptedPurpose(TaskPurposeRef {
                task,
                adopted_revision: TaskRevision::from_u64(2),
            }),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
            },
            acquired_at: WallClockWithTz::now(),
        }],
        workspace: None,
    });
    *repository
        .report_rows
        .lock()
        .expect("fixture script is never poisoned") = vec![TaskReportRow {
        kind: TaskReportRowKind::TaskResult,
        id: result.as_raw(),
        adopted_revision: None,
    }];
    *repository
        .results
        .lock()
        .expect("fixture script is never poisoned") = vec![TaskResultRecord {
        result,
        task: current,
        delegation,
        body: TaskAgentOutput::new(String::from("the sealed body")),
        attempt_refs: Vec::new(),
        adopted_revision: None,
        recorded_at: WallClockWithTz::now(),
    }];

    let outcome = orchestrate_resume(
        &repository,
        command(
            current,
            TaskPurposeRef {
                task,
                adopted_revision: TaskRevision::from_u64(2),
            },
        ),
        readiness(),
    )
    .await
    .expect("an available result is a domain outcome");

    assert_eq!(outcome, TaskResumeOutcome::ResultAvailable { task });
    let claims = repository.claims();
    assert_eq!(claims.len(), 1, "the sealed result is re-evaluated once");
    assert_eq!(
        claims.first().expect("the claim was captured").result,
        result
    );
}

#[tokio::test]
async fn result_available_skips_results_of_other_revisions() {
    let repository = FakeResumeRepository::new();
    let task = TaskId::generate();
    let current = task_ref(task, 3);
    let old = TaskResultId::generate();
    repository.script_commit(Ok(TaskResumeOutcome::ResultAvailable { task }));
    *repository
        .record
        .lock()
        .expect("fixture script is never poisoned") = Some(TaskRecord {
        task: Task {
            reference: current,
            purpose: TaskPurposeRef {
                task,
                adopted_revision: TaskRevision::from_u64(3),
            },
            assignee: AssigneeRef {
                companion: RawId::new(),
            },
            progress: TaskProgress::InProgress,
            adopted_result: None,
        },
        revision: TaskRevisionRecord {
            reference: current,
            purpose: TaskPurposeRef {
                task,
                adopted_revision: TaskRevision::from_u64(3),
            },
            purpose_text: TaskPurpose {
                text: String::from("probe purpose"),
            },
            assignee: AssigneeRef {
                companion: RawId::new(),
            },
        },
        context: Vec::new(),
        workspace: None,
    });
    *repository
        .report_rows
        .lock()
        .expect("fixture script is never poisoned") = vec![TaskReportRow {
        kind: TaskReportRowKind::TaskResult,
        id: old.as_raw(),
        adopted_revision: None,
    }];
    *repository
        .results
        .lock()
        .expect("fixture script is never poisoned") = vec![TaskResultRecord {
        result: old,
        task: task_ref(task, 2),
        delegation: DelegationId::generate(),
        body: TaskAgentOutput::new(String::from("the old answer")),
        attempt_refs: Vec::new(),
        adopted_revision: None,
        recorded_at: WallClockWithTz::now(),
    }];

    let outcome = orchestrate_resume(
        &repository,
        command(
            current,
            TaskPurposeRef {
                task,
                adopted_revision: TaskRevision::from_u64(3),
            },
        ),
        readiness(),
    )
    .await
    .expect("an available result is a domain outcome");

    assert_eq!(outcome, TaskResumeOutcome::ResultAvailable { task });
    assert!(
        repository.claims().is_empty(),
        "a moved revision's result is history, not a routing candidate"
    );
}

#[tokio::test]
async fn commit_technical_errors_stay_errors() {
    let repository = FakeResumeRepository::new();
    let task = TaskId::generate();
    repository.script_commit(Err(TaskTechnicalError::StorageUnavailable {
        reason: String::from("the commit is unreachable"),
    }));
    let outcome = orchestrate_resume(
        &repository,
        command(
            task_ref(task, 1),
            TaskPurposeRef {
                task,
                adopted_revision: TaskRevision::from_u64(1),
            },
        ),
        readiness(),
    )
    .await;
    assert_eq!(
        outcome,
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("the commit is unreachable"),
        })
    );
}

/// Guarded-commit fake: the same scripted answers behind the
/// conversation-sourced boundary.
struct GuardedResumeRepository {
    inner: FakeResumeRepository,
    currentness: Mutex<Vec<OwnerMessageCurrentness>>,
}

impl GuardedResumeRepository {
    fn new(inner: FakeResumeRepository) -> Self {
        Self {
            inner,
            currentness: Mutex::new(Vec::new()),
        }
    }
}

impl TaskRepository for GuardedResumeRepository {
    async fn record_task_agent_observation(
        &self,
        _premise: ene_task::TaskAgentObservationPremise,
    ) -> Result<ene_task::TaskAgentObservationId, ene_task::TaskTechnicalError> {
        Err(fixture::unsupported("record_task_agent_observation"))
    }
    async fn create_task(
        &self,
        premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError> {
        self.inner.create_task(premise).await
    }

    async fn forward_steering(
        &self,
        premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        self.inner.forward_steering(premise).await
    }

    async fn load_task(&self, task: TaskId) -> Result<Option<TaskRecord>, TaskTechnicalError> {
        self.inner.load_task(task).await
    }

    async fn cancel_task(&self, task: TaskId) -> Result<TaskCancelOutcome, TaskTechnicalError> {
        self.inner.cancel_task(task).await
    }

    async fn create_delegation(
        &self,
        premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError> {
        self.inner.create_delegation(premise).await
    }

    async fn load_delegation(
        &self,
        delegation: DelegationId,
    ) -> Result<Option<DelegationRef>, TaskTechnicalError> {
        self.inner.load_delegation(delegation).await
    }

    async fn record_task_result_arrival(
        &self,
        arrival: TaskAgentResultArrival,
    ) -> Result<ene_task::TaskResultArrivalOutcome, TaskTechnicalError> {
        self.inner.record_task_result_arrival(arrival).await
    }

    async fn load_task_result(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        self.inner.load_task_result(result).await
    }

    async fn load_delegation_result(
        &self,
        delegation: DelegationId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        self.inner.load_delegation_result(delegation).await
    }

    async fn delegation_has_started_work(
        &self,
        delegation: DelegationId,
    ) -> Result<bool, TaskTechnicalError> {
        self.inner.delegation_has_started_work(delegation).await
    }

    async fn adopt_result(
        &self,
        claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError> {
        self.inner.adopt_result(claim).await
    }

    async fn fail_task(
        &self,
        premise: ene_task::TaskFailurePremise,
    ) -> Result<ene_task::TaskFailureOutcome, TaskTechnicalError> {
        self.inner.fail_task(premise).await
    }

    async fn load_result_adoption_claim(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultAdoptionClaim>, TaskTechnicalError> {
        self.inner.load_result_adoption_claim(result).await
    }

    async fn list_unadopted_results_after(
        &self,
        after: Option<ene_task::UnadoptedResultCursor>,
        limit: u64,
    ) -> Result<Vec<ene_task::UnadoptedResultCursor>, TaskTechnicalError> {
        self.inner.list_unadopted_results_after(after, limit).await
    }

    async fn load_task_action_attempts(
        &self,
        task: TaskId,
    ) -> Result<Vec<RawId>, TaskTechnicalError> {
        self.inner.load_task_action_attempts(task).await
    }

    async fn list_tasks_after(
        &self,
        after: Option<TaskId>,
        limit: u32,
    ) -> Result<Vec<ene_task::TaskHeadline>, TaskTechnicalError> {
        self.inner.list_tasks_after(after, limit).await
    }

    async fn list_task_report_rows_after(
        &self,
        task: TaskId,
        after: Option<TaskReportRowCursor>,
        limit: u32,
    ) -> Result<Vec<TaskReportRow>, TaskTechnicalError> {
        self.inner
            .list_task_report_rows_after(task, after, limit)
            .await
    }

    async fn load_report_source_bounded(
        &self,
        source: ene_task::TaskReportSourceRef,
        cursor_bytes: u64,
        limit_bytes: u32,
    ) -> Result<Option<ene_task::TaskReportSourcePage>, TaskTechnicalError> {
        self.inner
            .load_report_source_bounded(source, cursor_bytes, limit_bytes)
            .await
    }

    async fn commit_task_resume(
        &self,
        premise: TaskResumeCommitPremise,
    ) -> Result<TaskResumeOutcome, TaskTechnicalError> {
        self.inner.commit_task_resume(premise).await
    }

    async fn load_past_executed_facts(
        &self,
        task: TaskId,
    ) -> Result<ene_task::PastExecutedFactsPage, TaskTechnicalError> {
        self.inner.load_past_executed_facts(task).await
    }
}

impl ConversationTaskRepository for GuardedResumeRepository {
    async fn create_task_from_conversation(
        &self,
        _premise: TaskCreationPremise,
        _currentness: OwnerMessageCurrentness,
    ) -> Result<ene_task::TaskProposalOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("creation"))
    }

    async fn forward_steering_from_conversation(
        &self,
        _premise: TaskCommitPremise,
        _currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("steering"))
    }

    async fn cancel_task_from_conversation(
        &self,
        _task: TaskId,
        _currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCancelOutcome, TaskTechnicalError> {
        Err(fixture::unsupported("cancel"))
    }

    async fn commit_task_resume_from_conversation(
        &self,
        premise: TaskResumeCommitPremise,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskResumeOutcome, TaskTechnicalError> {
        self.currentness
            .lock()
            .expect("fixture capture is never poisoned")
            .push(currentness);
        self.inner.commit_task_resume(premise).await
    }
}

#[tokio::test]
async fn guarded_resume_carries_the_currentness_to_the_commit() {
    let inner = FakeResumeRepository::new();
    let task = TaskId::generate();
    inner.script_commit(Ok(resumed(task_ref(task, 2))));
    let repository = GuardedResumeRepository::new(inner);
    let currentness = OwnerMessageCurrentness {
        companion: RawId::new(),
        message: RawId::new(),
    };

    let outcome = orchestrate_resume_current(
        &repository,
        command(
            task_ref(task, 1),
            TaskPurposeRef {
                task,
                adopted_revision: TaskRevision::from_u64(1),
            },
        ),
        readiness(),
        currentness,
    )
    .await
    .expect("a guarded resume is a domain outcome");

    assert!(matches!(outcome, TaskResumeOutcome::Resumed { .. }));
    assert_eq!(
        repository
            .currentness
            .lock()
            .expect("fixture capture is never poisoned")
            .clone(),
        vec![currentness]
    );
}

#[tokio::test]
async fn resume_commit_premise_has_no_workspace_field() {
    // The resume commit freezes its scope from the current association, so
    // this pins that the premise type carries no workspace copy of its own.
    let _premise = TaskResumeCommitPremise {
        command: command(
            task_ref(TaskId::generate(), 1),
            TaskPurposeRef {
                task: TaskId::generate(),
                adopted_revision: TaskRevision::from_u64(1),
            },
        ),
        readiness: readiness(),
        adopted_purpose_entry: TaskContextEntryId::generate(),
        adopted_instruction_entry: TaskContextEntryId::generate(),
        delegation: DelegationId::generate(),
        agent: TaskAgentEphemeralId::generate(),
        accepted_at: WallClockWithTz::now(),
    };
}
