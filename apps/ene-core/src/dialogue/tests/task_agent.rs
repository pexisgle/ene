//! End-to-end Task Agent inference turn: inherited consent, attempt claim,
//! provider dispatch, and the stale/steering boundary.

use super::{LearningAwareTransport, live_input, round_test_handle};
use crate::dialogue::HostInference;
use crate::task_agent::{OwnerInstructionSource, TaskAgentInferenceAdapter};
use ene_credential::CredentialScrubber;
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationScope,
    TaskAgentEphemeralId, TaskAgentNotSent, TaskAgentTurnOutcome, TaskAgentTurnPremise,
    TaskCommitOutcome, TaskCommitPremise, TaskContextEntryId, TaskContextOrigin,
    TaskContextOriginKind, TaskCreationPremise, TaskId, TaskInstructionAdoptionPremise,
    TaskProgress, TaskPurpose, TaskPurposeAdoptionPremise, TaskRef, TaskRepository as _,
    TaskResultAcceptance, TaskResultAdoptionClaim, orchestrate_task_agent_turn,
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

/// Uses the A1 production producer, with test-only confirmation evidence;
/// the first-party trusted confirmation issuer is the separate A1b slice.
async fn seed_covering_condition(store: &ene_store::Store, source: RawId) {
    use ene_preservation::*;
    let command = StartTargetedDeletionCommand::new(
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                "fixture".into(),
            )),
            semantic_hints: vec![],
        },
        DeletionPurpose::Privacy,
        ene_primitive::WallClockWithTz::now(),
        vec![source],
        vec![ParticipantOwnerRef::Companion],
    )
    .confirmed_for_tests();
    assert!(matches!(
        store.start_targeted_deletion(command).await.unwrap(),
        StartTargetedDeletionOutcome::Started(_)
    ));
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
    let adapter = TaskAgentInferenceAdapter::new(&executor, None);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let instructions = OwnerInstructionSource::new(&handle.store, &handle.store);
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation,
            exchanges: Vec::new(),
        },
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
    seed_covering_condition(&handle.store, purpose_source).await;

    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &transport,
    };
    let adapter = TaskAgentInferenceAdapter::new(&executor, None);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let instructions = OwnerInstructionSource::new(&handle.store, &handle.store);
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation,
            exchanges: Vec::new(),
        },
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
    let adapter = TaskAgentInferenceAdapter::new(&executor, None);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let instructions = OwnerInstructionSource::new(&handle.store, &handle.store);
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation,
            exchanges: Vec::new(),
        },
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
    let result =
        crate::test_support::record_result(&handle.store, delegation, "final report").await;
    assert!(result.adopted_revision.is_none());

    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &transport,
    };
    let adapter = TaskAgentInferenceAdapter::new(&executor, None);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let instructions = OwnerInstructionSource::new(&handle.store, &handle.store);
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation,
            exchanges: Vec::new(),
        },
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
    let result =
        crate::test_support::record_result(&handle.store, delegation, "final report").await;
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
    let adapter = TaskAgentInferenceAdapter::new(&executor, None);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let instructions = OwnerInstructionSource::new(&handle.store, &handle.store);
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation,
            exchanges: Vec::new(),
        },
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

