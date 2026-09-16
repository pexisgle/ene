//! Undelivered subscription, presentation receipts, and first-party Task
//! queries (IPC §13.3, §18.2).
//!
//! Composition over existing owner boundaries only: the companion owner's
//! [`UndeliveredRepository`] (slice C's
//! `list_unpresented` + pass bounds + excerpts), the Task owner's bounded
//! report queries, and the Task owner's resume gate (slice E). No lifecycle,
//! no durable state, no runner: subscriptions, receipts, wire-ref maps,
//! cursors, and retry-epoch slots are Host-memory only and drop on restart
//! (unresolvable refs then answer `UnknownRef`, rows re-present under new
//! receipts).
//!
//! Receipt rules (the whole §13.3 contract in one place):
//!
//! - One live receipt per Companion. Beginning commits the carried prefix
//!   Pending→PresentationUnknown and holds the receipt 30 s monotonic. Any
//!   ACK, expiry, or a newer connection's begin releases it; rows never move
//!   on send alone.
//! - A page carries the longest prefix of one bounded fetch that fits the
//!   agreed frame cap (the fetch bound itself shrinks, so the cursor the
//!   store returns always resumes exactly). Nothing fits, even one item →
//!   `FrameTooLarge` with zero mutation of rows, cursors, or receipts.
//! - `Unknown` / `Failed` re-present only on an explicit head pass or a new
//!   valid connection/presence. A new-arrival pass serves fresh Pending rows
//!   only and never auto-resends failed rows.
//! - ACKs validate connection + incarnation + receipt + Round + generation +
//!   carried ids. Only carried ids move (all-duplicate answers
//!   `AlreadyPresented` with no write); unknown receipts → `UnknownRef`,
//!   superseded/expired → `StalePresentation`, foreign connections →
//!   `StaleConnection`. ACKs never migrate across connections.
//!
//! Send policy (§13.3/§22): every response here is one bounded frame the
//! caller emits through non-blocking `try_send`; a full buffer ends the send
//! (the receipt is superseded on the next valid connection, or expires) and
//! never parks the Task runner behind capacity. The async gate below
//! serializes begin/ack transitions only (CCT §10.5), never provider I/O.
//!
//! Reads are pure: list/report/source/select touch no lifecycle, generation,
//! revision, or report status, and never start, repair, re-evaluate, or
//! register a runner (S5-12).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use ene_api::v1::payload::WirePayload;
use ene_api::v1::refs::{CompanionWireRef, ConnectionWireId, RoundWireId};
use ene_api::v1::reject::RejectKind;
use ene_api::v1::round::PresentationStatus;
use ene_api::v1::undelivered::{
    DEFAULT_PAGE_LIMIT, EXCERPT_MAX_BYTES, GetReportSource, GetTaskReport, ListTasks,
    MAX_PAGE_LIMIT, MAX_SOURCE_LIMIT_BYTES, MIN_SOURCE_LIMIT_BYTES, PageCursorWire,
    PresentationReceiptWireRef, ReportSourcePageView, ReportSourceResponse, ReportSourceWireRef,
    ResumeTask, ResumeTaskOutcomeWire, SelectTask, SelectTaskResponse, TaskListItem, TaskListPage,
    TaskListResponse, TaskReportPage, TaskReportResponse, TaskReportRowView, TaskReportView,
    TaskSelected, TaskWireRef, UndeliveredAck, UndeliveredAckOutcome, UndeliveredItemView,
    UndeliveredRequest, UndeliveredResponse, UndeliveredSourceView, UndeliveredSummary,
    UndeliveredWireRef,
};
use ene_companion::{
    ActivityRepository, CompanionId, CompanionRepository, RecordResumeActivityCommand,
    ReportStatus, TaskFact, UNDELIVERED_PAGE_MAX, UndeliveredCursor, UndeliveredId, UndeliveredRef,
    UndeliveredRepository, UndeliveredSource,
};
use ene_plugin_ipc::WireFrame;
use ene_presence::{ClientId, PresenceAttribution, PresenceRepository, PresenceState};
use ene_primitive::RawId;
use ene_task::{
    ResumeInstructionSource, ResumeTaskCommand, SteeringPremiseRef, TaskHeadline, TaskId,
    TaskPurposeRef, TaskRef, TaskReportRowCursor, TaskReportRowKind, TaskReportSourceRef,
    TaskRepository, TaskResultId, TaskResumeHold, TaskResumeOutcome, TaskRevision,
};
use uuid::Uuid;

use crate::serve::{
    HostHandle, LiveInput, device_client, outgoing_fact, outgoing_frame, reject_frame,
};

#[cfg(test)]
mod tests;

/// Keys per-connection presentation maps by the hyphenated wire form of the
/// id: the same string form the connection layer uses, so keys match across
/// the Host/connection boundary by construction.
fn conn_key(id: &ConnectionWireId) -> String {
    id.0.as_hyphenated().to_string()
}

/// How long a receipt waits for its ACK (monotonic; IPC §13.3).
const RECEIPT_TTL: Duration = Duration::from_secs(30);

/// Agreed presentation frame cap until capability negotiation carries client
/// limits: mirrors the memory section budget so one summary can never inflate
/// past the transport frame.
const PRESENTATION_FRAME_BUDGET: usize = 192 * 1024;

/// Per-item wire overhead added to excerpt bytes when fitting the cap.
const PER_ITEM_OVERHEAD: usize = 512;

/// Cap on remembered resume commands (epoch + id + fingerprint). Resume
/// commands are rare; beyond the cap the oldest decided slots drop while
/// in-flight slots are kept, so a dropped slot at worst replays one decided
/// outcome through the durable revision compare (never a double execution).
const RESUME_COMMAND_CAP: usize = 4096;

/// One connection's backlog subscription: how far drained passes consumed.
#[derive(Debug, Clone)]
struct Subscription {
    companion: RawId,
    /// Highest insertion bound a drained pass consumed. New-arrival passes
    /// start strictly after it; explicit passes rewind to the head.
    scan_floor: u64,
    /// Whether this subscription ever drained a pass. Only drained
    /// subscriptions take new-arrival passes: a new valid connection starts
    /// with an explicit head re-display (Unknown rows included), while a
    /// drained one serves arrivals only and never auto-resends failures.
    drained_once: bool,
    /// Undrained pass continuation: cursor, filter, and bound. A cursor-less
    /// request continues it before looking for arrivals, so an abandoned
    /// page never resends its head as "new".
    resume: Option<(UndeliveredCursor, bool, u32)>,
}

/// One live presentation receipt (at most one per Companion).
#[derive(Debug, Clone)]
struct Receipt {
    id: String,
    connection: String,
    incarnation: (u64, u64),
    companion: RawId,
    client: ClientId,
    round: ene_presentation::RoundId,
    round_wire: String,
    generation: u64,
    selected: Vec<UndeliveredId>,
    expires_at: Instant,
}

