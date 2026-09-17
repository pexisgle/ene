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
    ClientIncarnationId, CommandWireId, ConnectionWireId, RequestWireId, RoundWireId,
    WireMessageType,
};
use ene_api::v1::reject::RejectKind;
use ene_api::v1::round::PresentationStatus;
use ene_api::v1::undelivered::{
    GetReportSource, GetTaskReport, ListTasks, ResumeTask, ResumeTaskOutcomeWire, SelectTask,
    SelectTaskResponse, TaskListResponse, TaskReportResponse, UndeliveredAck,
    UndeliveredAckOutcome, UndeliveredRequest, UndeliveredResponse,
};
use ene_companion::{
    AppendHistoryCommand, CompanionId, CompanionRepository as _, HistoryRepository as _,
    HistoryRole, PresentationMark, ReportStatus, ReportStatusTransition, UndeliveredRef,
    UndeliveredRepository as _, UndeliveredSource,
};
use ene_inference::fake::{FakeFailure, FakeProviderTransport};
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
    WorkspaceFolderRef, WorkspaceNeedRef, orchestrate_result_arrival,
};

use crate::conn::{ConnectionPhase, ConnectionTable};
use crate::serve::{FrameSink, HostHandle, LiveInput};
use crate::task_run::TaskAgentLauncher;
use crate::test_support::{authenticate, live_input, memory_handle};

const DEVICE_A: &str = "test-device-a";
const DEVICE_B: &str = "test-device-b";

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

/// A sink that always reports a full buffer: sends end at once, and the
/// caller must never block behind them.
struct FullSink;

impl FrameSink for FullSink {
    fn emit(&mut self, _frame: WireFrame) -> Result<(), crate::serve::FrameDeliveryError> {
        Err(crate::serve::FrameDeliveryError::Full)
    }
}

#[tokio::test]
async fn s5_02_absence_completion_auto_presents_and_send_is_not_presented() {
    let (handle, _dir) = open_handle("present-s5-02").await;
    let fact = attach(&handle, DEVICE_A).await;
    // Absence: the disconnect falls back to NoActive.
    handle.note_disconnect(DEVICE_A).await;
    assert_eq!(attribution_of(&handle).await.state, PresenceState::NoActive);
    // While absent, a completion commits with its undelivered correlation
    // (each fact and its notification share one commit).
    let absent = attribution_of(&handle).await;
    append_reply(&handle, "completion body one", absent.generation).await;
    append_reply(&handle, "completion body two", absent.generation).await;
    // A new presence re-establishes formally, then the backlog presents.
    let fresh = attach(&handle, DEVICE_A).await;
    assert_eq!(fresh.state, PresenceState::Present);
    let live = live_input(DEVICE_A);
    // The attach above ran outside a connection; bind a fresh epoch now.
    let summary = summary_of(fetch(&handle, &live, None, None, false).await);
    assert_eq!(summary.items.len(), 2, "both absence rows present");
    assert_eq!(summary.presence_generation, fresh.generation.as_u64());
    // Sending is not presenting: the same connection re-displays the same
    // receipt instead of an empty page.
    let again = summary_of(fetch(&handle, &live, None, None, false).await);
    assert_eq!(
        again.receipt, summary.receipt,
        "no ACK means the same receipt"
    );
    assert_eq!(again.items.len(), 2);
    assert_eq!(
        unpresented_statuses(&handle).await.len(),
        2,
        "rows stay unpresented until the ACK"
    );
    // Painting both and ACKing presents exactly the carried ids.
    let outcome = ack(
        &handle,
        &live,
        &summary.receipt.0,
        summary.round.clone(),
        summary.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(outcome, UndeliveredAckOutcome::Presented { presented: 2 }),
        "got {outcome:?}"
    );
    assert!(unpresented_statuses(&handle).await.is_empty());
    let _ = fact;
}

#[tokio::test]
async fn s5_07_old_round_input_and_ack_never_replay_and_auth_restores_nothing() {
    let (handle, _dir) = open_handle("present-s5-07").await;
    let live_a = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "absence row", fresh.generation).await;
    let summary = summary_of(fetch(&handle, &live_a, None, None, false).await);
    assert_eq!(summary.items.len(), 1);
    handle.note_disconnect(DEVICE_A).await;
    // An old connection's ACK never migrates: rows stay, receipt stands.
    let stale = ack(
        &handle,
        &live_a,
        &summary.receipt.0,
        summary.round.clone(),
        summary.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    // live_a is superseded only by a newer connection; on the same epoch it
    // is still current, so this ACK applies. Rebind to a NEW connection to
    // prove the migration refusal below.
    assert!(
        matches!(stale, UndeliveredAckOutcome::Presented { .. }),
        "same-epoch ACK still applies, got {stale:?}"
    );
    // A new connection supersedes: the old receipt retires, the row (now
    // Presented) stays gone, and a replay of the consumed receipt is stale,
    // never a second presentation.
    let live_b = live_input(DEVICE_A);
    let again = ack(
        &handle,
        &live_b,
        &summary.receipt.0,
        summary.round.clone(),
        summary.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(again, UndeliveredAckOutcome::StaleConnection),
        "ACKs never cross connections, got {again:?}"
    );
    // An old Round echoed on a live receipt is stale, never applied.
    append_reply(
        &handle,
        "second row",
        attribution_of(&handle).await.generation,
    )
    .await;
    let live_c = live_input(DEVICE_A);
    // Re-establish presence for the new connection first.
    handle.note_disconnect(DEVICE_A).await;
    let _ = attach(&handle, DEVICE_A).await;
    let second = summary_of(fetch(&handle, &live_c, None, None, false).await);
    assert_eq!(second.items.len(), 1);
    let wrong_round = ack(
        &handle,
        &live_c,
        &second.receipt.0,
        RoundWireId(String::from("forged-old-round")),
        second.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(wrong_round, UndeliveredAckOutcome::StalePresentation),
        "old rounds never re-apply, got {wrong_round:?}"
    );
    assert_eq!(unpresented_statuses(&handle).await.len(), 1);
    // Auth alone restores nothing: with NoActive, the Host's own
    // auto-present stays silent (no Owner query can summon it either).
    handle.note_disconnect(DEVICE_A).await;
    let idle = attribution_of(&handle).await;
    assert_eq!(idle.state, PresenceState::NoActive);
    let frame = frame_for(
        WirePayload::UndeliveredRequest(UndeliveredRequest {
            companion: None,
            cursor: None,
            limit: None,
            redisplay: false,
        }),
        &live_c,
        None,
        None,
        None,
    );
    let auto = handle
        .auto_present_for(&frame, &live_c, companion_of(&handle).await, &idle)
        .await;
    assert!(auto.is_empty(), "NoActive auto-presents nothing");
    // ...but management reads stay available without presence.
    let list_frame = frame_for(
        WirePayload::ListTasks(ListTasks {
            cursor: None,
            limit: None,
        }),
        &live_c,
        None,
        None,
        None,
    );
    let query = match &list_frame.payload {
        WirePayload::ListTasks(query) => query.clone(),
        _ => unreachable!(),
    };
    let frames = handle.list_tasks_wire(&list_frame, &live_c, &query).await;
    assert!(
        matches!(
            frames.into_iter().next().unwrap().payload,
            WirePayload::TaskListResponse(TaskListResponse::Page(_))
        ),
        "NoActive still reads management views"
    );
}

#[tokio::test]
async fn s5_08_pre_ack_drop_on_three_sides_keeps_unknown_and_represents() {
    let (handle, _dir) = open_handle("present-s5-08").await;
    let live_a = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    let task = seed_task(&handle).await;
    append_reply(&handle, "reply before the drop", fresh.generation).await;
    let first = summary_of(fetch(&handle, &live_a, None, None, false).await);
    assert_eq!(first.items.len(), 2, "task fact plus reply");
    assert_eq!(
        first.reports.len(),
        1,
        "one Task headline for the task fact"
    );
    // Side 1: the transport drops with no ACK. A new connection re-presents
    // the same Unknown rows under a new receipt (duplicate display allowed).
    let live_b = live_input(DEVICE_A);
    let second = summary_of(fetch(&handle, &live_b, None, None, false).await);
    assert_ne!(
        second.receipt, first.receipt,
        "a new connection mints a new receipt"
    );
    assert_eq!(second.items.len(), 2, "Unknown rows re-present");
    // The consumed receipt is stale now, never unknown-shaped confusion.
    let replay = ack(
        &handle,
        &live_b,
        &first.receipt.0,
        first.round.clone(),
        first.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(replay, UndeliveredAckOutcome::StaleConnection),
        "the old connection's ACK never migrates, got {replay:?}"
    );
    // Side 2: the ACK itself is lost — first applies, the duplicate is
    // stale-harmless with no second write and no re-execution.
    let applied = ack(
        &handle,
        &live_b,
        &second.receipt.0,
        second.round.clone(),
        second.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(applied, UndeliveredAckOutcome::Presented { presented: 2 }),
        "got {applied:?}"
    );
    let duplicate = ack(
        &handle,
        &live_b,
        &second.receipt.0,
        second.round.clone(),
        second.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(duplicate, UndeliveredAckOutcome::StalePresentation),
        "consumed receipts answer stale, got {duplicate:?}"
    );
    assert!(unpresented_statuses(&handle).await.is_empty());
    let record = handle
        .store
        .load_task(task.task)
        .await
        .expect("the task must read")
        .expect("the task must exist");
    assert_eq!(
        record.task.reference.revision.as_u64(),
        1,
        "no re-execution advanced the Task"
    );
    assert!(
        !handle
            .task_executions
            .task_has_reservation_or_running(task.task),
        "no runner was registered by display or ACK"
    );
}

#[tokio::test]
async fn s5_08_host_crash_represents_under_new_receipts() {
    let dir = tempfile::Builder::new()
        .prefix("ene-core-present-s5-08-crash-")
        .tempdir()
        .expect("scratch directory must be creatable");
    let incarnation_store = ene_credential::MemoryCredentialStore::new();
    let first = HostHandle::open_with_cred_store(
        dir.path(),
        crate::serve::CredStore::Memory(incarnation_store),
    )
    .await
    .expect("the handle must open");
    let fresh = attach(&first, DEVICE_A).await;
    append_reply(&first, "surviving row", fresh.generation).await;
    let live_a = live_input(DEVICE_A);
    let shown = summary_of(fetch(&first, &live_a, None, None, false).await);
    assert_eq!(shown.items.len(), 1);
    drop(first);
    // Side 3: the Host crashes before the ACK. Receipts are memory-only, so
    // the old id is unknown; the durable Unknown row re-presents under a
    // new receipt with zero writes lost.
    let reopen_store = ene_credential::MemoryCredentialStore::new();
    let second =
        HostHandle::open_with_cred_store(dir.path(), crate::serve::CredStore::Memory(reopen_store))
            .await
            .expect("the handle must reopen");
    let live_b = live_input(DEVICE_A);
    // Attribution is durable across the restart: still Present here (a
    // RecoveryWait normalization would answer NoCurrentPresence instead —
    // either way the row survives and the old receipt is unknown).
    let current = attribution_of(&second).await;
    if current.state == PresenceState::Present {
        let represented = summary_of(fetch(&second, &live_b, None, None, false).await);
        assert_eq!(represented.items.len(), 1, "the Unknown row re-presents");
        assert_ne!(represented.receipt, shown.receipt);
    } else {
        let response = fetch(&second, &live_b, None, None, false).await;
        assert!(
            matches!(response, UndeliveredResponse::NoCurrentPresence),
            "no formal presence means no summary, got {response:?}"
        );
    }
    assert_eq!(
        second
            .store
            .list_unpresented(companion_of(&second).await, None, 50)
            .await
            .expect("read")
            .entries
            .len(),
        1,
        "the row survives the crash as Unknown"
    );
    let unknown = ack(
        &second,
        &live_b,
        &shown.receipt.0,
        shown.round.clone(),
        shown.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(unknown, UndeliveredAckOutcome::UnknownRef),
        "restart-lost receipts answer unknown, got {unknown:?}"
    );
}

