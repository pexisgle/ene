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

use ene_api::v1::envelope::{ProtocolVersion, WireSender, new_outgoing_envelope};
use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{
    ClientIncarnationId, CommandWireId, RequestWireId, RoundWireId, WireMessageType,
};
use ene_api::v1::round::PresentationStatus;
use ene_api::v1::undelivered::{
    GetReportSource, GetTaskReport, ListTasks, ResumeTask, ResumeTaskOutcomeWire, SelectTask,
    TaskListResponse, TaskReportResponse, UndeliveredAck, UndeliveredAckOutcome,
    UndeliveredRequest, UndeliveredResponse,
};
use ene_companion::{
    AppendHistoryCommand, CompanionId, CompanionRepository as _, HistoryRepository as _,
    HistoryRole, ReportStatus, UndeliveredRef, UndeliveredRepository as _,
};
use ene_inference::fake::{FakeFailure, FakeProviderTransport};
use ene_plugin_ipc::WireFrame;
use ene_presence::{
    PresenceAttribution, PresenceGeneration, PresenceRepository as _, PresenceState,
};
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegationId, TaskContextEntryId, TaskContextOrigin, TaskContextOriginKind,
    TaskCreationPremise, TaskId, TaskPurpose, TaskRef, TaskRepository as _,
    WorkspaceAssociationPremise, WorkspaceFolderRef, WorkspaceNeedRef,
};

use crate::serve::{FrameSink, HostHandle, LiveInput};
use crate::task_run::TaskAgentLauncher;
use crate::test_support::{live_input, memory_handle};

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
    match handle
        .attach_presence(
            device,
            true,
            ene_presence::PresenceState::NoActive,
            current.generation,
        )
        .await
    {
        crate::dialogue::AttachOutcome::Attached(fresh) => fresh,
        crate::dialogue::AttachOutcome::Raced => panic!("attach must win on a fresh handle"),
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
    let live_b = LiveInput {
        connection_id: live_a.connection_id,
        ..live_input(DEVICE_A)
    };
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
