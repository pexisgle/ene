//! Slice D acceptance: presentation subscription, receipts, Task
//! list/selection, and the wire query layer.
//!
//! Deterministic by construction: every race covers both orders by
//! sequencing (no sleeps), and the 30 s receipt TTL is forced through the
//! test hook. S5-02/07/08/09/10/12 plus provider-down independence.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "acceptance fixtures assert; unwraps in helpers fail the test loudly"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ene_action::{
    ActionAttemptId, ActionAttemptRepository as _, ActionCertainty, ActionStartOutcome,
    AttemptCommitPremise, CertaintyUpdateOutcome, EffectGrounds, OperationKind, RealTargetRef,
};
use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{
    ClientIncarnationId, CommandWireId, RequestWireId, RoundWireId, WireMessageType,
};
use ene_api::v1::round::PresentationStatus;
use ene_api::v1::undelivered::{
    ListTasks, ResumeTask, ResumeTaskOutcomeWire, SelectTask, TaskListResponse, UndeliveredAck,
    UndeliveredAckOutcome, UndeliveredRequest, UndeliveredResponse,
};
use ene_companion::{
    AppendHistoryCommand, CompanionId, CompanionRepository as _, HistoryRepository as _,
    HistoryRole, ReportStatus, UndeliveredRef, UndeliveredRepository as _,
};
use ene_plugin_ipc::WireFrame;
use ene_presence::{
    PresenceAttribution, PresenceGeneration, PresenceRepository as _, PresenceState,
};
use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegatedWorkspace, DelegationCreationPremise, DelegationId, DelegationOutcome,
    DelegationScope, TaskAgentEphemeralId, TaskContextEntryId, TaskContextOrigin,
    TaskContextOriginKind, TaskCreationPremise, TaskId, TaskPurpose, TaskRef, TaskRepository as _,
    TaskResultAcceptance, TaskResultAdoptionClaim, WorkspaceAssocId, WorkspaceAssociationPremise,
    WorkspaceFolderRef, WorkspaceNeedRef,
};

use crate::serve::{HostHandle, LiveInput};
use crate::task_run::TaskAgentLauncher;
use crate::test_support::{live_input, memory_handle};

const DEVICE_A: &str = "test-device-a";

fn incarnation() -> ClientIncarnationId {
    ClientIncarnationId {
        counter: 7,
        random: 8,
    }
}

/// Builds an authenticated frame for `live`, as the connection table would:
/// the envelope connection id echoes the table id, and observed marks plus
/// the command identity travel explicitly.
fn frame_for(
    payload: WirePayload,
    live: &LiveInput,
    observed_generation: Option<u64>,
    observed_round: Option<RoundWireId>,
    command: Option<CommandWireId>,
) -> WireFrame {
    let mut envelope = new_outgoing_envelope(
        ProtocolVersion::V1,
        WireSender {
            device_id: None,
            incarnation_id: incarnation(),
            connection_id: Some(live.connection_id),
        },
        WireMessageType(payload.message_type().to_string()),
    );
    envelope.correlation.request_id = Some(RequestWireId(uuid::Uuid::new_v4()));
    envelope.correlation.command_id = command;
    envelope.observed.presence_generation_view = observed_generation;
    envelope.observed.round_view = observed_round;
    WireFrame { envelope, payload }
}

async fn open_handle(tag: &str) -> (HostHandle, tempfile::TempDir) {
    memory_handle(tag).await.expect("the handle must open")
}

async fn companion_of(handle: &HostHandle) -> CompanionId {
    handle
        .store
        .ensure_running_companion()
        .await
        .expect("the companion must resolve")
}

async fn attribution_of(handle: &HostHandle) -> PresenceAttribution {
    let companion = companion_of(handle).await;
    handle
        .store
        .load_attribution(companion.as_raw())
        .await
        .expect("attribution must read")
        .expect("attribution must exist")
}

