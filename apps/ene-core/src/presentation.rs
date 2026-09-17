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
//!
//! Replacement lifecycle (CCT §10.4, IPC §9.3): every connection-scoped
//! entry here — subscription, receipt, Task/source/item ref, cursor, resume
//! retry slot — is installed under the connection-ownership section
//! (`HostHandle::with_current_connection` via `with_presentation_state`),
//! and the Host's single lifecycle boundary sweeps the whole world on
//! supersession and close. A stale operation therefore either commits before
//! the sweep (and is swept with its connection) or is refused with a typed
//! stale rejection; it can never recreate state for a superseded connection
//! or disturb the replacement's own entries.

use std::collections::HashMap;
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
    CompanionId, CompanionRepository, RecordResumeActivityCommand, ReportStatus, TaskFact,
    UndeliveredCursor, UndeliveredId, UndeliveredRef, UndeliveredRepository, UndeliveredSource,
};
use ene_plugin_ipc::WireFrame;
use ene_presence::{ClientId, PresenceAttribution, PresenceRepository, PresenceState};
use ene_primitive::RawId;
use ene_task::{
    SteeringPremiseRef, TaskHeadline, TaskId, TaskPurposeRef, TaskRef, TaskReportRowCursor,
    TaskReportRowKind, TaskReportSourceRef, TaskRepository, TaskResultId, TaskResumeHold,
    TaskResumeOutcome, TaskRevision,
};
use uuid::Uuid;

use crate::serve::{
    HostHandle, LiveInput, connection_key, device_client, outgoing_fact, outgoing_frame,
    reject_frame, stale_reject,
};

#[cfg(test)]
mod tests;

/// Keys per-connection presentation maps by the hyphenated wire form of the
/// id: the same string form the connection layer uses, so keys match across
/// the Host/connection boundary by construction.
fn conn_key(id: &ConnectionWireId) -> String {
    connection_key(id)
}

/// Stale refusal for a Client-dependent operation whose ownership section
/// found the connection superseded, closed, or replaced (IPC §11.3).
fn stale_operation(frame: &WireFrame, live: &LiveInput, detail: &str) -> WireFrame {
    stale_reject(frame, live, detail)
}

/// How long a receipt waits for its ACK (monotonic; IPC §13.3).
const RECEIPT_TTL: Duration = Duration::from_secs(30);

/// How one pass was triggered.
///
/// `Request` is the ordinary cursor-less Client catch-up (continuation, then
/// arrivals, then an explicit head re-display). `Redisplay` forces the head
/// re-display (the Client's `redisplay` flag and a fresh presence/connection
/// auto-present). `Push` is the connection-owned subscription advance: it
/// serves only a stored continuation or new arrivals and never re-displays
/// `Unknown` rows, because a push is not a new presence and not an explicit
/// Client request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PassTrigger {
    Request,
    Redisplay,
    Push,
}

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
    /// Issuing connection key (the epoch's connection component), so the
    /// slot can be dropped with that connection lifetime.
    connection: String,
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
    /// Receipt ACK deadline override (tests pin short TTLs so expiry-driven
    /// advance is deterministic; production keeps [`RECEIPT_TTL`]).
    receipt_ttl: Duration,
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
            receipt_ttl: RECEIPT_TTL,
        }
    }
}

/// Bound on remembered retired receipt ids: old receipts stay answerable as
/// stale without growing memory with the connection count.
const RETIRED_RECEIPT_CAP: usize = 64;

