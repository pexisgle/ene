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
    TaskPurpose, TaskPurposeAdoptionPremise, TaskRef, TaskRepository as _,
    orchestrate_task_agent_turn,
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