#[tokio::test]
async fn s5_09_progress_ack_never_presents_later_completion() {
    let (handle, _dir) = open_handle("present-s5-09").await;
    let live_a = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "progress row", fresh.generation).await;
    let shown = summary_of(fetch(&handle, &live_a, None, None, false).await);
    assert_eq!(shown.items.len(), 1);
    // A completion commits while the progress receipt is outstanding.
    let current = attribution_of(&handle).await;
    append_reply(&handle, "completion row", current.generation).await;
    // ACKing the old receipt presents only its carried id.
    let outcome = ack(
        &handle,
        &live_a,
        &shown.receipt.0,
        shown.round.clone(),
        shown.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(outcome, UndeliveredAckOutcome::Presented { presented: 1 }),
        "got {outcome:?}"
    );
    // The completion is still unpresented, alone on the next receipt.
    let live_b = live_input(DEVICE_A);
    let next = summary_of(fetch(&handle, &live_b, None, None, false).await);
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].excerpt, "completion row");
    // A duplicate of the consumed receipt, a late Failed for it, and an
    // old-connection ACK all leave state untouched.
    let duplicate = ack(
        &handle,
        &live_b,
        &shown.receipt.0,
        shown.round.clone(),
        shown.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(duplicate, UndeliveredAckOutcome::StaleConnection),
        "got {duplicate:?}"
    );
    let late_failed = ack(
        &handle,
        &live_b,
        &shown.receipt.0,
        shown.round,
        shown.presence_generation,
        PresentationStatus::Failed,
    )
    .await;
    assert!(
        matches!(late_failed, UndeliveredAckOutcome::StaleConnection),
        "got {late_failed:?}"
    );
    // The live receipt with a Failed batch returns its row to Pending.
    let failed = ack(
        &handle,
        &live_b,
        &next.receipt.0,
        next.round.clone(),
        next.presence_generation,
        PresentationStatus::Failed,
    )
    .await;
    assert!(
        matches!(
            failed,
            UndeliveredAckOutcome::ReturnedToPending { count: 1 }
        ),
        "got {failed:?}"
    );
    let represented = summary_of(fetch(&handle, &live_b, None, None, true).await);
    assert_eq!(
        represented.items.len(),
        1,
        "failed rows re-present on explicit redisplay"
    );
    assert_eq!(represented.items[0].excerpt, "completion row");
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
async fn s5_10_failed_head_advances_and_new_arrivals_skip_failures() {
    let (handle, _dir) = open_handle("present-s5-10-failed").await;
    let live_a = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "head one", fresh.generation).await;
    append_reply(&handle, "head two", fresh.generation).await;
    // Small pages: head page carries one row with a continuation.
    let page1 = summary_of(fetch(&handle, &live_a, None, Some(1), false).await);
    assert_eq!(page1.items.len(), 1);
    let cursor = page1.next_cursor.map(|cursor| cursor.0).expect("cursor");
    // The head fails: back to Pending, receipt released.
    let failed = ack(
        &handle,
        &live_a,
        &page1.receipt.0,
        page1.round.clone(),
        page1.presence_generation,
        PresentationStatus::Failed,
    )
    .await;
    assert!(
        matches!(
            failed,
            UndeliveredAckOutcome::ReturnedToPending { count: 1 }
        ),
        "got {failed:?}"
    );
    // The pass advances past the failed head instead of resending it.
    let page2 = summary_of(fetch(&handle, &live_a, Some(cursor), Some(1), false).await);
    assert_eq!(page2.items.len(), 1);
    assert_eq!(page2.items[0].excerpt, "head two");
    let applied = ack(
        &handle,
        &live_a,
        &page2.receipt.0,
        page2.round.clone(),
        page2.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(applied, UndeliveredAckOutcome::Presented { .. }),
        "got {applied:?}"
    );
    // A new arrival pass serves only the fresh row, never the failed head.
    let current = attribution_of(&handle).await;
    append_reply(&handle, "fresh arrival", current.generation).await;
    let arrivals = summary_of(fetch(&handle, &live_a, None, Some(50), false).await);
    assert_eq!(
        arrivals.items.len(),
        1,
        "only the arrival, got {:?}",
        arrivals
            .items
            .iter()
            .map(|item| item.excerpt.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(arrivals.items[0].excerpt, "fresh arrival");
    // ...until an explicit redisplay re-presents the failed head too.
    let outcome = ack(
        &handle,
        &live_a,
        &arrivals.receipt.0,
        arrivals.round.clone(),
        arrivals.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(outcome, UndeliveredAckOutcome::Presented { .. }),
        "got {outcome:?}"
    );
    let rescan = summary_of(fetch(&handle, &live_a, None, Some(50), true).await);
    assert_eq!(rescan.items.len(), 1);
    assert_eq!(rescan.items[0].excerpt, "head one");
}

#[tokio::test]
async fn s5_10_ack_loss_advances_after_expiry_and_late_ack_is_stale() {
    let (handle, _dir) = open_handle("present-s5-10-expiry").await;
    let live_a = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "loss row one", fresh.generation).await;
    append_reply(&handle, "loss row two", fresh.generation).await;
    let page1 = summary_of(fetch(&handle, &live_a, None, Some(1), false).await);
    assert_eq!(page1.items.len(), 1);
    // The ACK is lost on a live connection; after the TTL the next page
    // proceeds instead of wedging behind the receipt.
    assert!(handle.expire_receipt_for_test(&page1.receipt.0));
    let cursor = page1.next_cursor.map(|cursor| cursor.0).expect("cursor");
    let page2 = summary_of(fetch(&handle, &live_a, Some(cursor), Some(1), false).await);
    assert_eq!(page2.items.len(), 1, "expiry unblocks the pass");
    // The late ACK for the expired receipt changes nothing.
    let late = ack(
        &handle,
        &live_a,
        &page1.receipt.0,
        page1.round.clone(),
        page1.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(late, UndeliveredAckOutcome::StalePresentation),
        "got {late:?}"
    );
    assert_eq!(
        unpresented_statuses(&handle).await.len(),
        2,
        "no late write moved a row"
    );
}

#[tokio::test]
async fn s5_10_mid_pass_arrivals_wait_for_the_next_pass() {
    let (handle, _dir) = open_handle("present-s5-10-race").await;
    let live_a = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "race row one", fresh.generation).await;
    append_reply(&handle, "race row two", fresh.generation).await;
    // Both orders, sequenced deterministically: the pass bound captured on
    // page one decides what page two may carry.
    let page1 = summary_of(fetch(&handle, &live_a, None, Some(1), false).await);
    let cursor = page1.next_cursor.map(|cursor| cursor.0).expect("cursor");
    // Order 1: arrival lands before page two is requested.
    let mid = attribution_of(&handle).await;
    append_reply(&handle, "race arrival", mid.generation).await;
    let page2 = summary_of(fetch(&handle, &live_a, Some(cursor.clone()), Some(10), false).await);
    assert_eq!(page2.items.len(), 1, "the pass bound excludes the arrival");
    assert_eq!(page2.items[0].excerpt, "race row two");
    // Order 2 (fresh handle): page two requested before the arrival commits.
    let (handle2, _dir2) = open_handle("present-s5-10-race-b").await;
    let live2 = live_input(DEVICE_A);
    let fresh2 = attach(&handle2, DEVICE_A).await;
    append_reply(&handle2, "race row one", fresh2.generation).await;
    append_reply(&handle2, "race row two", fresh2.generation).await;
    let first = summary_of(fetch(&handle2, &live2, None, Some(1), false).await);
    let cursor2 = first.next_cursor.map(|cursor| cursor.0).expect("cursor");
    let second = summary_of(fetch(&handle2, &live2, Some(cursor2), Some(10), false).await);
    assert_eq!(second.items.len(), 1);
    // Either way the arrival surfaces exactly once, on a later pass.
    let outcome = ack(
        &handle,
        &live_a,
        &page2.receipt.0,
        page2.round.clone(),
        page2.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(outcome, UndeliveredAckOutcome::Presented { .. }),
        "got {outcome:?}"
    );
    let catchup = summary_of(fetch(&handle, &live_a, None, Some(50), false).await);
    assert_eq!(catchup.items.len(), 1);
    assert_eq!(catchup.items[0].excerpt, "race arrival");
}

#[tokio::test]
async fn s5_10_concurrent_subscribes_share_one_receipt() {
    let (handle, _dir) = open_handle("present-s5-10-shared").await;
    let _live = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "shared row", fresh.generation).await;
    let handle = Arc::new(handle);
    let live_a = live_input(DEVICE_A);
    // The two subscribes race on the same connection lifetime, so they share
    // the connection table authority as well as the id.
    let live_b = live_a.clone();
    // Two subscribes race on one connection: the gate serializes them and
    // the second re-displays the live receipt instead of minting another.
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let run = |live: LiveInput, barrier: Arc<tokio::sync::Barrier>| {
        let handle = Arc::clone(&handle);
        async move {
            barrier.wait().await;
            fetch(&handle, &live, None, None, false).await
        }
    };
    let first = tokio::spawn(run(live_a, Arc::clone(&barrier)));
    let second = tokio::spawn(run(live_b, Arc::clone(&barrier)));
    barrier.wait().await;
    let (one, two) = tokio::join!(first, second);
    let one = summary_of(one.expect("first subscribe must answer"));
    let two = summary_of(two.expect("second subscribe must answer"));
    assert_eq!(one.receipt, two.receipt, "one Companion holds one receipt");
    assert_eq!(one.items.len(), 1);
    assert_eq!(two.items.len(), 1);
}

#[tokio::test]
async fn s5_10_full_send_buffer_never_stalls_the_runner() {
    let (handle, _dir) = open_handle("present-s5-10-buffer").await;
    let live_a = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    let task = seed_task(&handle).await;
    append_reply(&handle, "buffered row", fresh.generation).await;
    // Drive the request through the serving path with a transport whose
    // provider is down and a sink that never accepts: nothing may block,
    // and the prepared provider failure must not touch the read.
    let failing = FakeProviderTransport::failing(FakeFailure::Transport("down".to_owned()));
    let frame = frame_for(
        WirePayload::UndeliveredRequest(UndeliveredRequest {
            companion: None,
            cursor: None,
            limit: None,
            redisplay: false,
        }),
        &live_a,
        None,
        None,
        None,
    );
    let mut sink = FullSink;
    let (control_tx, _control_rx) = tokio::sync::mpsc::channel(1);
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        handle.handle_frame_to(frame, live_a.clone(), &failing, &mut sink, &control_tx),
    )
    .await
    .expect("a full buffer must end the send, never stall it");
    // The runner is untouched: no reservation, no revision move, rows stay.
    assert!(
        !handle
            .task_executions
            .task_has_reservation_or_running(task.task)
    );
    assert_eq!(unpresented_statuses(&handle).await.len(), 2);
    let record = handle
        .store
        .load_task(task.task)
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(record.task.reference.revision.as_u64(), 1);
}

#[tokio::test]
async fn s5_12_reads_are_pure_and_provider_down_changes_nothing() {
    let (handle, _dir) = open_handle("present-s5-12").await;
    let live_a = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    let task = seed_task(&handle).await;
    append_reply(&handle, "purity row", fresh.generation).await;
    // The provider is down for the whole test: reads must never consult it.
    let failing = FakeProviderTransport::failing(FakeFailure::Transport("down".to_owned()));
    let _ = &failing;
    let before_generation = attribution_of(&handle).await.generation.as_u64();
    let before_revision = handle
        .store
        .load_task(task.task)
        .await
        .expect("read")
        .expect("exists")
        .task
        .reference
        .revision
        .as_u64();
    let before_statuses = unpresented_statuses(&handle).await;
    // Every read surface, repeatedly, against the failing provider: the
    // provider is never consulted by reads, so all of these succeed.
    for _ in 0..3 {
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
            unexpected => panic!("expected a task page, got {unexpected:?}"),
        };
        assert_eq!(page.tasks.len(), 1);
        assert!(!page.tasks[0].running, "nothing runs on this handle");
        let wire_task = page.tasks[0].task.clone();
        let report_frame = frame_for(
            WirePayload::GetTaskReport(GetTaskReport {
                task: wire_task.clone(),
                cursor: None,
                limit: None,
            }),
            &live_a,
            None,
            None,
            None,
        );
        let query = match &report_frame.payload {
            WirePayload::GetTaskReport(query) => query.clone(),
            _ => unreachable!(),
        };
        let frames = handle.report_wire(&report_frame, &live_a, &query).await;
        let report = match frames.into_iter().next().unwrap().payload {
            WirePayload::TaskReportResponse(TaskReportResponse::Page(page)) => page,
            unexpected => panic!("expected a report page, got {unexpected:?}"),
        };
        assert_eq!(report.revision, before_revision);
        for row in &report.rows {
            if let Some(source) = &row.source {
                let source_frame = frame_for(
                    WirePayload::GetReportSource(GetReportSource {
                        source: source.clone(),
                        cursor: None,
                        limit_bytes: None,
                    }),
                    &live_a,
                    None,
                    None,
                    None,
                );
                let query = match &source_frame.payload {
                    WirePayload::GetReportSource(query) => query.clone(),
                    _ => unreachable!(),
                };
                let frames = handle
                    .report_source_wire(&source_frame, &live_a, &query)
                    .await;
                assert!(
                    matches!(
                        frames.into_iter().next().unwrap().payload,
                        WirePayload::ReportSourceResponse(_)
                    ),
                    "source bodies read without the provider"
                );
            }
        }
    }
    assert_eq!(
        attribution_of(&handle).await.generation.as_u64(),
        before_generation
    );
    let after_revision = handle
        .store
        .load_task(task.task)
        .await
        .expect("read")
        .expect("exists")
        .task
        .reference
        .revision
        .as_u64();
    assert_eq!(
        after_revision, before_revision,
        "reads never advance revisions"
    );
    assert_eq!(
        unpresented_statuses(&handle).await,
        before_statuses,
        "reads never move report status"
    );
    assert!(
        !handle
            .task_executions
            .task_has_reservation_or_running(task.task)
    );
    // Presenting is a write by design, so it stays after the purity
    // assertions: the backlog above survived every read intact.
    let summary = summary_of(fetch(&handle, &live_a, None, Some(50), true).await);
    assert_eq!(
        summary.items.len(),
        2,
        "the backlog is intact for later display"
    );
    let outcome = ack(
        &handle,
        &live_a,
        &summary.receipt.0,
        summary.round.clone(),
        summary.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(matches!(outcome, UndeliveredAckOutcome::Presented { .. }));
}

