//! Stage 6 C2/C3: one registered secret, every surface (#1595, acceptance §7.2).
//!
//! The suite registers one distinctive value in the real credential registry
//! through the production management inlet and then drives the real routes —
//! dialogue, Learning formation, and a Task Agent turn — while scanning the
//! exact registered value out of every surface the acceptance text names:
//!
//! - the request bodies the provider transport actually receives,
//! - dialogue History,
//! - Learning Summary / Memory / revision history,
//! - Task purpose / instruction / result / report rows,
//! - undelivered and presentation excerpts,
//! - the management view,
//! - error and `Debug` renderings of the refusals themselves.
//!
//! The scans use `contains` on the whole registered value (the exact registered
//! value, never a fragment of it), and one test additionally scans every text
//! column of the durable SQLite file so a surface this list forgot cannot hide
//! a raw occurrence. A positive control proves the value was really present
//! before the sweep, so a scan cannot pass by looking at the wrong rows.
//!
//! The currentness tests make the race deterministic with the Host's test
//! gates and a delegating scrubber/port wrapper: the credential is rotated
//! after the proof is minted and before the commit/claim, so no `sleep` and no
//! fixture replaces the production boundary. Only a fresh scrub under the new
//! revision may retry.
//!
//! `ScrubbedText` cannot be minted outside the credential scrub boundary; the
//! compile-fail doctests on the type and on `SecretScrubber` pin that, so a
//! fake can only delegate to `CredentialScrubber` or fail closed.
//!
//! Path-valued fields (`action_attempt.real_target`, workspace folders) are
//! references to real-world objects, not content: the suite keeps them out of
//! scope as sweep targets because rewriting them would falsify what was
//! actually attempted. The durable scan still fails the suite if any path
//! ever carries the registered value.
//!
//! The Host has no logging subsystem, so "structured log capture" is covered
//! by the structured surfaces it does have: the encoded wire frames, the
//! `Debug` of every domain value and error the pass produced, and the
//! rendered error strings.

use std::sync::atomic::{AtomicBool, Ordering};

use super::{
    LearningAwareTransport, assign_learning, live_input, memory_handle_with,
    register_assign_complete, stamped, submit_frame, view_request_frame,
};
use crate::dialogue::HostInference;
use crate::serve::{CredStore, HostHandle, LiveInput};
use crate::task_agent::{OwnerInstructionSource, TaskAgentInferenceAdapter};
use ene_action::{ActionAttemptId, ActionAttemptRepository as _};
use ene_api::v1::envelope::{ProtocolVersion, new_outgoing_envelope};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::WireMessageType;
use ene_api::v1::round::RoundIntakeOutcomeWire;
use ene_api::v1::undelivered::{GetTaskReport, ListTasks, TaskWireRef, UndeliveredRequest};
use ene_companion::{
    ActivityId, ActivityRepository as _, AppendHistoryCommand, CompanionId,
    CompanionRepository as _, HistoryAppendOutcome, HistoryRepository as _, HistoryRole,
    RecordResumeActivityCommand, TaskFact, UndeliveredRepository as _, UndeliveredSource,
};
use ene_credential::{CredentialRef, CredentialScrubber, MemoryCredentialStore, SecretScrubber};
use ene_learning::{
    ChangeKind, ExperienceCandidate, ExperienceRole, ExperienceSourceKind, ExperienceTurn,
    Importance, LearningRepository as _, LearningScope, LearningTechnicalError, MemoryChange,
    MemoryChangeCommit, MemoryId, MemoryTarget, SourceRangeRef, SummaryId, SummaryRecord,
    TemporalMeaning,
};
use ene_presence::PresenceRepository as _;
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegationCreationPremise, DelegationId, DelegationOutcome, DelegationScope,
    TaskAgentEphemeralId, TaskAgentInference, TaskAgentInferenceError, TaskAgentInferenceOutcome,
    TaskAgentInferencePremise, TaskAgentTurnOutcome, TaskAgentTurnPremise, TaskCommitOutcome,
    TaskCommitPremise, TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind,
    TaskCreationPremise, TaskInstructionAdoptionPremise, TaskPurpose, TaskRef, TaskReportSourceRef,
    TaskRepository as _, TaskRevision, orchestrate_task_agent_turn,
};

/// The one registered secret every assertion scans for.
const SUITE_SECRET: &str = "sk-c2c3-7f31";

/// A second value registered mid-test to model an explicit rotation.
const ROTATED_SECRET: &str = "sk-c2c3-9b57";

fn credential() -> CredentialRef {
    CredentialRef::new("openai", "main").expect("the suite fixture ref is valid")
}

/// Opens the suite Host with one pinned but not yet registered value.
async fn suite_handle(tag: &str, secret: &str) -> (HostHandle, tempfile::TempDir) {
    memory_handle_with(tag, |store| store.insert(credential(), secret))
        .await
        .expect("the suite scratch directory must be creatable")
}

/// Replaces the pinned value the running Host serves, modelling an operator
/// value update; the revision is advanced by the approval write separately.
fn pin(handle: &HostHandle, secret: &str) {
    let CredStore::Memory(store) = &handle.cred_store else {
        panic!("the suite always pins a memory credential store");
    };
    store.insert(credential(), secret);
}

/// Rotates the registered pair through the production approval boundary: the
/// sweep and the revision advance commit atomically, then the running Host
/// pins the new value.
fn rotate_registered(handle: &HostHandle, secret: &str) {
    assert!(
        handle
            .store
            .approve_credential_with_sweep("openai", "main", secret)
            .expect("the rotation commit must run"),
        "the rotated pair stays usable"
    );
    pin(handle, secret);
}

fn expect_absent(label: &str, text: &str, secret: &str) {
    assert!(
        !text.contains(secret),
        "{label} must not contain the registered secret: {text}"
    );
}