/// Attaches `device` from `NoActive` through the Host's own attach path and
/// returns the fresh fact. Each call needs the current generation: presence
/// moves only forward.
async fn attach(handle: &HostHandle, device: &str) -> PresenceAttribution {
    let current = attribution_of(handle).await;
    assert_eq!(
        current.state,
        PresenceState::NoActive,
        "attach starts from NoActive, got {:?}",
        current.state
    );
    let live = crate::test_support::live_input(device);
    match handle
        .attach_presence(
            &live,
            device,
            true,
            ene_presence::PresenceState::NoActive,
            current.generation,
        )
        .await
    {
        crate::dialogue::AttachOutcome::Attached(fresh) => fresh,
        other => panic!("attach must win on a fresh handle, got {other:?}"),
    }
}

async fn append_reply(
    handle: &HostHandle,
    text: &str,
    generation: PresenceGeneration,
) -> UndeliveredRef {
    let companion = companion_of(handle).await;
    let (outcome, registered) = handle
        .store
        .append_reply_with_undelivered(
            AppendHistoryCommand {
                companion,
                round: RawId::new(),
                role: HistoryRole::Companion,
                text: text.to_string(),
                lang: String::from("en"),
                at: WallClockWithTz::now(),
                expected_generation: generation,
                expected_consent: None,
                expected_credential_set: None,
                expected_owner_message: None,
                command_id: None,
                round_wire: Some(uuid::Uuid::new_v4().as_hyphenated().to_string()),
                round_intent: None,
                incarnation: None,
                local_id: None,
            },
            true,
            None,
        )
        .await
        .expect("the append must commit");
    assert!(
        matches!(
            outcome,
            ene_companion::HistoryAppendOutcome::CommittedAs { .. }
        ),
        "the append must commit, got {outcome:?}"
    );
    registered.expect("registration must ride the append commit")
}

/// Seeds one Task with a confirmed workspace association (no delegation, no
/// runner): creation registers its own undelivered correlation.
async fn seed_task(handle: &HostHandle) -> TaskRef {
    let companion = companion_of(handle).await;
    handle
        .store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("read the input and write the report"),
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
            workspace: Some(WorkspaceAssociationPremise {
                assoc: ene_task::WorkspaceAssocId::generate(),
                need: WorkspaceNeedRef {
                    folder: WorkspaceFolderRef {
                        path: String::from("/tmp/ene-test-workspace"),
                    },
                    save_target: None,
                },
            }),
        })
        .await
        .expect("task creation commits")
}

/// One undelivered page request through dispatch, asserting the wire shape.
async fn fetch(
    handle: &HostHandle,
    live: &LiveInput,
    cursor: Option<String>,
    limit: Option<u32>,
    redisplay: bool,
) -> UndeliveredResponse {
    let frame = frame_for(
        WirePayload::UndeliveredRequest(UndeliveredRequest {
            companion: None,
            cursor: cursor.map(ene_api::v1::undelivered::PageCursorWire),
            limit,
            redisplay,
        }),
        live,
        None,
        None,
        None,
    );
    let request = match &frame.payload {
        WirePayload::UndeliveredRequest(request) => request.clone(),
        _ => unreachable!(),
    };
    let frames = handle.request_undelivered(&frame, live, &request).await;
    assert_eq!(frames.len(), 1, "one request answers one frame");
    match frames.into_iter().next().unwrap().payload {
        WirePayload::UndeliveredResponse(response) => response,
        unexpected => panic!("expected UndeliveredResponse, got {unexpected:?}"),
    }
}

fn summary_of(response: UndeliveredResponse) -> ene_api::v1::undelivered::UndeliveredSummary {
    match response {
        UndeliveredResponse::Summary(summary) => summary,
        other => panic!("expected a summary page, got {other:?}"),
    }
}