impl Receipt {
    fn expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// A stored page cursor: its kind binds it to one query (and Task).
#[derive(Debug, Clone)]
enum StoredCursor {
    TaskList {
        after: Option<TaskId>,
    },
    ReportRows {
        task: TaskId,
        after: Option<TaskReportRowCursor>,
    },
    Undelivered {
        companion: RawId,
        cursor: UndeliveredCursor,
        pending_only: bool,
        limit: u32,
    },
}

/// One remembered resume command in its sender epoch.
#[derive(Debug, Clone)]
struct ResumeSlot {
    epoch: String,
    fingerprint: String,
    state: ResumeSlotState,
    seq: u64,
}

#[derive(Debug, Clone)]
enum ResumeSlotState {
    InFlight,
    Done(ResumeTaskOutcomeWire),
}

/// Host-memory presentation state. Restart drops all of it; durable rows are
/// untouched, so nothing here can strand a report.
pub(crate) struct PresentationState {
    subs: HashMap<String, Subscription>,
    /// By companion key: at most one live receipt each.
    receipts: HashMap<String, Receipt>,
    /// Receipt id → companion key.
    receipt_ids: HashMap<String, String>,
    /// Consumed/superseded receipt ids answering `StalePresentation`
    /// (bounded; never-known ids still answer `UnknownRef`). The issuing
    /// connection rides along so a foreign connection still hears
    /// `StaleConnection` first: ACKs never migrate.
    retired: std::collections::VecDeque<(String, String)>,
    task_refs: HashMap<(String, String), TaskId>,
    /// Item ref → carried row, plus the Task behind it (when Task-owned).
    carried: HashMap<(String, String), (UndeliveredId, Option<TaskId>)>,
    source_refs: HashMap<(String, String), TaskReportSourceRef>,
    cursors: HashMap<(String, String), StoredCursor>,
    resume: HashMap<Uuid, ResumeSlot>,
    resume_seq: u64,
    /// Agreed frame cap override (tests pin small caps to exercise splits;
    /// production keeps the default until negotiation carries limits).
    frame_budget: usize,
}

impl Default for PresentationState {
    fn default() -> Self {
        Self {
            subs: HashMap::new(),
            receipts: HashMap::new(),
            receipt_ids: HashMap::new(),
            retired: std::collections::VecDeque::new(),
            task_refs: HashMap::new(),
            carried: HashMap::new(),
            source_refs: HashMap::new(),
            cursors: HashMap::new(),
            resume: HashMap::new(),
            resume_seq: 0,
            frame_budget: PRESENTATION_FRAME_BUDGET,
        }
    }
}

/// Bound on remembered retired receipt ids: old receipts stay answerable as
/// stale without growing memory with the connection count.
const RETIRED_RECEIPT_CAP: usize = 64;

/// Opaque purpose identity echoed by the Client: `{task}:{revision}`.
fn encode_purpose(task: TaskId, revision: TaskRevision) -> String {
    format!(
        "{}:{}",
        task.as_raw().as_uuid().as_hyphenated(),
        revision.as_u64()
    )
}

fn decode_purpose(raw: &str) -> Option<TaskPurposeRef> {
    let (task, revision) = raw.split_once(':')?;
    let task = TaskId::from_raw(RawId::from_uuid(Uuid::parse_str(task).ok()?));
    let revision = TaskRevision::from_u64(revision.parse().ok()?);
    Some(TaskPurposeRef {
        task,
        adopted_revision: revision,
    })
}

fn source_view(source: &UndeliveredSource) -> UndeliveredSourceView {
    match source {
        UndeliveredSource::HistoryMessage(id) => UndeliveredSourceView {
            kind: String::from("history_message"),
            subject: id.as_uuid().as_hyphenated().to_string(),
            certainty: None,
        },
        UndeliveredSource::ActivityRecord(id) => UndeliveredSourceView {
            kind: String::from("activity_record"),
            subject: id.as_uuid().as_hyphenated().to_string(),
            certainty: None,
        },
        UndeliveredSource::TaskRecord { fact, .. } => match fact {
            TaskFact::TaskRevision { task, revision } => UndeliveredSourceView {
                kind: String::from("task_revision"),
                subject: format!("{}:{revision}", task.as_uuid().as_hyphenated()),
                certainty: None,
            },
            TaskFact::Delegation(id) => UndeliveredSourceView {
                kind: String::from("delegation"),
                subject: id.as_uuid().as_hyphenated().to_string(),
                certainty: None,
            },
            TaskFact::ActionAttempt { attempt, certainty } => UndeliveredSourceView {
                kind: String::from("action_attempt"),
                subject: attempt.as_uuid().as_hyphenated().to_string(),
                certainty: Some(certainty.as_str().to_string()),
            },
            TaskFact::ResultRecorded(id) => UndeliveredSourceView {
                kind: String::from("result_recorded"),
                subject: id.as_uuid().as_hyphenated().to_string(),
                certainty: None,
            },
            TaskFact::ResultAdopted(id) => UndeliveredSourceView {
                kind: String::from("result_adopted"),
                subject: id.as_uuid().as_hyphenated().to_string(),
                certainty: None,
            },
            TaskFact::Terminal { task, progress } => UndeliveredSourceView {
                kind: String::from("terminal"),
                subject: task.as_uuid().as_hyphenated().to_string(),
                certainty: Some(progress.as_str().to_string()),
            },
        },
    }
}

/// The Task behind one undelivered source, when the fact is Task-owned.
fn task_behind_source(source: &UndeliveredSource) -> Option<TaskId> {
    match source {
        UndeliveredSource::TaskRecord { task, .. } => Some(TaskId::from_raw(*task)),
        UndeliveredSource::HistoryMessage(_) | UndeliveredSource::ActivityRecord(_) => None,
    }
}

fn hold_name(hold: TaskResumeHold) -> &'static str {
    match hold {
        TaskResumeHold::CompanionUnavailable => "companion_unavailable",
        TaskResumeHold::WorkspaceUnavailable => "workspace_unavailable",
        TaskResumeHold::InstructionUnavailable => "instruction_unavailable",
        TaskResumeHold::PermissionUnavailable => "permission_unavailable",
        TaskResumeHold::DataUseHeld => "data_use_held",
        TaskResumeHold::ExecutionUnavailable => "execution_unavailable",
    }
}

fn limit_reject(frame: &WireFrame, live: &LiveInput, detail: &str) -> WireFrame {
    reject_frame(
        frame,
        live,
        RejectKind::UnsupportedFieldValue,
        detail.to_string(),
    )
}

fn checked_limit(limit: Option<u32>) -> Option<u32> {
    match limit {
        None => Some(DEFAULT_PAGE_LIMIT),
        Some(value) if (1..=MAX_PAGE_LIMIT).contains(&value) => Some(value),
        Some(_) => None,
    }
}

impl HostHandle {
    /// Serializes begin/ack transitions (CCT §10.5). Held only across short
    /// store roundtrips, never across provider I/O.
    async fn presentation_gate(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.presentation_lock.lock().await
    }

    /// Sender epoch binding one retry namespace (IPC §6.2): device,
    /// incarnation, and connection. A new connection or process is a new
    /// epoch; old commands never auto-resend into it.
    fn epoch_key(live: &LiveInput, frame: &WireFrame) -> String {
        format!(
            "{}:{}:{}:{}",
            live.client_ref,
            frame.envelope.sender.incarnation_id.counter,
            frame.envelope.sender.incarnation_id.random,
            conn_key(&live.connection_id),
        )
    }

    fn mint_cursor(
        state: &mut PresentationState,
        conn: &str,
        cursor: StoredCursor,
    ) -> PageCursorWire {
        let wire = Uuid::new_v4().as_hyphenated().to_string();
        state
            .cursors
            .insert((conn.to_string(), wire.clone()), cursor);
        PageCursorWire(wire)
    }

    fn mint_task_ref(state: &mut PresentationState, conn: &str, task: TaskId) -> TaskWireRef {
        let wire = Uuid::new_v4().as_hyphenated().to_string();
        state
            .task_refs
            .insert((conn.to_string(), wire.clone()), task);
        TaskWireRef(wire)
    }

    fn mint_source_ref(
        state: &mut PresentationState,
        conn: &str,
        source: TaskReportSourceRef,
    ) -> ReportSourceWireRef {
        let wire = Uuid::new_v4().as_hyphenated().to_string();
        state
            .source_refs
            .insert((conn.to_string(), wire.clone()), source);
        ReportSourceWireRef(wire)
    }

    /// Retires one receipt id with its issuing connection: it stays
    /// answerable as `StalePresentation` (or `StaleConnection` from a
    /// foreign connection) instead of decaying into `UnknownRef`. Bounded;
    /// never-known ids are unaffected.
    fn retire(state: &mut PresentationState, id: &str, connection: &str) {
        if state.retired.iter().any(|(known, _)| known == id) {
            return;
        }
        state
            .retired
            .push_back((id.to_string(), connection.to_string()));
        while state.retired.len() > RETIRED_RECEIPT_CAP {
            state.retired.pop_front();
        }
    }

    /// Drops one connection's carried item refs: every begin re-registers
    /// its own wires, so a polling connection cannot grow the map. Item refs
    /// are never echoed back for state changes (ACKs are receipt-scoped),
    /// so no live reference dies here.
    fn sweep_carried(state: &mut PresentationState, conn: &str) {
        state.carried.retain(|(c, _), _| c != conn);
    }

    /// Consumes one single-use page cursor: forward-only paging never
    /// rewinds through an old cursor, and the map cannot grow with the page
    /// count.
    fn take_cursor(state: &mut PresentationState, conn: &str, cursor: Option<&str>) {
        if let Some(wire) = cursor {
            state.cursors.remove(&(conn.to_string(), wire.to_string()));
        }
    }

    /// Removes one live receipt, retiring its id.
    fn remove_receipt(state: &mut PresentationState, companion_key: &str) {
        if let Some(receipt) = state.receipts.remove(companion_key) {
            state.receipt_ids.remove(&receipt.id);
            Self::retire(state, &receipt.id, &receipt.connection);
        }
    }