fn expect_absent_in_frames(label: &str, frames: &[ene_plugin_ipc::WireFrame], secret: &str) {
    for (index, frame) in frames.iter().enumerate() {
        let encoded = ene_plugin_ipc::encode_frame(frame).expect("the frame must encode");
        expect_absent(
            &format!("{label} frame {index} wire bytes"),
            &String::from_utf8_lossy(&encoded),
            secret,
        );
        expect_absent(
            &format!("{label} frame {index} payload debug"),
            &format!("{:?}", frame.payload),
            secret,
        );
        expect_absent(
            &format!("{label} frame {index} envelope debug"),
            &format!("{:?}", frame.envelope),
            secret,
        );
    }
}

/// Counts exact occurrences of `secret` in every text column of every durable
/// table. This is the backstop for surfaces the explicit list might miss: the
/// value is bound as a parameter and compared with `instr`, so the scan is an
/// exact match on the whole registered value, not a fragment probe.
fn durable_occurrences(db_path: &std::path::Path, secret: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(db_path).expect("the suite app.db must open");
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .expect("the schema probe must prepare")
        .query_map((), |row| row.get(0))
        .expect("the schema probe must run")
        .collect::<Result<_, _>>()
        .expect("the schema rows must decode");
    let mut hits = Vec::new();
    for table in tables {
        let columns: Vec<String> = conn
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .expect("the column probe must prepare")
            .query_map((), |row| row.get(1))
            .expect("the column probe must run")
            .collect::<Result<_, _>>()
            .expect("the column rows must decode");
        for column in columns {
            let sql = format!(
                "SELECT COUNT(*) FROM \"{table}\" WHERE instr(CAST(\"{column}\" AS TEXT), ?1) > 0"
            );
            let count: i64 = conn
                .query_row(&sql, [secret], |row| row.get(0))
                .expect("the leak count must read");
            if count > 0 {
                hits.push(format!("{table}.{column} x{count}"));
            }
        }
    }
    hits
}

fn domain_frame(live: &LiveInput, payload: WirePayload) -> ene_plugin_ipc::WireFrame {
    let frame = ene_plugin_ipc::WireFrame {
        envelope: new_outgoing_envelope(
            ProtocolVersion::V1,
            super::sender(),
            WireMessageType(payload.message_type().to_string()),
        ),
        payload,
    };
    stamped(frame, live.connection_id)
}

fn task_wire_ref(task: ene_task::TaskId) -> TaskWireRef {
    TaskWireRef(format!("task:{}", task.as_raw().as_uuid().as_hyphenated()))
}

async fn running_companion(handle: &HostHandle) -> CompanionId {
    handle
        .store
        .ensure_running_companion()
        .await
        .expect("the suite companion must resolve")
}

async fn current_generation(handle: &HostHandle) -> ene_presence::PresenceGeneration {
    let companion = running_companion(handle).await;
    handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("the attribution must load")
        .expect("the attribution must exist")
        .generation
}

/// One seeded Task with one instruction and two live delegations: the result
/// delegation holds the legacy recorded result, the turn delegation is left
/// unsealed for the Task Agent turn.
struct SeededTask {
    task: TaskRef,
    turn_delegation: DelegationId,
    result_delegation: DelegationId,
}

/// Seeds a Task through the canonical producer boundaries with a purpose and
/// one adopted Owner instruction, exactly like the production task inlet.
async fn seed_task(handle: &HostHandle, purpose_text: &str, instruction_body: &str) -> SeededTask {
    let companion = running_companion(handle).await;
    let generation = current_generation(handle).await;
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
        .expect("the instruction History row must commit");
    let HistoryAppendOutcome::CommittedAs {
        message: instruction_row,
    } = appended
    else {
        panic!("the instruction History row must commit, got {appended:?}");
    };
    let created = handle
        .store
        .create_task(TaskCreationPremise {
            task: ene_task::TaskId::generate(),
            purpose: TaskPurpose {
                text: purpose_text.to_owned(),
            },
            entry: TaskContextEntryId::generate(),
            origin: TaskContextOrigin {
                kind: TaskContextOriginKind::OwnerConversation,
                source: RawId::new(),
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
                    source: instruction_row,
                },
                acquired_at: WallClockWithTz::now(),
            }),
        })
        .await
        .expect("the instruction adoption must commit");
    let TaskCommitOutcome::CommittedAs(current) = advanced else {
        panic!("the instruction adoption must commit, got {advanced:?}");
    };
    let result_delegation = DelegationId::generate();
    let result_premise = DelegationCreationPremise {
        delegation: result_delegation,
        task: current,
        agent: TaskAgentEphemeralId::generate(),
        scope_copy: DelegationScope { workspace: None },
    };
    assert!(matches!(
        handle
            .store
            .create_delegation(result_premise)
            .await
            .expect("the result delegation must commit"),
        DelegationOutcome::Delegated(_)
    ));
    let turn_delegation = DelegationId::generate();
    let turn_premise = DelegationCreationPremise {
        delegation: turn_delegation,
        task: current,
        agent: TaskAgentEphemeralId::generate(),
        scope_copy: DelegationScope { workspace: None },
    };
    assert!(matches!(
        handle
            .store
            .create_delegation(turn_premise)
            .await
            .expect("the turn delegation must commit"),
        DelegationOutcome::Delegated(_)
    ));
    SeededTask {
        task: current,
        turn_delegation,
        result_delegation,
    }
}

/// Legacy plaintext the registration sweep must cover: the same value recorded
/// as ordinary text through the canonical owner boundaries while it is not a
/// registered credential yet.
struct LegacySeed {
    task: TaskRef,
    turn_delegation: DelegationId,
    result_delegation: DelegationId,
    result: ene_task::TaskResultId,
    activity: ActivityId,
    memory: MemoryId,
    summary: SummaryId,
    history_row: RawId,
}