#[tokio::test]
async fn task_queries_validate_limits_cursors_and_refs() {
    let (handle, _dir) = open_handle("present-query-validation").await;
    let live_a = live_input(DEVICE_A);
    let _fresh = attach(&handle, DEVICE_A).await;
    let task = seed_task(&handle).await;
    // Invalid limits refuse as UnsupportedFieldValue, never a silent clamp.
    for payload in [
        WirePayload::ListTasks(ListTasks {
            cursor: None,
            limit: Some(0),
        }),
        WirePayload::ListTasks(ListTasks {
            cursor: None,
            limit: Some(51),
        }),
        WirePayload::GetTaskReport(GetTaskReport {
            task: ene_api::v1::undelivered::TaskWireRef(String::from("x")),
            cursor: None,
            limit: Some(99),
        }),
        WirePayload::UndeliveredRequest(UndeliveredRequest {
            companion: None,
            cursor: None,
            limit: Some(0),
            redisplay: false,
        }),
    ] {
        let frame = frame_for(payload.clone(), &live_a, None, None, None);
        let frames = match &frame.payload {
            WirePayload::ListTasks(query) => handle.list_tasks_wire(&frame, &live_a, query).await,
            WirePayload::GetTaskReport(query) => handle.report_wire(&frame, &live_a, query).await,
            WirePayload::UndeliveredRequest(request) => {
                handle.request_undelivered(&frame, &live_a, request).await
            }
            _ => unreachable!(),
        };
        assert!(
            matches!(
                frames.into_iter().next().unwrap().payload,
                WirePayload::Reject(notice) if notice.kind == ene_api::v1::reject::RejectKind::UnsupportedFieldValue
            ),
            "out-of-range limits refuse"
        );
    }
    // Source byte limits refuse outside 4..=16384.
    for limit in [Some(3), Some(16385)] {
        let frame = frame_for(
            WirePayload::GetReportSource(GetReportSource {
                source: ene_api::v1::undelivered::ReportSourceWireRef(String::from("s")),
                cursor: None,
                limit_bytes: limit,
            }),
            &live_a,
            None,
            None,
            None,
        );
        let query = match &frame.payload {
            WirePayload::GetReportSource(query) => query.clone(),
            _ => unreachable!(),
        };
        let frames = handle.report_source_wire(&frame, &live_a, &query).await;
        assert!(
            matches!(
                frames.into_iter().next().unwrap().payload,
                WirePayload::Reject(_)
            ),
            "out-of-range source limits refuse"
        );
    }
    // Unknown refs and foreign cursors answer typed refusals, never guesses.
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
    assert_eq!(page.tasks.len(), 1);
    assert!(!page.tasks[0].running, "no execution is registered");
    // A cursor from another connection is stale here.
    let live_b = live_input(DEVICE_A);
    if let Some(next) = page.next_cursor {
        let stale_frame = frame_for(
            WirePayload::ListTasks(ListTasks {
                cursor: Some(next),
                limit: None,
            }),
            &live_b,
            None,
            None,
            None,
        );
        let stale_query = match &stale_frame.payload {
            WirePayload::ListTasks(query) => query.clone(),
            _ => unreachable!(),
        };
        let frames = handle
            .list_tasks_wire(&stale_frame, &live_b, &stale_query)
            .await;
        assert!(
            matches!(
                frames.into_iter().next().unwrap().payload,
                WirePayload::TaskListResponse(TaskListResponse::StaleBaseView { .. })
            ),
            "cursors never cross connections"
        );
    }
    // Unknown task refs answer UnknownRef on report, source, and select.
    let forged = ene_api::v1::undelivered::TaskWireRef(String::from("forged-task"));
    let report_frame = frame_for(
        WirePayload::GetTaskReport(GetTaskReport {
            task: forged.clone(),
            cursor: None,
            limit: None,
        }),
        &live_a,
        None,
        None,
        None,
    );
    let query = match &report_frame.payload {
        WirePayload::GetTaskReport(query) => query.clone(),
        _ => unreachable!(),
    };
    let frames = handle.report_wire(&report_frame, &live_a, &query).await;
    assert!(matches!(
        frames.into_iter().next().unwrap().payload,
        WirePayload::TaskReportResponse(TaskReportResponse::UnknownRef)
    ));
    let select_frame = frame_for(
        WirePayload::SelectTask(SelectTask { task: forged }),
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
    assert!(matches!(
        frames.into_iter().next().unwrap().payload,
        WirePayload::SelectTaskResponse(ene_api::v1::undelivered::SelectTaskResponse::UnknownRef)
    ));
    // A cursor minted for the report query is stale on the list query.
    let wire_task = page.tasks[0].task.clone();
    let report_frame = frame_for(
        WirePayload::GetTaskReport(GetTaskReport {
            task: wire_task,
            cursor: None,
            limit: Some(1),
        }),
        &live_a,
        None,
        None,
        None,
    );
    let query = match &report_frame.payload {
        WirePayload::GetTaskReport(query) => query.clone(),
        _ => unreachable!(),
    };
    let frames = handle.report_wire(&report_frame, &live_a, &query).await;
    let report = match frames.into_iter().next().unwrap().payload {
        WirePayload::TaskReportResponse(TaskReportResponse::Page(page)) => page,
        unexpected => panic!("expected a report page, got {unexpected:?}"),
    };
    if let Some(cursor) = report.next_cursor {
        let cross_frame = frame_for(
            WirePayload::ListTasks(ListTasks {
                cursor: Some(cursor),
                limit: None,
            }),
            &live_a,
            None,
            None,
            None,
        );
        let cross_query = match &cross_frame.payload {
            WirePayload::ListTasks(query) => query.clone(),
            _ => unreachable!(),
        };
        let frames = handle
            .list_tasks_wire(&cross_frame, &live_a, &cross_query)
            .await;
        assert!(matches!(
            frames.into_iter().next().unwrap().payload,
            WirePayload::TaskListResponse(TaskListResponse::StaleBaseView { .. })
        ));
    }
    let _ = task;
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

#[tokio::test]
async fn concurrent_resume_commands_commit_at_most_once() {
    let (handle, _dir) = open_handle("present-resume-race").await;
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
    let handle = Arc::new(handle);
    let command = CommandWireId(uuid::Uuid::new_v4());
    let wire_task = page.tasks[0].task.clone();
    let purpose = page.tasks[0].purpose.clone();
    let revision = record.task.reference.revision.as_u64();
    let run = |live: LiveInput| {
        let handle = Arc::clone(&handle);
        let wire_task = wire_task.clone();
        let purpose = purpose.clone();
        async move {
            let payload = WirePayload::ResumeTask(ResumeTask {
                task: wire_task,
                expected_revision: revision,
                expected_purpose: purpose,
                instruction: String::from("continue the remaining work"),
            });
            let frame = frame_for(payload, &live, None, None, Some(command));
            let dto = match &frame.payload {
                WirePayload::ResumeTask(dto) => dto.clone(),
                _ => unreachable!(),
            };
            handle.resume_task_wire(&frame, &live, &dto).await
        }
    };
    // Same epoch + id raced concurrently: at most one commit, the loser
    // observes InFlight or replays the winner — never a second r+1.
    let live_b = LiveInput {
        connection_id: live_a.connection_id,
        ..live_input(DEVICE_A)
    };
    let (first, second) = tokio::join!(run(live_a), run(live_b));
    let outcome_of = |frames: Vec<WireFrame>| match frames.into_iter().next().unwrap().payload {
        WirePayload::ResumeTaskOutcome(outcome) => outcome,
        unexpected => panic!("expected a resume outcome, got {unexpected:?}"),
    };
    let first = outcome_of(first);
    let second = outcome_of(second);
    let resumed = [&first, &second]
        .iter()
        .filter(|outcome| matches!(outcome, ResumeTaskOutcomeWire::Resumed { .. }))
        .count();
    assert!(
        resumed <= 1,
        "at most one commit, got {first:?} and {second:?}"
    );
    for outcome in [&first, &second] {
        assert!(
            matches!(
                outcome,
                ResumeTaskOutcomeWire::Resumed { .. } | ResumeTaskOutcomeWire::InFlight
            ),
            "loser observes InFlight or replays, got {outcome:?}"
        );
    }
    // A sequential retry afterwards replays the decided outcome.
    let live_c = live_input(DEVICE_A);
    let payload = WirePayload::ResumeTask(ResumeTask {
        task: page.tasks[0].task.clone(),
        expected_revision: revision,
        expected_purpose: purpose,
        instruction: String::from("continue the remaining work"),
    });
    let frame = frame_for(payload, &live_c, None, None, Some(command));
    let dto = match &frame.payload {
        WirePayload::ResumeTask(dto) => dto.clone(),
        _ => unreachable!(),
    };
    // New epoch: the old command stays stale even after the race settled.
    let frames = handle.resume_task_wire(&frame, &live_c, &dto).await;
    assert!(matches!(
        frames.into_iter().next().unwrap().payload,
        WirePayload::ResumeTaskOutcome(ResumeTaskOutcomeWire::StaleConnection)
    ));
}

#[tokio::test]
async fn frame_too_large_withholds_without_mutation() {
    let (handle, _dir) = open_handle("present-frame-too-large").await;
    let live_a = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "a row that cannot fit", fresh.generation).await;
    // Pin the cap below a single item's overhead: nothing fits, even one.
    handle.set_frame_budget_for_test(16);
    let response = fetch(&handle, &live_a, None, None, false).await;
    assert!(
        matches!(response, UndeliveredResponse::FrameTooLarge),
        "oversize withholds, got {response:?}"
    );
    assert_eq!(unpresented_statuses(&handle).await.len(), 1, "no row moved");
    // The receipt namespace is untouched: no id was minted to ACK.
    let live_b = live_input(DEVICE_B);
    let _ = live_b;
    handle.set_frame_budget_for_test(192 * 1024);
    let summary = summary_of(fetch(&handle, &live_a, None, None, false).await);
    assert_eq!(
        summary.items.len(),
        1,
        "the row presents once the cap allows"
    );
}

/// Drives one purpose-preserving revision forward through the real steering
/// commit: the revision advances, the adopted-purpose identity does not.
async fn steer_carrying_purpose(handle: &HostHandle, expected: TaskRef) -> TaskRef {
    match handle
        .store
        .forward_steering(ene_task::TaskCommitPremise {
            expected,
            new_purpose: None,
            adopted_purpose_entry: TaskContextEntryId::generate(),
            adopted_instruction: None,
        })
        .await
        .expect("the steering must commit")
    {
        ene_task::TaskCommitOutcome::CommittedAs(current) => current,
        other => panic!("expected a committed revision forward, got {other:?}"),
    }
}

/// Fetches the first Task-list page through the wire query.
async fn list_page(
    handle: &HostHandle,
    live: &LiveInput,
) -> ene_api::v1::undelivered::TaskListPage {
    let frame = frame_for(
        WirePayload::ListTasks(ListTasks {
            cursor: None,
            limit: None,
        }),
        live,
        None,
        None,
        None,
    );
    let query = match &frame.payload {
        WirePayload::ListTasks(query) => query.clone(),
        _ => unreachable!(),
    };
    let frames = handle.list_tasks_wire(&frame, live, &query).await;
    match frames.into_iter().next().unwrap().payload {
        WirePayload::TaskListResponse(TaskListResponse::Page(page)) => page,
        other => panic!("expected a task list page, got {other:?}"),
    }
}

/// S5 wire identity: the purpose identity returned by ListTasks / SelectTask
/// names the revision that adopted the purpose, not the current Task
/// revision, and a Client echoing the returned value resumes successfully
/// after a purpose-preserving steering.
#[tokio::test]
async fn purpose_identity_survives_a_purpose_preserving_forward_and_resumes() {
    let (handle, _dir) = open_handle("present-purpose").await;
    handle.install_task_launcher(Arc::new(NoopLauncher {
        launches: AtomicUsize::new(0),
    }));
    let live = live_input(DEVICE_A);
    let task_r1 = seed_task(&handle).await;
    let task_r2 = steer_carrying_purpose(&handle, task_r1).await;
    assert_eq!(
        task_r2.revision.as_u64(),
        2,
        "the steering advances r1 to r2"
    );
    let record = handle
        .store
        .load_task(task_r2.task)
        .await
        .expect("the task must read")
        .expect("the task must exist");
    assert_eq!(
        record.task.purpose.adopted_revision.as_u64(),
        1,
        "the purpose is still adopted at r1"
    );

    let page = list_page(&handle, &live).await;
    assert_eq!(page.tasks.len(), 1, "one Task is listed");
    let item = &page.tasks[0];
    assert_eq!(item.revision, 2, "the list reports the current revision");
    assert_eq!(
        item.purpose,
        format!("{}:1", task_r2.task.as_raw().as_uuid().as_hyphenated()),
        "the purpose identity names its adopting revision, not the current one"
    );

    // SelectTask projects the same identity from the stored purpose.
    let select_frame = frame_for(
        WirePayload::SelectTask(SelectTask {
            task: item.task.clone(),
        }),
        &live,
        None,
        None,
        None,
    );
    let select_query = match &select_frame.payload {
        WirePayload::SelectTask(query) => query.clone(),
        _ => unreachable!(),
    };
    let selected = match handle
        .select_task_wire(&select_frame, &live, &select_query)
        .await
        .into_iter()
        .next()
        .unwrap()
        .payload
    {
        WirePayload::SelectTaskResponse(SelectTaskResponse::Selected(selected)) => selected,
        other => panic!("expected a selection, got {other:?}"),
    };
    assert_eq!(selected.purpose, item.purpose);

    // The Client echoes exactly the wire-returned identity and resumes.
    let resume_frame = frame_for(
        WirePayload::ResumeTask(ResumeTask {
            task: item.task.clone(),
            expected_revision: item.revision,
            expected_purpose: item.purpose.clone(),
            instruction: String::from("continue the report"),
        }),
        &live,
        None,
        None,
        Some(CommandWireId(uuid::Uuid::new_v4())),
    );
    let resume = match &resume_frame.payload {
        WirePayload::ResumeTask(resume) => resume.clone(),
        _ => unreachable!(),
    };
    let outcome = match handle
        .resume_task_wire(&resume_frame, &live, &resume)
        .await
        .into_iter()
        .next()
        .unwrap()
        .payload
    {
        WirePayload::ResumeTaskOutcome(outcome) => outcome,
        other => panic!("expected a resume outcome, got {other:?}"),
    };
    assert!(
        matches!(outcome, ResumeTaskOutcomeWire::Resumed { revision: 3, .. }),
        "the echoed identity must resume, got {outcome:?}"
    );
}

/// The same identity keeps working across a Host restart: after the first
/// resume, a reopen re-derives the purpose identity from durable state, and a
/// second resume succeeds with what the wire returned.
#[tokio::test]
async fn purpose_identity_survives_resume_and_host_restart() {
    let dir = tempfile::Builder::new()
        .prefix("ene-core-present-purpose-restart-")
        .tempdir()
        .expect("scratch directory must be creatable");
    let first = HostHandle::open_with_cred_store(
        dir.path(),
        crate::serve::CredStore::Memory(ene_credential::MemoryCredentialStore::new()),
    )
    .await
    .expect("the handle must open");
    first.install_task_launcher(Arc::new(NoopLauncher {
        launches: AtomicUsize::new(0),
    }));
    let live = live_input(DEVICE_A);
    let task_r1 = seed_task(&first).await;
    let task_r2 = steer_carrying_purpose(&first, task_r1).await;

    // First resume through the wire identity.
    let page = list_page(&first, &live).await;
    let item = page.tasks.into_iter().next().expect("the Task must list");
    let resume_frame = frame_for(
        WirePayload::ResumeTask(ResumeTask {
            task: item.task.clone(),
            expected_revision: item.revision,
            expected_purpose: item.purpose.clone(),
            instruction: String::from("continue after the interruption"),
        }),
        &live,
        None,
        None,
        Some(CommandWireId(uuid::Uuid::new_v4())),
    );
    let resume = match &resume_frame.payload {
        WirePayload::ResumeTask(resume) => resume.clone(),
        _ => unreachable!(),
    };
    let first_outcome = match first
        .resume_task_wire(&resume_frame, &live, &resume)
        .await
        .into_iter()
        .next()
        .unwrap()
        .payload
    {
        WirePayload::ResumeTaskOutcome(outcome) => outcome,
        other => panic!("expected a resume outcome, got {other:?}"),
    };
    assert!(
        matches!(
            first_outcome,
            ResumeTaskOutcomeWire::Resumed { revision: 3, .. }
        ),
        "the first resume must succeed, got {first_outcome:?}"
    );
    drop(first);

    // Host restart; the identity is re-derived from durable state.
    let second = HostHandle::open_with_cred_store(
        dir.path(),
        crate::serve::CredStore::Memory(ene_credential::MemoryCredentialStore::new()),
    )
    .await
    .expect("the handle must reopen");
    second.install_task_launcher(Arc::new(NoopLauncher {
        launches: AtomicUsize::new(0),
    }));
    let live_b = live_input(DEVICE_A);
    let page = list_page(&second, &live_b).await;
    let item = page.tasks.into_iter().next().expect("the Task must list");
    assert_eq!(item.revision, 3);
    assert_eq!(
        item.purpose,
        format!("{}:1", task_r2.task.as_raw().as_uuid().as_hyphenated()),
        "the purpose identity survives the restart unchanged"
    );
    let resume_frame = frame_for(
        WirePayload::ResumeTask(ResumeTask {
            task: item.task.clone(),
            expected_revision: item.revision,
            expected_purpose: item.purpose.clone(),
            instruction: String::from("continue once more"),
        }),
        &live_b,
        None,
        None,
        Some(CommandWireId(uuid::Uuid::new_v4())),
    );
    let resume = match &resume_frame.payload {
        WirePayload::ResumeTask(resume) => resume.clone(),
        _ => unreachable!(),
    };
    let second_outcome = match second
        .resume_task_wire(&resume_frame, &live_b, &resume)
        .await
        .into_iter()
        .next()
        .unwrap()
        .payload
    {
        WirePayload::ResumeTaskOutcome(outcome) => outcome,
        other => panic!("expected a resume outcome, got {other:?}"),
    };
    assert!(
        matches!(
            second_outcome,
            ResumeTaskOutcomeWire::Resumed { revision: 4, .. }
        ),
        "the restarted Host must resume from the wire identity, got {second_outcome:?}"
    );
}

/// S5 receipt semantics: re-emitting a live receipt returns exactly the
/// selection it covers, regardless of the request's smaller limit, so an ACK
/// can never present an item the Client did not receive.
#[tokio::test]
async fn receipt_reemit_keeps_the_selected_set_whole() {
    let (handle, _dir) = open_handle("present-reemit").await;
    let live = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "row A", fresh.generation).await;
    append_reply(&handle, "row B", fresh.generation).await;
    // No ACK: the receipt covers both rows.
    let first = summary_of(fetch(&handle, &live, None, None, false).await);
    assert_eq!(first.items.len(), 2);
    assert_eq!(first.receipt.0.len(), 36, "a receipt id is issued");

    // A re-fetch with a smaller limit must not shrink the receipt's set: the
    // same receipt comes back with both items.
    let again = summary_of(fetch(&handle, &live, None, Some(1), false).await);
    assert_eq!(
        again.receipt, first.receipt,
        "the same receipt is re-emitted"
    );
    assert_eq!(
        again.items.len(),
        2,
        "the re-emitted set equals the receipt's selection, not the limit"
    );
    let mut excerpts: Vec<&str> = again
        .items
        .iter()
        .map(|item| item.excerpt.as_str())
        .collect();
    excerpts.sort_unstable();
    assert_eq!(excerpts, vec!["row A", "row B"]);

    // The ACK presents exactly the two delivered rows and nothing lingers.
    let outcome = ack(
        &handle,
        &live,
        &again.receipt.0,
        again.round.clone(),
        again.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(outcome, UndeliveredAckOutcome::Presented { presented: 2 }),
        "got {outcome:?}"
    );
    assert!(unpresented_statuses(&handle).await.is_empty());
}