/// ACKs one receipt through dispatch, echoing the round and generation the
/// summary showed (the Host compares them; self-claims prove nothing).
async fn ack(
    handle: &HostHandle,
    live: &LiveInput,
    receipt: &str,
    round: RoundWireId,
    generation: u64,
    status: PresentationStatus,
) -> UndeliveredAckOutcome {
    let frame = frame_for(
        WirePayload::UndeliveredAck(UndeliveredAck {
            receipt: ene_api::v1::undelivered::PresentationReceiptWireRef(receipt.to_string()),
            status,
        }),
        live,
        Some(generation),
        Some(round),
        None,
    );
    let ack = match &frame.payload {
        WirePayload::UndeliveredAck(ack) => ack.clone(),
        _ => unreachable!(),
    };
    let frames = handle.ack_undelivered(&frame, live, &ack).await;
    assert_eq!(frames.len(), 1, "one ACK answers one outcome");
    match frames.into_iter().next().unwrap().payload {
        WirePayload::UndeliveredAckOutcome(outcome) => outcome,
        unexpected => panic!("expected UndeliveredAckOutcome, got {unexpected:?}"),
    }
}

async fn unpresented_statuses(handle: &HostHandle) -> Vec<(String, ReportStatus)> {
    let companion = companion_of(handle).await;
    let page = handle
        .store
        .list_unpresented(companion, None, 50)
        .await
        .expect("the listing must read");
    assert!(page.next.is_none(), "fixtures stay within one page");
    page.entries
        .iter()
        .map(|entry| {
            (
                entry.id.as_raw().as_uuid().as_hyphenated().to_string(),
                entry.status,
            )
        })
        .collect()
}

/// Noop launcher: reserves the launch decision (readiness sees a runner)
/// without starting any execution or provider I/O.
struct NoopLauncher {
    launches: AtomicUsize,
}