async fn seed_legacy(handle: &HostHandle, secret: &str) -> LegacySeed {
    let companion = running_companion(handle).await;
    let generation = current_generation(handle).await;
    let appended = handle
        .store
        .append_message(AppendHistoryCommand {
            companion,
            round: RawId::new(),
            role: HistoryRole::Owner,
            text: format!("the owner mentioned the key {secret} yesterday"),
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
        .expect("the legacy History row must commit");
    let HistoryAppendOutcome::CommittedAs {
        message: history_row,
    } = appended
    else {
        panic!("the legacy History row must commit, got {appended:?}");
    };

    let seeded = seed_task(
        handle,
        &format!("draft the report that quotes {secret}"),
        &format!("use the key {secret} when reading the notes"),
    )
    .await;
    let arrival = crate::test_support::record_result(
        &handle.store,
        seeded.result_delegation,
        &format!("the draft report quotes {secret}"),
    )
    .await;
    let activity = handle
        .store
        .record_resume_activity(RecordResumeActivityCommand {
            companion,
            task: seeded.task,
            purpose: handle
                .store
                .load_task(seeded.task.task)
                .await
                .expect("the task must load")
                .expect("the task exists")
                .task
                .purpose,
            body: format!("continue the report with {secret}"),
            command: RawId::new(),
        })
        .await
        .expect("the legacy activity must record");
    let ene_companion::ResumeActivityOutcome::Recorded(activity) = activity else {
        panic!("the legacy activity must record");
    };

    let memory = MemoryId::generate();
    let summary = SummaryRecord {
        id: SummaryId::generate(),
        scope: LearningScope::companion(companion.as_raw()),
        content: format!("the owner keeps {secret} safe"),
        source: SourceRangeRef {
            kind: ExperienceSourceKind::Dialogue,
            start: RawId::new(),
            end: RawId::new(),
        },
        formed_at: WallClockWithTz::now(),
    };
    let committed = handle
        .store
        .commit_memory_change(MemoryChangeCommit {
            summary: Some(summary.clone()),
            secret_premise: None,
            claim: None,
            change: MemoryChange {
                target: MemoryTarget::New { id: memory },
                scope: LearningScope::companion(companion.as_raw()),
                content: format!("the owner's key is {secret}"),
                importance: Importance::default(),
                temporal: TemporalMeaning::Enduring,
                change: ChangeKind::Initial,
                recall_suppressed: false,
                at: WallClockWithTz::now(),
            },
        })
        .await
        .expect("the legacy memory must commit");
    assert!(
        matches!(
            committed,
            ene_learning::MemoryChangeOutcome::Committed { .. }
        ),
        "the legacy memory must commit, got {committed:?}"
    );

    LegacySeed {
        task: seeded.task,
        turn_delegation: seeded.turn_delegation,
        result_delegation: seeded.result_delegation,
        result: arrival.result,
        activity,
        memory,
        summary: summary.id,
        history_row,
    }
}

/// The positive control: every legacy surface really holds the raw value
/// while it is still ordinary text, so a later scan cannot pass by looking at
/// the wrong rows.
async fn assert_legacy_present(handle: &HostHandle, seed: &LegacySeed, secret: &str) {
    let companion = running_companion(handle).await;
    let timeline = handle
        .store
        .load_timeline(companion, None, None, 50)
        .await
        .expect("the timeline must load");
    let legacy = timeline
        .iter()
        .find(|item| item.id == seed.history_row)
        .expect("the legacy History row is present");
    assert!(
        legacy.text.contains(secret),
        "the raw value is recorded before it becomes a registered credential: {}",
        legacy.text
    );
    let record = handle
        .store
        .load_task(seed.task.task)
        .await
        .expect("the task must load")
        .expect("the task exists");
    assert!(
        record.revision.purpose_text.text.contains(secret),
        "the raw value is recorded in the Task purpose"
    );
    let result = handle
        .store
        .load_report_source_bounded(TaskReportSourceRef::ResultBody(seed.result), 0, 4096)
        .await
        .expect("the result body must read")
        .expect("the result row exists");
    assert!(
        result.text.contains(secret),
        "the raw value is recorded in the Task result"
    );
    let memories = handle
        .store
        .list_current_memories(companion.as_raw(), None, 10)
        .await
        .expect("the memories must list");
    assert!(
        memories
            .iter()
            .any(|memory| memory.content.contains(secret)),
        "the raw value is recorded in Memory"
    );
}

/// After the production approval boundary, every legacy surface is redacted:
/// nothing keeps the raw value and the redaction marker stays visible.
async fn assert_legacy_swept(handle: &HostHandle, seed: &LegacySeed, secret: &str) {
    let companion = running_companion(handle).await;
    let timeline = handle
        .store
        .load_timeline(companion, None, None, 50)
        .await
        .expect("the timeline must load after the approval");
    let swept = timeline
        .iter()
        .find(|item| item.id == seed.history_row)
        .expect("the swept History row is still present");
    assert!(
        !swept.text.contains(secret) && swept.text.contains("[credential]"),
        "the approval sweep must redact the legacy History row: {}",
        swept.text
    );

    let record = handle
        .store
        .load_task(seed.task.task)
        .await
        .expect("the task must load")
        .expect("the task still exists");
    assert!(
        !record.revision.purpose_text.text.contains(secret),
        "the current Task purpose must be swept: {}",
        record.revision.purpose_text.text
    );
    let original = handle
        .store
        .load_report_source_bounded(
            TaskReportSourceRef::RevisionPurpose {
                task: seed.task.task,
                revision: TaskRevision::initial(),
            },
            0,
            4096,
        )
        .await
        .expect("the revision history must read")
        .expect("the original revision is retained");
    assert!(
        !original.text.contains(secret),
        "the Task revision history must be swept: {}",
        original.text
    );
    let result = handle
        .store
        .load_report_source_bounded(TaskReportSourceRef::ResultBody(seed.result), 0, 4096)
        .await
        .expect("the result body must read")
        .expect("the result row exists");
    assert!(
        !result.text.contains(secret),
        "the recorded result body must be swept: {}",
        result.text
    );
    let activity = handle
        .store
        .load_activity(seed.activity)
        .await
        .expect("the activity must load")
        .expect("the activity still exists");
    assert!(
        !activity.body.contains(secret),
        "the resume instruction must be swept: {}",
        activity.body
    );
    let memories = handle
        .store
        .list_current_memories(running_companion(handle).await.as_raw(), None, 10)
        .await
        .expect("the memories must list");
    assert!(
        memories
            .iter()
            .all(|memory| !memory.content.contains(secret)),
        "every Memory must be swept"
    );
    let summaries = handle
        .store
        .load_summaries(&[seed.summary])
        .await
        .expect("the summary must load");
    assert!(
        summaries
            .iter()
            .all(|summary| !summary.content.contains(secret)),
        "every Summary must be swept"
    );
    let revisions = handle
        .store
        .list_memory_revisions(seed.memory, None, 100)
        .await
        .expect("the memory revisions must list");
    assert!(
        !revisions.is_empty()
            && revisions
                .iter()
                .all(|revision| !revision.content.contains(secret)),
        "every Memory revision must be swept"
    );
}

/// The single-secret cross-surface regression: a value registered through the
/// production inlet is absent from every provider capture and every durable
/// surface, both after the legacy sweep and during new dialogue, Learning, and
/// Task Agent work where the model itself echoes the value back.
#[tokio::test]
async fn registered_secret_is_absent_from_provider_captures_and_every_surface() {
    let live = live_input("client-credential-suite");
    let formation = serde_json::json!({
        "summary": format!("The owner keeps {SUITE_SECRET} nearby."),
        "memories": [{
            "action": "create",
            "content": format!("The owner's key is {SUITE_SECRET}."),
            "importance": 5,
            "temporal": "enduring",
        }],
    })
    .to_string();
    let transport = LearningAwareTransport::new("noted", Some(formation.as_str()));
    let (handle, dir) = suite_handle("dlg-credential-suite", SUITE_SECRET).await;

    // Legacy plaintext across the surfaces (positive control), then the
    // production registration intent plus approval: the sweep must redact
    // what the new credential set now covers.
    let seed = seed_legacy(&handle, SUITE_SECRET).await;
    assert_legacy_present(&handle, &seed, SUITE_SECRET).await;
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "the production registration and dialogue setup must complete"
    );
    assert_legacy_swept(&handle, &seed, SUITE_SECRET).await;
    assert!(
        assign_learning(&handle, &live, &transport).await,
        "the Learning capability assignment must commit"
    );

    // New work: the owner asks the partner to remember the (now registered)
    // value, and the scripted model echoes it back in the formation answer.
    let frame = submit_frame(
        handle.companion_wire(),
        Some(0),
        None,
        "local-credential-suite",
        &format!("please remember my api key is {SUITE_SECRET}"),
        live.connection_id,
    );
    let responses = handle.handle_frame(frame, live.clone(), &transport).await;
    assert!(
        responses.iter().any(|frame| matches!(
            &frame.payload,
            WirePayload::TextStreamClose(close)
                if close.status == ene_api::v1::round::StreamClose::Completed
        )),
        "the dialogue round must complete"
    );
    handle.run_pending_learning(&transport).await;

    // One Task Agent turn over the seeded instruction (its body was swept to
    // the redaction marker) must reach the provider scrubbed.
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
    let turn = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation: seed.turn_delegation,
            exchanges: Vec::new(),
        },
    )
    .await
    .expect("the Task Agent turn must answer");
    assert!(
        matches!(turn, TaskAgentTurnOutcome::Produced(_)),
        "the Task Agent turn must produce, got {turn:?}"
    );
    expect_absent(
        "Task Agent outcome debug",
        &format!("{turn:?}"),
        SUITE_SECRET,
    );

    // Provider captures: dialogue, Learning formation, and Task Agent.
    let inputs = transport.inputs();
    assert!(
        inputs.len() >= 3,
        "the suite must observe all three provider routes, saw {}",
        inputs.len()
    );
    for (index, input) in inputs.iter().enumerate() {
        expect_absent(&format!("provider capture {index}"), input, SUITE_SECRET);
    }
    assert!(
        inputs.iter().any(|input| input.contains("[credential]")),
        "the redaction marker reaches the provider instead of the value"
    );

    // Dialogue History.
    let timeline = handle
        .store
        .load_timeline(running_companion(&handle).await, None, None, 50)
        .await
        .expect("the timeline must load");
    for item in &timeline {
        expect_absent("History text", &item.text, SUITE_SECRET);
        expect_absent("History debug", &format!("{item:?}"), SUITE_SECRET);
    }

    // Learning Summary / Memory / revision history.
    let companion = running_companion(&handle).await;
    let memories = handle
        .store
        .list_current_memories(companion.as_raw(), None, 10)
        .await
        .expect("the memories must list");
    assert!(!memories.is_empty(), "the formation must store a Memory");
    let mut summary_ids = vec![seed.summary];
    for memory in &memories {
        expect_absent("Memory content", &memory.content, SUITE_SECRET);
        expect_absent("Memory debug", &format!("{memory:?}"), SUITE_SECRET);
        let revisions = handle
            .store
            .list_memory_revisions(memory.id, None, 100)
            .await
            .expect("the revisions must list");
        assert!(!revisions.is_empty(), "every Memory keeps its revision");
        for revision in &revisions {
            expect_absent("Memory revision content", &revision.content, SUITE_SECRET);
            expect_absent(
                "Memory revision debug",
                &format!("{revision:?}"),
                SUITE_SECRET,
            );
            if let Some(summary) = revision.summary
                && !summary_ids.contains(&summary)
            {
                summary_ids.push(summary);
            }
        }
    }
    let summaries = handle
        .store
        .load_summaries(&summary_ids)
        .await
        .expect("the summaries must load");
    for summary in &summaries {
        expect_absent("Summary content", &summary.content, SUITE_SECRET);
        expect_absent("Summary debug", &format!("{summary:?}"), SUITE_SECRET);
    }

    // Task purpose / result / report rows.
    let record = handle
        .store
        .load_task(seed.task.task)
        .await
        .expect("the task must load")
        .expect("the task still exists");
    expect_absent(
        "Task purpose text",
        &record.revision.purpose_text.text,
        SUITE_SECRET,
    );
    expect_absent("Task record debug", &format!("{record:?}"), SUITE_SECRET);
    let result = handle
        .store
        .load_task_result(seed.result)
        .await
        .expect("the result must load")
        .expect("the result still exists");
    expect_absent("Task result body", result.body.text(), SUITE_SECRET);
    expect_absent("Task result debug", &format!("{result:?}"), SUITE_SECRET);
    let rows = handle
        .store
        .list_task_report_rows_after(seed.task.task, None, 50)
        .await
        .expect("the report rows must list");
    for row in &rows {
        expect_absent("Task report row debug", &format!("{row:?}"), SUITE_SECRET);
    }
    for attempt in handle
        .store
        .load_task_action_attempts(seed.task.task)
        .await
        .expect("the action attempts must list")
    {
        let stored = handle
            .store
            .load_attempt(ActionAttemptId::from_raw(attempt))
            .await
            .expect("the attempt must load")
            .expect("the attempt still exists");
        expect_absent(
            "Action detail target",
            stored.real_target.as_path(),
            SUITE_SECRET,
        );
        expect_absent("Action detail debug", &format!("{stored:?}"), SUITE_SECRET);
    }
    let original_revision = handle
        .store
        .load_report_source_bounded(
            TaskReportSourceRef::RevisionPurpose {
                task: seed.task.task,
                revision: TaskRevision::initial(),
            },
            0,
            4096,
        )
        .await
        .expect("the report source must read")
        .expect("the revision is retained");
    expect_absent(
        "Task report source text",
        &original_revision.text,
        SUITE_SECRET,
    );
    // The composed user-facing report renders the result body and the Action
    // attempt targets; its own rendering is a surface too.
    let composed = handle
        .task_report(seed.task.task, Some(seed.result_delegation))
        .await
        .expect("the composed report must answer")
        .expect("the task exists");
    expect_absent("Task report render", &composed.render(), SUITE_SECRET);
    expect_absent("Task report debug", &format!("{composed:?}"), SUITE_SECRET);

    // Task report and task list frames.
    let listed = handle
        .handle_frame(
            domain_frame(
                &live,
                WirePayload::ListTasks(ListTasks {
                    cursor: None,
                    limit: None,
                }),
            ),
            live.clone(),
            &transport,
        )
        .await;
    expect_absent_in_frames("task list", &listed, SUITE_SECRET);
    let report = handle
        .handle_frame(
            domain_frame(
                &live,
                WirePayload::GetTaskReport(GetTaskReport {
                    task: task_wire_ref(seed.task.task),
                    cursor: None,
                    limit: None,
                }),
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        report
            .iter()
            .any(|frame| matches!(&frame.payload, WirePayload::TaskReportResponse(_))),
        "the task report must answer, got {report:?}"
    );
    expect_absent_in_frames("task report", &report, SUITE_SECRET);

    // Undelivered and presentation excerpts read the canonical swept rows.
    let undelivered = handle
        .handle_frame(
            domain_frame(
                &live,
                WirePayload::UndeliveredRequest(UndeliveredRequest {
                    companion: None,
                    cursor: None,
                    limit: None,
                    redisplay: false,
                }),
            ),
            live.clone(),
            &transport,
        )
        .await;
    expect_absent_in_frames("undelivered", &undelivered, SUITE_SECRET);
    let entries = handle
        .store
        .list_unpresented(companion, None, 50)
        .await
        .expect("the unpresented rows must list")
        .entries;
    assert!(
        !entries.is_empty(),
        "the suite must observe undelivered correlations"
    );
    for entry in &entries {
        expect_absent("undelivered ref debug", &format!("{entry:?}"), SUITE_SECRET);
        if let Some(excerpt) = handle
            .store
            .load_undelivered_excerpt(entry.source, 65_536)
            .await
            .expect("the excerpt must read")
        {
            expect_absent("presentation excerpt", &excerpt.text, SUITE_SECRET);
        }
    }
    // The Task result excerpt is one of the presented bodies; read it
    // explicitly so a missing registration cannot hide the surface.
    let task_excerpt = handle
        .store
        .load_undelivered_excerpt(
            UndeliveredSource::TaskRecord {
                task: seed.task.task.as_raw(),
                fact: TaskFact::ResultRecorded(seed.result.as_raw()),
            },
            65_536,
        )
        .await
        .expect("the Task result excerpt must read")
        .expect("the result source carries a bounded body");
    expect_absent(
        "Task result presentation excerpt",
        &task_excerpt.text,
        SUITE_SECRET,
    );

    // Management view: every section body plus the frame bytes.
    let view = handle
        .handle_frame(
            view_request_frame(live.connection_id),
            live.clone(),
            &transport,
        )
        .await;
    expect_absent_in_frames("management view", &view, SUITE_SECRET);
    let Some(WirePayload::ManagementView(view)) = view.first().map(|frame| &frame.payload) else {
        panic!("the view request must answer a view, got {view:?}");
    };
    assert!(!view.sections.is_empty(), "the view carries its sections");
    for section in &view.sections {
        expect_absent(
            &format!("management view section {}", section.kind),
            &section.body,
            SUITE_SECRET,
        );
    }

    // Response frames of the whole dialogue round, and the debug renderings of
    // the errors this suite produced.
    expect_absent_in_frames("dialogue round", &responses, SUITE_SECRET);
    expect_absent(
        "credential store debug",
        &format!("{:?}", handle.cred_store),
        SUITE_SECRET,
    );

    // The full durable backstop: no text column anywhere holds the value.
    let hits = durable_occurrences(&dir.path().join("app.db"), SUITE_SECRET);
    assert!(
        hits.is_empty(),
        "the registered value must not survive anywhere durable: {hits:?}"
    );
}