    /// Dispatch entry: one undelivered subscription/page request.
    pub(crate) async fn request_undelivered(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        request: &UndeliveredRequest,
    ) -> Vec<WireFrame> {
        let Some(limit) = checked_limit(request.limit) else {
            return vec![limit_reject(frame, live, "query limit must be 1..=50")];
        };
        let response = self
            .present_page(
                frame,
                live,
                request.companion.as_ref(),
                request.cursor.as_ref(),
                limit,
                request.redisplay,
            )
            .await;
        vec![outgoing_frame(
            frame,
            live,
            WirePayload::UndeliveredResponse(response),
        )]
    }

    /// Presents one page: presence-checked, receipt-backed, frame-capped.
    /// Store failures answer drained-empty (rows stay; the next request
    /// retries the same pass) rather than a lying page.
    async fn present_page(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        companion: Option<&CompanionWireRef>,
        cursor: Option<&PageCursorWire>,
        limit: u32,
        redisplay: bool,
    ) -> UndeliveredResponse {
        let companion = match companion {
            Some(wire) => match self.resolve_companion(&wire.0).await {
                Err(_) => return UndeliveredResponse::Summary(self.empty_shell()),
                Ok(None) => return UndeliveredResponse::UnknownCompanion,
                Ok(Some(companion)) => companion,
            },
            None => match self.store.ensure_running_companion().await {
                Err(_) => return UndeliveredResponse::Summary(self.empty_shell()),
                Ok(companion) => companion,
            },
        };
        // Companion summary presentation needs formal presence: Present for
        // this connection's client. NoActive reconnects keep management
        // reads; talk needs a fresh summon.
        let attribution = match self.store.load_attribution(companion.as_raw()).await {
            Ok(Some(attribution)) => attribution,
            _ => return UndeliveredResponse::NoCurrentPresence,
        };
        let Some(device_wire) = live.paired_device.clone() else {
            return UndeliveredResponse::NoCurrentPresence;
        };
        let client = device_client(&device_wire);
        if attribution.state != PresenceState::Present
            || attribution.active_client != Some(client)
            || !live.connection_live
        {
            return UndeliveredResponse::NoCurrentPresence;
        }
        let _gate = self.presentation_gate().await;
        let conn = conn_key(&live.connection_id);
        let incarnation = (
            frame.envelope.sender.incarnation_id.counter,
            frame.envelope.sender.incarnation_id.random,
        );
        let companion_key = companion.as_raw().as_uuid().as_hyphenated().to_string();
        // A live receipt on this connection re-displays its own selection:
        // duplicate display without a new commit or cursor move.
        if cursor.is_none()
            && let Some(receipt) = self.live_receipt(&companion_key, &conn)
        {
            return self.reemit_receipt(&conn, &receipt, limit).await;
        }
        // A foreign cursor is StaleBaseView, never a silent restart.
        if let Some(cursor) = cursor
            && !self.valid_undelivered_cursor(&conn, companion, cursor)
        {
            return UndeliveredResponse::StaleBaseView { current: None };
        }
        self.begin_pass(
            &conn,
            incarnation,
            client,
            companion,
            &attribution,
            cursor,
            limit,
            redisplay,
        )
        .await
    }

    /// The live receipt for this companion + connection, if any.
    fn live_receipt(&self, companion_key: &str, conn: &str) -> Option<Receipt> {
        let state = crate::lock_unpoison(&self.presentations);
        let receipt = state.receipts.get(companion_key)?.clone();
        (receipt.connection == conn && !receipt.expired()).then_some(receipt)
    }

    fn valid_undelivered_cursor(
        &self,
        conn: &str,
        companion: CompanionId,
        cursor: &PageCursorWire,
    ) -> bool {
        let state = crate::lock_unpoison(&self.presentations);
        matches!(
            state.cursors.get(&(conn.to_string(), cursor.0.clone())),
            Some(StoredCursor::Undelivered { companion: bound, .. })
                if *bound == companion.as_raw()
        )
    }

    /// Re-emits a live receipt's own selection: no new receipt, no commit,
    /// no cursor move. Over budget now (excerpts only shrink) answers
    /// `FrameTooLarge` with the receipt standing.
    async fn reemit_receipt(
        &self,
        conn: &str,
        receipt: &Receipt,
        limit: u32,
    ) -> UndeliveredResponse {
        let page = match self
            .store
            .list_unpresented(
                CompanionId::from_raw(receipt.companion),
                None,
                UNDELIVERED_PAGE_MAX,
            )
            .await
        {
            Ok(page) => page,
            Err(_) => {
                return UndeliveredResponse::Summary(self.receipt_shell(receipt, Vec::new(), true));
            }
        };
        Self::sweep_carried(&mut crate::lock_unpoison(&self.presentations), conn);
        let wanted: HashSet<UndeliveredId> = receipt.selected.iter().copied().collect();
        let entries: Vec<UndeliveredRef> = page
            .entries
            .into_iter()
            .filter(|entry| wanted.contains(&entry.id))
            .take(limit as usize)
            .collect();
        let items = self.carry_items(conn, &entries).await;
        if items.is_empty() && !entries.is_empty() {
            return UndeliveredResponse::FrameTooLarge;
        }
        if estimate_summary_bytes(&items) > self.frame_budget() {
            return UndeliveredResponse::FrameTooLarge;
        }
        let mut summary = self.receipt_shell(receipt, items, true);
        self.attach_reports(conn, &mut summary).await;
        UndeliveredResponse::Summary(summary)
    }

    /// Begins or continues one pass: fetches the longest fitting prefix of
    /// one bounded fetch, commits it, installs the receipt.
    #[allow(clippy::too_many_arguments)]
    async fn begin_pass(
        &self,
        conn: &str,
        incarnation: (u64, u64),
        client: ClientId,
        companion: CompanionId,
        attribution: &PresenceAttribution,
        cursor: Option<&PageCursorWire>,
        limit: u32,
        redisplay: bool,
    ) -> UndeliveredResponse {
        let from_cursor = cursor.map(|cursor| cursor.0.as_str());
        // Resolve the fetch window: continue a stored pass, catch up on new
        // arrivals, or rewind to the head for an explicit pass.
        let start: PlanStart = if let Some(cursor) = cursor {
            let state = crate::lock_unpoison(&self.presentations);
            match state.cursors.get(&(conn.to_string(), cursor.0.clone())) {
                Some(StoredCursor::Undelivered {
                    cursor,
                    pending_only,
                    limit,
                    ..
                }) => PlanStart::Continued {
                    cursor: *cursor,
                    pending_only: *pending_only,
                    limit: *limit,
                },
                _ => return UndeliveredResponse::StaleBaseView { current: None },
            }
        } else {
            // A newer connection supersedes a live receipt from a dead one:
            // old rows stay Unknown and re-present under the new receipt.
            let bound = self.store.undelivered_pass_bound().await.unwrap_or(0);
            let mut state = crate::lock_unpoison(&self.presentations);
            let ckey = companion.as_raw().as_uuid().as_hyphenated().to_string();
            if let Some(receipt) = state.receipts.get(&ckey).cloned()
                && (receipt.connection != conn || receipt.expired())
            {
                Self::remove_receipt(&mut state, &ckey);
            }
            let sub = state.subs.entry(conn.to_string()).or_insert(Subscription {
                companion: companion.as_raw(),
                scan_floor: 0,
                drained_once: false,
                resume: None,
            });
            // A connection retargeted at another companion restarts its
            // scan: floors and continuations never cross companions.
            if sub.companion != companion.as_raw() {
                sub.scan_floor = 0;
                sub.drained_once = false;
                sub.resume = None;
            }
            // A cursor-less request is the next logical page: an undrained
            // pass continues first (its rows are bounded to the captured
            // upper, so mid-pass arrivals wait), then new arrivals, then an
            // explicit head re-display. `redisplay` forces the head pass.
            // The resume binding is checked before the subscription is
            // re-pointed at this companion.
            let plan = if !redisplay
                && let Some((cursor, pending_only, saved)) = sub.resume
                && sub.companion == companion.as_raw()
            {
                PlanStart::Continued {
                    cursor,
                    pending_only,
                    limit: saved,
                }
            } else if !redisplay
                && sub.drained_once
                && sub.scan_floor < bound
                && sub.companion == companion.as_raw()
            {
                PlanStart::Arrivals {
                    after: sub.scan_floor,
                    upper: bound,
                }
            } else {
                PlanStart::Explicit { upper: bound }
            };
            sub.companion = companion.as_raw();
            plan
        };
        // Fetch the longest fitting prefix: shrink the fetch bound itself so
        // the store-returned cursor always resumes exactly. A continued pass
        // keeps the bound it started with.
        let pass_limit = match start {
            PlanStart::Continued { limit, .. } => limit,
            PlanStart::Arrivals { .. } | PlanStart::Explicit { .. } => limit,
        };
        Self::sweep_carried(&mut crate::lock_unpoison(&self.presentations), conn);
        let mut fetch_limit = pass_limit;
        let budget = self.frame_budget();
        let fitted = loop {
            let (start_cursor, upper, pending_only) = match start {
                PlanStart::Continued {
                    cursor,
                    pending_only,
                    limit: _,
                } => (Some(cursor), cursor.pass_upper_bound(), pending_only),
                PlanStart::Arrivals { after, upper } => {
                    (Some(UndeliveredCursor::begin(after, upper)), upper, true)
                }
                PlanStart::Explicit { upper } => (None, upper, false),
            };
            let page = match self
                .store
                .list_unpresented(companion, start_cursor, fetch_limit)
                .await
            {
                Ok(page) => page,
                Err(_) => {
                    return UndeliveredResponse::Summary(self.empty_attributed(attribution));
                }
            };
            let entries: Vec<UndeliveredRef> = if pending_only {
                page.entries
                    .into_iter()
                    .filter(|entry| entry.status == ReportStatus::Pending)
                    .collect()
            } else {
                page.entries
            };
            let items = self.carry_items(conn, &entries).await;
            if estimate_summary_bytes(&items) <= budget {
                break (entries, items, page.next, upper, pending_only);
            }
            if fetch_limit <= 1 {
                // Even one item overflows the cap: withhold without mutating
                // any row, cursor, subscription, or receipt.
                self.forget_carried(conn, &items).await;
                return UndeliveredResponse::FrameTooLarge;
            }
            fetch_limit /= 2;
            self.forget_carried(conn, &items).await;
        };
        let (entries, items, fetched_next, upper, pending_only) = fitted;
        if entries.is_empty() {
            // A drained continued pass falls through to an explicit head
            // re-display in the same response instead of stranding failed
            // rows behind an empty arrival check.
            if matches!(start, PlanStart::Continued { .. }) {
                return Box::pin(self.begin_pass(
                    conn,
                    incarnation,
                    client,
                    companion,
                    attribution,
                    None,
                    limit,
                    true,
                ))
                .await;
            }
            if fetched_next.is_none() {
                self.mark_drained(conn, companion, upper);
            }
            let mut summary = self.empty_attributed(attribution);
            // A filtered-empty page with more waiting still pages next.
            if let Some(next) = fetched_next {
                let mut state = crate::lock_unpoison(&self.presentations);
                Self::take_cursor(&mut state, conn, from_cursor);
                summary.has_more = true;
                if let Some(sub) = state.subs.get_mut(conn) {
                    sub.resume = Some((next, pending_only, pass_limit));
                }
                summary.next_cursor = Some(Self::mint_cursor(
                    &mut state,
                    conn,
                    StoredCursor::Undelivered {
                        companion: companion.as_raw(),
                        cursor: next,
                        pending_only,
                        limit: pass_limit,
                    },
                ));
            }
            return UndeliveredResponse::Summary(summary);
        }
        self.commit_install(
            conn,
            incarnation,
            client,
            companion,
            attribution,
            entries,
            items,
            fetched_next,
            upper,
            pending_only,
            pass_limit,
            from_cursor,
        )
        .await
    }