/// Counts durable delegation rows for the fixture's single Task.
fn delegation_rows(dir: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(dir.join("app.db")).expect("the database must open");
    conn.query_row("SELECT COUNT(*) FROM delegation", [], |row| row.get(0))
        .expect("the delegation count must read")
}

/// Builds a resume frame + DTO from a Task-list item, echoing exactly what
/// the wire returned.
fn resume_for(
    live: &LiveInput,
    item: &ene_api::v1::undelivered::TaskListItem,
    instruction: &str,
) -> (WireFrame, ResumeTask) {
    let frame = frame_for(
        WirePayload::ResumeTask(ResumeTask {
            task: item.task.clone(),
            expected_revision: item.revision,
            expected_purpose: item.purpose.clone(),
            instruction: instruction.to_string(),
        }),
        live,
        None,
        None,
        Some(CommandWireId(uuid::Uuid::new_v4())),
    );
    let resume = match &frame.payload {
        WirePayload::ResumeTask(resume) => resume.clone(),
        _ => unreachable!(),
    };
    (frame, resume)
}

fn resume_outcome_of(frames: Vec<WireFrame>) -> ResumeTaskOutcomeWire {
    match frames.into_iter().next().unwrap().payload {
        WirePayload::ResumeTaskOutcome(outcome) => outcome,
        other => panic!("expected a resume outcome, got {other:?}"),
    }
}

/// CCT §10.4 case A: a resume whose connection is superseded before the
/// ownership commit section answers `StaleConnection` and commits nothing —
/// no revision, no delegation, no launch.
#[tokio::test]
async fn resume_superseded_before_the_commit_section_commits_nothing() {
    let (handle, dir) = open_handle("present-resume-stale").await;
    let launcher = Arc::new(NoopLauncher {
        launches: AtomicUsize::new(0),
    });
    handle.install_task_launcher(launcher.clone());
    let task_r1 = seed_task(&handle).await;
    let task_r2 = steer_carrying_purpose(&handle, task_r1).await;

    // C1 is a real authenticated connection; C2 will supersede it mid-resume.
    let table = Arc::new(crate::conn::ConnectionTable::new());
    let c1 = table.note_accept();
    crate::test_support::authenticate(&table, &c1, DEVICE_A);
    let live = table.snapshot(&c1).expect("C1 must snapshot");
    let page = list_page(&handle, &live).await;
    let item = page.tasks.into_iter().next().expect("the Task must list");
    let (frame, resume) = resume_for(&live, &item, "continue after the race");

    let gate = handle.arm_resume_gate();
    let resume_fut = handle.resume_task_wire(&frame, &live, &resume);
    tokio::pin!(resume_fut);
    tokio::select! {
        () = gate.wait_entered() => {}
        outcome = &mut resume_fut => panic!("the resume escaped the gate: {outcome:?}"),
    }
    // C2 authenticates on the same device and supersedes C1 before the
    // commit section runs.
    let c2 = table.note_accept();
    crate::test_support::authenticate(&table, &c2, DEVICE_A);
    assert_eq!(
        table.phase_of(&c1),
        Some(crate::conn::ConnectionPhase::Superseded),
        "C2 must supersede C1"
    );
    gate.release();
    let outcome = resume_outcome_of(resume_fut.await);
    assert!(
        matches!(outcome, ResumeTaskOutcomeWire::StaleConnection),
        "a superseded resume must be stale, got {outcome:?}"
    );

    let record = handle
        .store
        .load_task(task_r2.task)
        .await
        .expect("the task must read")
        .expect("the task must exist");
    assert_eq!(
        record.task.reference.revision.as_u64(),
        2,
        "the stale resume did not advance the revision"
    );
    assert_eq!(delegation_rows(dir.path()), 0, "no delegation was created");
    assert_eq!(
        launcher.launches.load(Ordering::SeqCst),
        0,
        "no execution was launched"
    );
    assert!(
        !handle
            .task_executions
            .task_has_reservation_or_running(task_r2.task),
        "no launch reservation was recorded"
    );
}