/// Error and debug renderings of the credential boundary itself never carry
/// the value: an unreadable registered bearer fails closed with a reason that
/// names the credential, not its secret.
#[tokio::test]
async fn credential_failures_render_without_the_secret() {
    let live = live_input("client-credential-errors");
    let transport = LearningAwareTransport::new("noted", None);
    let (handle, _dir) = suite_handle("dlg-credential-errors", SUITE_SECRET).await;
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "the production registration and dialogue setup must complete"
    );

    // The registered ref is readable, but the pinned value is gone: the
    // scrubber must fail closed and the error must name the class only.
    let empty = MemoryCredentialStore::new();
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &empty,
    };
    let error = scrubber
        .scrub(&format!("the key is {SUITE_SECRET}"))
        .await
        .expect_err("an unreadable registered bearer cannot prove absence");
    expect_absent("scrub error display", &error.to_string(), SUITE_SECRET);
    expect_absent("scrub error debug", &format!("{error:?}"), SUITE_SECRET);
    expect_absent(
        "learning error display",
        &LearningTechnicalError::SecretBoundaryUnavailable {
            reason: error.to_string(),
        }
        .to_string(),
        SUITE_SECRET,
    );
    expect_absent(
        "credential store debug",
        &format!("{:?}", handle.cred_store),
        SUITE_SECRET,
    );
}