    /// Commits the carried prefix (Pending→PresentationUnknown), installs
    /// the receipt, advances the cursor past the carried prefix only.
    #[allow(clippy::too_many_arguments)]
    async fn commit_install(
        &self,
        conn: &str,
        incarnation: (u64, u64),
        client: ClientId,
        companion: CompanionId,
        attribution: &PresenceAttribution,
        entries: Vec<UndeliveredRef>,
        items: Vec<UndeliveredItemView>,
        fetched_next: Option<UndeliveredCursor>,
        upper: u64,
        pending_only: bool,
        limit: u32,
        from_cursor: Option<&str>,
    ) -> UndeliveredResponse {
        let companion_key = companion.as_raw().as_uuid().as_hyphenated().to_string();
        let generation = attribution.generation.as_u64();
        let round = ene_presentation::RoundId::from_raw(RawId::new());
        let round_wire = self.round_wire_or_mint(&round);
        let mark = ene_companion::PresentationMark {
            round: round.as_raw(),
            presented: false,
        };
        let mut selected = Vec::new();
        for entry in &entries {
            if entry.status == ReportStatus::Pending
                && let Err(_) = self
                    .store
                    .compare_and_mark_reported(entry.id, ReportStatus::Pending, mark)
                    .await
            {
                // A lost compare leaves the row for the next pass; only
                // successfully-marked entries enter the receipt selection.
                continue;
            }
            selected.push(entry.id);
        }
        let receipt_id = Uuid::new_v4().as_hyphenated().to_string();
        let receipt = Receipt {
            id: receipt_id.clone(),
            connection: conn.to_string(),
            incarnation,
            companion: companion.as_raw(),
            client,
            round,
            round_wire: round_wire.0.clone(),
            generation,
            selected,
            expires_at: Instant::now() + RECEIPT_TTL,
        };
        // The carried prefix IS the fetched prefix (the fetch bound shrank
        // instead), so the store cursor resumes exactly: a fetched
        // continuation pages next, a bound-reaching fetch drains the floor.
        // The guard's scope ends before the bound check below awaits.
        let (next_cursor, drained) = {
            let mut state = crate::lock_unpoison(&self.presentations);
            // Forward-only paging consumes the cursor it continued from.
            Self::take_cursor(&mut state, conn, from_cursor);
            // One receipt per Companion: installing supersedes any leftover.
            if let Some(old) = state.receipts.insert(companion_key.clone(), receipt) {
                state.receipt_ids.remove(&old.id);
                Self::retire(&mut state, &old.id, &old.connection);
            }
            state.receipt_ids.insert(receipt_id.clone(), companion_key);
            match fetched_next {
                Some(next) => {
                    let resume = (next, pending_only, limit);
                    if let Some(sub) = state.subs.get_mut(conn) {
                        sub.resume = Some(resume);
                    }
                    (
                        Some(Self::mint_cursor(
                            &mut state,
                            conn,
                            StoredCursor::Undelivered {
                                companion: companion.as_raw(),
                                cursor: next,
                                pending_only,
                                limit,
                            },
                        )),
                        false,
                    )
                }
                None => {
                    if let Some(sub) = state.subs.get_mut(conn)
                        && sub.companion == companion.as_raw()
                    {
                        sub.scan_floor = sub.scan_floor.max(upper);
                        sub.drained_once = true;
                        sub.resume = None;
                    }
                    (None, true)
                }
            }
        };
        let has_more =
            !drained || self.store.undelivered_pass_bound().await.unwrap_or(upper) > upper;
        let mut summary = UndeliveredSummary {
            receipt: PresentationReceiptWireRef(receipt_id),
            round: round_wire,
            presence_generation: generation,
            items,
            reports: Vec::new(),
            has_more,
            next_cursor,
        };
        self.attach_reports(conn, &mut summary).await;
        UndeliveredResponse::Summary(summary)
    }

    /// Builds carried items (with bounded excerpts) for entries in order,
    /// registering per-connection item refs. Unreadable excerpts keep their
    /// correlation with an empty excerpt; the body stays pageable and nothing
    /// is marked presented.
    async fn carry_items(
        &self,
        conn: &str,
        entries: &[UndeliveredRef],
    ) -> Vec<UndeliveredItemView> {
        let mut items = Vec::with_capacity(entries.len());
        for entry in entries {
            let (excerpt, truncated) = match self
                .store
                .load_undelivered_excerpt(entry.source, EXCERPT_MAX_BYTES)
                .await
            {
                Ok(Some(page)) => {
                    let truncated = page.total_bytes > page.text.len() as u64;
                    (page.text, truncated)
                }
                Ok(None) | Err(_) => (String::new(), false),
            };
            let wire = Uuid::new_v4().as_hyphenated().to_string();
            crate::lock_unpoison(&self.presentations).carried.insert(
                (conn.to_string(), wire.clone()),
                (entry.id, task_behind_source(&entry.source)),
            );
            items.push(UndeliveredItemView {
                reference: UndeliveredWireRef(wire),
                source: source_view(&entry.source),
                excerpt,
                truncated,
            });
        }
        items
    }

    /// Drops carried registrations for items that will not be committed
    /// (shrunk fetch iterations), so refs never outlive their receipt.
    async fn forget_carried(&self, conn: &str, items: &[UndeliveredItemView]) {
        let mut state = crate::lock_unpoison(&self.presentations);
        for item in items {
            state
                .carried
                .remove(&(conn.to_string(), item.reference.0.clone()));
        }
    }