/// Seeds a Task whose context adopts one Owner instruction and its canonical
/// History source, then one delegation bound to the resulting current
/// revision. Returns `(current TaskRef, delegation, instruction source,
/// purpose source)`.
async fn seed_task_with_instruction(
    handle: &crate::serve::HostHandle,
    instruction_body: &str,
) -> (TaskRef, DelegationId, RawId, RawId) {
    use ene_companion::{
        AppendHistoryCommand, CompanionRepository as _, HistoryAppendOutcome,
        HistoryRepository as _, HistoryRole,
    };
    use ene_presence::PresenceRepository as _;

    let companion = handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve");
    let generation = handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("attribution must load")
        .expect("attribution must exist")
        .generation;
    let appended = handle
        .store
        .append_message(AppendHistoryCommand {
            companion,
            round: RawId::new(),
            role: HistoryRole::Owner,
            text: instruction_body.to_owned(),
            lang: String::from("en"),
            at: WallClockWithTz::now(),
            expected_generation: generation,
            expected_consent: None,
            expected_credential_set: None,
            expected_owner_message: None,
            command_id: None,
            round_wire: None,
            round_intent: None,
            incarnation: None,
            local_id: None,
        })
        .await
        .expect("the Owner instruction must commit");
    let HistoryAppendOutcome::CommittedAs { message: source } = appended else {
        panic!("the Owner instruction must commit, got {appended:?}");
    };

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
                companion: companion.as_raw(),
            },
            workspace: None,
        })
        .await
        .expect("the task must commit");
    let advanced = handle
        .store
        .forward_steering(TaskCommitPremise {
            expected: created,
            new_purpose: None,
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: Some(TaskInstructionAdoptionPremise {
                entry: TaskContextEntryId::generate(),
                origin: TaskContextOrigin {
                    kind: TaskContextOriginKind::OwnerConversation,
                    source,
                },
                acquired_at: WallClockWithTz::now(),
            }),
        })
        .await
        .expect("the steering must answer");
    let TaskCommitOutcome::CommittedAs(current) = advanced else {
        panic!("the instruction adoption must commit, got {advanced:?}");
    };
    let delegation = DelegationId::generate();
    let delegated = handle
        .store
        .create_delegation(DelegationCreationPremise {
            delegation,
            task: current,
            agent: TaskAgentEphemeralId::generate(),
            scope_copy: DelegationScope { workspace: None },
        })
        .await
        .expect("the delegation must commit");
    assert!(matches!(delegated, DelegationOutcome::Delegated(_)));
    (current, delegation, source, purpose_source)
}

fn rendered(raw: RawId) -> String {
    raw.as_uuid().as_hyphenated().to_string()
}

#[tokio::test]
async fn task_agent_turn_sends_purpose_and_instruction_through_the_real_composition_path() {
    use ene_companion::HistoryRepository as _;

    let live = live_input("dlg-task-agent-instruction");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, dir) = round_test_handle("dlg-task-agent-instruction", &live, &transport)
        .await
        .expect("setup must complete");
    let (created, delegation, instruction_source, purpose_source) =
        seed_task_with_instruction(&handle, "read the notes first").await;

    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &transport,
    };
    let adapter = TaskAgentInferenceAdapter::new(&executor, None);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let instructions = OwnerInstructionSource::new(&handle.store, &handle.store);
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation,
            exchanges: Vec::new(),
        },
    )
    .await
    .expect("the turn must answer");
    assert!(
        matches!(outcome, TaskAgentTurnOutcome::Produced(_)),
        "the empty condition set admits the send, got {outcome:?}"
    );

    {
        let inputs = transport.inputs.lock().expect("input capture lock");
        assert_eq!(inputs.len(), 1, "exactly one provider call");
        let expected = "[RESPONSE FORMAT]\n\
Respond with exactly one JSON object and no other text. One of:\n\
{\"tool\":\"list\",\"path\":\"<workspace-relative directory>\"}\n\
{\"tool\":\"read\",\"path\":\"<workspace-relative file>\"}\n\
{\"tool\":\"create\",\"path\":\"<workspace-relative file>\",\"content\":\"<UTF-8 text>\"}\n\
{\"tool\":\"edit\",\"path\":\"<workspace-relative file>\",\"content\":\"<UTF-8 text>\"}\n\
{\"final\":\"<final answer>\"}\n\
[PURPOSE]\nwrite the report\n[INSTRUCTION]\nread the notes first\n[PAST EXECUTED FACTS]\n";
        assert_eq!(
            inputs[0], expected,
            "the logical input frames the response format, purpose, the adopted instruction body, then the past-facts block"
        );
    }
    assert_eq!(
        durable_data_use_sources(dir.path()),
        vec![rendered(purpose_source), rendered(instruction_source)],
        "the durable correlation is purpose then instruction, in logical-input order"
    );
    assert_eq!(durable_attempt_count(dir.path()), 1);
    // The body stays canonical in History; the Task context holds only the
    // adopted entry reference.
    let after = handle
        .store
        .load_task(created.task)
        .await
        .expect("the task must reload")
        .expect("the task still exists");
    assert_eq!(
        after.context.len(),
        2,
        "purpose plus one adopted instruction"
    );
    let body = handle
        .store
        .load_message(instruction_source)
        .await
        .expect("the History source must read")
        .expect("the History source is still canonical");
    assert_eq!(body.text, "read the notes first");
}