/// A credential rotation that lands between the dialogue input scrub and the
/// owner History append must refuse the append (the prepared input may carry
/// the newly registered value); only a fresh submit, re-scrubbed under the new
/// revision, may commit. Both the refusal frames and the durable rows stay
/// free of the value.
#[tokio::test]
async fn rotation_between_dialogue_scrub_and_history_append_refuses_then_rescrubbed_retry_commits()
{
    let live = live_input("client-credential-history-race");
    let transport = LearningAwareTransport::new("noted", None);
    let (handle, dir) = suite_handle("dlg-credential-history-race", SUITE_SECRET).await;
    let setup = register_assign_complete(&handle, &live, &transport).await;
    assert!(
        setup,
        "the production registration and dialogue setup must complete"
    );
    let handle = std::sync::Arc::new(handle);
    let text = format!("the new api key is {ROTATED_SECRET}");

    let gate = handle.arm_submit_accept_gate();
    let submit = {
        let handle = std::sync::Arc::clone(&handle);
        let live = live.clone();
        let text = text.clone();
        tokio::spawn(async move {
            let transport = LearningAwareTransport::new("noted", None);
            handle
                .handle_frame(
                    submit_frame(
                        handle.companion_wire(),
                        Some(0),
                        None,
                        "local-history-race",
                        &text,
                        live.connection_id,
                    ),
                    live,
                    &transport,
                )
                .await
        })
    };
    gate.wait_entered().await;
    rotate_registered(&handle, ROTATED_SECRET);
    handle.disarm_submit_accept_gate();
    gate.release();
    let refused = submit.await.expect("the raced submit must join");

    assert!(
        refused.iter().any(|frame| matches!(
            &frame.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::HeldForTransition)
        )),
        "the stale credential premise must hold the append, got {refused:?}"
    );
    expect_absent_in_frames("refused submit", &refused, ROTATED_SECRET);
    let timeline = handle
        .store
        .load_timeline(running_companion(&handle).await, None, None, 50)
        .await
        .expect("the timeline must load");
    assert!(
        timeline
            .iter()
            .all(|item| !item.text.contains(ROTATED_SECRET)),
        "no stale append may land raw text: {timeline:?}"
    );

    // Retry: the same text is re-scrubbed under the new revision and commits
    // only in its redacted form.
    let retry = handle
        .handle_frame(
            submit_frame(
                handle.companion_wire(),
                Some(current_generation(&handle).await.as_u64()),
                None,
                "local-history-race-retry",
                &text,
                live.connection_id,
            ),
            live.clone(),
            &transport,
        )
        .await;
    assert!(
        retry.iter().any(|frame| matches!(
            &frame.payload,
            WirePayload::RoundIntakeOutcome(RoundIntakeOutcomeWire::AcceptedForRound { .. })
        )),
        "the re-scrubbed retry must be accepted, got {retry:?}"
    );
    expect_absent_in_frames("retry submit", &retry, ROTATED_SECRET);
    let owner = handle
        .store
        .load_timeline(running_companion(&handle).await, None, None, 50)
        .await
        .expect("the timeline must load")
        .into_iter()
        .find(|item| item.role == HistoryRole::Owner)
        .expect("the retried Owner row commits");
    assert!(
        !owner.text.contains(ROTATED_SECRET) && owner.text.contains("[credential]"),
        "the retried row carries the redaction marker only: {}",
        owner.text
    );
    for input in transport.inputs() {
        expect_absent("provider capture", &input, ROTATED_SECRET);
    }
    assert!(
        durable_occurrences(&dir.path().join("app.db"), ROTATED_SECRET).is_empty(),
        "the rotated value must never land durable"
    );
}

