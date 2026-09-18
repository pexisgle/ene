//! Task proposal orchestration (AU2) and sealed-result re-evaluation
//! orchestration: identity minting on the Task side and the reuse of the
//! existing adoption gate.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures and helpers live outside #[test] functions, where clippy.toml's test allowances do not apply"
)]

use std::sync::Mutex;

use ene_primitive::RawId;
use ene_task::{
    AssigneeRef, DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationRef,
    TaskCancelOutcome, TaskCommitOutcome, TaskCommitPremise, TaskContextOrigin,
    TaskContextOriginKind, TaskCreationPremise, TaskId, TaskProposalOutcome, TaskProposalPremise,
    TaskPurpose, TaskRef, TaskRepository, TaskResultAcceptance, TaskResultAdoptionClaim,
    TaskResultId, TaskResultRecord, TaskRevision, TaskTechnicalError, WorkspaceFolderRef,
    WorkspaceNeedRef, orchestrate_task_creation, reevaluate_result_adoption,
};

#[derive(Default)]
struct FakeTaskRepository {
    created: Mutex<Vec<TaskCreationPremise>>,
    /// `None` models an absent result row.
    claim: Mutex<Option<TaskResultAdoptionClaim>>,
    adopted: Mutex<Vec<TaskResultAdoptionClaim>>,
    acceptance: Mutex<Option<TaskResultAcceptance>>,
}

impl FakeTaskRepository {
    fn created(&self) -> Vec<TaskCreationPremise> {
        self.created
            .lock()
            .expect("fixture capture is never poisoned")
            .clone()
    }

    fn adopted(&self) -> Vec<TaskResultAdoptionClaim> {
        self.adopted
            .lock()
            .expect("fixture capture is never poisoned")
            .clone()
    }

    fn script_claim(&self, claim: TaskResultAdoptionClaim) {
        *self.claim.lock().expect("fixture script is never poisoned") = Some(claim);
    }

    fn script_acceptance(&self, acceptance: TaskResultAcceptance) {
        *self
            .acceptance
            .lock()
            .expect("fixture script is never poisoned") = Some(acceptance);
    }
}

fn unsupported(method: &str) -> TaskTechnicalError {
    TaskTechnicalError::StorageUnavailable {
        reason: format!("{method} is outside this fixture's scope"),
    }
}