#[tokio::test]
async fn task_agent_turn_is_data_use_held_when_the_instruction_source_is_covered() {
    use ene_companion::HistoryRepository as _;

    let live = live_input("dlg-task-agent-instruction-held");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, dir) = round_test_handle("dlg-task-agent-instruction-held", &live, &transport)
        .await
        .expect("setup must complete");
    let (created, delegation, instruction_source, _purpose_source) =
        seed_task_with_instruction(&handle, "read the notes first").await;
    // The History row was readable and stays readable, but a Targeted
    // Deletion condition covering it is durable before the send admission.
    assert!(
        handle
            .store
            .load_message(instruction_source)
            .await
            .expect("the source must read")
            .is_some(),
        "the source exists at read time: the refusal must not be a missing source"
    );
    seed_covering_condition(&handle.store, instruction_source).await;

    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &transport,
    };
    let adapter = TaskAgentInferenceAdapter::new(&executor, None);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let instructions = OwnerInstructionSource::new(&handle.store, &handle.store);
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation,
            exchanges: Vec::new(),
        },
    )
    .await
    .expect("the turn must answer");
    assert_eq!(
        outcome,
        TaskAgentTurnOutcome::NotSent(TaskAgentNotSent::DataUseHeld),
        "the AU14 claim holds the covered instruction source, not a missing-source outcome"
    );
    assert!(
        transport
            .inputs
            .lock()
            .expect("input capture lock")
            .is_empty(),
        "a held send receives zero provider bytes"
    );
    assert_eq!(durable_attempt_count(dir.path()), 0);
    assert_eq!(durable_data_use_sources(dir.path()), Vec::<String>::new());
    let after = handle
        .store
        .load_task(created.task)
        .await
        .expect("the task must reload")
        .expect("the task still exists");
    assert_eq!(
        after.context.len(),
        2,
        "the hold rewrites no Task context entry"
    );
    assert!(
        handle
            .store
            .load_message(instruction_source)
            .await
            .expect("the source must still read")
            .is_some(),
        "a hold neither erases nor hides the canonical History row"
    );
}

#[tokio::test]
async fn task_agent_turn_with_an_instruction_never_sends_after_steering_wins() {
    let live = live_input("dlg-task-agent-instruction-stale");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, dir) = round_test_handle("dlg-task-agent-instruction-stale", &live, &transport)
        .await
        .expect("setup must complete");
    let (created, delegation, _source, _purpose_source) =
        seed_task_with_instruction(&handle, "read the notes first").await;
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
    let adapter = TaskAgentInferenceAdapter::new(&executor, None);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let instructions = OwnerInstructionSource::new(&handle.store, &handle.store);
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation,
            exchanges: Vec::new(),
        },
    )
    .await
    .expect("the turn must answer");
    assert_eq!(
        outcome,
        TaskAgentTurnOutcome::StaleTaskRevision { current },
        "a steering winner refuses the old revision before the assembled prompt is sent"
    );
    assert!(
        transport
            .inputs
            .lock()
            .expect("input capture lock")
            .is_empty(),
        "the old prompt, instruction body included, never reaches the provider"
    );
    assert_eq!(durable_attempt_count(dir.path()), 0);
}