/// A scrubber that delegates to the credential-owned boundary and then rotates
/// the registered pair exactly once, after the proof it returned was minted.
/// This is the deterministic rotation window the production commit compares
/// against; it never mints a proof of its own.
struct RotatingScrubber<'a> {
    handle: &'a HostHandle,
    rotate_to: &'a str,
    rotate_on: &'a str,
    rotated: AtomicBool,
}

impl<'a> RotatingScrubber<'a> {
    fn new(handle: &'a HostHandle, rotate_on: &'a str, rotate_to: &'a str) -> Self {
        Self {
            handle,
            rotate_to,
            rotate_on,
            rotated: AtomicBool::new(false),
        }
    }
}

impl SecretScrubber for RotatingScrubber<'_> {
    async fn scrub(
        &self,
        text: &str,
    ) -> Result<ene_credential::ScrubbedText, ene_credential::SecretScrubError> {
        let proof = CredentialScrubber {
            refs: &self.handle.store,
            store: &self.handle.cred_store,
        }
        .scrub(text)
        .await?;
        if text.contains(self.rotate_on) && !self.rotated.swap(true, Ordering::SeqCst) {
            rotate_registered(self.handle, self.rotate_to);
        }
        Ok(proof)
    }
}

fn learning_candidate(companion: RawId, transcript: &str) -> ExperienceCandidate {
    ExperienceCandidate {
        companion,
        source: SourceRangeRef {
            kind: ExperienceSourceKind::Dialogue,
            start: RawId::new(),
            end: RawId::new(),
        },
        // The production pin always names at least the transcript message it
        // read; the claim refuses an empty correlation.
        sources: vec![RawId::new()],
        transcript: vec![ExperienceTurn {
            role: ExperienceRole::Owner,
            text: transcript.to_owned(),
            at: Some(WallClockWithTz::now()),
        }],
        at: WallClockWithTz::now(),
    }
}