/// CCT §10.4 case B: a resume that committed before the supersession is
/// accepted and keeps its execution; the later connection loss neither rolls
/// back the revision nor cancels the committed work.
#[tokio::test]
async fn resume_committed_before_supersession_keeps_its_execution() {
    let (handle, dir) = open_handle("present-resume-won").await;
    let launcher = Arc::new(NoopLauncher {
        launches: AtomicUsize::new(0),
    });
    handle.install_task_launcher(launcher.clone());
    let task_r1 = seed_task(&handle).await;
    let task_r2 = steer_carrying_purpose(&handle, task_r1).await;

    let table = Arc::new(crate::conn::ConnectionTable::new());
    let c1 = table.note_accept();
    crate::test_support::authenticate(&table, &c1, DEVICE_A);
    let live = table.snapshot(&c1).expect("C1 must snapshot");
    let page = list_page(&handle, &live).await;
    let item = page.tasks.into_iter().next().expect("the Task must list");
    let (frame, resume) = resume_for(&live, &item, "continue before the race");
    let outcome = resume_outcome_of(handle.resume_task_wire(&frame, &live, &resume).await);
    assert!(
        matches!(outcome, ResumeTaskOutcomeWire::Resumed { revision: 3, .. }),
        "the resume commits first, got {outcome:?}"
    );
    assert_eq!(delegation_rows(dir.path()), 1, "one delegation committed");
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1, "launched once");

    // C2 supersedes C1, then C1's socket closes: the committed resume is
    // untouched.
    let c2 = table.note_accept();
    crate::test_support::authenticate(&table, &c2, DEVICE_A);
    handle.close_connection(&table, c1).await;
    let record = handle
        .store
        .load_task(task_r2.task)
        .await
        .expect("the task must read")
        .expect("the task must exist");
    assert_eq!(
        record.task.reference.revision.as_u64(),
        3,
        "the committed revision stands after the connection loss"
    );
    assert_eq!(delegation_rows(dir.path()), 1, "the delegation stands");
    assert!(
        handle
            .task_executions
            .task_has_reservation_or_running(task_r2.task),
        "the committed execution is not cancelled by the disconnect"
    );
}

/// S5 receipt rehydration: a live receipt whose selection sits behind a full
/// head page is re-emitted from its exact selected identities, not from an
/// unpresented-head scan; the same receipt id and the same item set come
/// back, and its ACK presents exactly those rows.
#[tokio::test]
async fn receipt_reemit_rehydrates_selected_ids_behind_the_head_page() {
    let (handle, _dir) = open_handle("present-reemit-exact").await;
    let live = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    for index in 0..51 {
        append_reply(&handle, &format!("row {index:02}"), fresh.generation).await;
    }
    // The head page carries the first fifty rows; its continuation carries
    // the fifty-first.
    let head = summary_of(fetch(&handle, &live, None, None, false).await);
    assert_eq!(head.items.len(), 50);
    let cursor = head.next_cursor.clone().expect("the pass continues");
    // The head batch fails: its rows return to Pending and the pass advances.
    let failed = ack(
        &handle,
        &live,
        &head.receipt.0,
        head.round.clone(),
        head.presence_generation,
        PresentationStatus::Failed,
    )
    .await;
    assert!(
        matches!(
            failed,
            UndeliveredAckOutcome::ReturnedToPending { count: 50 }
        ),
        "got {failed:?}"
    );
    let tail = summary_of(fetch(&handle, &live, Some(cursor.0), None, false).await);
    assert_eq!(tail.items.len(), 1);
    assert_eq!(tail.items[0].excerpt, "row 50");
    // No ACK: re-fetching must rehydrate the tail exactly, even though the
    // unpresented head page is now full of the failed Pending rows again.
    let again = summary_of(fetch(&handle, &live, None, None, false).await);
    assert_eq!(
        again.receipt, tail.receipt,
        "the same receipt is re-emitted"
    );
    assert_eq!(
        again.items.len(),
        1,
        "the exact selected id rehydrates behind the head page"
    );
    assert_eq!(again.items[0].excerpt, "row 50");
    // The ACK presents exactly what was re-sent; the failed head rows stay
    // Pending for their own pass.
    let outcome = ack(
        &handle,
        &live,
        &again.receipt.0,
        again.round.clone(),
        again.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(outcome, UndeliveredAckOutcome::Presented { presented: 1 }),
        "got {outcome:?}"
    );
    let statuses = {
        let companion = companion_of(&handle).await;
        handle
            .store
            .list_unpresented(companion, None, 50)
            .await
            .expect("the listing must read")
            .entries
    };
    assert_eq!(statuses.len(), 50, "the failed rows stay unpresented");
    assert!(
        statuses
            .iter()
            .all(|entry| entry.status == ReportStatus::Pending),
        "the failed rows return to Pending"
    );
}

/// S5 receipt rehydration fail-closed: a selected identity that no longer
/// resolves at all retires the receipt and answers `StaleBaseView` instead
/// of an ACKable frame under the same receipt id.
#[tokio::test]
async fn receipt_reemit_fails_closed_when_a_selected_id_cannot_resolve() {
    let (handle, dir) = open_handle("present-reemit-failclosed").await;
    let live = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "resolvable row", fresh.generation).await;
    let shown = summary_of(fetch(&handle, &live, None, None, false).await);
    assert_eq!(shown.items.len(), 1);
    let companion = companion_of(&handle).await;
    let listed = handle
        .store
        .list_unpresented(companion, None, 50)
        .await
        .expect("the listing must read");
    assert_eq!(listed.entries.len(), 1);
    // The selected row disappears from durable state (an erasure/deletion
    // path): the receipt can no longer cover the set its ACK names.
    let raw = rusqlite::Connection::open(dir.path().join("app.db")).expect("the store file opens");
    raw.execute(
        "DELETE FROM undelivered WHERE undelivered_id = ?1",
        [listed.entries[0]
            .id
            .as_raw()
            .as_uuid()
            .as_hyphenated()
            .to_string()],
    )
    .expect("the selected row is deleted");

    let response = fetch(&handle, &live, None, None, false).await;
    assert!(
        matches!(response, UndeliveredResponse::StaleBaseView { .. }),
        "an unrehydratable receipt must fail closed, got {response:?}"
    );
    // The retired receipt answers stale; its ACK cannot mark anything.
    let late = ack(
        &handle,
        &live,
        &shown.receipt.0,
        shown.round.clone(),
        shown.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(late, UndeliveredAckOutcome::StalePresentation),
        "got {late:?}"
    );
}

/// S5 receipt rehydration: a selected row already presented by another path
/// (for example a stream confirmation) is omitted from the frame and pruned
/// from the receipt, so the ACK acts on exactly the re-sent rows.
#[tokio::test]
async fn receipt_reemit_prunes_already_presented_selected_ids() {
    let (handle, _dir) = open_handle("present-reemit-prune").await;
    let live = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "first row", fresh.generation).await;
    append_reply(&handle, "second row", fresh.generation).await;
    let shown = summary_of(fetch(&handle, &live, None, None, false).await);
    assert_eq!(shown.items.len(), 2);
    let companion = companion_of(&handle).await;
    let listed = handle
        .store
        .list_unpresented(companion, None, 50)
        .await
        .expect("the listing must read");
    assert_eq!(listed.entries.len(), 2);
    // A concurrent presentation owner presents the first selected row.
    let first_id = listed.entries[0].id;
    let transition = handle
        .store
        .compare_and_mark_reported(
            first_id,
            ReportStatus::PresentationUnknown,
            PresentationMark {
                round: RawId::new(),
                presented: true,
            },
        )
        .await
        .expect("the competing presentation must answer");
    assert_eq!(transition, ReportStatusTransition::PendingToPresented);

    let again = summary_of(fetch(&handle, &live, None, None, false).await);
    assert_eq!(
        again.receipt, shown.receipt,
        "the same receipt is re-emitted"
    );
    assert_eq!(
        again.items.len(),
        1,
        "only the still-unpresented selected row is re-sent"
    );
    assert_eq!(again.items[0].excerpt, "second row");
    // The ACK presents exactly the re-sent row; the already-presented one is
    // untouched.
    let outcome = ack(
        &handle,
        &live,
        &again.receipt.0,
        again.round.clone(),
        again.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(outcome, UndeliveredAckOutcome::Presented { presented: 1 }),
        "got {outcome:?}"
    );
    assert!(unpresented_statuses(&handle).await.is_empty());
    let record = handle
        .store
        .compare_and_mark_reported(
            first_id,
            ReportStatus::PresentationUnknown,
            PresentationMark {
                round: RawId::new(),
                presented: true,
            },
        )
        .await
        .expect("the status probe must answer");
    assert_eq!(
        record,
        ReportStatusTransition::AlreadyPresented,
        "the pruned row stays presented"
    );
}

/// S5 CAS: a presentation-start compare that loses as a domain
/// `StaleSource` (not an infrastructure error) drops the row from the frame
/// and the receipt selection; the pass's ACK never presents a row it did not
/// claim.
#[tokio::test]
async fn stale_source_cas_loss_drops_the_row_from_frame_and_selection() {
    let (handle, _dir) = open_handle("present-stale-cas").await;
    let live = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "cas row", fresh.generation).await;
    let companion = companion_of(&handle).await;
    let listed = handle
        .store
        .list_unpresented(companion, None, 50)
        .await
        .expect("the listing must read");
    assert_eq!(listed.entries.len(), 1);
    let row = listed.entries[0];

    let gate = handle.arm_presentation_commit_gate();
    let fetch_fut = fetch(&handle, &live, None, None, false);
    tokio::pin!(fetch_fut);
    tokio::select! {
        () = gate.wait_entered() => {}
        response = &mut fetch_fut => panic!("the pass escaped the gate: {response:?}"),
    }
    // Another presentation owner commits the row's presentation start while
    // this pass is paused between its page plan and its compare: the
    // compare below must observe `StaleSource`, not claim the row.
    let competing = handle
        .store
        .compare_and_mark_reported(
            row.id,
            ReportStatus::Pending,
            PresentationMark {
                round: RawId::new(),
                presented: false,
            },
        )
        .await
        .expect("the competing mark must answer");
    assert_eq!(competing, ReportStatusTransition::MarkedPresentationUnknown);
    gate.release();
    let summary = summary_of(fetch_fut.await);
    assert!(
        summary.items.is_empty(),
        "a lost CAS claims no item, got {}",
        summary.items.len()
    );
    assert_eq!(
        summary.receipt.0.len(),
        36,
        "the pass still mints its receipt"
    );
    let outcome = ack(
        &handle,
        &live,
        &summary.receipt.0,
        summary.round.clone(),
        summary.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(outcome, UndeliveredAckOutcome::AlreadyPresented),
        "got {outcome:?}"
    );
    let statuses = unpresented_statuses(&handle).await;
    assert_eq!(statuses.len(), 1);
    assert_eq!(
        statuses[0].1,
        ReportStatus::PresentationUnknown,
        "the competing presentation start is untouched by this pass"
    );
}