#[tokio::test]
async fn reopened_handle_does_not_replay_a_turn_and_resolves_the_instruction_again() {
    use crate::serve::CredStore;
    use ene_credential::{CredentialRef, MemoryCredentialStore};

    let live = live_input("dlg-task-agent-instruction-reopen");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, dir) = round_test_handle("dlg-task-agent-instruction-reopen", &live, &transport)
        .await
        .expect("setup must complete");
    let (created, delegation, _source, _purpose_source) =
        seed_task_with_instruction(&handle, "read the notes first").await;
    let first = {
        let executor = HostInference {
            store: &handle.store,
            cred_store: &handle.cred_store,
            tracker: &handle.tracker,
            transport: &transport,
        };
        let adapter = TaskAgentInferenceAdapter::new(&executor, None);
        let scrubber = CredentialScrubber {
            refs: &handle.store,
            store: &handle.cred_store,
        };
        let instructions = OwnerInstructionSource::new(&handle.store, &handle.store);
        orchestrate_task_agent_turn(
            &handle.store,
            &instructions,
            &adapter,
            &scrubber,
            TaskAgentTurnPremise {
                delegation,
                exchanges: Vec::new(),
            },
        )
        .await
        .expect("the turn must answer")
    };
    assert!(matches!(first, TaskAgentTurnOutcome::Produced(_)));
    assert_eq!(
        transport.inputs.lock().expect("input capture lock").len(),
        1
    );
    drop(handle);

    let fresh = MemoryCredentialStore::new();
    fresh.insert(
        CredentialRef::new("openai", "main").expect("valid test fixture"),
        "test-bearer",
    );
    let reopened =
        crate::serve::HostHandle::open_with_cred_store(dir.path(), CredStore::Memory(fresh))
            .await
            .expect("the reopen must succeed");
    assert_eq!(
        transport.inputs.lock().expect("input capture lock").len(),
        1,
        "reopening never auto-replays a turn or a provider call"
    );

    // An explicit invocation runs again and re-resolves the same canonical
    // History body: the adoption correspondence survived the restart.
    let executor = HostInference {
        store: &reopened.store,
        cred_store: &reopened.cred_store,
        tracker: &reopened.tracker,
        transport: &transport,
    };
    let adapter = TaskAgentInferenceAdapter::new(&executor, None);
    let scrubber = CredentialScrubber {
        refs: &reopened.store,
        store: &reopened.cred_store,
    };
    let instructions = OwnerInstructionSource::new(&reopened.store, &reopened.store);
    let second = orchestrate_task_agent_turn(
        &reopened.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation,
            exchanges: Vec::new(),
        },
    )
    .await
    .expect("the reopened turn must answer");
    assert!(
        matches!(second, TaskAgentTurnOutcome::Produced(_)),
        "an explicit turn after reopen re-resolves the adopted instruction, got {second:?}"
    );
    let inputs = transport.inputs.lock().expect("input capture lock");
    assert_eq!(inputs.len(), 2, "only the explicit second turn adds a call");
    assert!(
        inputs[1].contains("read the notes first"),
        "the same canonical History body is resolved after restart"
    );
    drop(inputs);
    assert_eq!(
        durable_attempt_count(dir.path()),
        2,
        "each explicit turn claims its own attempt; reopen replays nothing"
    );
    let _ = created;
}

#[tokio::test]
async fn task_agent_turn_scrubs_a_registered_secret_in_an_instruction_body() {
    use ene_credential::REDACTED_CREDENTIAL;

    let live = live_input("dlg-task-agent-instruction-secret");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, _dir) = round_test_handle("dlg-task-agent-instruction-secret", &live, &transport)
        .await
        .expect("setup must complete");
    // The registered bearer is `test-bearer` (setup_handle); an instruction
    // body containing it must never reach the provider raw.
    let (_created, delegation, _source, _purpose_source) =
        seed_task_with_instruction(&handle, "the key is test-bearer, keep it safe").await;

    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &transport,
    };
    let adapter = TaskAgentInferenceAdapter::new(&executor, None);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let instructions = OwnerInstructionSource::new(&handle.store, &handle.store);
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation,
            exchanges: Vec::new(),
        },
    )
    .await
    .expect("the turn must answer");
    assert!(matches!(outcome, TaskAgentTurnOutcome::Produced(_)));
    let inputs = transport.inputs.lock().expect("input capture lock");
    assert_eq!(inputs.len(), 1);
    assert!(
        !inputs[0].contains("test-bearer"),
        "the registered secret must not reach the provider"
    );
    assert!(
        inputs[0].contains(REDACTED_CREDENTIAL),
        "the single scrub over the whole input redacts the secret in place"
    );
}