/// A rotation that lands after the formation's provider answer has been
/// scrubbed into Summary/Memory proofs and before the commit must refuse the
/// whole pass with a technical secret-boundary error that carries no value;
/// nothing is stored. The prompt itself never carries the value, so the
/// refusal is the commit's currentness compare, not a stale send. Only the
/// retry whose content is re-scrubbed under the new revision forms Memory.
#[tokio::test]
async fn rotation_during_learning_formation_refuses_the_commit_then_rescrubbed_retry_forms() {
    let live = live_input("client-credential-learning-race");
    // The model echoes a value the transcript never mentioned: the rotation
    // window therefore opens exactly at the answer's Summary scrub, after the
    // provider request was already admitted and answered.
    let formation = serde_json::json!({
        "summary": format!("The owner rotates to {ROTATED_SECRET}."),
        "memories": [{
            "action": "create",
            "content": format!("The rotated key is {ROTATED_SECRET}."),
            "importance": 5,
            "temporal": "enduring",
        }],
    })
    .to_string();
    let transport = LearningAwareTransport::new("noted", Some(formation.as_str()));
    let (handle, dir) = suite_handle("dlg-credential-learning-race", SUITE_SECRET).await;
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "the production registration and dialogue setup must complete"
    );
    assert!(
        assign_learning(&handle, &live, &transport).await,
        "the Learning capability assignment must commit"
    );
    let companion = running_companion(&handle).await;
    let candidate = learning_candidate(companion.as_raw(), "remember the new key I mentioned");

    let executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &transport,
    };
    let rotating = RotatingScrubber::new(&handle, ROTATED_SECRET, ROTATED_SECRET);
    let outcome = ene_companion::dialogue::propose_experience(
        candidate.clone(),
        &handle.store,
        &executor,
        &rotating,
    )
    .await;
    let error = outcome.expect_err("the rotated premise must refuse the commit");
    assert!(
        matches!(
            error,
            LearningTechnicalError::SecretBoundaryUnavailable { .. }
        ),
        "the stale credential premise is a secret-boundary refusal, got {error:?}"
    );
    expect_absent(
        "formation error display",
        &error.to_string(),
        ROTATED_SECRET,
    );
    expect_absent(
        "formation error debug",
        &format!("{error:?}"),
        ROTATED_SECRET,
    );
    assert!(
        handle
            .store
            .list_current_memories(companion.as_raw(), None, 10)
            .await
            .expect("the memories must list")
            .is_empty(),
        "a refused formation stores no Memory"
    );
    let inputs = transport.inputs();
    assert!(
        inputs.iter().all(|input| !input.contains(ROTATED_SECRET)),
        "even the refused pass must not send the value: {inputs:?}"
    );

    // Retry with the production scrubber: the input is re-scrubbed under the
    // new revision, so the formation commits its redacted content.
    let scrubber = CredentialScrubber {
        refs: &handle.store,
        store: &handle.cred_store,
    };
    let retried =
        ene_companion::dialogue::propose_experience(candidate, &handle.store, &executor, &scrubber)
            .await
            .expect("the re-scrubbed retry must form");
    assert!(
        matches!(retried, ene_learning::FormationDecision::Formed { .. }),
        "the retry must form Memory, got {retried:?}"
    );
    let memories = handle
        .store
        .list_current_memories(companion.as_raw(), None, 10)
        .await
        .expect("the memories must list");
    assert_eq!(memories.len(), 1, "the retry stores exactly one Memory");
    assert!(
        !memories[0].content.contains(ROTATED_SECRET)
            && memories[0].content.contains("[credential]"),
        "the stored Memory carries the redaction marker: {}",
        memories[0].content
    );
    for input in transport.inputs() {
        expect_absent("provider capture", &input, ROTATED_SECRET);
    }
    assert!(
        durable_occurrences(&dir.path().join("app.db"), ROTATED_SECRET).is_empty(),
        "the rotated value must never land durable"
    );
}

/// A Task Agent inference port wrapper that rotates the registered pair just
/// before delegating the already-scrubbed premise to the real adapter: the
/// durable attempt claim must refuse the send with zero provider bytes and
/// zero attempt rows, and only the next turn's re-scrub may claim.
struct RotatingTaskAgent<'a> {
    handle: &'a HostHandle,
    transport: &'a LearningAwareTransport,
    rotate_to: &'a str,
    rotated: AtomicBool,
}