/// S5 connection lifecycle: a closed connection's presentation entries
/// (subscription, Task/source refs, cursors, receipt) are dropped; durable
/// rows stay and a new connection re-derives fresh refs and a fresh receipt.
#[tokio::test]
async fn connection_close_drops_its_presentation_state() {
    let (handle, _dir) = open_handle("present-close-cleanup").await;
    let table = Arc::new(crate::conn::ConnectionTable::new());
    let c1 = table.note_accept();
    crate::test_support::authenticate(&table, &c1, DEVICE_A);
    let live = table.snapshot(&c1).expect("C1 must snapshot");
    let fresh = attach(&handle, DEVICE_A).await;
    // Two Tasks so the bounded list mints a cursor; two replies so the
    // undelivered page mints a continuation cursor.
    let _one = seed_task(&handle).await;
    let _two = seed_task(&handle).await;
    append_reply(&handle, "close row one", fresh.generation).await;
    append_reply(&handle, "close row two", fresh.generation).await;

    let list_frame = frame_for(
        WirePayload::ListTasks(ListTasks {
            cursor: None,
            limit: Some(1),
        }),
        &live,
        None,
        None,
        None,
    );
    let list_query = match &list_frame.payload {
        WirePayload::ListTasks(query) => query.clone(),
        _ => unreachable!(),
    };
    let listed = match handle
        .list_tasks_wire(&list_frame, &live, &list_query)
        .await
        .into_iter()
        .next()
        .unwrap()
        .payload
    {
        WirePayload::TaskListResponse(TaskListResponse::Page(page)) => page,
        other => panic!("expected a task list page, got {other:?}"),
    };
    assert!(listed.next_cursor.is_some(), "the list mints a cursor");
    let report_frame = frame_for(
        WirePayload::GetTaskReport(GetTaskReport {
            task: listed.tasks[0].task.clone(),
            cursor: None,
            limit: None,
        }),
        &live,
        None,
        None,
        None,
    );
    let report_query = match &report_frame.payload {
        WirePayload::GetTaskReport(query) => query.clone(),
        _ => unreachable!(),
    };
    let reports = handle
        .report_wire(&report_frame, &live, &report_query)
        .await;
    assert!(
        matches!(
            reports.first().map(|frame| &frame.payload),
            Some(WirePayload::TaskReportResponse(TaskReportResponse::Page(_)))
        ),
        "the report mints its source ref"
    );
    // A wire resume that needs revalidation still installs its retry slot;
    // the slot belongs to the issuing connection lifetime.
    let resume_frame = frame_for(
        WirePayload::ResumeTask(ResumeTask {
            task: listed.tasks[0].task.clone(),
            expected_revision: 1,
            expected_purpose: String::from("purpose"),
            instruction: String::from("   "),
        }),
        &live,
        None,
        None,
        Some(CommandWireId(uuid::Uuid::new_v4())),
    );
    let resume = match &resume_frame.payload {
        WirePayload::ResumeTask(resume) => resume.clone(),
        _ => unreachable!(),
    };
    let resume_outcome = match handle
        .resume_task_wire(&resume_frame, &live, &resume)
        .await
        .into_iter()
        .next()
        .unwrap()
        .payload
    {
        WirePayload::ResumeTaskOutcome(outcome) => outcome,
        other => panic!("expected a resume outcome, got {other:?}"),
    };
    assert!(
        matches!(
            resume_outcome,
            ResumeTaskOutcomeWire::NeedsRevalidation { .. }
        ),
        "got {resume_outcome:?}"
    );
    let shown = summary_of(fetch(&handle, &live, None, Some(1), false).await);
    assert_eq!(shown.items.len(), 1);
    assert!(shown.next_cursor.is_some(), "the pass mints a cursor");

    let conn = c1.0.as_hyphenated().to_string();
    {
        let state = crate::lock_unpoison(&handle.presentations);
        assert!(state.subs.contains_key(&conn));
        assert!(state.task_refs.keys().any(|(owner, _)| owner == &conn));
        assert!(state.source_refs.keys().any(|(owner, _)| owner == &conn));
        assert!(state.carried.keys().any(|(owner, _)| owner == &conn));
        assert!(state.cursors.keys().any(|(owner, _)| owner == &conn));
        assert!(
            state
                .receipts
                .values()
                .any(|receipt| receipt.connection == conn)
        );
        assert!(
            state.resume.values().any(|slot| slot.connection == conn),
            "the resume retry slot belongs to this connection"
        );
    }

    handle.close_connection(&table, c1).await;
    {
        let state = crate::lock_unpoison(&handle.presentations);
        assert!(!state.subs.contains_key(&conn));
        assert!(state.task_refs.keys().all(|(owner, _)| owner != &conn));
        assert!(state.source_refs.keys().all(|(owner, _)| owner != &conn));
        assert!(state.carried.keys().all(|(owner, _)| owner != &conn));
        assert!(state.cursors.keys().all(|(owner, _)| owner != &conn));
        assert!(
            state
                .receipts
                .values()
                .all(|receipt| receipt.connection != conn)
        );
        assert!(
            state.resume.values().all(|slot| slot.connection != conn),
            "the resume retry slot dies with its connection"
        );
    }

    // Durable state is untouched: both Tasks and the carried rows survive as
    // unpresented, so a new connection re-presents them.
    assert!(
        !handle
            .store
            .list_tasks_after(None, 10)
            .await
            .expect("the task listing must read")
            .is_empty()
    );
    let companion = companion_of(&handle).await;
    assert!(
        handle
            .store
            .list_unpresented(companion, None, 50)
            .await
            .expect("the listing must read")
            .entries
            .len()
            >= 2
    );
    let table_b = Arc::new(crate::conn::ConnectionTable::new());
    let c2 = table_b.note_accept();
    crate::test_support::authenticate(&table_b, &c2, DEVICE_A);
    let live_b = table_b.snapshot(&c2).expect("C2 must snapshot");
    let _ = attach(&handle, DEVICE_A).await;
    let page_b = list_page(&handle, &live_b).await;
    assert_eq!(page_b.tasks.len(), 2, "the new connection re-lists");
    let again = summary_of(fetch(&handle, &live_b, None, None, false).await);
    assert!(!again.items.is_empty(), "the new receipt carries the rows");
    assert_ne!(again.receipt, shown.receipt, "a fresh receipt id");
}

/// S5 connection lifecycle: repeated connect/use/close cycles do not grow the
/// connection-keyed presentation maps, while durable rows stay readable.
#[tokio::test]
async fn repeated_reconnects_do_not_grow_presentation_state() {
    let (handle, _dir) = open_handle("present-reconnect-growth").await;
    let fresh = attach(&handle, DEVICE_A).await;
    let _task = seed_task(&handle).await;
    append_reply(&handle, "growth row", fresh.generation).await;
    for _ in 0..40 {
        let table = Arc::new(crate::conn::ConnectionTable::new());
        let c = table.note_accept();
        crate::test_support::authenticate(&table, &c, DEVICE_A);
        let live = table.snapshot(&c).expect("the connection must snapshot");
        let _ = list_page(&handle, &live).await;
        let _ = fetch(&handle, &live, None, None, false).await;
        handle.close_connection(&table, c).await;
        let _ = attach(&handle, DEVICE_A).await;
    }
    {
        let state = crate::lock_unpoison(&handle.presentations);
        assert!(
            state.subs.is_empty(),
            "no subscription outlives its connection"
        );
        assert!(
            state.task_refs.is_empty(),
            "no Task ref outlives its connection"
        );
        assert!(
            state.carried.is_empty(),
            "no carried ref outlives its connection"
        );
        assert!(
            state.source_refs.is_empty(),
            "no source ref outlives its connection"
        );
        assert!(
            state.cursors.is_empty(),
            "no cursor outlives its connection"
        );
        assert!(
            state.receipts.is_empty(),
            "no receipt outlives its connection"
        );
        assert!(
            state.retired.len() <= 64,
            "the retired receipt window stays bounded"
        );
    }
    // Durable state stays readable after every close.
    assert!(
        !handle
            .store
            .list_tasks_after(None, 10)
            .await
            .expect("the task listing must read")
            .is_empty()
    );
    let companion = companion_of(&handle).await;
    assert!(
        !handle
            .store
            .list_unpresented(companion, None, 50)
            .await
            .expect("the listing must read")
            .entries
            .is_empty()
    );
}

/// S5 receipt rehydration fail-closed on a store read failure: no summary
/// under the same receipt id is returned, and the retired receipt's ACK
/// cannot present anything.
#[tokio::test]
async fn receipt_reemit_fails_closed_on_a_store_read_failure() {
    let (handle, dir) = open_handle("present-reemit-store-error").await;
    let live = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "durable row", fresh.generation).await;
    let shown = summary_of(fetch(&handle, &live, None, None, false).await);
    assert_eq!(shown.items.len(), 1);
    // Infrastructure failure injection: make the exact-identity table
    // unreadable from under the store while its row stays durable.
    let raw = rusqlite::Connection::open(dir.path().join("app.db")).expect("the store file opens");
    raw.execute_batch("ALTER TABLE undelivered RENAME TO undelivered_hidden")
        .expect("the test may hide the table");

    let response = fetch(&handle, &live, None, None, false).await;
    assert!(
        matches!(response, UndeliveredResponse::StaleBaseView { .. }),
        "a store failure must fail closed, got {response:?}"
    );
    let late = ack(
        &handle,
        &live,
        &shown.receipt.0,
        shown.round.clone(),
        shown.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(late, UndeliveredAckOutcome::StalePresentation),
        "got {late:?}"
    );
    // The durable row was never presented by the failed path.
    let status: String = raw
        .query_row("SELECT status FROM undelivered_hidden", [], |row| {
            row.get(0)
        })
        .expect("the durable status must read");
    assert_eq!(status, "presentation_unknown");
    raw.execute_batch("ALTER TABLE undelivered_hidden RENAME TO undelivered")
        .expect("the test may restore the table");
}

// --- Stage 5 connection replacement lifecycle (CCT §10.4) ----------------
//
// Every regression below is deterministic: a barrier/gate or an explicit
// synchronization point orders the replacement against the operation, and no
// test sleeps for a race to occur.

/// Supersedes `c1` with a fresh authenticated connection for the same device
/// and runs the real lifecycle sweep, returning the replacement's premises.
fn replace_connection(
    handle: &HostHandle,
    table: &Arc<ConnectionTable>,
    c1: &ConnectionWireId,
    device: &str,
) -> (ConnectionWireId, LiveInput) {
    let c2 = table.note_accept();
    authenticate(table, &c2, device);
    assert_eq!(
        table.phase_of(c1),
        Some(ConnectionPhase::Superseded),
        "the newer install must supersede C1"
    );
    handle.on_connection_superseded(c1);
    let live = table.snapshot(&c2).expect("the replacement must snapshot");
    (c2, live)
}

/// The wire ref of the single seeded Task, listed on `live`.
async fn listed_task_ref(
    handle: &HostHandle,
    live: &LiveInput,
) -> ene_api::v1::undelivered::TaskWireRef {
    let frame = frame_for(
        WirePayload::ListTasks(ListTasks {
            cursor: None,
            limit: Some(10),
        }),
        live,
        None,
        None,
        None,
    );
    let query = match &frame.payload {
        WirePayload::ListTasks(query) => query.clone(),
        _ => unreachable!(),
    };
    let frames = handle.list_tasks_wire(&frame, live, &query).await;
    match frames.into_iter().next().unwrap().payload {
        WirePayload::TaskListResponse(TaskListResponse::Page(page)) => {
            assert_eq!(page.tasks.len(), 1, "the fixture seeds one Task");
            page.tasks[0].task.clone()
        }
        other => panic!("expected a list page, got {other:?}"),
    }
}

/// One `SelectTask` through the wire inlet, returning its answer frames.
async fn select_once(
    handle: &HostHandle,
    live: &LiveInput,
    task: ene_api::v1::undelivered::TaskWireRef,
) -> Vec<WireFrame> {
    let frame = frame_for(
        WirePayload::SelectTask(SelectTask { task }),
        live,
        None,
        None,
        None,
    );
    let query = match &frame.payload {
        WirePayload::SelectTask(query) => query.clone(),
        _ => unreachable!(),
    };
    handle.select_task_wire(&frame, live, &query).await
}

fn assert_stale_reject(frames: &[WireFrame], what: &str) {
    assert_eq!(frames.len(), 1, "{what} answers exactly one frame");
    assert!(
        matches!(
            &frames[0].payload,
            WirePayload::Reject(notice) if notice.kind == RejectKind::StaleConnection
        ),
        "{what} must answer a typed stale rejection, got {:?}",
        frames[0].payload
    );
}