/// Transport that advances the dialogue consent revision while the provider
/// call is in flight, so the dispatch's post-await adoption re-check sees a
/// moved premise. The directory is set after the Host is opened because the
/// setup path runs before the task turn.
#[derive(Default)]
struct ConsentMovingTransport {
    data_dir: std::sync::Mutex<Option<std::path::PathBuf>>,
}

impl ene_inference::ProviderTransport for ConsentMovingTransport {
    fn complete(
        &self,
        _req: ene_inference::ProviderRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        ene_inference::ProviderResponse,
                        ene_inference::InferenceTechnicalError,
                    >,
                > + Send
                + '_,
        >,
    > {
        let data_dir = self
            .data_dir
            .lock()
            .expect("consent transport directory lock")
            .clone();
        Box::pin(async move {
            if let Some(data_dir) = data_dir {
                let conn = rusqlite::Connection::open(data_dir.join("app.db"))
                    .expect("the store file must open for the consent move");
                conn.execute("UPDATE consent_record SET rev = rev + 1", [])
                    .expect("the consent revision move must apply");
            }
            Ok(ene_inference::ProviderResponse {
                text: String::from("agent report"),
                usage: Some(ene_inference::RawUsage {
                    input_tokens: 9,
                    cached_input_tokens: 2,
                    output_tokens: 4,
                }),
            })
        })
    }
}

fn durable_usage_count(data_dir: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(data_dir.join("app.db"))
        .expect("the store file must open for the probe");
    conn.query_row("SELECT COUNT(*) FROM usage_fact", (), |row| row.get(0))
        .expect("the usage count must read")
}

#[tokio::test]
async fn a_consent_move_during_the_provider_wait_is_reported_with_its_sent_fact() {
    let live = live_input("dlg-task-agent-consent-lapsed");
    let transport = ConsentMovingTransport::default();
    let (handle, dir) = round_test_handle("dlg-task-agent-consent-lapsed", &live, &transport)
        .await
        .expect("setup must complete");
    *transport
        .data_dir
        .lock()
        .expect("consent transport directory lock") = Some(dir.path().to_path_buf());
    let (_created, delegation, _purpose_source) = seed_task_and_delegation(&handle).await;

    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &transport,
    };
    let adapter = TaskAgentInferenceAdapter::new(&executor, None);
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let instructions = OwnerInstructionSource::new(&handle.store, &handle.store);
    let outcome = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation,
            exchanges: Vec::new(),
        },
    )
    .await
    .expect("the turn must answer");
    let TaskAgentTurnOutcome::Produced(produced) = outcome else {
        panic!("expected Produced, got {outcome:?}");
    };
    assert!(
        !produced.adoption_consent_current,
        "a consent that moved during the wait is reported, not hidden as not-sent"
    );
    assert_eq!(produced.output.text(), "agent report");
    // The send already happened: the claimed attempt and its usage fact are
    // durable even though the output can no longer be adopted.
    assert_eq!(
        durable_attempt_count(dir.path()),
        1,
        "the claimed attempt survives the discarded output"
    );
    assert_eq!(
        durable_usage_count(dir.path()),
        1,
        "the answered call keeps its usage accounting"
    );
    // The stale answer still settles its reported token shape: adoption and
    // usage settlement are separate decisions, and cached input travels with
    // the durable fact rather than being folded into input.
    let conn = rusqlite::Connection::open(dir.path().join("app.db"))
        .expect("the store file must open for the probe");
    let (source, input, cached, output): (String, Option<i64>, Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT source, input_tokens, cached_input_tokens, output_tokens FROM usage_fact LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            },
        )
        .expect("the usage row must read");
    assert_eq!(source, "reported");
    assert_eq!((input, cached, output), (Some(9), Some(2), Some(4)));
}