/// Opaque purpose identity echoed by the Client: `{task}:{adopted_revision}`.
///
/// Takes the stored [`TaskPurposeRef`] itself, never a bare revision: the
/// identity names the revision that adopted the purpose, which is not the
/// Task's current revision after a purpose-preserving steering or resume.
fn encode_purpose(purpose: TaskPurposeRef) -> String {
    format!(
        "{}:{}",
        purpose.task.as_raw().as_uuid().as_hyphenated(),
        purpose.adopted_revision.as_u64()
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
    pub(crate) async fn presentation_gate(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.presentation_lock.lock().await
    }

    /// Runs one short presentation-state mutation under the connection
    /// ownership section (CCT §10.4).
    ///
    /// The lock order is connection table → presentation memory lock; the
    /// closure is synchronous (no await while either lock is held) and returns
    /// [`None`] — mutating nothing — when the connection is no longer current.
    /// Every connection-scoped ref, cursor, carried item, subscription,
    /// receipt, and retry-slot install goes through here, so a superseded or
    /// closed connection cannot recreate state after the lifecycle sweep.
    fn with_presentation_state<R>(
        &self,
        live: &LiveInput,
        install: impl FnOnce(&mut PresentationState) -> R,
    ) -> Option<R> {
        self.with_current_connection(live, || {
            let mut state = crate::lock_unpoison(&self.presentations);
            install(&mut state)
        })
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
        match response {
            Some(response) => vec![outgoing_frame(
                frame,
                live,
                WirePayload::UndeliveredResponse(response),
            )],
            // The ownership section refused the install: this connection was
            // superseded or closed while the page was prepared, so no
            // receipt, ref, cursor, or subscription may be created for it.
            None => vec![stale_operation(
                frame,
                live,
                "undelivered request on a superseded connection",
            )],
        }
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
    ) -> Option<UndeliveredResponse> {
        let companion = match companion {
            Some(wire) => match self.resolve_companion(&wire.0).await {
                Err(_) => return Some(UndeliveredResponse::Summary(self.empty_shell())),
                Ok(None) => return Some(UndeliveredResponse::UnknownCompanion),
                Ok(Some(companion)) => companion,
            },
            None => match self.store.ensure_running_companion().await {
                Err(_) => return Some(UndeliveredResponse::Summary(self.empty_shell())),
                Ok(companion) => companion,
            },
        };
        // Companion summary presentation needs formal presence: Present for
        // this connection's client. NoActive reconnects keep management
        // reads; talk needs a fresh summon.
        let attribution = match self.store.load_attribution(companion.as_raw()).await {
            Ok(Some(attribution)) => attribution,
            _ => return Some(UndeliveredResponse::NoCurrentPresence),
        };
        let Some(device_wire) = live.paired_device.clone() else {
            return Some(UndeliveredResponse::NoCurrentPresence);
        };
        let client = device_client(&device_wire);
        if attribution.state != PresenceState::Present
            || attribution.active_client != Some(client)
            || !live.connection_live
        {
            return Some(UndeliveredResponse::NoCurrentPresence);
        }
        // Test-only race gate: pause before the transition lock so a test can
        // replace the connection and let the replacement run a full pass.
        #[cfg(test)]
        if let Some(gate) = self.fetch_gate() {
            gate.pause().await;
        }
        let _gate = self.presentation_gate().await;
        let conn = conn_key(&live.connection_id);
        let incarnation = (
            frame.envelope.sender.incarnation_id.counter,
            frame.envelope.sender.incarnation_id.random,
        );
        let companion_key = companion.as_raw().as_uuid().as_hyphenated().to_string();
        // A live receipt on this connection re-displays its own selection:
        // duplicate display without a new commit or cursor move. The
        // requested limit never shrinks a re-emit: the receipt and the item
        // set its ACK covers must stay identical (IPC §13.3).
        if cursor.is_none()
            && let Some(receipt) = self.live_receipt(&companion_key, &conn)
        {
            return self.reemit_receipt(live, &conn, &receipt).await;
        }
        // A foreign cursor is StaleBaseView, never a silent restart.
        if let Some(cursor) = cursor
            && !self.valid_undelivered_cursor(&conn, companion, cursor)
        {
            return Some(UndeliveredResponse::StaleBaseView { current: None });
        }
        self.begin_pass(
            live,
            &conn,
            incarnation,
            client,
            companion,
            &attribution,
            cursor,
            limit,
            if redisplay {
                PassTrigger::Redisplay
            } else {
                PassTrigger::Request
            },
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
    /// no cursor move. The item set is rehydrated from the receipt's exact
    /// selected identities in their original order — never by re-scanning
    /// the unpresented head, so rows before the selection, later backlog,
    /// and continuation position cannot shrink or empty it. Over budget now
    /// (excerpts only shrink) answers `FrameTooLarge` with the receipt
    /// standing.
    ///
    /// A selected id that no longer resolves at all (deleted or foreign)
    /// means the receipt can no longer cover the set its ACK names: the
    /// receipt is retired and the request answers `StaleBaseView` so the
    /// Client re-queries for a fresh page. A store failure fails closed the
    /// same way. A selected row that is already `Presented` cannot be
    /// re-painted and its ACK is already satisfied, so it is pruned from the
    /// receipt's selection as it is omitted from the frame: the receipt then
    /// covers exactly the delivered items.
    async fn reemit_receipt(
        &self,
        live: &LiveInput,
        conn: &str,
        receipt: &Receipt,
    ) -> Option<UndeliveredResponse> {
        let selected = receipt.selected.clone();
        let loaded = match self
            .store
            .load_undelivered_by_ids(CompanionId::from_raw(receipt.companion), &selected)
            .await
        {
            Ok(loaded) => loaded,
            Err(_) => return Some(self.retire_unrehydratable_receipt(receipt)),
        };
        // Exact one-to-one resolution: every selected identity must resolve,
        // in the original selection order. Anything else is a receipt whose
        // ACK would claim rows that no longer exist.
        if loaded.len() != selected.len()
            || loaded
                .iter()
                .zip(&selected)
                .any(|(entry, id)| entry.id != *id)
        {
            return Some(self.retire_unrehydratable_receipt(receipt));
        }
        // Already-presented ids are omitted from the frame and pruned from
        // the live receipt, so its selection stays exactly what the ACK acts
        // on. The presentation gate is held across this rehydration, so the
        // receipt cannot be replaced underneath the prune.
        let mut carried = Vec::with_capacity(loaded.len());
        let mut satisfied: Vec<UndeliveredId> = Vec::new();
        for entry in loaded {
            if entry.status == ReportStatus::Presented {
                satisfied.push(entry.id);
            } else {
                carried.push(entry);
            }
        }
        // Drop the previous attempt's refs, then install this re-emit's refs
        // under the ownership section: a supersession that wins the section
        // installs nothing, and a sweep that already ran cannot be undone by
        // a late re-registration.
        Self::sweep_carried(&mut crate::lock_unpoison(&self.presentations), conn);
        let items = self.carry_items(live, conn, &carried).await?;
        let companion_key = receipt.companion.as_uuid().as_hyphenated().to_string();
        let pruned = self
            .with_presentation_state(live, |state| {
                if !satisfied.is_empty()
                    && let Some(live_receipt) = state.receipts.get_mut(&companion_key)
                    && live_receipt.id == receipt.id
                {
                    live_receipt.selected.retain(|id| !satisfied.contains(id));
                }
            })
            .is_some();
        if !pruned {
            self.forget_carried(conn, &items).await;
            return None;
        }
        if items.is_empty() && !carried.is_empty() {
            return Some(UndeliveredResponse::FrameTooLarge);
        }
        if estimate_summary_bytes(&items) > self.frame_budget() {
            return Some(UndeliveredResponse::FrameTooLarge);
        }
        let mut summary = self.receipt_shell(receipt, items, true);
        if !self.attach_reports(live, conn, &mut summary).await {
            return None;
        }
        Some(UndeliveredResponse::Summary(summary))
    }

    /// Retires a receipt whose selection cannot be exactly rehydrated and
    /// answers the stale base view.
    ///
    /// The durable rows are untouched: whatever is still unpresented keeps
    /// its status and re-presents on the Client's next head pass. Retiring
    /// the id keeps a late ACK answerable as stale instead of unknown.
    fn retire_unrehydratable_receipt(&self, receipt: &Receipt) -> UndeliveredResponse {
        let mut state = crate::lock_unpoison(&self.presentations);
        let companion_key = receipt.companion.as_uuid().as_hyphenated().to_string();
        if state
            .receipts
            .get(&companion_key)
            .is_some_and(|live| live.id == receipt.id)
        {
            Self::remove_receipt(&mut state, &companion_key);
        }
        UndeliveredResponse::StaleBaseView { current: None }
    }

    /// Begins or continues one pass: fetches the longest fitting prefix of
    /// one bounded fetch, commits it, installs the receipt.
    ///
    /// Returns `None` when the connection-ownership section refused the
    /// install: the connection was superseded or closed while the page was
    /// prepared, so nothing is created for it.
    #[expect(
        clippy::too_many_arguments,
        reason = "pass state is intentionally explicit"
    )]
    async fn begin_pass(
        &self,
        live: &LiveInput,
        conn: &str,
        incarnation: (u64, u64),
        client: ClientId,
        companion: CompanionId,
        attribution: &PresenceAttribution,
        cursor: Option<&PageCursorWire>,
        limit: u32,
        trigger: PassTrigger,
    ) -> Option<UndeliveredResponse> {
        let from_cursor = cursor.map(|cursor| cursor.0.as_str());
        // Resolve the fetch window: continue a stored pass, catch up on new
        // arrivals, or rewind to the head for an explicit pass. The plan
        // installs this connection's subscription, so it runs under the
        // ownership section; a superseded connection plans nothing.
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
                _ => return Some(UndeliveredResponse::StaleBaseView { current: None }),
            }
        } else {
            // A newer connection supersedes a live receipt from a dead one:
            // old rows stay Unknown and re-present under the new receipt.
            let bound = self.store.undelivered_pass_bound().await.unwrap_or(0);
            let planned = self.with_presentation_state(live, |state| {
                let ckey = companion.as_raw().as_uuid().as_hyphenated().to_string();
                if let Some(receipt) = state.receipts.get(&ckey).cloned()
                    && (receipt.connection != conn || receipt.expired())
                {
                    Self::remove_receipt(state, &ckey);
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
                // explicit head re-display. `Redisplay` forces the head pass;
                // `Push` serves only a continuation or new arrivals and stays
                // silent otherwise (a push is never a new presence). The resume
                // binding is checked before the subscription is re-pointed at
                // this companion.
                let continuation = (trigger != PassTrigger::Redisplay)
                    .then_some(sub.resume)
                    .flatten()
                    .filter(|_| sub.companion == companion.as_raw());
                let arrivals = trigger != PassTrigger::Redisplay
                    && sub.drained_once
                    && sub.scan_floor < bound
                    && sub.companion == companion.as_raw();
                let plan = if let Some((cursor, pending_only, saved)) = continuation {
                    Some(PlanStart::Continued {
                        cursor,
                        pending_only,
                        limit: saved,
                    })
                } else if arrivals {
                    Some(PlanStart::Arrivals {
                        after: sub.scan_floor,
                        upper: bound,
                    })
                } else if trigger == PassTrigger::Push {
                    None
                } else {
                    Some(PlanStart::Explicit { upper: bound })
                };
                sub.companion = companion.as_raw();
                plan
            })?;
            let Some(plan) = planned else {
                return Some(UndeliveredResponse::Summary(
                    self.empty_attributed(attribution),
                ));
            };
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
                    return Some(UndeliveredResponse::Summary(
                        self.empty_attributed(attribution),
                    ));
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
            let items = self.carry_items(live, conn, &entries).await?;
            if estimate_summary_bytes(&items) <= budget {
                break (entries, items, page.next, upper, pending_only);
            }
            if fetch_limit <= 1 {
                // Even one item overflows the cap: withhold without mutating
                // any row, cursor, subscription, or receipt.
                self.forget_carried(conn, &items).await;
                return Some(UndeliveredResponse::FrameTooLarge);
            }
            fetch_limit /= 2;
            self.forget_carried(conn, &items).await;
        };
        let (entries, items, fetched_next, upper, pending_only) = fitted;
        if entries.is_empty() {
            // A drained continued pass falls through to an explicit head
            // re-display in the same response instead of stranding failed
            // rows behind an empty arrival check. A push never does: it
            // must not re-display Unknown rows without an explicit request
            // or a new presence.
            if matches!(start, PlanStart::Continued { .. }) && trigger != PassTrigger::Push {
                return Box::pin(self.begin_pass(
                    live,
                    conn,
                    incarnation,
                    client,
                    companion,
                    attribution,
                    None,
                    limit,
                    PassTrigger::Redisplay,
                ))
                .await;
            }
            if fetched_next.is_none() && !self.mark_drained(live, conn, companion, upper) {
                return None;
            }
            let mut summary = self.empty_attributed(attribution);
            // A filtered-empty page with more waiting still pages next.
            if let Some(next) = fetched_next {
                let installed = self
                    .with_presentation_state(live, |state| {
                        Self::take_cursor(state, conn, from_cursor);
                        summary.has_more = true;
                        if let Some(sub) = state.subs.get_mut(conn) {
                            sub.resume = Some((next, pending_only, pass_limit));
                        }
                        summary.next_cursor = Some(Self::mint_cursor(
                            state,
                            conn,
                            StoredCursor::Undelivered {
                                companion: companion.as_raw(),
                                cursor: next,
                                pending_only,
                                limit: pass_limit,
                            },
                        ));
                    })
                    .is_some();
                if !installed {
                    return None;
                }
            }
            return Some(UndeliveredResponse::Summary(summary));
        }
        self.commit_install(
            live,
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
    ///
    /// The whole commit — the per-row presentation-start CAS and the
    /// receipt/cursor/subscription install — runs inside one
    /// connection-ownership section (CCT §10.4): a replacement that wins the
    /// table commits no row transition and installs no receipt, and a
    /// commit that wins the table survives the later lifecycle sweep as a
    /// durable row (CCT §10.5). Returns `None` when the ownership section
    /// refused: no receipt, cursor, or subscription entry is created for a
    /// superseded connection, and the attempt's carried refs are dropped.
    #[expect(
        clippy::too_many_arguments,
        reason = "commit state is intentionally explicit"
    )]
    async fn commit_install(
        &self,
        live: &LiveInput,
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
    ) -> Option<UndeliveredResponse> {
        let companion_key = companion.as_raw().as_uuid().as_hyphenated().to_string();
        let generation = attribution.generation.as_u64();
        // Test-only: pause after the page plan and before the per-row
        // presentation-start compares so a test can move a row's durable
        // status and pin the domain CAS-loss handling.
        #[cfg(test)]
        {
            let gate = crate::lock_unpoison(&self.presentation_commit_gate).clone();
            if let Some(gate) = gate {
                gate.pause().await;
            }
        }
        let round = ene_presentation::RoundId::from_raw(RawId::new());
        let mark = ene_companion::PresentationMark {
            round: round.as_raw(),
            presented: false,
        };
        let store = self.store.clone();
        let presentations = std::sync::Arc::clone(&self.presentations);
        let rounds = std::sync::Arc::clone(&self.rounds);
        let commit_conn = conn.to_string();
        let from_cursor = from_cursor.map(str::to_string);
        let installed = self
            .with_current_connection_blocking(live, move || {
                let mut state_guard = crate::lock_unpoison(&presentations);
                let state = &mut *state_guard;
                let conn = commit_conn.as_str();
                let from_cursor = from_cursor.as_deref();
                let mut selected = Vec::with_capacity(entries.len());
                let mut carried = Vec::with_capacity(items.len());
                let mut dropped = Vec::new();
                for (entry, item) in entries.into_iter().zip(items) {
                    // Only a committed presentation start claims the row. A domain
                    // `StaleSource` is not an infrastructure error: the row's status
                    // moved between the page plan and this compare (another
                    // receipt/pass owns it, it was presented, or it is gone), so the
                    // row must stay out of the receipt and the frame and be left to
                    // the next pass. An `Err` is a rolled-back compare (the row was
                    // not marked) and takes the same drop; any other transition is
                    // not this call's success either.
                    if entry.status != ReportStatus::Pending {
                        // Already `PresentationUnknown`: re-displayed under this
                        // receipt without a status write, so it is claimed.
                        selected.push(entry.id);
                        carried.push(item);
                        continue;
                    }
                    match store.compare_and_mark_reported_sync(
                        entry.id,
                        ReportStatus::Pending,
                        mark,
                    ) {
                        Ok(ene_companion::ReportStatusTransition::MarkedPresentationUnknown) => {
                            selected.push(entry.id);
                            carried.push(item);
                        }
                        Ok(ene_companion::ReportStatusTransition::StaleSource) => {
                            dropped.push(item);
                        }
                        Ok(_) => {
                            // expected=Pending + presented=false can only commit
                            // `MarkedPresentationUnknown`; anything else (for
                            // example an already-presented row) is not a
                            // presentation start and is never claimed.
                            dropped.push(item);
                        }
                        Err(_) => {
                            // One atomic per-row compare: the failure rolled back,
                            // so the row was not marked and stays for the next pass.
                            dropped.push(item);
                        }
                    }
                }
                for item in dropped {
                    state.carried.remove(&(conn.to_string(), item.reference.0));
                }
                let receipt_id = Uuid::new_v4().as_hyphenated().to_string();
                let ttl = state.receipt_ttl;
                // The carried prefix IS the fetched prefix (the fetch bound shrank
                // instead), so the store cursor resumes exactly: a fetched
                // continuation pages next, a bound-reaching fetch drains the floor.
                // The round projection is minted inside the section: a stale
                // attempt leaves no round wire behind either.
                let round_wire = RoundWireId(RawId::new().as_uuid().to_string());
                crate::lock_unpoison(&rounds).insert(round_wire.0.clone(), round);
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
                    expires_at: Instant::now() + ttl,
                };
                // Forward-only paging consumes the cursor it continued from.
                Self::take_cursor(state, conn, from_cursor);
                // One receipt per Companion: installing supersedes any leftover.
                if let Some(old) = state.receipts.insert(companion_key.clone(), receipt) {
                    state.receipt_ids.remove(&old.id);
                    Self::retire(state, &old.id, &old.connection);
                }
                state.receipt_ids.insert(receipt_id.clone(), companion_key);
                let (next_cursor, drained) = match fetched_next {
                    Some(next) => {
                        let resume = (next, pending_only, limit);
                        if let Some(sub) = state.subs.get_mut(conn) {
                            sub.resume = Some(resume);
                        }
                        (
                            Some(Self::mint_cursor(
                                state,
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
                };
                (round_wire, next_cursor, drained, receipt_id, carried)
            })
            .await;
        let (round_wire, next_cursor, drained, receipt_id, carried) = installed?;
        let has_more =
            !drained || self.store.undelivered_pass_bound().await.unwrap_or(upper) > upper;
        let mut summary = UndeliveredSummary {
            receipt: PresentationReceiptWireRef(receipt_id),
            round: round_wire,
            presence_generation: generation,
            items: carried,
            reports: Vec::new(),
            has_more,
            next_cursor,
        };
        if !self.attach_reports(live, conn, &mut summary).await {
            return None;
        }
        Some(UndeliveredResponse::Summary(summary))
    }

    /// Builds carried items (with bounded excerpts) for entries in order,
    /// registering per-connection item refs under the ownership section.
    /// Unreadable excerpts keep their correlation with an empty excerpt; the
    /// body stays pageable and nothing is marked presented.
    ///
    /// Returns `None` when the connection was superseded before the refs
    /// were installed: no ref is registered for it.
    async fn carry_items(
        &self,
        live: &LiveInput,
        conn: &str,
        entries: &[UndeliveredRef],
    ) -> Option<Vec<UndeliveredItemView>> {
        // Load the excerpts first (no state touched), then install every ref
        // in one ownership section.
        let mut loaded = Vec::with_capacity(entries.len());
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
            loaded.push((
                entry.id,
                task_behind_source(&entry.source),
                source_view(&entry.source),
                excerpt,
                truncated,
            ));
        }
        self.with_presentation_state(live, |state| {
            loaded
                .into_iter()
                .map(|(id, task, source, excerpt, truncated)| {
                    let wire = Uuid::new_v4().as_hyphenated().to_string();
                    state
                        .carried
                        .insert((conn.to_string(), wire.clone()), (id, task));
                    UndeliveredItemView {
                        reference: UndeliveredWireRef(wire),
                        source,
                        excerpt,
                        truncated,
                    }
                })
                .collect()
        })
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
    ///
    /// Returns `false` when the connection was superseded before the Task
    /// refs were installed: the caller abandons the pass instead of handing
    /// out refs that no longer exist.
    async fn attach_reports(
        &self,
        live: &LiveInput,
        conn: &str,
        summary: &mut UndeliveredSummary,
    ) -> bool {
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
        let mut loaded = Vec::new();
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
            loaded.push((
                task,
                record.task.reference.revision.as_u64(),
                record.task.progress.as_str().to_string(),
                details,
            ));
        }
        let installed = self.with_presentation_state(live, |state| {
            loaded
                .into_iter()
                .map(|(task, revision, progress, details_available)| {
                    let task_ref = Self::mint_task_ref(state, conn, task);
                    TaskReportView {
                        task: task_ref,
                        revision,
                        progress,
                        details_available,
                    }
                })
                .collect::<Vec<_>>()
        });
        match installed {
            Some(views) => {
                summary.reports = views;
                true
            }
            None => false,
        }
    }

    /// Records one drained scan floor under the ownership section. Returns
    /// `false` when the connection is no longer current.
    fn mark_drained(
        &self,
        live: &LiveInput,
        conn: &str,
        companion: CompanionId,
        upper: u64,
    ) -> bool {
        self.with_presentation_state(live, |state| {
            if let Some(sub) = state.subs.get_mut(conn)
                && sub.companion == companion.as_raw()
            {
                sub.scan_floor = sub.scan_floor.max(upper);
                sub.drained_once = true;
                sub.resume = None;
            }
        })
        .is_some()
    }

    /// Agreed frame cap in force (tests pin small caps; production keeps the
    /// default until negotiation carries client limits).
    fn frame_budget(&self) -> usize {
        crate::lock_unpoison(&self.presentations).frame_budget
    }

    /// Test-only: pin the receipt ACK deadline so expiry-driven advance is
    /// deterministic without waiting the production 30 s.
    ///
    /// Unix-gated with the socket-loop subscription tests that use it; the
    /// Windows lib test build would otherwise see it as dead code under the
    /// warnings-as-errors configuration.
    #[cfg(all(test, unix))]
    pub(crate) fn set_receipt_ttl_for_test(&self, ttl: Duration) {
        crate::lock_unpoison(&self.presentations).receipt_ttl = ttl;
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
    /// The whole verdict and its durable marks run inside one
    /// connection-ownership section (CCT §10.4–10.5): the receipt consume is
    /// the ACK's linearization point, and the bounded per-row store compares
    /// commit with the same currentness guarantee, so a supersession either
    /// precedes the whole section (nothing consumed, nothing marked) or
    /// follows the accepted observation (rows keep the ACK's decision while
    /// the old connection's memory world is swept). The guard never crosses
    /// an await: every store call here is the sync primitive.
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
        // The whole validation/consumption runs under the ownership section
        // (CCT §10.4): a connection superseded before the section refuses with
        // the typed stale outcome and consumes nothing, so an in-flight ACK
        // cannot release or move rows after the lifecycle sweep.
        let presentations = std::sync::Arc::clone(&self.presentations);
        let store = self.store.clone();
        let paired_client = live.paired_device.as_deref().map(device_client);
        let ack = ack.clone();
        let applied = self
            .with_current_connection_blocking(live, move || {
                let verdict = (|| {
                    let mut state = crate::lock_unpoison(&presentations);
                    let Some(companion_key) = state.receipt_ids.get(&ack.receipt.0).cloned() else {
                        // Consumed or superseded receipts stay stale (never silently
                        // unknown); never-issued ids are unknown. A foreign
                        // connection still hears StaleConnection first: ACKs never
                        // migrate.
                        return match state
                            .retired
                            .iter()
                            .find(|(known, _)| known == &ack.receipt.0)
                        {
                            Some((_, issued)) if *issued != conn => {
                                Verdict::Refuse(UndeliveredAckOutcome::StaleConnection)
                            }
                            Some(_) => Verdict::Refuse(UndeliveredAckOutcome::StalePresentation),
                            None => Verdict::Refuse(UndeliveredAckOutcome::UnknownRef),
                        };
                    };
                    let Some(receipt) = state.receipts.get(&companion_key).cloned() else {
                        return Verdict::Refuse(UndeliveredAckOutcome::UnknownRef);
                    };
                    if receipt.expired() {
                        Self::remove_receipt(&mut state, &companion_key);
                        Verdict::Refuse(UndeliveredAckOutcome::StalePresentation)
                    } else if receipt.connection != conn || receipt.incarnation != incarnation {
                        // The ACK never migrates across connections or incarnations.
                        Verdict::Refuse(UndeliveredAckOutcome::StaleConnection)
                    } else if paired_client != Some(receipt.client) {
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
                })();
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
                            if let Ok(ene_companion::ReportStatusTransition::PendingToPresented) =
                                store.compare_and_mark_reported_sync(
                                    *id,
                                    ReportStatus::PresentationUnknown,
                                    mark,
                                )
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
                            if let Ok(ene_companion::ReportStatusTransition::FailedToPending) =
                                store.compare_and_mark_reported_sync(
                                    *id,
                                    ReportStatus::PresentationUnknown,
                                    mark,
                                )
                            {
                                count += 1;
                            }
                        }
                        UndeliveredAckOutcome::ReturnedToPending { count }
                    }
                    PresentationStatus::Unknown => UndeliveredAckOutcome::KeptUnknown,
                }
            })
            .await;
        applied.unwrap_or(UndeliveredAckOutcome::StaleConnection)
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
        // Test-only race gate: pause after the durable read and before the
        // guarded mint.
        #[cfg(test)]
        if let Some(gate) = self.ref_mint_gate() {
            gate.pause().await;
        }
        // Ref and cursor mint run under the ownership section: a connection
        // superseded while the page was read creates no refs (IPC §11.3).
        let minted = self.with_presentation_state(live, |state| {
            Self::take_cursor(
                state,
                &conn,
                query.cursor.as_ref().map(|cursor| cursor.0.as_str()),
            );
            let mut tasks = Vec::with_capacity(headlines.len());
            for headline in &headlines {
                tasks.push(TaskListItem {
                    task: Self::mint_task_ref(state, &conn, headline.task),
                    revision: headline.revision.as_u64(),
                    progress: headline.progress.as_str().to_string(),
                    running: self
                        .task_executions
                        .task_has_reservation_or_running(headline.task),
                    purpose: encode_purpose(headline.purpose),
                });
            }
            let next_cursor = if headlines.len() as u32 == limit
                && let Some(last) = headlines.last()
            {
                Some(Self::mint_cursor(
                    state,
                    &conn,
                    StoredCursor::TaskList {
                        after: Some(last.task),
                    },
                ))
            } else {
                None
            };
            (tasks, next_cursor)
        });
        let Some((tasks, next_cursor)) = minted else {
            return vec![stale_operation(
                frame,
                live,
                "task list on a superseded connection",
            )];
        };
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
        // Test-only race gate: pause after the durable read and before the
        // guarded mint.
        #[cfg(test)]
        if let Some(gate) = self.ref_mint_gate() {
            gate.pause().await;
        }
        // Ref and cursor mint run under the ownership section: a connection
        // superseded while the report was read creates no refs.
        let minted = self.with_presentation_state(live, |state| {
            Self::take_cursor(
                state,
                &conn,
                query.cursor.as_ref().map(|cursor| cursor.0.as_str()),
            );
            let purpose_source = Self::mint_source_ref(
                state,
                &conn,
                TaskReportSourceRef::RevisionPurpose {
                    task,
                    revision: record.task.purpose.adopted_revision,
                },
            );
            let mut views = Vec::with_capacity(rows.len());
            for row in &rows {
                let source = match row.kind {
                    TaskReportRowKind::TaskResult => Some(Self::mint_source_ref(
                        state,
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
                    state,
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
            (purpose_source, views, next_cursor)
        });
        let Some((purpose_source, views, next_cursor)) = minted else {
            return vec![stale_operation(
                frame,
                live,
                "task report on a superseded connection",
            )];
        };
        vec![outgoing_frame(
            frame,
            live,
            WirePayload::TaskReportResponse(TaskReportResponse::Page(TaskReportPage {
                task: query.task.clone(),
                revision: record.task.reference.revision.as_u64(),
                progress: record.task.progress.as_str().to_string(),
                purpose: encode_purpose(record.task.purpose),
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
        // Test-only race gate: pause before the guarded selection commit.
        #[cfg(test)]
        if let Some(gate) = self.ref_mint_gate() {
            gate.pause().await;
        }
        // The selection commit runs under the ownership section: a
        // connection superseded while the Task was read selects nothing, and
        // the new connection must select again (IPC §9.3 replacement).
        let selected = self
            .with_current_connection(live, || {
                self.conversation_tasks
                    .select(running, task, live.connection_id);
            })
            .is_some();
        if !selected {
            return vec![stale_operation(
                frame,
                live,
                "task selection on a superseded connection",
            )];
        }
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
                purpose: encode_purpose(record.task.purpose),
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
        let connection = conn_key(&live.connection_id);
        let fingerprint = format!(
            "{}:{}:{}:{}",
            command.task.0,
            command.expected_revision,
            command.expected_purpose,
            command.instruction
        );
        {
            let state = crate::lock_unpoison(&self.presentations);
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
        }
        // Claim the retry slot under the ownership section: a connection
        // superseded while the command was validated claims nothing, so the
        // lifecycle sweep cannot be undone by a late slot re-creation.
        let claimed = self.with_presentation_state(live, |state| {
            state.resume_seq += 1;
            let seq = state.resume_seq;
            state.resume.insert(
                command_id,
                ResumeSlot {
                    epoch,
                    connection,
                    fingerprint,
                    state: ResumeSlotState::InFlight,
                    seq,
                },
            );
            if state.resume.len() > RESUME_COMMAND_CAP {
                // Bounded epoch memory; decided slots drop first,
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
        });
        if claimed.is_none() {
            return ResumeApply::Outcome(ResumeTaskOutcomeWire::StaleConnection);
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
        // The activity record and the AU17 commit run inside the guarded
        // connection section, so a connection superseded before the section
        // leaves no activity row, revision change, delegation, or launch.
        let command_raw = frame
            .envelope
            .correlation
            .command_id
            .map(|id| id.0)
            .unwrap_or_else(Uuid::new_v4);
        let outcome = match self
            .resume_task_guarded_by_connection(
                live,
                SteeringPremiseRef {
                    expected: TaskRef {
                        task,
                        revision: TaskRevision::from_u64(command.expected_revision),
                    },
                    purpose,
                },
                RecordResumeActivityCommand {
                    companion: CompanionId::from_raw(record.task.assignee.companion),
                    task: record.task.reference,
                    purpose: record.task.purpose,
                    body: command.instruction.clone(),
                    command: RawId::from_uuid(command_raw),
                },
            )
            .await
        {
            Ok(Some(outcome)) => outcome,
            // The connection was superseded before the commit section: the
            // typed stale outcome, with nothing written anywhere.
            Ok(None) => return ResumeTaskOutcomeWire::StaleConnection,
            Err(_) => return ResumeTaskOutcomeWire::Unavailable,
        };
        map_resume_outcome(&command.task, outcome)
    }

    /// Best-effort auto-present after presence establishment (recovery or
    /// summon): no Owner query, one bounded summary, silence when empty.
    /// At most one summary frame; the caller emits it non-blocking. The
    /// pass runs through the subscription's ordinary explicit head
    /// re-display, so continuations, scan floors, and receipts stay
    /// consistent with the connection-owned push loop.
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
        // The reset installs this connection's subscription under the
        // ownership section: a superseded connection starts no pass.
        let prepared = self
            .with_presentation_state(live, |state| {
                let sub = state.subs.entry(conn.clone()).or_insert(Subscription {
                    companion: companion.as_raw(),
                    scan_floor: 0,
                    drained_once: false,
                    resume: None,
                });
                // A fresh presence starts a new pass: rewind to the head so the
                // absence backlog (Unknown rows included) presents without an
                // Owner query.
                sub.companion = companion.as_raw();
                sub.scan_floor = 0;
                sub.drained_once = false;
                sub.resume = None;
            })
            .is_some();
        if !prepared {
            return Vec::new();
        }
        let response = self
            .begin_pass(
                live,
                &conn,
                incarnation,
                client,
                companion,
                attribution,
                None,
                DEFAULT_PAGE_LIMIT,
                PassTrigger::Redisplay,
            )
            .await;
        match response {
            Some(UndeliveredResponse::Summary(summary)) if !summary.items.is_empty() => {
                vec![outgoing_fact(
                    frame,
                    live,
                    WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(summary)),
                )]
            }
            _ => Vec::new(),
        }
    }

    /// Drops every memory-only presentation entry owned by one ended
    /// connection.
    ///
    /// Called when the transport closes a connection and when a newer
    /// authentication supersedes it: subscriptions, query-scoped Task /
    /// source refs, page cursors, live receipts, carried item refs, and the
    /// connection's resume retry slots all exist only for that connection
    /// lifetime. Durable state is untouched: a released receipt leaves its
    /// rows `Pending` / `PresentationUnknown`, so a new connection
    /// re-presents them under a fresh receipt, and a retired id keeps a late
    /// ACK answerable as stale instead of unknown.
    pub(crate) fn drop_presentation_connection_state(&self, connection: &ConnectionWireId) {
        let conn = conn_key(connection);
        let mut state = crate::lock_unpoison(&self.presentations);
        state.subs.remove(&conn);
        state.task_refs.retain(|(owner, _), _| owner != &conn);
        state.carried.retain(|(owner, _), _| owner != &conn);
        state.source_refs.retain(|(owner, _), _| owner != &conn);
        state.cursors.retain(|(owner, _), _| owner != &conn);
        state.resume.retain(|_, slot| slot.connection != conn);
        let receipts: Vec<String> = state
            .receipts
            .iter()
            .filter(|(_, receipt)| receipt.connection == conn)
            .map(|(key, _)| key.clone())
            .collect();
        for key in receipts {
            Self::remove_receipt(&mut state, &key);
        }
    }

    /// Releases this connection's expired receipts (memory-only state
    /// progression).
    ///
    /// The connection loop runs this on every receipt deadline before it
    /// decides whether an unsolicited write is still possible: a failed push
    /// write must not leave an expired receipt behind, or the same elapsed
    /// deadline would keep firing forever. Durable rows are untouched:
    /// released rows keep their status and re-present on the next pass.
    pub(crate) fn expire_due_receipts(&self, connection: &ConnectionWireId) {
        #[cfg(all(test, unix))]
        self.receipt_expiry_runs
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let conn = conn_key(connection);
        let mut state = crate::lock_unpoison(&self.presentations);
        let expired: Vec<String> = state
            .receipts
            .iter()
            .filter(|(_, receipt)| receipt.connection == conn && receipt.expired())
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            Self::remove_receipt(&mut state, &key);
        }
    }

    /// One connection-owned subscription advance, driven without an inbound
    /// request (CCT §10.5, IPC §13.3).
    ///
    /// The connection loop calls this after a registration hint or a
    /// receipt deadline. It serves only a stored continuation or new
    /// arrivals — never an explicit head re-display, so `Unknown` rows are
    /// not auto-resent within one subscription. A live receipt waits for
    /// its ACK or deadline; an expired one is released first (even when
    /// presence no longer holds) so the deadline cannot spin immediately.
    /// Every decision re-reads durable state; the wakeup is only a hint.
    ///
    /// Returns at most one unsolicited summary frame, and only when it
    /// carried items.
    pub(crate) async fn push_undelivered(
        &self,
        template: &WireFrame,
        live: &LiveInput,
    ) -> Option<WireFrame> {
        let _gate = self.presentation_gate().await;
        let conn = conn_key(&live.connection_id);
        // Release this connection's expired receipts up front: the caller's
        // deadline already elapsed, and a lingering expired row would wake
        // the loop again immediately.
        self.expire_due_receipts(&live.connection_id);
        if !live.authed || live.phase.is_superseded() || !live.connection_live {
            return None;
        }
        let companion = self.store.ensure_running_companion().await.ok()?;
        let attribution = match self.store.load_attribution(companion.as_raw()).await {
            Ok(Some(attribution)) => attribution,
            _ => return None,
        };
        let device_wire = live.paired_device.clone()?;
        let client = device_client(&device_wire);
        if attribution.state != PresenceState::Present || attribution.active_client != Some(client)
        {
            return None;
        }
        let companion_key = companion.as_raw().as_uuid().as_hyphenated().to_string();
        // A live receipt is the page in flight: its ACK or expiry drives the
        // next step, and pushing it again would only duplicate display.
        if self.live_receipt(&companion_key, &conn).is_some() {
            return None;
        }
        let bound = self.store.undelivered_pass_bound().await.unwrap_or(0);
        let has_plan = {
            let state = crate::lock_unpoison(&self.presentations);
            state.subs.get(&conn).is_some_and(|sub| {
                sub.companion == companion.as_raw()
                    && (sub.resume.is_some() || (sub.drained_once && sub.scan_floor < bound))
            })
        };
        if !has_plan {
            return None;
        }
        let incarnation = (
            template.envelope.sender.incarnation_id.counter,
            template.envelope.sender.incarnation_id.random,
        );
        let response = self
            .begin_pass(
                live,
                &conn,
                incarnation,
                client,
                companion,
                &attribution,
                None,
                DEFAULT_PAGE_LIMIT,
                PassTrigger::Push,
            )
            .await;
        match response {
            Some(UndeliveredResponse::Summary(summary)) if !summary.items.is_empty() => {
                Some(outgoing_fact(
                    template,
                    live,
                    WirePayload::UndeliveredResponse(UndeliveredResponse::Summary(summary)),
                ))
            }
            _ => None,
        }
    }

    /// Earliest receipt deadline on this connection, expired or not.
    ///
    /// The connection loop arms its timer on this value; an already-elapsed
    /// deadline fires immediately and [`Self::expire_due_receipts`] releases
    /// the expired receipt. [`None`] means no receipt is outstanding, so
    /// only a wakeup or inbound frame can advance the subscription.
    pub(crate) fn receipt_deadline_for(&self, connection: &ConnectionWireId) -> Option<Instant> {
        let conn = conn_key(connection);
        let state = crate::lock_unpoison(&self.presentations);
        state
            .receipts
            .values()
            .filter(|receipt| receipt.connection == conn)
            .map(|receipt| receipt.expires_at)
            .min()
    }

    /// Coalesced undelivered-registration hint subscription (CCT §10.5).
    ///
    /// The connection loop subscribes before it starts reading so a
    /// registration committing during the first pass still wakes it.
    #[must_use]
    pub(crate) fn undelivered_wakeup(&self) -> tokio::sync::watch::Receiver<u64> {
        self.store.undelivered_wakeup()
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

/// Deterministic race gate for one presentation-start commit (test-only).
///
/// Pauses `commit_install` after the page plan and before the per-row
/// presentation-start compares, so a test can change a row's durable status
/// in between and pin that a domain `StaleSource` (not only an
/// infrastructure error) drops the row from the frame and the receipt.
#[cfg(test)]
pub(crate) struct TestPresentationCommitGate {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

#[cfg(test)]
impl Default for TestPresentationCommitGate {
    fn default() -> Self {
        Self {
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        }
    }
}

#[cfg(test)]
impl TestPresentationCommitGate {
    /// Pauses until the test releases the gate, marking entry first.
    pub(crate) async fn pause(&self) {
        self.entered.add_permits(1);
        let permit = self.release.acquire().await.expect("gate stays open");
        permit.forget();
    }

    /// Waits until a paused commit has entered the gate.
    pub(crate) async fn wait_entered(&self) {
        let permit = self.entered.acquire().await.expect("gate is entered");
        permit.forget();
    }

    /// Releases one paused commit.
    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

/// Test-only per-connection view of the memory-only presentation state.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PresentationConnectionCounts {
    /// Whether a subscription entry exists for this connection.
    pub subscription: bool,
    pub task_refs: usize,
    pub carried: usize,
    pub source_refs: usize,
    pub cursors: usize,
    pub receipts: usize,
    pub resume_slots: usize,
}

#[cfg(test)]
impl PresentationConnectionCounts {
    /// Whether the connection owns no presentation entry at all.
    #[must_use]
    pub(crate) fn is_empty(self) -> bool {
        !self.subscription
            && self.task_refs == 0
            && self.carried == 0
            && self.source_refs == 0
            && self.cursors == 0
            && self.receipts == 0
            && self.resume_slots == 0
    }
}

#[cfg(test)]
impl HostHandle {
    /// Test-only: counts of one connection's presentation-owned entries.
    pub(crate) fn presentation_counts_for_test(
        &self,
        connection: &ConnectionWireId,
    ) -> PresentationConnectionCounts {
        let conn = conn_key(connection);
        let state = crate::lock_unpoison(&self.presentations);
        PresentationConnectionCounts {
            subscription: state.subs.contains_key(&conn),
            task_refs: state
                .task_refs
                .keys()
                .filter(|(owner, _)| owner == &conn)
                .count(),
            carried: state
                .carried
                .keys()
                .filter(|(owner, _)| owner == &conn)
                .count(),
            source_refs: state
                .source_refs
                .keys()
                .filter(|(owner, _)| owner == &conn)
                .count(),
            cursors: state
                .cursors
                .keys()
                .filter(|(owner, _)| owner == &conn)
                .count(),
            receipts: state
                .receipts
                .values()
                .filter(|receipt| receipt.connection == conn)
                .count(),
            resume_slots: state
                .resume
                .values()
                .filter(|slot| slot.connection == conn)
                .count(),
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