impl TaskAgentLauncher for NoopLauncher {
    fn launch(&self, _delegation: DelegationId) {
        self.launches.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn s5_10_backlog_pages_splits_advance_and_expire() {
    let (handle, _dir) = open_handle("present-s5-10").await;
    let live_a = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    for index in 0..51 {
        append_reply(
            &handle,
            &format!("backlog row {index:02}"),
            fresh.generation,
        )
        .await;
    }
    // 51 rows page 50 + 1 under the default cap.
    let page1 = summary_of(fetch(&handle, &live_a, None, None, false).await);
    assert_eq!(page1.items.len(), 50);
    assert!(page1.next_cursor.is_some(), "the pass continues");
    assert!(page1.has_more);
    let applied = ack(
        &handle,
        &live_a,
        &page1.receipt.0,
        page1.round.clone(),
        page1.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(applied, UndeliveredAckOutcome::Presented { presented: 50 }),
        "got {applied:?}"
    );
    let cursor = page1.next_cursor.map(|cursor| cursor.0).expect("cursor");
    let page2 = summary_of(fetch(&handle, &live_a, Some(cursor), None, false).await);
    assert_eq!(page2.items.len(), 1, "no head-of-line loss at the tail");
    // Sub-50 frame splits under a small agreed cap still lose nothing.
    handle.set_frame_budget_for_test(3 * 1024);
    for index in 0..6 {
        append_reply(
            &handle,
            &format!("wide row {index} {}", "x".repeat(900)),
            attribution_of(&handle).await.generation,
        )
        .await;
    }
    let live_b = live_input(DEVICE_A);
    let mut seen = 0_usize;
    let mut cursor: Option<String> = None;
    for _ in 0..8 {
        let page = summary_of(fetch(&handle, &live_b, cursor.clone(), None, false).await);
        assert!(page.items.len() < 50, "the cap splits below fifty");
        assert!(!page.items.is_empty());
        seen += page.items.len();
        let receipt = page.receipt.0.clone();
        let outcome = ack(
            &handle,
            &live_b,
            &receipt,
            page.round.clone(),
            page.presence_generation,
            PresentationStatus::Presented,
        )
        .await;
        assert!(
            matches!(outcome, UndeliveredAckOutcome::Presented { .. }),
            "got {outcome:?}"
        );
        cursor = page.next_cursor.map(|cursor| cursor.0);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen, 7, "all six wide rows plus the tail row, got {seen}");
}

#[tokio::test]
async fn select_task_resolves_without_mutation_or_execution() {
    let (handle, _dir) = open_handle("present-select").await;
    let live_a = live_input(DEVICE_A);
    let _fresh = attach(&handle, DEVICE_A).await;
    let task = seed_task(&handle).await;
    let before = handle
        .store
        .load_task(task.task)
        .await
        .expect("read")
        .expect("exists");
    // List first: selection rides the connection-scoped refs the query
    // issued (restart drops them back to UnknownRef).
    let list_frame = frame_for(
        WirePayload::ListTasks(ListTasks {
            cursor: None,
            limit: None,
        }),
        &live_a,
        None,
        None,
        None,
    );
    let query = match &list_frame.payload {
        WirePayload::ListTasks(query) => query.clone(),
        _ => unreachable!(),
    };
    let frames = handle.list_tasks_wire(&list_frame, &live_a, &query).await;
    let page = match frames.into_iter().next().unwrap().payload {
        WirePayload::TaskListResponse(TaskListResponse::Page(page)) => page,
        unexpected => panic!("expected a page, got {unexpected:?}"),
    };
    let wire_task = page.tasks[0].task.clone();
    let select_frame = frame_for(
        WirePayload::SelectTask(SelectTask {
            task: wire_task.clone(),
        }),
        &live_a,
        None,
        None,
        None,
    );
    let query = match &select_frame.payload {
        WirePayload::SelectTask(query) => query.clone(),
        _ => unreachable!(),
    };
    let frames = handle
        .select_task_wire(&select_frame, &live_a, &query)
        .await;
    let selected = match frames.into_iter().next().unwrap().payload {
        WirePayload::SelectTaskResponse(
            ene_api::v1::undelivered::SelectTaskResponse::Selected(selected),
        ) => selected,
        unexpected => panic!("expected a selection, got {unexpected:?}"),
    };
    assert_eq!(selected.revision, 1);
    assert_eq!(selected.progress, "started");
    assert!(
        !selected.purpose.is_empty(),
        "the premise echo travels back"
    );
    // Nothing durable moved and nothing executes: same revision, no
    // reservation, and the conversation projection answers from the owner.
    let after = handle
        .store
        .load_task(task.task)
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(after.task.reference, before.task.reference);
    assert!(
        !handle
            .task_executions
            .task_has_reservation_or_running(task.task)
    );
}

#[tokio::test]
async fn resume_wire_maps_to_the_owner_gate_with_epoch_replay() {
    let (handle, _dir) = open_handle("present-resume").await;
    let live_a = live_input(DEVICE_A);
    let _fresh = attach(&handle, DEVICE_A).await;
    handle.install_task_launcher(Arc::new(NoopLauncher {
        launches: AtomicUsize::new(0),
    }));
    let task = seed_task(&handle).await;
    let record = handle
        .store
        .load_task(task.task)
        .await
        .expect("read")
        .expect("exists");
    // List first for the connection-scoped task ref.
    let list_frame = frame_for(
        WirePayload::ListTasks(ListTasks {
            cursor: None,
            limit: None,
        }),
        &live_a,
        None,
        None,
        None,
    );
    let query = match &list_frame.payload {
        WirePayload::ListTasks(query) => query.clone(),
        _ => unreachable!(),
    };
    let frames = handle.list_tasks_wire(&list_frame, &live_a, &query).await;
    let page = match frames.into_iter().next().unwrap().payload {
        WirePayload::TaskListResponse(TaskListResponse::Page(page)) => page,
        unexpected => panic!("expected a page, got {unexpected:?}"),
    };
    let wire_task = page.tasks[0].task.clone();
    let purpose = page.tasks[0].purpose.clone();
    let send = |live: LiveInput, command: CommandWireId, instruction: &str| {
        let payload = WirePayload::ResumeTask(ResumeTask {
            task: wire_task.clone(),
            expected_revision: record.task.reference.revision.as_u64(),
            expected_purpose: purpose.clone(),
            instruction: instruction.to_string(),
        });
        let frame = frame_for(payload, &live, None, None, Some(command));
        let command_dto = match &frame.payload {
            WirePayload::ResumeTask(command_dto) => command_dto.clone(),
            _ => unreachable!(),
        };
        let handle = &handle;
        async move { handle.resume_task_wire(&frame, &live, &command_dto).await }
    };
    // First send resumes r+1 exactly once.
    let first_id = CommandWireId(uuid::Uuid::new_v4());
    let frames = send(live_a.clone(), first_id, "continue the remaining work").await;
    let outcome = match frames.into_iter().next().unwrap().payload {
        WirePayload::ResumeTaskOutcome(outcome) => outcome,
        unexpected => panic!("expected a resume outcome, got {unexpected:?}"),
    };
    assert!(
        matches!(outcome, ResumeTaskOutcomeWire::Resumed { revision: 2, .. }),
        "got {outcome:?}"
    );
    // Same epoch + id + fingerprint replays the original outcome with no
    // second commit.
    let frames = send(live_a.clone(), first_id, "continue the remaining work").await;
    let replay = match frames.into_iter().next().unwrap().payload {
        WirePayload::ResumeTaskOutcome(outcome) => outcome,
        unexpected => panic!("expected a resume outcome, got {unexpected:?}"),
    };
    assert_eq!(replay, outcome, "retries replay instead of recommitting");
    let current = handle
        .store
        .load_task(task.task)
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(
        current.task.reference.revision.as_u64(),
        2,
        "exactly one resume committed"
    );
    // Same id with different content conflicts without side effects.
    let frames = send(live_a.clone(), first_id, "a different instruction").await;
    assert!(
        matches!(
            frames.into_iter().next().unwrap().payload,
            WirePayload::Reject(_)
        ),
        "content conflicts refuse"
    );
    // The old command never auto-resends into a new epoch.
    let live_b = live_input(DEVICE_A);
    let frames = send(live_b.clone(), first_id, "continue the remaining work").await;
    let stale = match frames.into_iter().next().unwrap().payload {
        WirePayload::ResumeTaskOutcome(outcome) => outcome,
        unexpected => panic!("expected a resume outcome, got {unexpected:?}"),
    };
    assert!(
        matches!(stale, ResumeTaskOutcomeWire::StaleConnection),
        "got {stale:?}"
    );
    // A new command on the new epoch sees the committed r+1 as stale premise.
    // (Refs are connection-scoped, so the new connection re-lists first.)
    let list_frame = frame_for(
        WirePayload::ListTasks(ListTasks {
            cursor: None,
            limit: None,
        }),
        &live_b,
        None,
        None,
        None,
    );
    let query = match &list_frame.payload {
        WirePayload::ListTasks(query) => query.clone(),
        _ => unreachable!(),
    };
    let frames = handle.list_tasks_wire(&list_frame, &live_b, &query).await;
    let page_b = match frames.into_iter().next().unwrap().payload {
        WirePayload::TaskListResponse(TaskListResponse::Page(page)) => page,
        unexpected => panic!("expected a page, got {unexpected:?}"),
    };
    let wire_task_b = page_b.tasks[0].task.clone();
    let send_b = |live: LiveInput, command: CommandWireId, revision: u64, instruction: &str| {
        let payload = WirePayload::ResumeTask(ResumeTask {
            task: wire_task_b.clone(),
            expected_revision: revision,
            expected_purpose: purpose.clone(),
            instruction: instruction.to_string(),
        });
        let frame = frame_for(payload, &live, None, None, Some(command));
        let command_dto = match &frame.payload {
            WirePayload::ResumeTask(command_dto) => command_dto.clone(),
            _ => unreachable!(),
        };
        let handle = &handle;
        async move { handle.resume_task_wire(&frame, &live, &command_dto).await }
    };
    let frames = send_b(
        live_b.clone(),
        CommandWireId(uuid::Uuid::new_v4()),
        1,
        "continue again",
    )
    .await;
    let refused = match frames.into_iter().next().unwrap().payload {
        WirePayload::ResumeTaskOutcome(outcome) => outcome,
        unexpected => panic!("expected a resume outcome, got {unexpected:?}"),
    };
    assert!(
        matches!(
            refused,
            ResumeTaskOutcomeWire::StalePremise {
                current_revision: 2
            }
        ),
        "the owner compare is the restart safety, got {refused:?}"
    );
    // Empty instructions never reach the owner.
    let frames = send_b(live_b, CommandWireId(uuid::Uuid::new_v4()), 2, "   ").await;
    let held = match frames.into_iter().next().unwrap().payload {
        WirePayload::ResumeTaskOutcome(outcome) => outcome,
        unexpected => panic!("expected a resume outcome, got {unexpected:?}"),
    };
    assert!(
        matches!(
            held,
            ResumeTaskOutcomeWire::NeedsRevalidation { ref hold } if hold == "instruction_unavailable"
        ),
        "got {held:?}"
    );
}

// --- Stage 5 connection replacement lifecycle (CCT §10.4) ----------------
//
// Every regression below is deterministic: a barrier/gate or an explicit
// synchronization point orders the replacement against the operation, and no
// test sleeps for a race to occur.

/// Seeds one Task with a caller-known workspace association, returning both,
/// so delegation and attempt premises can name the association.
async fn seed_task_with_assoc(handle: &HostHandle) -> (TaskRef, WorkspaceAssocId) {
    let companion = companion_of(handle).await;
    let assoc = WorkspaceAssocId::generate();
    let created = handle
        .store
        .create_task(TaskCreationPremise {
            task: TaskId::generate(),
            purpose: TaskPurpose {
                text: String::from("read the input and write the report"),
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
            workspace: Some(WorkspaceAssociationPremise {
                assoc,
                need: WorkspaceNeedRef {
                    folder: WorkspaceFolderRef {
                        path: String::from("/tmp/ene-test-workspace"),
                    },
                    save_target: None,
                },
            }),
        })
        .await
        .expect("task creation commits");
    (created, assoc)
}

/// Delegation, attempt, and result notifications resolve to their owning Task
/// at read time, so the presented summary attaches the Task report for every
/// fact kind — never a fabricated identity that misses.
#[tokio::test]
async fn task_fact_notifications_attach_their_task_report() {
    let (handle, _dir) = open_handle("present-report-attach").await;
    let live_a = live_input(DEVICE_A);
    let _fresh = attach(&handle, DEVICE_A).await;
    let (task, assoc) = seed_task_with_assoc(&handle).await;
    // Present the creation revision first, so the next page carries only the
    // delegation, attempt, and result notifications.
    let first = summary_of(fetch(&handle, &live_a, None, None, false).await);
    assert!(!first.items.is_empty(), "the revision must display");
    let applied = ack(
        &handle,
        &live_a,
        &first.receipt.0,
        first.round.clone(),
        first.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(applied, UndeliveredAckOutcome::Presented { .. }),
        "got {applied:?}"
    );

    let delegation = DelegationId::generate();
    let outcome = handle
        .store
        .create_delegation(DelegationCreationPremise {
            delegation,
            task,
            agent: TaskAgentEphemeralId::generate(),
            scope_copy: DelegationScope {
                workspace: Some(DelegatedWorkspace {
                    assoc,
                    folder: WorkspaceFolderRef {
                        path: String::from("/tmp/ene-test-workspace"),
                    },
                    save_target: None,
                }),
            },
        })
        .await
        .expect("the delegation must commit");
    assert!(
        matches!(outcome, DelegationOutcome::Delegated(_)),
        "got {outcome:?}"
    );
    let attempt = ActionAttemptId::generate();
    let started = handle
        .store
        .insert_attempt_if_current(AttemptCommitPremise {
            attempt,
            delegation: delegation.as_raw(),
            task: task.task.as_raw(),
            task_revision: RevisionInner::from_u64(task.revision.as_u64()),
            workspace: assoc.as_raw(),
            real_target: RealTargetRef::from_canonical_path(String::from(
                "/tmp/ene-test-workspace/input.txt",
            )),
            operation: OperationKind::Create,
            relied_evaluation: RawId::new(),
        })
        .await
        .expect("the attempt must start");
    assert_eq!(started, ActionStartOutcome::Started);
    let settled = handle
        .store
        .compare_and_set_certainty(
            attempt,
            ActionCertainty::Unknown,
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget,
        )
        .await
        .expect("the certainty CAS must answer");
    assert_eq!(settled, CertaintyUpdateOutcome::Updated);
    let arrival = crate::test_support::record_result(&handle.store, delegation, "done").await;
    let acceptance = handle
        .store
        .adopt_result(TaskResultAdoptionClaim {
            result: arrival.result,
            attempt_refs: vec![attempt.as_raw()],
        })
        .await
        .expect("adoption must answer");
    assert!(
        matches!(acceptance, TaskResultAcceptance::AdoptedAsCompletion(_)),
        "got {acceptance:?}"
    );

    // Five notifications past the acknowledged revision: the delegation, the
    // attempt at two certainties, and the recorded plus adopted result. Every
    // one resolves to the same owning Task, so exactly one headline attaches.
    let next = summary_of(fetch(&handle, &live_a, None, None, false).await);
    assert_eq!(
        next.items.len(),
        5,
        "delegation plus two attempt phases plus recorded plus adopted, got {}",
        next.items.len()
    );
    assert_eq!(
        next.reports.len(),
        1,
        "every fact kind attaches the same Task report, got {:?}",
        next.reports.len()
    );
    assert_eq!(next.reports[0].revision, 1);
    assert_eq!(next.reports[0].progress, "completed");
}

// ---------------------------------------------------------------------------
// Stage 6 A4: Targeted Deletion boundary gating at the presentation boundary.
// ---------------------------------------------------------------------------

async fn admit_condition_only(handle: &HostHandle, text: &str) {
    use ene_preservation::{
        DeletionPurpose, DeletionSearchMaterial, MechanicalDeletionTarget, ParticipantOwnerRef,
        PreservationRepository as _, StartTargetedDeletionCommand, StartTargetedDeletionOutcome,
        TargetedDeletionTarget,
    };
    let command = StartTargetedDeletionCommand::new(
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                text.to_owned(),
            )),
            semantic_hints: Vec::new(),
        },
        DeletionPurpose::Privacy,
        WallClockWithTz::now(),
        Vec::new(),
        vec![ParticipantOwnerRef::Companion],
    )
    .confirmed_for_tests();
    match handle
        .store
        .start_targeted_deletion(command)
        .await
        .expect("admission must commit")
    {
        StartTargetedDeletionOutcome::Started(_) => {}
        other => panic!("unexpected admission outcome: {other:?}"),
    }
}