    /// Attaches one headline per distinct Task behind the carried items.
    async fn attach_reports(&self, conn: &str, summary: &mut UndeliveredSummary) {
        let tasks: Vec<Option<TaskId>> = {
            let state = crate::lock_unpoison(&self.presentations);
            summary
                .items
                .iter()
                .map(|item| {
                    state
                        .carried
                        .get(&(conn.to_string(), item.reference.0.clone()))
                        .and_then(|(_, task)| *task)
                })
                .collect()
        };
        let mut seen: Vec<TaskId> = Vec::new();
        for task in tasks.into_iter().flatten() {
            if seen.contains(&task) {
                continue;
            }
            seen.push(task);
            let Ok(Some(record)) = self.store.load_task(task).await else {
                continue;
            };
            let details = record.task.adopted_result.is_some()
                || self
                    .store
                    .list_task_report_rows_after(task, None, 1)
                    .await
                    .is_ok_and(|rows| !rows.is_empty());
            let task_ref =
                Self::mint_task_ref(&mut crate::lock_unpoison(&self.presentations), conn, task);
            summary.reports.push(TaskReportView {
                task: task_ref,
                revision: record.task.reference.revision.as_u64(),
                progress: record.task.progress.as_str().to_string(),
                details_available: details,
            });
        }
    }

    fn mark_drained(&self, conn: &str, companion: CompanionId, upper: u64) {
        let mut state = crate::lock_unpoison(&self.presentations);
        if let Some(sub) = state.subs.get_mut(conn)
            && sub.companion == companion.as_raw()
        {
            sub.scan_floor = sub.scan_floor.max(upper);
            sub.drained_once = true;
            sub.resume = None;
        }
    }

    /// Agreed frame cap in force (tests pin small caps; production keeps the
    /// default until negotiation carries client limits).
    fn frame_budget(&self) -> usize {
        crate::lock_unpoison(&self.presentations).frame_budget
    }

    /// Test-only: pin the frame cap so paging splits stay deterministic
    /// without multi-megabyte fixtures.
    #[cfg(test)]
    pub(crate) fn set_frame_budget_for_test(&self, bytes: usize) {
        crate::lock_unpoison(&self.presentations).frame_budget = bytes;
    }

    fn receipt_shell(
        &self,
        receipt: &Receipt,
        items: Vec<UndeliveredItemView>,
        has_more: bool,
    ) -> UndeliveredSummary {
        UndeliveredSummary {
            receipt: PresentationReceiptWireRef(receipt.id.clone()),
            round: RoundWireId(receipt.round_wire.clone()),
            presence_generation: receipt.generation,
            items,
            reports: Vec::new(),
            has_more,
            next_cursor: None,
        }
    }

    /// Empty-page shell with a fresh round for correlation. No receipt is
    /// minted, so there is nothing to ACK; the next request retries.
    fn empty_attributed(&self, attribution: &PresenceAttribution) -> UndeliveredSummary {
        let round = ene_presentation::RoundId::from_raw(RawId::new());
        let round_wire = self.round_wire_or_mint(&round);
        UndeliveredSummary {
            receipt: PresentationReceiptWireRef(String::new()),
            round: round_wire,
            presence_generation: attribution.generation.as_u64(),
            items: Vec::new(),
            reports: Vec::new(),
            has_more: false,
            next_cursor: None,
        }
    }

    /// Store-failure shell: like [`Self::empty_attributed`] without a known
    /// generation. Rows stay; the next request retries the same pass.
    fn empty_shell(&self) -> UndeliveredSummary {
        let round = ene_presentation::RoundId::from_raw(RawId::new());
        let round_wire = self.round_wire_or_mint(&round);
        UndeliveredSummary {
            receipt: PresentationReceiptWireRef(String::new()),
            round: round_wire,
            presence_generation: 0,
            items: Vec::new(),
            reports: Vec::new(),
            has_more: false,
            next_cursor: None,
        }
    }

    /// Dispatch entry: one receipt ACK, answering its typed outcome.
    pub(crate) async fn ack_undelivered(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        ack: &UndeliveredAck,
    ) -> Vec<WireFrame> {
        let outcome = self.apply_ack(frame, live, ack).await;
        vec![outgoing_frame(
            frame,
            live,
            WirePayload::UndeliveredAckOutcome(outcome),
        )]
    }

    /// Applies one ACK: validates epoch + receipt + Round + generation, then
    /// moves only the carried ids. Every refusal leaves all rows untouched.
    ///
    /// The state guard never crosses an await: validation (including receipt
    /// release) computes an owned verdict in one scoped section, and the
    /// store commits run after, so the connection future stays `Send`.
    async fn apply_ack(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        ack: &UndeliveredAck,
    ) -> UndeliveredAckOutcome {
        let _gate = self.presentation_gate().await;
        let conn = conn_key(&live.connection_id);
        let incarnation = (
            frame.envelope.sender.incarnation_id.counter,
            frame.envelope.sender.incarnation_id.random,
        );
        let round_view = frame
            .envelope
            .observed
            .round_view
            .as_ref()
            .map(|round| round.0.clone());
        let generation_view = frame.envelope.observed.presence_generation_view;
        enum Verdict {
            Refuse(UndeliveredAckOutcome),
            Proceed(Receipt),
        }
        let verdict = {
            let mut state = crate::lock_unpoison(&self.presentations);
            let Some(companion_key) = state.receipt_ids.get(&ack.receipt.0).cloned() else {
                // Consumed or superseded receipts stay stale (never silently
                // unknown); never-issued ids are unknown. A foreign
                // connection still hears StaleConnection first: ACKs never
                // migrate.
                match state
                    .retired
                    .iter()
                    .find(|(known, _)| known == &ack.receipt.0)
                {
                    Some((_, issued)) if *issued != conn => {
                        return UndeliveredAckOutcome::StaleConnection;
                    }
                    Some(_) => return UndeliveredAckOutcome::StalePresentation,
                    None => return UndeliveredAckOutcome::UnknownRef,
                }
            };
            let Some(receipt) = state.receipts.get(&companion_key).cloned() else {
                return UndeliveredAckOutcome::UnknownRef;
            };
            if receipt.expired() {
                Self::remove_receipt(&mut state, &companion_key);
                Verdict::Refuse(UndeliveredAckOutcome::StalePresentation)
            } else if receipt.connection != conn || receipt.incarnation != incarnation {
                // The ACK never migrates across connections or incarnations.
                Verdict::Refuse(UndeliveredAckOutcome::StaleConnection)
            } else if live.paired_device.as_deref().map(device_client) != Some(receipt.client) {
                // The connection's client moved under the receipt.
                Verdict::Refuse(UndeliveredAckOutcome::StaleConnection)
            } else if round_view.as_deref() != Some(receipt.round_wire.as_str())
                || generation_view != Some(receipt.generation)
            {
                // Round + generation ride the envelope observed marks: the
                // Client echoes what the summary showed, and the Host
                // compares.
                Verdict::Refuse(UndeliveredAckOutcome::StalePresentation)
            } else {
                // Consumed: any ACK releases the receipt; the rows keep
                // whatever the status below decided.
                Self::remove_receipt(&mut state, &companion_key);
                Verdict::Proceed(receipt)
            }
        };
        let receipt = match verdict {
            Verdict::Refuse(outcome) => return outcome,
            Verdict::Proceed(receipt) => receipt,
        };
        let mark = ene_companion::PresentationMark {
            round: receipt.round.as_raw(),
            presented: matches!(ack.status, PresentationStatus::Presented),
        };
        match ack.status {
            PresentationStatus::Presented => {
                let mut presented = 0_u32;
                for id in &receipt.selected {
                    // Bounded to the carried id; later arrivals are never
                    // touched by this ACK.
                    if let Ok(ene_companion::ReportStatusTransition::PendingToPresented) = self
                        .store
                        .compare_and_mark_reported(*id, ReportStatus::PresentationUnknown, mark)
                        .await
                    {
                        presented += 1;
                    }
                }
                if presented > 0 {
                    UndeliveredAckOutcome::Presented { presented }
                } else {
                    // Every carried row was already presented (a parallel
                    // round observation got there first): no write.
                    UndeliveredAckOutcome::AlreadyPresented
                }
            }
            PresentationStatus::Failed => {
                let mut count = 0_u32;
                for id in &receipt.selected {
                    if let Ok(ene_companion::ReportStatusTransition::FailedToPending) = self
                        .store
                        .compare_and_mark_reported(*id, ReportStatus::PresentationUnknown, mark)
                        .await
                    {
                        count += 1;
                    }
                }
                UndeliveredAckOutcome::ReturnedToPending { count }
            }
            PresentationStatus::Unknown => UndeliveredAckOutcome::KeptUnknown,
        }
    }