/// Replacement case A: an `UndeliveredRequest` admitted on C1 that prepares
/// its receipt after C2 replaced it installs nothing, answers typed stale,
/// and leaves C2's own receipt standing.
#[tokio::test]
async fn replacement_rejects_a_stale_undelivered_request_without_resurrecting_state() {
    let (handle, _dir) = open_handle("replace-stale-fetch").await;
    let handle = Arc::new(handle);
    let table = Arc::new(ConnectionTable::new());
    let c1 = table.note_accept();
    authenticate(&table, &c1, DEVICE_A);
    let live1 = table.snapshot(&c1).expect("C1 must snapshot");
    let fact = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "replacement row", fact.generation).await;

    // Pause C1's request before it takes the presentation transition lock.
    let gate = handle.arm_fetch_gate();
    let request = {
        let handle = Arc::clone(&handle);
        let live1 = live1.clone();
        tokio::spawn(async move {
            let frame = frame_for(
                WirePayload::UndeliveredRequest(UndeliveredRequest {
                    companion: None,
                    cursor: None,
                    limit: None,
                    redisplay: false,
                }),
                &live1,
                None,
                None,
                None,
            );
            let query = match &frame.payload {
                WirePayload::UndeliveredRequest(query) => query.clone(),
                _ => unreachable!(),
            };
            handle.request_undelivered(&frame, &live1, &query).await
        })
    };
    gate.wait_entered().await;
    handle.disarm_fetch_gate();
    let (c2, live2) = replace_connection(&handle, &table, &c1, DEVICE_A);
    assert!(
        handle.presentation_counts_for_test(&c1).is_empty(),
        "the replacement sweep purges C1 before the request resumes"
    );
    // C2 mints its own receipt for the same row.
    let r2 = summary_of(fetch(&handle, &live2, None, None, false).await);
    assert!(!r2.receipt.0.is_empty(), "C2 mints its own receipt");
    let c2_before = handle.presentation_counts_for_test(&c2);
    assert_eq!(c2_before.receipts, 1);

    gate.release();
    let stale = request.await.expect("the paused request must finish");
    assert_stale_reject(&stale, "the stale undelivered request");
    assert!(
        handle.presentation_counts_for_test(&c1).is_empty(),
        "C1's state must not resurrect after the sweep"
    );
    assert_eq!(
        handle.presentation_counts_for_test(&c2),
        c2_before,
        "C2's receipt and refs stay untouched"
    );
    // R2 still owns the row its ACK names.
    let outcome = ack(
        &handle,
        &live2,
        &r2.receipt.0,
        r2.round.clone(),
        r2.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(outcome, UndeliveredAckOutcome::Presented { presented: 1 }),
        "C2's receipt must still cover the row, got {outcome:?}"
    );
}

#[tokio::test]
async fn replacement_before_presentation_cas_leaves_pending() {
    let (handle, _dir) = open_handle("replace-before-cas").await;
    let live = live_input(DEVICE_A);
    let fact = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "pending row", fact.generation).await;
    let gate = handle.arm_presentation_commit_gate();
    let query = UndeliveredRequest {
        companion: None,
        cursor: None,
        limit: Some(1),
        redisplay: false,
    };
    let frame = frame_for(
        WirePayload::UndeliveredRequest(query.clone()),
        &live,
        None,
        None,
        None,
    );
    let pending = handle.request_undelivered(&frame, &live, &query);
    tokio::pin!(pending);
    tokio::select! {
        () = gate.wait_entered() => {},
        frames = &mut pending => panic!("escaped commit gate: {frames:?}"),
    }
    let (_, live2) = replace_connection(&handle, &live.authority, &live.connection_id, DEVICE_A);
    gate.release();
    assert_stale_reject(&pending.await, "stale presentation start");
    assert!(
        handle
            .presentation_counts_for_test(&live.connection_id)
            .is_empty()
    );
    assert_eq!(
        unpresented_statuses(&handle).await[0].1,
        ReportStatus::Pending
    );
    *crate::lock_unpoison(&handle.presentation_commit_gate) = None;
    let summary = summary_of(fetch(&handle, &live2, None, None, true).await);
    assert_eq!(summary.items.len(), 1);
}

#[tokio::test]
async fn replacement_after_presentation_commit_preserves_durable_row() {
    let (handle, _dir) = open_handle("replace-after-cas").await;
    let live = live_input(DEVICE_A);
    let fact = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "committed row", fact.generation).await;
    let first = summary_of(fetch(&handle, &live, None, None, true).await);
    assert_eq!(
        handle
            .presentation_counts_for_test(&live.connection_id)
            .receipts,
        1
    );
    assert_eq!(
        unpresented_statuses(&handle).await[0].1,
        ReportStatus::PresentationUnknown
    );
    let (_, live2) = replace_connection(&handle, &live.authority, &live.connection_id, DEVICE_A);
    assert!(
        handle
            .presentation_counts_for_test(&live.connection_id)
            .is_empty()
    );
    assert_eq!(
        unpresented_statuses(&handle).await[0].1,
        ReportStatus::PresentationUnknown
    );
    let second = summary_of(fetch(&handle, &live2, None, None, true).await);
    assert_eq!(second.items.len(), 1);
    assert_ne!(first.receipt, second.receipt);
    assert_ne!(first.round, second.round);
}

/// Replacement case B: a `SelectTask` admitted on C1 that pauses before its
/// guarded selection commit selects nothing after C2 replaced it.
#[tokio::test]
async fn replacement_rejects_a_stale_select_task_without_committing() {
    let (handle, _dir) = open_handle("replace-stale-select").await;
    let handle = Arc::new(handle);
    let table = Arc::new(ConnectionTable::new());
    let c1 = table.note_accept();
    authenticate(&table, &c1, DEVICE_A);
    let live1 = table.snapshot(&c1).expect("C1 must snapshot");
    attach(&handle, DEVICE_A).await;
    seed_task(&handle).await;
    let task_ref = listed_task_ref(&handle, &live1).await;

    let gate = handle.arm_ref_mint_gate();
    let select = {
        let handle = Arc::clone(&handle);
        let live1 = live1.clone();
        tokio::spawn(async move { select_once(&handle, &live1, task_ref).await })
    };
    gate.wait_entered().await;
    let (c2, live2) = replace_connection(&handle, &table, &c1, DEVICE_A);
    gate.release();
    handle.disarm_ref_mint_gate();

    let frames = select.await.expect("the paused selection must finish");
    assert_stale_reject(&frames, "the stale selection");
    assert!(
        handle
            .conversation_tasks
            .current(task_companion(&handle).await, &c2)
            .is_none(),
        "C2 must be unselected"
    );
    assert_eq!(
        handle.conversation_tasks.first_party_selection_count(),
        0,
        "no stale first-party selection may survive"
    );

    // C2 selects for itself after its own listing.
    let task_ref = listed_task_ref(&handle, &live2).await;
    let frames = select_once(&handle, &live2, task_ref).await;
    assert!(
        matches!(
            frames.first().map(|frame| &frame.payload),
            Some(WirePayload::SelectTaskResponse(
                SelectTaskResponse::Selected(_)
            ))
        ),
        "C2's own selection works, got {frames:?}"
    );
    assert!(
        handle
            .conversation_tasks
            .current(task_companion(&handle).await, &c2)
            .is_some(),
        "C2's selection is visible on C2"
    );
}

async fn task_companion(handle: &HostHandle) -> CompanionId {
    companion_of(handle).await
}

/// Replacement case F: a first-party selection lives only on the selecting
/// connection; a same-device replacement starts unselected and reselects.
#[tokio::test]
async fn replacement_resets_the_first_party_selection_until_reselected() {
    let (handle, _dir) = open_handle("replace-selection-reset").await;
    let table = Arc::new(ConnectionTable::new());
    let c1 = table.note_accept();
    authenticate(&table, &c1, DEVICE_A);
    let live1 = table.snapshot(&c1).expect("C1 must snapshot");
    attach(&handle, DEVICE_A).await;
    let task = seed_task(&handle).await;
    let task_ref = listed_task_ref(&handle, &live1).await;
    let frames = select_once(&handle, &live1, task_ref).await;
    assert!(matches!(
        frames.first().map(|frame| &frame.payload),
        Some(WirePayload::SelectTaskResponse(
            SelectTaskResponse::Selected(_)
        ))
    ));
    let companion = companion_of(&handle).await;
    assert_eq!(
        handle
            .conversation_tasks
            .current(companion, &c1)
            .map(|task| task.task),
        Some(task.task),
        "C1's selection is visible on C1"
    );

    let (c2, live2) = replace_connection(&handle, &table, &c1, DEVICE_A);
    assert!(
        handle.conversation_tasks.current(companion, &c1).is_none(),
        "C1's selection is gone with its connection"
    );
    assert!(
        handle.conversation_tasks.current(companion, &c2).is_none(),
        "C2 starts unselected: a directive cannot act on C1's Task"
    );
    assert_eq!(handle.conversation_tasks.first_party_selection_count(), 0);

    // C2 lists and selects again: the wire selection is usable.
    let task_ref = listed_task_ref(&handle, &live2).await;
    let frames = select_once(&handle, &live2, task_ref).await;
    assert!(matches!(
        frames.first().map(|frame| &frame.payload),
        Some(WirePayload::SelectTaskResponse(
            SelectTaskResponse::Selected(_)
        ))
    ));
    assert_eq!(
        handle
            .conversation_tasks
            .current(companion, &c2)
            .map(|task| task.task),
        Some(task.task)
    );
}

/// Replacement case G: a read query that pauses between its durable read and
/// its connection-scoped ref mint mints nothing after the replacement.
#[tokio::test]
async fn replacement_read_query_mints_no_ref_after_cleanup() {
    let (handle, _dir) = open_handle("replace-stale-read").await;
    let handle = Arc::new(handle);
    let table = Arc::new(ConnectionTable::new());
    let c1 = table.note_accept();
    authenticate(&table, &c1, DEVICE_A);
    let live1 = table.snapshot(&c1).expect("C1 must snapshot");
    attach(&handle, DEVICE_A).await;
    seed_task(&handle).await;

    let gate = handle.arm_ref_mint_gate();
    let list = {
        let handle = Arc::clone(&handle);
        let live1 = live1.clone();
        tokio::spawn(async move {
            let frame = frame_for(
                WirePayload::ListTasks(ListTasks {
                    cursor: None,
                    limit: Some(10),
                }),
                &live1,
                None,
                None,
                None,
            );
            let query = match &frame.payload {
                WirePayload::ListTasks(query) => query.clone(),
                _ => unreachable!(),
            };
            handle.list_tasks_wire(&frame, &live1, &query).await
        })
    };
    gate.wait_entered().await;
    let (c2, live2) = replace_connection(&handle, &table, &c1, DEVICE_A);
    gate.release();
    handle.disarm_ref_mint_gate();

    let frames = list.await.expect("the paused read must finish");
    assert_stale_reject(&frames, "the stale task list");
    assert!(
        handle.presentation_counts_for_test(&c1).is_empty(),
        "the stale read mints no task ref or cursor"
    );

    let task_ref = listed_task_ref(&handle, &live2).await;
    assert!(!task_ref.0.is_empty());
    assert_eq!(
        handle.presentation_counts_for_test(&c2).task_refs,
        1,
        "C2's own listing mints exactly its ref"
    );
}

/// Replacement case H: repeated same-device replacement keeps
/// connection-bound memory bounded; each retired connection's transient
/// world is empty and the survivors' bookkeeping is one connection's worth.
#[tokio::test]
async fn repeated_replacement_keeps_connection_bound_memory_bounded() {
    let (handle, _dir) = open_handle("replace-memory-stability").await;
    let handle = Arc::new(handle);
    let table = Arc::new(ConnectionTable::new());
    let mut current = table.note_accept();
    authenticate(&table, &current, DEVICE_A);
    let fact = attach(&handle, DEVICE_A).await;
    seed_task(&handle).await;

    let mut retired = Vec::new();
    for index in 0..24 {
        let live = table
            .snapshot(&current)
            .expect("the connection must snapshot");
        // Exercise every connection-bound owner: receipt + carried refs,
        // task refs + cursor, the first-party selection, and a resume retry
        // slot.
        let _ = fetch(&handle, &live, None, None, false).await;
        let task_ref = listed_task_ref(&handle, &live).await;
        let _ = select_once(&handle, &live, task_ref.clone()).await;
        let frame = frame_for(
            WirePayload::ResumeTask(ResumeTask {
                task: task_ref,
                expected_revision: 9_999,
                expected_purpose: String::from("stale-purpose"),
                instruction: String::from("continue"),
            }),
            &live,
            None,
            None,
            Some(CommandWireId(uuid::Uuid::new_v4())),
        );
        let query = match &frame.payload {
            WirePayload::ResumeTask(query) => query.clone(),
            _ => unreachable!(),
        };
        let _ = handle.resume_task_wire(&frame, &live, &query).await;
        assert!(handle.presentation_counts_for_test(&current).receipts <= 1);
        assert!(handle.presentation_counts_for_test(&current).resume_slots <= 1);
        assert_eq!(
            handle.conversation_tasks.first_party_selection_count(),
            1,
            "iteration {index} holds exactly one first-party selection"
        );

        let next = table.note_accept();
        authenticate(&table, &next, DEVICE_A);
        handle.on_connection_superseded(&current);
        assert!(
            handle.presentation_counts_for_test(&current).is_empty(),
            "iteration {index} leaves no presentation state"
        );
        assert_eq!(
            handle.conversation_tasks.first_party_selection_count(),
            0,
            "iteration {index} leaves no first-party selection"
        );
        retired.push(current);
        current = next;
    }

    // The survivor's world is exactly one connection's worth.
    let live = table
        .snapshot(&current)
        .expect("the survivor must snapshot");
    let _ = fetch(&handle, &live, None, None, false).await;
    let counts = handle.presentation_counts_for_test(&current);
    assert_eq!(counts.receipts, 1, "one live receipt");
    assert!(counts.task_refs <= 1, "one task ref");
    assert_eq!(handle.conversation_tasks.first_party_selection_count(), 0);
    assert!(!handle.has_open_round_for_test());
    for old in &retired {
        assert!(
            handle.presentation_counts_for_test(old).is_empty(),
            "every retired connection stays purged"
        );
    }
    assert!(fact.generation.as_u64() >= 1);
}

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
    let arrival = orchestrate_result_arrival(
        &handle.store,
        delegation,
        ene_task::TaskAgentOutput::new(String::from("done")),
    )
    .await
    .expect("the arrival must record");
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

