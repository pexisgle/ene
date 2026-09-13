//! End-to-end Task Agent inference turn: inherited consent, attempt claim,
//! provider dispatch, and the stale/steering boundary.

use super::{LearningAwareTransport, live_input, round_test_handle};
use crate::dialogue::{CredentialScrubber, HostInference};
use crate::task_agent::TaskAgentInferenceAdapter;
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationScope,
    TaskAgentEphemeralId, TaskAgentOutput, TaskAgentTurnOutcome, TaskAgentTurnPremise,
    TaskCommitOutcome, TaskCommitPremise, TaskContextEntryId, TaskContextOrigin,
    TaskContextOriginKind, TaskCreationPremise, TaskId, TaskProgress, TaskPurpose,
    TaskPurposeAdoptionPremise, TaskRef, TaskRepository as _, TaskResultAcceptance,
    TaskResultAdoptionClaim, orchestrate_result_arrival, orchestrate_task_agent_turn,
};

async fn seed_task_and_delegation(handle: &crate::serve::HostHandle) -> (TaskRef, DelegationId) {
    let created = handle
        .store
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
            acquired_at: WallClockWithTz::now(),
            assignee: AssigneeRef {
                companion: RawId::new(),
            },
            workspace: None,
        })
        .await
        .expect("the task must commit");
    let delegation = DelegationId::generate();
    let delegated = handle
        .store
        .create_delegation(DelegationCreationPremise {
            delegation,
            task: created,
            agent: TaskAgentEphemeralId::generate(),
            scope_copy: DelegationScope { workspace: None },
        })
        .await
        .expect("the delegation must commit");
    assert!(matches!(delegated, DelegationOutcome::Delegated(_)));
    (created, delegation)
}

#[tokio::test]
async fn task_agent_turn_dispatches_under_the_inherited_consent() {
    let live = live_input("dlg-task-agent");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, _dir) = round_test_handle("dlg-task-agent", &live, &transport)
        .await
        .expect("setup must complete");
    let (created, delegation) = seed_task_and_delegation(&handle).await;
    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &transport,
    };
    let adapter = TaskAgentInferenceAdapter::new(&executor);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise { delegation },
    )
    .await
    .expect("the turn must answer");
    let TaskAgentTurnOutcome::Produced(produced) = outcome else {
        panic!("expected Produced, got {outcome:?}");
    };
    assert_eq!(produced.output.text(), "agent report");
    assert!(
        produced.adoption_consent_current,
        "the inherited consent still holds after the await"
    );
    assert_eq!(produced.delegation, delegation);
    assert_eq!(produced.task, created);
    {
        let inputs = transport.inputs.lock().expect("input capture lock");
        assert_eq!(inputs.len(), 1, "exactly one provider call");
        assert!(
            inputs[0].contains("write the report"),
            "the prompt carries the relied revision purpose, got {}",
            inputs[0]
        );
    }
    // Provider text is not Task completion: the Task unit is unchanged.
    let after = handle
        .store
        .load_task(created.task)
        .await
        .expect("the task must reload")
        .expect("the task still exists");
    assert_eq!(after.task.reference, created);
    assert_eq!(
        after.context.len(),
        1,
        "no progress or result entry was written"
    );
}

#[tokio::test]
async fn task_agent_turn_is_stale_after_steering_and_never_sends() {
    let live = live_input("dlg-task-agent-stale");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, _dir) = round_test_handle("dlg-task-agent-stale", &live, &transport)
        .await
        .expect("setup must complete");
    let (created, delegation) = seed_task_and_delegation(&handle).await;
    let advanced = handle
        .store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: Some(TaskPurposeAdoptionPremise {
                purpose: TaskPurpose {
                    text: String::from("write the follow-up"),
                },
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source: RawId::new(),
                },
                acquired_at: WallClockWithTz::now(),
            }),
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .expect("steering must answer");
    let TaskCommitOutcome::CommittedAs(current) = advanced else {
        panic!("expected CommittedAs, got {advanced:?}");
    };
    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &transport,
    };
    let adapter = TaskAgentInferenceAdapter::new(&executor);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise { delegation },
    )
    .await
    .expect("the turn must answer");
    assert_eq!(
        outcome,
        TaskAgentTurnOutcome::StaleTaskRevision { current },
        "a delegation bound to the old revision must not start"
    );
    assert!(
        transport
            .inputs
            .lock()
            .expect("input capture lock")
            .is_empty(),
        "a stale start never reaches the provider"
    );
}

#[tokio::test]
async fn task_agent_turn_is_execution_sealed_after_finalization() {
    let live = live_input("dlg-task-agent-sealed");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, _dir) = round_test_handle("dlg-task-agent-sealed", &live, &transport)
        .await
        .expect("setup must complete");
    let (_created, delegation) = seed_task_and_delegation(&handle).await;
    let result = orchestrate_result_arrival(
        &handle.store,
        delegation,
        TaskAgentOutput::new(String::from("final report")),
    )
    .await
    .expect("the finalization records the result");
    assert!(result.adopted_revision.is_none());

    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &transport,
    };
    let adapter = TaskAgentInferenceAdapter::new(&executor);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise { delegation },
    )
    .await
    .expect("the turn must answer");
    assert_eq!(
        outcome,
        TaskAgentTurnOutcome::ExecutionSealed { delegation },
        "the sealed execution refuses new inference while the Task is InProgress"
    );
    assert!(
        transport
            .inputs
            .lock()
            .expect("input capture lock")
            .is_empty(),
        "a sealed execution never reaches the provider"
    );
}

#[tokio::test]
async fn task_agent_turn_is_terminal_after_completion() {
    let live = live_input("dlg-task-agent-terminal");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, _dir) = round_test_handle("dlg-task-agent-terminal", &live, &transport)
        .await
        .expect("setup must complete");
    let (created, delegation) = seed_task_and_delegation(&handle).await;
    let result = orchestrate_result_arrival(
        &handle.store,
        delegation,
        TaskAgentOutput::new(String::from("final report")),
    )
    .await
    .expect("the finalization records the result");
    let adopted = handle
        .store
        .adopt_result(TaskResultAdoptionClaim {
            result: result.result,
            attempt_refs: Vec::new(),
        })
        .await
        .expect("the adoption answers");
    assert_eq!(adopted, TaskResultAcceptance::AdoptedAsCompletion(created));

    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &transport,
    };
    let adapter = TaskAgentInferenceAdapter::new(&executor);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise { delegation },
    )
    .await
    .expect("the turn must answer");
    assert_eq!(
        outcome,
        TaskAgentTurnOutcome::TaskTerminal {
            task: created.task,
            progress: TaskProgress::Completed,
        }
    );
    assert!(
        transport
            .inputs
            .lock()
            .expect("input capture lock")
            .is_empty(),
        "a terminal Task never reaches the provider"
    );
}
