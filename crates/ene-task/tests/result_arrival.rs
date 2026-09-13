//! Explicit Task result finalization boundary checks (AU15a).
//!
//! The finalization path is deliberately separate from
//! `TaskAgentTurnOutcome::Produced`: the caller states that a body is the
//! final result, the Task owner mints the identity, and the durable arrival is
//! recorded before anything is visible. A single inference turn's provider
//! output is never routed here automatically, so this test pins the identity
//! minting and the exact arrival the repository receives.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "integration-test fixtures and helpers live outside #[test] functions, where clippy.toml's test allowances do not apply"
)]

use std::sync::Mutex;

use ene_primitive::WallClockWithTz;
use ene_task::{
    DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationRef, TaskAgentOutput,
    TaskAgentResultArrival, TaskCommitOutcome, TaskCommitPremise, TaskId, TaskRef, TaskRepository,
    TaskResultAcceptance, TaskResultAdoptionClaim, TaskResultId, TaskResultRecord, TaskRevision,
    TaskTechnicalError, orchestrate_result_arrival,
};

/// Captures every arrival the orchestration records; it implements no other
/// behavior, so a finalization that tried to adopt, complete, or infer would
/// have to route through an unsupported method.
#[derive(Default)]
struct CapturingRepository {
    arrivals: Mutex<Vec<TaskAgentResultArrival>>,
}

impl CapturingRepository {
    fn arrivals(&self) -> Vec<TaskAgentResultArrival> {
        self.arrivals
            .lock()
            .expect("capture lock is never poisoned")
            .clone()
    }
}

impl TaskRepository for CapturingRepository {
    async fn create_task(
        &self,
        _premise: ene_task::TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError> {
        Err(unsupported("create_task"))
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
        arrival: TaskAgentResultArrival,
    ) -> Result<TaskResultRecord, TaskTechnicalError> {
        let record = TaskResultRecord {
            result: arrival.result,
            task: TaskRef {
                task: TaskId::generate(),
                revision: TaskRevision::initial(),
            },
            delegation: arrival.delegation,
            body: arrival.body.clone(),
            attempt_refs: Vec::new(),
            adopted_revision: None,
            recorded_at: WallClockWithTz::now(),
        };
        self.arrivals
            .lock()
            .expect("capture lock is never poisoned")
            .push(arrival);
        Ok(record)
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

    async fn adopt_result(
        &self,
        _claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError> {
        Err(unsupported("adopt_result"))
    }
}

fn unsupported(method: &str) -> TaskTechnicalError {
    TaskTechnicalError::StorageUnavailable {
        reason: format!("{method} is outside this fixture's scope"),
    }
}

#[tokio::test]
async fn finalization_mints_the_identity_and_records_the_arrival_once() {
    let repository = CapturingRepository::default();
    let delegation = DelegationId::generate();

    let first = orchestrate_result_arrival(
        &repository,
        delegation,
        TaskAgentOutput::new(String::from("first body")),
    )
    .await
    .expect("the arrival records");
    let second = orchestrate_result_arrival(
        &repository,
        delegation,
        TaskAgentOutput::new(String::from("second body")),
    )
    .await
    .expect("the arrival records");

    assert_ne!(
        first.result, second.result,
        "each finalization gets a fresh durable identity"
    );
    let arrivals = repository.arrivals();
    assert_eq!(arrivals.len(), 2);
    assert_eq!(arrivals[0].delegation, delegation);
    assert_eq!(arrivals[0].result, first.result);
    assert_eq!(arrivals[0].body.text(), "first body");
    assert_eq!(arrivals[1].result, second.result);
    assert_eq!(arrivals[1].body.text(), "second body");
    assert_eq!(first.adopted_revision, None);
}
