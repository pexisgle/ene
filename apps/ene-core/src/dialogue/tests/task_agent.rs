//! End-to-end Task Agent inference turn: inherited consent, attempt claim,
//! provider dispatch, and the stale/steering boundary.

use super::{LearningAwareTransport, live_input, round_test_handle};
use crate::dialogue::{CredentialScrubber, HostInference};
use crate::task_agent::TaskAgentInferenceAdapter;
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationScope,
    TaskAgentEphemeralId, TaskAgentNotSent, TaskAgentOutput, TaskAgentTurnOutcome,
    TaskAgentTurnPremise, TaskCommitOutcome, TaskCommitPremise, TaskContextEntryId,
    TaskContextOrigin, TaskContextOriginKind, TaskCreationPremise, TaskId, TaskProgress,
    TaskPurpose, TaskPurposeAdoptionPremise, TaskRef, TaskRepository as _, TaskResultAcceptance,
    TaskResultAdoptionClaim, orchestrate_result_arrival, orchestrate_task_agent_turn,
};

async fn seed_task_and_delegation(
    handle: &crate::serve::HostHandle,
) -> (TaskRef, DelegationId, RawId) {
    let purpose_source = RawId::new();
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
                source: purpose_source,
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
    (created, delegation, purpose_source)
}

/// Writes one canonical erasure condition covering `source` directly into the
/// canonical Group J tables. Stage 4 has no production deletion-operation
/// producer, so the fixture writes exactly the rows the Stage 6 producer will
/// write; the gate then reads them through the real AU14 claim.
fn seed_covering_condition(data_dir: &std::path::Path, source: RawId) {
    let rendered = |raw: RawId| raw.as_uuid().as_hyphenated().to_string();
    let conn = rusqlite::Connection::open(data_dir.join("app.db"))
        .expect("the store file must open for the fixture");
    let operation = rendered(RawId::new());
    conn.execute(
        "INSERT INTO erasure_condition (operation_id, sweep) VALUES (?1, 1)",
        rusqlite::params![operation],
    )
    .expect("the condition row must seed");
    conn.execute(
        "INSERT INTO erasure_condition_source (operation_id, sweep, source) VALUES (?1, 1, ?2)",
        rusqlite::params![operation, rendered(source)],
    )
    .expect("the source coverage row must seed");
}

fn durable_data_use_sources(data_dir: &std::path::Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(data_dir.join("app.db"))
        .expect("the store file must open for the probe");
    let mut statement = conn
        .prepare("SELECT source FROM inference_attempt_data_use ORDER BY ticket, ordinal")
        .expect("the probe statement must prepare");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("the probe must run");
    rows.collect::<Result<Vec<_>, _>>()
        .expect("the probe rows must decode")
}

fn durable_attempt_count(data_dir: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(data_dir.join("app.db"))
        .expect("the store file must open for the probe");
    conn.query_row("SELECT COUNT(*) FROM inference_attempt", (), |row| {
        row.get(0)
    })
    .expect("the attempt count must read")
}

#[tokio::test]
async fn task_agent_turn_dispatches_under_the_inherited_consent() {
    let live = live_input("dlg-task-agent");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, dir) = round_test_handle("dlg-task-agent", &live, &transport)
        .await
        .expect("setup must complete");
    let (created, delegation, purpose_source) = seed_task_and_delegation(&handle).await;
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
    assert_eq!(
        durable_data_use_sources(dir.path()),
        vec![purpose_source.as_uuid().as_hyphenated().to_string()],
        "the claim durably records the purpose entry's canonical source"
    );
    assert_eq!(durable_attempt_count(dir.path()), 1);
}

#[tokio::test]
async fn task_agent_turn_is_data_use_held_when_the_purpose_source_is_covered() {
    let live = live_input("dlg-task-agent-held");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, dir) = round_test_handle("dlg-task-agent-held", &live, &transport)
        .await
        .expect("setup must complete");
    let (created, delegation, purpose_source) = seed_task_and_delegation(&handle).await;
    // A condition covering the logical input's only source lands before the
    // send admission; the AU14 claim must see it in the same transaction.
    seed_covering_condition(dir.path(), purpose_source);

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
        TaskAgentTurnOutcome::NotSent(TaskAgentNotSent::DataUseHeld),
        "a covered source is a data-use hold, never revision staleness or a technical error"
    );
    assert!(
        transport
            .inputs
            .lock()
            .expect("input capture lock")
            .is_empty(),
        "a held send receives zero provider bytes"
    );
    assert_eq!(
        durable_attempt_count(dir.path()),
        0,
        "a held send starts no attempt"
    );
    assert_eq!(durable_data_use_sources(dir.path()), Vec::<String>::new());
    // The held refusal changes no Task context: the source was present at
    // read time and the refusal is not a missing source.
    let after = handle
        .store
        .load_task(created.task)
        .await
        .expect("the task must reload")
        .expect("the task still exists");
    assert_eq!(after.task.reference, created);
    assert_eq!(after.context.len(), 1);
}

#[tokio::test]
async fn task_agent_turn_is_stale_after_steering_and_never_sends() {
    let live = live_input("dlg-task-agent-stale");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, _dir) = round_test_handle("dlg-task-agent-stale", &live, &transport)
        .await
        .expect("setup must complete");
    let (created, delegation, _purpose_source) = seed_task_and_delegation(&handle).await;
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
    let (_created, delegation, _purpose_source) = seed_task_and_delegation(&handle).await;
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
    let (created, delegation, _purpose_source) = seed_task_and_delegation(&handle).await;
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