    /// Dispatch entry: first-party Task list. Pure read: no presence needed,
    /// no mutation; stored lifecycle and the execution flag stay separate.
    pub(crate) async fn list_tasks_wire(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        query: &ListTasks,
    ) -> Vec<WireFrame> {
        let Some(limit) = checked_limit(query.limit) else {
            return vec![limit_reject(frame, live, "query limit must be 1..=50")];
        };
        let conn = conn_key(&live.connection_id);
        let after = match &query.cursor {
            None => None,
            Some(cursor) => {
                let state = crate::lock_unpoison(&self.presentations);
                match state.cursors.get(&(conn.clone(), cursor.0.clone())) {
                    Some(StoredCursor::TaskList { after }) => *after,
                    _ => {
                        return vec![outgoing_frame(
                            frame,
                            live,
                            WirePayload::TaskListResponse(TaskListResponse::StaleBaseView {
                                current: None,
                            }),
                        )];
                    }
                }
            }
        };
        let headlines: Vec<TaskHeadline> = self
            .store
            .list_tasks_after(after, limit)
            .await
            .unwrap_or_default();
        let mut state = crate::lock_unpoison(&self.presentations);
        Self::take_cursor(
            &mut state,
            &conn,
            query.cursor.as_ref().map(|cursor| cursor.0.as_str()),
        );
        let mut tasks = Vec::with_capacity(headlines.len());
        for headline in &headlines {
            tasks.push(TaskListItem {
                task: Self::mint_task_ref(&mut state, &conn, headline.task),
                revision: headline.revision.as_u64(),
                progress: headline.progress.as_str().to_string(),
                running: self
                    .task_executions
                    .task_has_reservation_or_running(headline.task),
                purpose: encode_purpose(headline.task, headline.revision),
            });
        }
        let next_cursor = if headlines.len() as u32 == limit
            && let Some(last) = headlines.last()
        {
            Some(Self::mint_cursor(
                &mut state,
                &conn,
                StoredCursor::TaskList {
                    after: Some(last.task),
                },
            ))
        } else {
            None
        };
        drop(state);
        vec![outgoing_frame(
            frame,
            live,
            WirePayload::TaskListResponse(TaskListResponse::Page(TaskListPage {
                tasks,
                next_cursor,
            })),
        )]
    }