impl TaskAgentInference for RotatingTaskAgent<'_> {
    fn input_budget(&self) -> usize {
        ene_inference::MAX_INPUT_CHARS
    }

    async fn infer(
        &self,
        premise: TaskAgentInferencePremise,
    ) -> Result<TaskAgentInferenceOutcome, TaskAgentInferenceError> {
        if !self.rotated.swap(true, Ordering::SeqCst) {
            rotate_registered(self.handle, self.rotate_to);
        }
        let executor = HostInference {
            store: &self.handle.store,
            cred_store: &self.handle.cred_store,
            tracker: &self.handle.tracker,
            transport: self.transport,
        };
        TaskAgentInferenceAdapter::new(&executor, None)
            .infer(premise)
            .await
    }
}

fn durable_attempt_count(db_path: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(db_path).expect("the suite app.db must open");
    conn.query_row("SELECT COUNT(*) FROM inference_attempt", (), |row| {
        row.get(0)
    })
    .expect("the attempt count must read")
}

/// A rotation that lands between the Task Agent scrub and the attempt claim
/// must refuse the send: the provider sees zero bytes, no attempt is durable,
/// and the refusal diagnostic carries no value. The retry over the same
/// delegation is re-scrubbed and claims normally.
#[tokio::test]
async fn rotation_between_task_agent_scrub_and_claim_refuses_then_retry_claims() {
    let live = live_input("client-credential-task-race");
    let transport = LearningAwareTransport::new("agent report", None);
    let (handle, dir) = suite_handle("dlg-credential-task-race", SUITE_SECRET).await;
    assert!(
        register_assign_complete(&handle, &live, &transport).await,
        "the production registration and dialogue setup must complete"
    );
    let seeded = seed_task(
        &handle,
        "write the private note",
        &format!("use the key {ROTATED_SECRET} when reading the notes"),
    )
    .await;
    let instructions = OwnerInstructionSource::new(&handle.store, &handle.store);
    let rotating = RotatingTaskAgent {
        handle: &handle,
        transport: &transport,
        rotate_to: ROTATED_SECRET,
        rotated: AtomicBool::new(false),
    };
    let refused = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &rotating,
        &CredentialScrubber {
            refs: &handle.store,
            store: &handle.cred_store,
        },
        TaskAgentTurnPremise {
            delegation: seeded.turn_delegation,
            exchanges: Vec::new(),
        },
    )
    .await
    .expect("the refused turn must answer a domain outcome");
    let TaskAgentTurnOutcome::NotSent(_) = &refused else {
        panic!("the rotated premise must refuse the send, got {refused:?}");
    };
    expect_absent(
        "task refusal debug",
        &format!("{refused:?}"),
        ROTATED_SECRET,
    );
    assert!(
        transport.inputs().is_empty(),
        "a refused claim sends zero provider bytes"
    );
    assert_eq!(
        durable_attempt_count(&dir.path().join("app.db")),
        0,
        "a refused claim starts no durable attempt"
    );
    let instruction = handle
        .store
        .load_timeline(running_companion(&handle).await, None, None, 50)
        .await
        .expect("the timeline must load")
        .into_iter()
        .find(|item| item.role == HistoryRole::Owner)
        .expect("the instruction row is retained");
    assert!(
        !instruction.text.contains(ROTATED_SECRET) && instruction.text.contains("[credential]"),
        "the rotation sweep redacts the instruction: {}",
        instruction.text
    );

    // Retry over the same delegation with the production port: the re-scrub
    // sees the registered value and the claim starts the attempt.
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
    let retried = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation: seeded.turn_delegation,
            exchanges: Vec::new(),
        },
    )
    .await
    .expect("the retried turn must answer");
    assert!(
        matches!(retried, TaskAgentTurnOutcome::Produced(_)),
        "the re-scrubbed retry must produce, got {retried:?}"
    );
    let inputs = transport.inputs();
    assert_eq!(
        inputs.len(),
        1,
        "the retry sends exactly one provider request"
    );
    expect_absent("task provider capture", &inputs[0], ROTATED_SECRET);
    assert!(
        inputs[0].contains("[credential]"),
        "the provider sees the redaction marker in place of the value"
    );
    assert_eq!(
        durable_attempt_count(&dir.path().join("app.db")),
        1,
        "the retried claim records exactly one attempt"
    );

    // A provider failure on a later turn renders its class only: the task
    // failure diagnostic never carries the value or the scrubbed prompt.
    let failing = ene_inference::fake::FakeProviderTransport::failing(
        ene_inference::fake::FakeFailure::Transport(String::from("provider down")),
    );
    let failing_executor = HostInference {
        store: &handle.store,
        cred_store: &handle.cred_store,
        tracker: &handle.tracker,
        transport: &failing,
    };
    let failing_adapter = TaskAgentInferenceAdapter::new(&failing_executor, None);
    let error = orchestrate_task_agent_turn(
        &handle.store,
        &instructions,
        &failing_adapter,
        &scrubber,
        TaskAgentTurnPremise {
            delegation: seeded.turn_delegation,
            exchanges: Vec::new(),
        },
    )
    .await
    .expect_err("a provider failure is a technical error");
    expect_absent("task failure display", &error.to_string(), ROTATED_SECRET);
    expect_absent("task failure debug", &format!("{error:?}"), ROTATED_SECRET);

    let record = handle
        .store
        .load_task(seeded.task.task)
        .await
        .expect("the task must load")
        .expect("the task still exists");
    expect_absent(
        "Task purpose text",
        &record.revision.purpose_text.text,
        ROTATED_SECRET,
    );
    assert!(
        durable_occurrences(&dir.path().join("app.db"), ROTATED_SECRET).is_empty(),
        "the rotated value must never land durable"
    );
}