impl TaskRepository for FakeTaskRepository {
    async fn record_task_agent_observation(
        &self,
        _premise: ene_task::TaskAgentObservationPremise,
    ) -> Result<ene_task::TaskAgentObservationId, ene_task::TaskTechnicalError> {
        Err(ene_task::TaskTechnicalError::StorageUnavailable {
            reason: String::from("record_task_agent_observation is outside this fixture's scope"),
        })
    }
    async fn create_task(
        &self,
        premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError> {
        let reference = TaskRef {
            task: premise.task,
            revision: TaskRevision::initial(),
        };
        self.created
            .lock()
            .expect("fixture capture is never poisoned")
            .push(premise);
        Ok(reference)
    }

    async fn forward_steering(
        &self,
        _premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        Err(unsupported("forward_steering"))
    }

    async fn load_task(
        &self,
        _task: TaskId,
    ) -> Result<Option<ene_task::TaskRecord>, TaskTechnicalError> {
        Err(unsupported("load_task"))
    }

    async fn cancel_task(&self, _task: TaskId) -> Result<TaskCancelOutcome, TaskTechnicalError> {
        Err(unsupported("cancel_task"))
    }

    async fn fail_task(
        &self,
        _premise: ene_task::TaskFailurePremise,
    ) -> Result<ene_task::TaskFailureOutcome, TaskTechnicalError> {
        Err(unsupported("fail_task"))
    }

    async fn create_delegation(
        &self,
        _premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError> {
        Err(unsupported("create_delegation"))
    }

    async fn load_delegation(
        &self,
        _delegation: DelegationId,
    ) -> Result<Option<DelegationRef>, TaskTechnicalError> {
        Err(unsupported("load_delegation"))
    }

    async fn record_task_result_arrival(
        &self,
        _arrival: ene_task::TaskAgentResultArrival,
    ) -> Result<ene_task::TaskResultArrivalOutcome, TaskTechnicalError> {
        Err(unsupported("record_task_result_arrival"))
    }

    async fn load_task_result(
        &self,
        _result: TaskResultId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        Err(unsupported("load_task_result"))
    }

    async fn load_delegation_result(
        &self,
        _delegation: DelegationId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        Err(unsupported("load_delegation_result"))
    }

    async fn delegation_has_started_work(
        &self,
        _delegation: DelegationId,
    ) -> Result<bool, TaskTechnicalError> {
        Err(unsupported("delegation_has_started_work"))
    }

    async fn adopt_result(
        &self,
        claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError> {
        self.adopted
            .lock()
            .expect("fixture capture is never poisoned")
            .push(claim);
        self.acceptance
            .lock()
            .expect("fixture script is never poisoned")
            .clone()
            .ok_or_else(|| unsupported("adopt_result"))
    }

    async fn load_result_adoption_claim(
        &self,
        _result: TaskResultId,
    ) -> Result<Option<TaskResultAdoptionClaim>, TaskTechnicalError> {
        Ok(self
            .claim
            .lock()
            .expect("fixture script is never poisoned")
            .clone())
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
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("list_tasks_after is outside this fixture's scope"),
        })
    }

    async fn list_task_report_rows_after(
        &self,
        _task: TaskId,
        _after: Option<ene_task::TaskReportRowCursor>,
        _limit: u32,
    ) -> Result<Vec<ene_task::TaskReportRow>, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("list_task_report_rows_after is outside this fixture's scope"),
        })
    }

    async fn load_report_source_bounded(
        &self,
        _source: ene_task::TaskReportSourceRef,
        _cursor_bytes: u64,
        _limit_bytes: u32,
    ) -> Result<Option<ene_task::TaskReportSourcePage>, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("load_report_source_bounded is outside this fixture's scope"),
        })
    }

    async fn commit_task_resume(
        &self,
        _premise: ene_task::TaskResumeCommitPremise,
    ) -> Result<ene_task::TaskResumeOutcome, TaskTechnicalError> {
        Err(TaskTechnicalError::StorageUnavailable {
            reason: String::from("commit_task_resume is outside this fixture's scope"),
        })
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

fn proposal(workspace: bool) -> TaskProposalPremise {
    TaskProposalPremise {
        requester: AssigneeRef {
            companion: RawId::new(),
        },
        purpose: TaskPurpose {
            text: String::from("read the input and write the report"),
        },
        origin: TaskContextOrigin {
            kind: TaskContextOriginKind::OwnerConversation,
            source: RawId::new(),
        },
        workspace_need: workspace.then(|| WorkspaceNeedRef {
            folder: WorkspaceFolderRef {
                path: String::from("/srv/workspace/ene"),
            },
            save_target: None,
        }),
    }
}

#[tokio::test]
async fn creation_mints_every_identity_and_commits_the_whole_premise() {
    let repository = FakeTaskRepository::default();
    let premise = proposal(true);
    let outcome = orchestrate_task_creation(&repository, premise.clone())
        .await
        .expect("the creation must answer");
    let TaskProposalOutcome::AcceptedAsTask(created) = outcome else {
        panic!("a fresh creation accepts, got {outcome:?}");
    };
    assert_eq!(created.revision, TaskRevision::initial());

    let captured = repository.created();
    assert_eq!(captured.len(), 1);
    let unit = &captured[0];
    assert_eq!(unit.task, created.task);
    assert_eq!(unit.assignee, premise.requester);
    assert_eq!(unit.purpose, premise.purpose);
    assert_eq!(unit.origin, premise.origin);
    let association = unit
        .workspace
        .as_ref()
        .expect("the association is proposed");
    assert_eq!(
        association.need,
        premise.workspace_need.clone().expect("proposed need")
    );

    // A second proposal mints entirely fresh identities: the caller never
    // names a TaskId, entry, or association.
    let second = orchestrate_task_creation(&repository, premise)
        .await
        .expect("the second creation must answer");
    let TaskProposalOutcome::AcceptedAsTask(second) = second else {
        panic!("a fresh creation accepts");
    };
    assert_ne!(created.task, second.task);
    let captured = repository.created();
    assert_ne!(captured[0].entry, captured[1].entry);
    assert_ne!(
        captured[0]
            .workspace
            .as_ref()
            .expect("first association")
            .assoc,
        captured[1]
            .workspace
            .as_ref()
            .expect("second association")
            .assoc
    );
}

#[tokio::test]
async fn a_proposal_without_a_workspace_commits_no_association() {
    let repository = FakeTaskRepository::default();
    let outcome = orchestrate_task_creation(&repository, proposal(false))
        .await
        .expect("the creation must answer");
    assert!(matches!(outcome, TaskProposalOutcome::AcceptedAsTask(_)));
    let captured = repository.created();
    assert_eq!(captured.len(), 1);
    assert!(captured[0].workspace.is_none());
}

#[tokio::test]
async fn a_missing_result_answers_missing_without_reaching_adoption() {
    let repository = FakeTaskRepository::default();
    let result = TaskResultId::generate();
    let outcome = reevaluate_result_adoption(&repository, result)
        .await
        .expect("the re-evaluation must answer");
    assert_eq!(outcome, TaskResultAcceptance::MissingResult { result });
    assert!(
        repository.adopted().is_empty(),
        "a missing result never calls the adoption gate"
    );
}

#[tokio::test]
async fn a_stored_result_reuses_the_adoption_gate_with_the_derived_claim() {
    let repository = FakeTaskRepository::default();
    let result = TaskResultId::generate();
    let blocker = RawId::new();
    let claim = TaskResultAdoptionClaim {
        result,
        attempt_refs: vec![RawId::new(), blocker],
    };
    repository.script_claim(claim.clone());
    repository.script_acceptance(TaskResultAcceptance::WithheldByEffectFacts {
        attempts: vec![blocker],
    });
    let outcome = reevaluate_result_adoption(&repository, result)
        .await
        .expect("the re-evaluation must answer");
    assert_eq!(
        outcome,
        TaskResultAcceptance::WithheldByEffectFacts {
            attempts: vec![blocker]
        },
        "the owner's answer passes through unchanged, withheld included"
    );
    assert_eq!(
        repository.adopted(),
        vec![claim],
        "the derived claim is the only input to the existing adoption gate"
    );
}