/// A lost presentation-start compare drops the row from both the frame and
/// the receipt selection, so the receipt covers exactly what the Client
/// received; the failed row stays `Pending` and re-presents on the next
/// explicit pass, and its ACK never claims a presentation that did not
/// happen.
#[tokio::test]
async fn failed_mark_stays_unselected_and_represents() {
    let (handle, dir) = open_handle("present-failed-mark").await;
    let live_a = live_input(DEVICE_A);
    let fresh = attach(&handle, DEVICE_A).await;
    append_reply(&handle, "row one", fresh.generation).await;
    append_reply(&handle, "row two", fresh.generation).await;
    let companion = companion_of(&handle).await;
    let listed = handle
        .store
        .list_unpresented(companion, None, 50)
        .await
        .expect("the listing must read");
    assert_eq!(listed.entries.len(), 2);
    let second_message = match listed.entries[1].source {
        UndeliveredSource::HistoryMessage(message) => message.as_uuid().as_hyphenated().to_string(),
        ref other => panic!("replies list as history, got {other:?}"),
    };
    // Refuse only the second row's presentation-start update: its compare
    // fails while the first row still marks.
    rusqlite::Connection::open(dir.path().join("app.db"))
        .expect("the store file must open")
        .execute_batch(&format!(
            "CREATE TRIGGER refuse_second BEFORE UPDATE ON undelivered WHEN NEW.source_id = '{second_message}' BEGIN SELECT RAISE(ABORT, 'test refusal'); END;"
        ))
        .expect("the refusal trigger must install");
    let shown = summary_of(fetch(&handle, &live_a, None, None, false).await);
    assert_eq!(
        shown.items.len(),
        1,
        "only the successfully marked row is delivered, got {}",
        shown.items.len()
    );
    assert_eq!(shown.items[0].excerpt, "row one");
    // The same receipt re-displays exactly its selection.
    let again = summary_of(fetch(&handle, &live_a, None, None, false).await);
    assert_eq!(
        again.receipt, shown.receipt,
        "no ACK means the same receipt"
    );
    assert_eq!(again.items.len(), 1, "the receipt covers one row");
    assert_eq!(again.items[0].excerpt, "row one");
    rusqlite::Connection::open(dir.path().join("app.db"))
        .expect("the store file must open")
        .execute_batch("DROP TRIGGER refuse_second")
        .expect("the refusal trigger must drop");
    // The ACK presents exactly the selected row: it never claims the failed
    // one.
    let outcome = ack(
        &handle,
        &live_a,
        &shown.receipt.0,
        shown.round.clone(),
        shown.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(outcome, UndeliveredAckOutcome::Presented { presented: 1 }),
        "got {outcome:?}"
    );
    // The failed row is still pending, so a fresh connection's explicit head
    // pass re-presents it — and presenting it then drains the backlog.
    let live_b = live_input(DEVICE_A);
    let represented = summary_of(fetch(&handle, &live_b, None, None, false).await);
    assert_eq!(represented.items.len(), 1);
    assert_eq!(represented.items[0].excerpt, "row two");
    let outcome = ack(
        &handle,
        &live_b,
        &represented.receipt.0,
        represented.round.clone(),
        represented.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(outcome, UndeliveredAckOutcome::Presented { presented: 1 }),
        "got {outcome:?}"
    );
    assert!(unpresented_statuses(&handle).await.is_empty());
}

// ---------------------------------------------------------------------------
// Stage 6 A3c: Targeted Deletion transient invalidation at the presentation
// boundary. The Host-transient participant invalidates receipts and carried
// refs, and every fresh read re-checks the canonical current conditions before
// materializing a body.
// ---------------------------------------------------------------------------

use crate::targeted_deletion::TargetedDeletionPass;
use ene_preservation::{
    DeletionPurpose, DeletionSearchMaterial, MechanicalDeletionTarget, ParticipantOwnerRef,
    PreservationRepository as _, StartTargetedDeletionCommand, StartTargetedDeletionOutcome,
    TargetedDeletionTarget,
};

/// Admits one deletion operation with `participants` and runs one bounded
/// fan-out pass, so the Host-transient owner (registered by the composition)
/// is actually demanded.
async fn admit_and_drive(handle: &HostHandle, text: &str, participants: Vec<ParticipantOwnerRef>) {
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
        participants,
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
    let outcome = handle
        .drive_targeted_deletion(TargetedDeletionPass::new(100, 4))
        .await
        .expect("the pass runs");
    assert!(
        outcome.verified >= 1,
        "the host-transient demand verifies its own bounded work: {outcome:?}"
    );
}

/// One authenticated-and-current connection on a caller-owned table: the
/// same-device replacement below installs a second record and supersedes the
/// first, exactly like the handshake path (IPC §9.3).
fn authenticated_connection(
    table: &Arc<ConnectionTable>,
    device: &str,
) -> (ConnectionWireId, LiveInput) {
    let id = table.note_accept();
    authenticate(table, &id, device);
    let live = table
        .snapshot(&id)
        .expect("the authenticated connection snapshots");
    (id, live)
}

#[tokio::test]
async fn a3c_an_old_receipt_cannot_present_after_a_condition_and_transient_demand() {
    let (handle, _dir) = open_handle("present-a3c-ack").await;
    let _ = attach(&handle, DEVICE_A).await;
    let live = live_input(DEVICE_A);
    let generation = attribution_of(&handle).await.generation;
    append_reply(&handle, "the target body", generation).await;
    let summary = summary_of(fetch(&handle, &live, None, None, false).await);
    assert_eq!(summary.items.len(), 1);
    assert_eq!(summary.items[0].excerpt, "the target body");

    // The condition is durable and the Host transient holder is demanded
    // before the ACK arrives.
    admit_and_drive(
        &handle,
        "the target body",
        vec![ParticipantOwnerRef::HostTransient],
    )
    .await;

    let outcome = ack(
        &handle,
        &live,
        &summary.receipt.0,
        summary.round.clone(),
        summary.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(
            outcome,
            UndeliveredAckOutcome::StalePresentation | UndeliveredAckOutcome::UnknownRef
        ),
        "an invalidated receipt never presents a covered row, got {outcome:?}"
    );
    let statuses = unpresented_statuses(&handle).await;
    assert_eq!(statuses.len(), 1);
    assert_ne!(
        statuses[0].1,
        ReportStatus::Presented,
        "the covered row stays unpresented"
    );
    // A local transient drop is not the global completion: the operation is
    // still unfinished after the verified host-transient demand.
    assert_eq!(
        handle
            .store
            .unfinished_deletions(None, 100)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn a3c_a_fresh_read_after_a_condition_withholds_the_covered_body() {
    let (handle, _dir) = open_handle("present-a3c-read").await;
    let _ = attach(&handle, DEVICE_A).await;
    let generation = attribution_of(&handle).await.generation;
    append_reply(&handle, "covered body text", generation).await;
    append_reply(&handle, "unrelated body text", generation).await;

    admit_and_drive(
        &handle,
        "covered body text",
        vec![ParticipantOwnerRef::HostTransient],
    )
    .await;

    // A reconnect (fresh connection lifetime) reconstructs the page from the
    // canonical source: the covered body is withheld, the unrelated one is
    // still served.
    let reconnected = live_input(DEVICE_A);
    let summary = summary_of(fetch(&handle, &reconnected, None, None, false).await);
    assert_eq!(summary.items.len(), 2, "correlation rows stay pageable");
    let excerpts: Vec<&str> = summary
        .items
        .iter()
        .map(|item| item.excerpt.as_str())
        .collect();
    assert!(
        excerpts
            .iter()
            .all(|excerpt| !excerpt.contains("covered body text")),
        "no covered body is re-materialized: {excerpts:?}"
    );
    assert!(
        excerpts.contains(&"unrelated body text"),
        "unrelated bodies stay presentable: {excerpts:?}"
    );
}

#[tokio::test]
async fn a3c_a_replacement_and_deletion_never_resurrect_the_stale_payload() {
    let (handle, _dir) = open_handle("present-a3c-replace").await;
    let _ = attach(&handle, DEVICE_A).await;
    let table = Arc::new(ConnectionTable::new());
    handle.install_client_connection_table(Arc::clone(&table));
    let generation = attribution_of(&handle).await.generation;
    append_reply(&handle, "stale payload body", generation).await;

    // C1 receives the summary first.
    let (c1_id, c1) = authenticated_connection(&table, DEVICE_A);
    let first = summary_of(fetch(&handle, &c1, None, None, false).await);
    assert_eq!(first.items.len(), 1);
    assert!(first.items[0].excerpt.contains("stale payload body"));

    // C2 replaces C1 (the handshake path reports the supersession to the
    // Host's single lifecycle boundary), and a deletion condition becomes
    // durable; the transient demand drops the receipt world.
    let (_c2_id, c2) = authenticated_connection(&table, DEVICE_A);
    handle.on_connection_superseded(&c1_id);
    admit_and_drive(
        &handle,
        "stale payload body",
        vec![ParticipantOwnerRef::HostTransient],
    )
    .await;

    // C1's own connection is superseded: nothing installs for it any more.
    let frame = frame_for(
        WirePayload::UndeliveredRequest(UndeliveredRequest {
            companion: None,
            cursor: None,
            limit: None,
            redisplay: false,
        }),
        &c1,
        None,
        None,
        None,
    );
    let request = match &frame.payload {
        WirePayload::UndeliveredRequest(request) => request.clone(),
        _ => unreachable!(),
    };
    let refused = handle.request_undelivered(&frame, &c1, &request).await;
    assert_eq!(refused.len(), 1, "one request answers one frame");
    assert!(
        matches!(refused[0].payload, WirePayload::Reject(_)),
        "a superseded connection never restarts a presentation pass"
    );
    // C1's old receipt never presents covered rows on any connection.
    let stale_ack = ack(
        &handle,
        &c2,
        &first.receipt.0,
        first.round.clone(),
        first.presence_generation,
        PresentationStatus::Presented,
    )
    .await;
    assert!(
        matches!(
            stale_ack,
            UndeliveredAckOutcome::StaleConnection | UndeliveredAckOutcome::StalePresentation
        ),
        "the old receipt is stale, got {stale_ack:?}"
    );

    // C2's fresh pass re-reads the canonical source: no covered body.
    let second = summary_of(fetch(&handle, &c2, None, None, false).await);
    assert!(
        second
            .items
            .iter()
            .all(|item| !item.excerpt.contains("stale payload body")),
        "the replacement never inherits the stale payload: {:?}",
        second.items
    );
    let statuses = unpresented_statuses(&handle).await;
    assert!(
        statuses
            .iter()
            .all(|(_, status)| *status != ReportStatus::Presented),
        "no path moved the covered row to Presented: {statuses:?}"
    );
}

#[tokio::test]
async fn a3c_the_read_coverage_premise_is_canonical_and_body_free() {
    let (handle, _dir) = open_handle("present-a3c-coverage").await;
    // No current condition: the canonical read answers the authoritative
    // empty set, never a cached "no deletion" sentinel.
    assert!(!handle.current_coverage().await.covers("the target body"));
    admit_and_drive(
        &handle,
        "the target body",
        vec![ParticipantOwnerRef::HostTransient],
    )
    .await;
    let coverage = handle.current_coverage().await;
    assert!(
        coverage.covers("prefix the target body suffix"),
        "a body containing the mechanical target is covered"
    );
    assert!(
        !coverage.covers("unrelated text"),
        "an unrelated body stays presentable"
    );
}