    /// Dispatch entry: one Task's paged report. Pure read; attempt rows sort
    /// before result rows in canonical id order.
    pub(crate) async fn report_wire(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        query: &GetTaskReport,
    ) -> Vec<WireFrame> {
        let Some(limit) = checked_limit(query.limit) else {
            return vec![limit_reject(frame, live, "query limit must be 1..=50")];
        };
        let conn = conn_key(&live.connection_id);
        let task = {
            let state = crate::lock_unpoison(&self.presentations);
            match state
                .task_refs
                .get(&(conn.clone(), query.task.0.clone()))
                .copied()
            {
                Some(task) => task,
                None => {
                    return vec![outgoing_frame(
                        frame,
                        live,
                        WirePayload::TaskReportResponse(TaskReportResponse::UnknownRef),
                    )];
                }
            }
        };
        let after = match &query.cursor {
            None => None,
            Some(cursor) => {
                let state = crate::lock_unpoison(&self.presentations);
                match state.cursors.get(&(conn.clone(), cursor.0.clone())) {
                    Some(StoredCursor::ReportRows { task: bound, after }) if *bound == task => {
                        after.clone()
                    }
                    _ => {
                        return vec![outgoing_frame(
                            frame,
                            live,
                            WirePayload::TaskReportResponse(TaskReportResponse::StaleBaseView {
                                current: None,
                            }),
                        )];
                    }
                }
            }
        };
        let record = match self.store.load_task(task).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                return vec![outgoing_frame(
                    frame,
                    live,
                    WirePayload::TaskReportResponse(TaskReportResponse::UnknownRef),
                )];
            }
            Err(_) => {
                return vec![outgoing_frame(
                    frame,
                    live,
                    WirePayload::TaskReportResponse(TaskReportResponse::StaleBaseView {
                        current: None,
                    }),
                )];
            }
        };
        let rows = match self
            .store
            .list_task_report_rows_after(task, after, limit)
            .await
        {
            Ok(rows) => rows,
            Err(_) => {
                return vec![outgoing_frame(
                    frame,
                    live,
                    WirePayload::TaskReportResponse(TaskReportResponse::StaleBaseView {
                        current: None,
                    }),
                )];
            }
        };
        let mut state = crate::lock_unpoison(&self.presentations);
        Self::take_cursor(
            &mut state,
            &conn,
            query.cursor.as_ref().map(|cursor| cursor.0.as_str()),
        );
        let purpose_source = Self::mint_source_ref(
            &mut state,
            &conn,
            TaskReportSourceRef::RevisionPurpose {
                task,
                revision: record.task.reference.revision,
            },
        );
        let mut views = Vec::with_capacity(rows.len());
        for row in &rows {
            let source = match row.kind {
                TaskReportRowKind::TaskResult => Some(Self::mint_source_ref(
                    &mut state,
                    &conn,
                    TaskReportSourceRef::ResultBody(TaskResultId::from_raw(row.id)),
                )),
                TaskReportRowKind::ActionAttempt => None,
            };
            views.push(TaskReportRowView {
                kind: match row.kind {
                    TaskReportRowKind::ActionAttempt => String::from("action_attempt"),
                    TaskReportRowKind::TaskResult => String::from("task_result"),
                },
                id: row.id.as_uuid().as_hyphenated().to_string(),
                adopted_revision: row.adopted_revision.map(|revision| revision.as_u64()),
                source,
            });
        }
        let next_cursor = if rows.len() as u32 == limit
            && let Some(last) = rows.last()
        {
            Some(Self::mint_cursor(
                &mut state,
                &conn,
                StoredCursor::ReportRows {
                    task,
                    after: Some(TaskReportRowCursor {
                        kind: last.kind,
                        id: last.id,
                    }),
                },
            ))
        } else {
            None
        };
        drop(state);
        vec![outgoing_frame(
            frame,
            live,
            WirePayload::TaskReportResponse(TaskReportResponse::Page(TaskReportPage {
                task: query.task.clone(),
                revision: record.task.reference.revision.as_u64(),
                progress: record.task.progress.as_str().to_string(),
                purpose: encode_purpose(task, record.task.reference.revision),
                purpose_source,
                rows: views,
                next_cursor,
            })),
        )]
    }

    /// Dispatch entry: one bounded source body page. An unbuildable view
    /// answers `InputUnavailable` and sends no body.
    pub(crate) async fn report_source_wire(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        query: &GetReportSource,
    ) -> Vec<WireFrame> {
        let limit = match query.limit_bytes {
            None => ene_api::v1::undelivered::DEFAULT_SOURCE_LIMIT_BYTES,
            Some(value) if (MIN_SOURCE_LIMIT_BYTES..=MAX_SOURCE_LIMIT_BYTES).contains(&value) => {
                value
            }
            Some(_) => {
                return vec![limit_reject(
                    frame,
                    live,
                    "source limit must be 4..=16384 bytes",
                )];
            }
        };
        let conn = conn_key(&live.connection_id);
        let source = {
            let state = crate::lock_unpoison(&self.presentations);
            match state
                .source_refs
                .get(&(conn, query.source.0.clone()))
                .copied()
            {
                Some(source) => source,
                None => {
                    return vec![outgoing_frame(
                        frame,
                        live,
                        WirePayload::ReportSourceResponse(ReportSourceResponse::UnknownRef),
                    )];
                }
            }
        };
        match self
            .store
            .load_report_source_bounded(source, query.cursor.unwrap_or(0), limit)
            .await
        {
            Ok(Some(page)) => vec![outgoing_frame(
                frame,
                live,
                WirePayload::ReportSourceResponse(ReportSourceResponse::Page(
                    ReportSourcePageView {
                        text: page.text,
                        total_bytes: page.total_bytes,
                        next: page.next,
                    },
                )),
            )],
            Ok(None) | Err(_) => vec![outgoing_frame(
                frame,
                live,
                WirePayload::ReportSourceResponse(ReportSourceResponse::InputUnavailable),
            )],
        }
    }

    /// Dispatch entry: first-party Task selection into the conversation
    /// projection. In-memory only: no durable mutation, no execution start.
    /// A restart or reconnect resets to unselected.
    pub(crate) async fn select_task_wire(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        query: &SelectTask,
    ) -> Vec<WireFrame> {
        let conn = conn_key(&live.connection_id);
        let task = {
            let state = crate::lock_unpoison(&self.presentations);
            match state
                .task_refs
                .get(&(conn.clone(), query.task.0.clone()))
                .copied()
            {
                Some(task) => task,
                None => {
                    return vec![outgoing_frame(
                        frame,
                        live,
                        WirePayload::SelectTaskResponse(SelectTaskResponse::UnknownRef),
                    )];
                }
            }
        };
        let record = match self.store.load_task(task).await {
            Ok(Some(record)) => record,
            _ => {
                return vec![outgoing_frame(
                    frame,
                    live,
                    WirePayload::SelectTaskResponse(SelectTaskResponse::UnknownRef),
                )];
            }
        };
        // A Task owned by another companion is never selected here: the
        // projection is keyed by companion, and guessing across owners is a
        // fabrication, not a selection.
        let running = match self.store.ensure_running_companion().await {
            Ok(running) => running,
            Err(_) => {
                return vec![outgoing_frame(
                    frame,
                    live,
                    WirePayload::SelectTaskResponse(SelectTaskResponse::UnknownRef),
                )];
            }
        };
        if record.task.assignee.companion != running.as_raw() {
            return vec![outgoing_frame(
                frame,
                live,
                WirePayload::SelectTaskResponse(SelectTaskResponse::UnknownRef),
            )];
        }
        self.conversation_tasks.select(running, task);
        let details = record.task.adopted_result.is_some()
            || self
                .store
                .list_task_report_rows_after(task, None, 1)
                .await
                .is_ok_and(|rows| !rows.is_empty());
        vec![outgoing_frame(
            frame,
            live,
            WirePayload::SelectTaskResponse(SelectTaskResponse::Selected(TaskSelected {
                task: query.task.clone(),
                revision: record.task.reference.revision.as_u64(),
                progress: record.task.progress.as_str().to_string(),
                purpose: encode_purpose(task, record.task.reference.revision),
                details_available: details,
            })),
        )]
    }

    /// Dispatch entry: explicit first-party resume on the wire. Maps onto
    /// the existing owner gate; the envelope `command_id` keys the retry
    /// epoch. No presence and no provider success required.
    pub(crate) async fn resume_task_wire(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        command: &ResumeTask,
    ) -> Vec<WireFrame> {
        let Some(command_id) = frame.envelope.correlation.command_id else {
            return vec![reject_frame(
                frame,
                live,
                RejectKind::MissingRequiredField,
                String::from("resume requires a command id"),
            )];
        };
        match self.apply_resume(frame, live, command_id.0, command).await {
            ResumeApply::Outcome(outcome) => vec![outgoing_frame(
                frame,
                live,
                WirePayload::ResumeTaskOutcome(outcome),
            )],
            ResumeApply::Conflict => vec![reject_frame(
                frame,
                live,
                RejectKind::ConflictingCommand,
                String::from("command id reused with different content"),
            )],
        }
    }

    /// Applies one wire resume: epoch-conflict check, idempotent replay,
    /// then at most one owner commit per command id per epoch. The durable
    /// revision compare keeps restarts safe without any resume receipt
    /// table.
    async fn apply_resume(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        command_id: Uuid,
        command: &ResumeTask,
    ) -> ResumeApply {
        let epoch = Self::epoch_key(live, frame);
        let fingerprint = format!(
            "{}:{}:{}:{}",
            command.task.0,
            command.expected_revision,
            command.expected_purpose,
            command.instruction
        );
        {
            let mut state = crate::lock_unpoison(&self.presentations);
            if let Some(slot) = state.resume.get(&command_id) {
                if slot.epoch != epoch {
                    // An old command never auto-resends into a new epoch.
                    return ResumeApply::Outcome(ResumeTaskOutcomeWire::StaleConnection);
                }
                if slot.fingerprint != fingerprint {
                    return ResumeApply::Conflict;
                }
                match &slot.state {
                    ResumeSlotState::InFlight => {
                        return ResumeApply::Outcome(ResumeTaskOutcomeWire::InFlight);
                    }
                    ResumeSlotState::Done(outcome) => {
                        return ResumeApply::Outcome(outcome.clone());
                    }
                }
            }
            state.resume_seq += 1;
            let seq = state.resume_seq;
            state.resume.insert(
                command_id,
                ResumeSlot {
                    epoch,
                    fingerprint,
                    state: ResumeSlotState::InFlight,
                    seq,
                },
            );
            if state.resume.len() > RESUME_COMMAND_CAP {
                // ponytail: bounded epoch memory; decided slots drop first,
                // in-flight slots are never evicted under a live command.
                let mut decided: Vec<(u64, Uuid)> = state
                    .resume
                    .iter()
                    .filter(|(_, slot)| matches!(slot.state, ResumeSlotState::Done(_)))
                    .map(|(id, slot)| (slot.seq, *id))
                    .collect();
                decided.sort();
                let drop_count = state.resume.len() - RESUME_COMMAND_CAP;
                for (_, id) in decided.into_iter().take(drop_count) {
                    state.resume.remove(&id);
                }
            }
        }
        let outcome = self.commit_resume(frame, live, command).await;
        // Technical failures free the slot so the same command retries
        // cleanly; every decided answer persists for replay.
        if matches!(outcome, ResumeTaskOutcomeWire::Unavailable) {
            crate::lock_unpoison(&self.presentations)
                .resume
                .remove(&command_id);
        } else {
            let mut state = crate::lock_unpoison(&self.presentations);
            if let Some(slot) = state.resume.get_mut(&command_id) {
                slot.state = ResumeSlotState::Done(outcome.clone());
            }
        }
        ResumeApply::Outcome(outcome)
    }

    /// Runs the single owner resume commit for one validated wire command.
    async fn commit_resume(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        command: &ResumeTask,
    ) -> ResumeTaskOutcomeWire {
        let conn = conn_key(&live.connection_id);
        let task = {
            let state = crate::lock_unpoison(&self.presentations);
            match state
                .task_refs
                .get(&(conn, command.task.0.clone()))
                .copied()
            {
                Some(task) => task,
                None => return ResumeTaskOutcomeWire::UnknownRef,
            }
        };
        if command.instruction.trim().is_empty() {
            return ResumeTaskOutcomeWire::NeedsRevalidation {
                hold: hold_name(TaskResumeHold::InstructionUnavailable).to_string(),
            };
        }
        let Some(purpose) = decode_purpose(&command.expected_purpose) else {
            return ResumeTaskOutcomeWire::UnknownRef;
        };
        if purpose.task != task {
            // The purpose identity names another Task: stale against this
            // one, never rebound.
            return match self.store.load_task(task).await {
                Ok(Some(record)) => ResumeTaskOutcomeWire::StalePremise {
                    current_revision: record.task.reference.revision.as_u64(),
                },
                Ok(None) => ResumeTaskOutcomeWire::MissingTask,
                Err(_) => ResumeTaskOutcomeWire::Unavailable,
            };
        }
        let record = match self.store.load_task(task).await {
            Ok(Some(record)) => record,
            Ok(None) => return ResumeTaskOutcomeWire::MissingTask,
            Err(_) => return ResumeTaskOutcomeWire::Unavailable,
        };
        // The activity is the first-party instruction record the resume
        // commit resolves (same shape as the management inlet; idempotent by
        // command id: a retry observes the same activity).
        let command_raw = frame
            .envelope
            .correlation
            .command_id
            .map(|id| id.0)
            .unwrap_or_else(Uuid::new_v4);
        let activity = match self
            .store
            .record_resume_activity(RecordResumeActivityCommand {
                companion: CompanionId::from_raw(record.task.assignee.companion),
                task: record.task.reference,
                purpose: record.task.purpose,
                body: command.instruction.clone(),
                command: RawId::from_uuid(command_raw),
            })
            .await
        {
            Ok(activity) => activity,
            Err(_) => return ResumeTaskOutcomeWire::Unavailable,
        };
        let outcome = match self
            .resume_task(ResumeTaskCommand {
                premise: SteeringPremiseRef {
                    expected: TaskRef {
                        task,
                        revision: TaskRevision::from_u64(command.expected_revision),
                    },
                    purpose,
                },
                instruction: ResumeInstructionSource::OwnerManagement {
                    activity: activity.as_raw(),
                },
            })
            .await
        {
            Ok(outcome) => outcome,
            Err(_) => return ResumeTaskOutcomeWire::Unavailable,
        };
        map_resume_outcome(&command.task, outcome)
    }

    /// Best-effort auto-present after presence establishment (recovery or
    /// summon): no Owner query, one bounded summary, silence when empty.
    /// At most one summary frame; the caller emits it non-blocking.
    pub(crate) async fn auto_present_for(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        companion: CompanionId,
        attribution: &PresenceAttribution,
    ) -> Vec<WireFrame> {
        let Some(device_wire) = live.paired_device.clone() else {
            return Vec::new();
        };
        let client = device_client(&device_wire);
        if attribution.state != PresenceState::Present
            || attribution.active_client != Some(client)
            || !live.connection_live
        {
            return Vec::new();
        }
        let _gate = self.presentation_gate().await;
        let conn = conn_key(&live.connection_id);
        let incarnation = (
            frame.envelope.sender.incarnation_id.counter,
            frame.envelope.sender.incarnation_id.random,
        );
        let bound = self.store.undelivered_pass_bound().await.unwrap_or(0);
        {
            let mut state = crate::lock_unpoison(&self.presentations);
            let sub = state.subs.entry(conn.clone()).or_insert(Subscription {
                companion: companion.as_raw(),
                scan_floor: 0,
                drained_once: false,
                resume: None,
            });
            sub.companion = companion.as_raw();
            // A fresh presence starts a new pass: rewind to the head so the
            // absence backlog presents without an Owner query.
            sub.scan_floor = 0;
            sub.resume = None;
        }
        match self
            .begin_auto(&conn, incarnation, client, companion, attribution, bound)
            .await
        {
            Some(summary) if !summary.items.is_empty() => vec![outgoing_fact(
                frame,
                live,
                WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(summary)),
            )],
            _ => Vec::new(),
        }
    }

    /// Auto-present begin: explicit head pass, best-effort; silence on any
    /// refusal or oversize (the explicit request path stays authoritative).
    async fn begin_auto(
        &self,
        conn: &str,
        incarnation: (u64, u64),
        client: ClientId,
        companion: CompanionId,
        attribution: &PresenceAttribution,
        upper: u64,
    ) -> Option<UndeliveredSummary> {
        let page = self
            .store
            .list_unpresented(companion, None, UNDELIVERED_PAGE_MAX)
            .await
            .ok()?;
        if page.entries.is_empty() {
            self.mark_drained(conn, companion, upper);
            return None;
        }
        let generation = attribution.generation.as_u64();
        Self::sweep_carried(&mut crate::lock_unpoison(&self.presentations), conn);
        let all = self.carry_items(conn, &page.entries).await;
        // Fit the frame cap on the prefix: the slimmed-off tail keeps no
        // refs (a later pass re-registers it), so refs never outlive their
        // receipt.
        let mut keep = all.len();
        let budget = self.frame_budget();
        while keep > 0 && estimate_summary_bytes(&all[..keep]) > budget {
            keep -= 1;
        }
        if keep == 0 {
            self.forget_carried(conn, &all).await;
            return None;
        }
        self.forget_carried(conn, &all[keep..]).await;
        let slim = all[..keep].to_vec();
        let round = ene_presentation::RoundId::from_raw(RawId::new());
        let round_wire = self.round_wire_or_mint(&round);
        let mark = ene_companion::PresentationMark {
            round: round.as_raw(),
            presented: false,
        };
        let mut selected = Vec::new();
        let wanted: HashSet<UndeliveredId> = {
            let state = crate::lock_unpoison(&self.presentations);
            slim.iter()
                .filter_map(|item| {
                    state
                        .carried
                        .get(&(conn.to_string(), item.reference.0.clone()))
                        .map(|(id, _)| *id)
                })
                .collect()
        };
        for entry in &page.entries {
            if wanted.contains(&entry.id) {
                if entry.status == ReportStatus::Pending
                    && self
                        .store
                        .compare_and_mark_reported(entry.id, ReportStatus::Pending, mark)
                        .await
                        .is_err()
                {
                    continue;
                }
                selected.push(entry.id);
            }
        }
        if selected.is_empty() {
            return None;
        }
        let receipt_id = Uuid::new_v4().as_hyphenated().to_string();
        let companion_key = companion.as_raw().as_uuid().as_hyphenated().to_string();
        let receipt = Receipt {
            id: receipt_id.clone(),
            connection: conn.to_string(),
            incarnation,
            companion: companion.as_raw(),
            client,
            round,
            round_wire: round_wire.0.clone(),
            generation,
            selected,
            expires_at: Instant::now() + RECEIPT_TTL,
        };
        {
            let mut state = crate::lock_unpoison(&self.presentations);
            if let Some(old) = state.receipts.insert(companion_key.clone(), receipt) {
                state.receipt_ids.remove(&old.id);
                Self::retire(&mut state, &old.id, &old.connection);
            }
            state.receipt_ids.insert(receipt_id.clone(), companion_key);
            if page.next.is_none()
                && let Some(sub) = state.subs.get_mut(conn)
            {
                sub.scan_floor = sub.scan_floor.max(upper);
            }
        }
        let mut summary = UndeliveredSummary {
            receipt: PresentationReceiptWireRef(receipt_id),
            round: round_wire,
            presence_generation: generation,
            items: slim,
            reports: Vec::new(),
            has_more: page.next.is_some() || keep < page.entries.len(),
            next_cursor: None,
        };
        self.attach_reports(conn, &mut summary).await;
        Some(summary)
    }

    /// Test-only: force-expire one receipt so ACK-loss advance is
    /// deterministic without sleeping past the 30 s TTL.
    #[cfg(test)]
    pub(crate) fn expire_receipt_for_test(&self, receipt: &str) -> bool {
        let mut state = crate::lock_unpoison(&self.presentations);
        let Some(companion_key) = state.receipt_ids.get(receipt).cloned() else {
            return false;
        };
        match state.receipts.get_mut(&companion_key) {
            Some(entry) => {
                entry.expires_at = Instant::now() - Duration::from_secs(1);
                true
            }
            None => false,
        }
    }
}