#[tokio::test]
async fn a4_a_covered_item_is_neither_started_nor_acked() {
    let (handle, _dir) = open_handle("present-a4-ack").await;
    let _ = attach(&handle, DEVICE_A).await;
    let live = live_input(DEVICE_A);
    let generation = attribution_of(&handle).await.generation;
    append_reply(&handle, "the target body", generation).await;

    // The condition is durable but no participant is driven: the fence is
    // untouched, so the receipt created below is current and the store's
    // canonical gate is the only refusal.
    admit_condition_only(&handle, "the target body").await;

    // Presentation start: the covered row is never claimed (its status stays
    // Pending) and its excerpt is withheld by both the read-time coverage
    // check and the in-transaction start gate.
    let summary = summary_of(fetch(&handle, &live, None, None, false).await);
    assert_eq!(summary.items.len(), 1);
    assert_eq!(
        summary.items[0].excerpt, "",
        "a covered body is never carried into a receipt"
    );
    let statuses = unpresented_statuses(&handle).await;
    assert_eq!(statuses.len(), 1);
    assert_eq!(
        statuses[0].1,
        ReportStatus::Pending,
        "the covered row stays Pending, never PresentationUnknown or Presented"
    );

    // ACK: the held start is not a confirmation; the row still never reaches
    // Presented.
    let acked = ack(
        &handle,
        &live,
        &summary.receipt.0,
        summary.round.clone(),
        summary.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        !matches!(acked, UndeliveredAckOutcome::Presented { .. }),
        "a covered row is never confirmed presented, got {acked:?}"
    );
    let statuses = unpresented_statuses(&handle).await;
    assert_eq!(statuses[0].1, ReportStatus::Pending);
}