/// One pass plan for a cursor-less request.
#[derive(Debug, Clone, Copy)]
enum PlanStart {
    Continued {
        cursor: UndeliveredCursor,
        pending_only: bool,
        limit: u32,
    },
    Arrivals {
        after: u64,
        upper: u64,
    },
    Explicit {
        upper: u64,
    },
}

/// One wire resume application: a decided outcome or a content conflict
/// under a reused id (refused without side effects at dispatch).
#[derive(Debug, Clone)]
enum ResumeApply {
    Outcome(ResumeTaskOutcomeWire),
    Conflict,
}

fn estimate_summary_bytes(items: &[UndeliveredItemView]) -> usize {
    items
        .iter()
        .map(|item| item.excerpt.len() + PER_ITEM_OVERHEAD)
        .sum::<usize>()
        + 1024
}

fn map_resume_outcome(task: &TaskWireRef, outcome: TaskResumeOutcome) -> ResumeTaskOutcomeWire {
    match outcome {
        TaskResumeOutcome::Resumed {
            task: current,
            delegation,
        } => ResumeTaskOutcomeWire::Resumed {
            task: task.clone(),
            revision: current.revision.as_u64(),
            delegation: delegation
                .delegation
                .as_raw()
                .as_uuid()
                .as_hyphenated()
                .to_string(),
        },
        TaskResumeOutcome::StalePremise { current } => ResumeTaskOutcomeWire::StalePremise {
            current_revision: current.revision.as_u64(),
        },
        TaskResumeOutcome::Superseded => ResumeTaskOutcomeWire::Superseded,
        TaskResumeOutcome::TaskTerminal { progress, .. } => ResumeTaskOutcomeWire::TaskTerminal {
            progress: progress.as_str().to_string(),
        },
        TaskResumeOutcome::AlreadyRunning { .. } => ResumeTaskOutcomeWire::AlreadyRunning,
        TaskResumeOutcome::HeldByUnknownEffects { .. } => {
            ResumeTaskOutcomeWire::HeldByUnknownEffects
        }
        TaskResumeOutcome::ResultAvailable { .. } => ResumeTaskOutcomeWire::ResultAvailable,
        TaskResumeOutcome::NeedsRevalidation(hold) => ResumeTaskOutcomeWire::NeedsRevalidation {
            hold: hold_name(hold).to_string(),
        },
        TaskResumeOutcome::MissingTask { .. } => ResumeTaskOutcomeWire::MissingTask,
        TaskResumeOutcome::RevisionExhausted { .. } => ResumeTaskOutcomeWire::RevisionExhausted,
    }
}
