use std::collections::HashMap;
use std::time::{Duration, Instant};

use ene_api::v1::command::CommandReplayRejectWire;
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
use ene_preservation::{
    DeletionMaterialOutcome, MechanicalDeletionTarget, PreservationRepository as _,
};
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

fn conn_key(id: &ConnectionWireId) -> String {
    connection_key(id)
}

pub(crate) fn stale_operation(frame: &WireFrame, live: &LiveInput, detail: &str) -> WireFrame {
    stale_reject(frame, live, detail)
}

const RECEIPT_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PassTrigger {
    Request,
    Redisplay,
    Push,
}

const PRESENTATION_FRAME_BUDGET: usize = 192 * 1024;

const PER_ITEM_OVERHEAD: usize = 512;

const RESUME_COMMAND_CAP: usize = 4096;

#[derive(Debug, Clone)]
struct Subscription {
    companion: RawId,
    scan_floor: u64,
    drained_once: bool,
    /// Undrained pass continuation: cursor, filter, bound, and the wire of the
    /// cursor minted for it. A cursor-less request continues it before looking
    /// for arrivals, so an abandoned page never resends its head as "new"; the
    /// stored wire is consumed on that advance so the connection's cursor map
    /// stays at one entry (forward-only paging contract).
    resume: Option<(UndeliveredCursor, bool, u32, String)>,
}

/// Records a drained scan floor: advance the floor to `upper`, mark the
/// subscription drained, and clear any pending continuation. No-op when the
/// subscription is bound to a different companion.
fn drain_sub(sub: &mut Subscription, companion: RawId, upper: u64) {
    if sub.companion == companion {
        sub.scan_floor = sub.scan_floor.max(upper);
        sub.drained_once = true;
        sub.resume = None;
    }
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

#[derive(Debug, Clone)]
pub(crate) enum StoredCursor {
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
    UsageSummary {
        premise: crate::usage::UsageQueryPremise,
        from: ene_primitive::WallClockWithTz,
        to: ene_primitive::WallClockWithTz,
        after: Option<ene_inference::UsageSummaryCursor>,
    },
}

#[derive(Debug, Clone)]
struct ResumeSlot {
    epoch: String,
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

pub(crate) struct PresentationState {
    subs: HashMap<String, Subscription>,
    receipts: HashMap<String, Receipt>,
    receipt_ids: HashMap<String, String>,
    retired: std::collections::VecDeque<(String, String)>,
    task_refs: HashMap<(String, String), TaskId>,
    carried: HashMap<(String, String), (UndeliveredId, Option<TaskId>)>,
    source_refs: HashMap<(String, String), TaskReportSourceRef>,
    cursors: HashMap<(String, String), StoredCursor>,
    resume: HashMap<Uuid, ResumeSlot>,
    resume_seq: u64,
    frame_budget: usize,
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

const RETIRED_RECEIPT_CAP: usize = 64;

impl PresentationState {
    pub(crate) fn retire(&mut self, id: &str, connection: &str) {
        if self.retired.iter().any(|(known, _)| known == id) {
            return;
        }
        self.retired
            .push_back((id.to_string(), connection.to_string()));
        while self.retired.len() > RETIRED_RECEIPT_CAP {
            self.retired.pop_front();
        }
    }

    pub(crate) fn remove_receipt(&mut self, companion_key: &str) {
        if let Some(receipt) = self.receipts.remove(companion_key) {
            self.receipt_ids.remove(&receipt.id);
            self.retire(&receipt.id, &receipt.connection);
        }
    }

    pub(crate) fn invalidate_for_erasure(&mut self) -> u64 {
        let dropped = (self.receipts.len() + self.carried.len()) as u64;
        let receipts: Vec<(String, String)> = self
            .receipts
            .values()
            .map(|receipt| (receipt.id.clone(), receipt.connection.clone()))
            .collect();
        self.receipts.clear();
        self.receipt_ids.clear();
        for (id, connection) in receipts {
            self.retire(&id, &connection);
        }
        self.subs.clear();
        self.task_refs.clear();
        self.carried.clear();
        self.source_refs.clear();
        self.cursors.clear();
        self.resume.clear();
        dropped
    }

    pub(crate) fn usage_cursor_page(
        &self,
        conn: &str,
        wire: &str,
        premise: &crate::usage::UsageQueryPremise,
    ) -> Option<crate::usage::UsageCursorPage> {
        match self.cursors.get(&(conn.to_string(), wire.to_string())) {
            Some(StoredCursor::UsageSummary {
                premise: bound,
                from,
                to,
                after,
            }) if bound == premise => Some(crate::usage::UsageCursorPage {
                from: *from,
                to: *to,
                after: *after,
            }),
            _ => None,
        }
    }
}
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

pub(crate) fn field_reject(frame: &WireFrame, live: &LiveInput, detail: &str) -> WireFrame {
    reject_frame(
        frame,
        live,
        RejectKind::UnsupportedFieldValue,
        detail.to_string(),
    )
}

pub(crate) fn checked_limit(limit: Option<u32>) -> Option<u32> {
    match limit {
        None => Some(DEFAULT_PAGE_LIMIT),
        Some(value) if (1..=MAX_PAGE_LIMIT).contains(&value) => Some(value),
        Some(_) => None,
    }
}

pub(crate) struct CurrentCoverage {
    targets: Vec<String>,
    readable: bool,
}

impl CurrentCoverage {
    fn unreadable() -> Self {
        Self {
            targets: Vec::new(),
            readable: false,
        }
    }

    pub(crate) fn covers(&self, text: &str) -> bool {
        if !self.readable {
            return true;
        }
        self.targets
            .iter()
            .any(|target| !target.is_empty() && text.contains(target.as_str()))
    }
}

impl HostHandle {
    pub(crate) async fn presentation_gate(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.presentation_lock.lock().await
    }

    pub(crate) async fn current_coverage(&self) -> CurrentCoverage {
        let mut targets = Vec::new();
        let mut after = None;
        loop {
            let page = match self.store.current_erasure_conditions(after, 100).await {
                Ok(page) => page,
                Err(_) => return CurrentCoverage::unreadable(),
            };
            if page.is_empty() {
                break;
            }
            let page_len = page.len();
            for condition in &page {
                after = Some(condition.condition.operation);
                match self
                    .store
                    .deletion_operation_material(condition.condition.operation)
                    .await
                {
                    Ok(DeletionMaterialOutcome::Material(material)) => {
                        let MechanicalDeletionTarget::ExactText(exact) =
                            &material.target().mechanical;
                        let text = exact.expose_for_erasure();
                        if !text.is_empty() {
                            targets.push(text.to_string());
                        }
                    }
                    Ok(DeletionMaterialOutcome::Destroyed | DeletionMaterialOutcome::Missing) => {
                        return CurrentCoverage::unreadable();
                    }
                    Err(_) => return CurrentCoverage::unreadable(),
                }
            }
            if page_len < 100 {
                break;
            }
        }
        CurrentCoverage {
            targets,
            readable: true,
        }
    }

    pub(crate) fn with_presentation_state<R>(
        &self,
        live: &LiveInput,
        install: impl FnOnce(&mut PresentationState) -> R,
    ) -> Option<R> {
        self.with_current_connection(live, || {
            let mut state = crate::lock_unpoison(&self.presentations);
            install(&mut state)
        })
    }

    fn epoch_key(live: &LiveInput, frame: &WireFrame) -> String {
        format!(
            "{}:{}:{}:{}",
            live.client_ref,
            frame.envelope.sender.incarnation_id.counter,
            frame.envelope.sender.incarnation_id.random,
            conn_key(&live.connection_id),
        )
    }

    pub(crate) fn mint_cursor(
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
        if let Some(((_, wire), _)) = state
            .task_refs
            .iter()
            .find(|((owner, _), mapped)| owner == conn && **mapped == task)
        {
            return TaskWireRef(wire.clone());
        }
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
        if let Some(((_, wire), _)) = state
            .source_refs
            .iter()
            .find(|((owner, _), mapped)| owner == conn && **mapped == source)
        {
            return ReportSourceWireRef(wire.clone());
        }
        let wire = Uuid::new_v4().as_hyphenated().to_string();
        state
            .source_refs
            .insert((conn.to_string(), wire.clone()), source);
        ReportSourceWireRef(wire)
    }

    fn sweep_carried(state: &mut PresentationState, conn: &str) {
        state.carried.retain(|(c, _), _| c != conn);
    }

    pub(crate) fn take_cursor(state: &mut PresentationState, conn: &str, cursor: Option<&str>) {
        if let Some(wire) = cursor {
            state.cursors.remove(&(conn.to_string(), wire.to_string()));
        }
    }

    pub(crate) async fn request_undelivered(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        request: &UndeliveredRequest,
    ) -> Vec<WireFrame> {
        let Some(limit) = checked_limit(request.limit) else {
            return vec![field_reject(frame, live, "query limit must be 1..=50")];
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
            None => vec![stale_operation(
                frame,
                live,
                "undelivered request on a superseded connection",
            )],
        }
    }

    /// Presents one page: presence-checked, receipt-backed, frame-capped.
    /// Store failures answer `Unavailable`; no row, cursor, or receipt moves,
    /// and the next request retries the same pass rather than receiving a
    /// lying page.
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
                Err(_) => return Some(UndeliveredResponse::Unavailable),
                Ok(None) => return Some(UndeliveredResponse::UnknownCompanion),
                Ok(Some(companion)) => companion,
            },
            None => match self.store.ensure_running_companion().await {
                Err(_) => return Some(UndeliveredResponse::Unavailable),
                Ok(companion) => companion,
            },
        };
        let attribution = match self.store.load_attribution(companion.as_raw()).await {
            Ok(Some(attribution)) => attribution,
            Ok(None) => return Some(UndeliveredResponse::NoCurrentPresence),
            Err(_) => return Some(UndeliveredResponse::Unavailable),
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
        if cursor.is_none()
            && let Some(receipt) = self.live_receipt(&companion_key, &conn)
        {
            return self.reemit_receipt(live, &conn, &receipt).await;
        }
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
        if loaded.len() != selected.len()
            || loaded
                .iter()
                .zip(&selected)
                .any(|(entry, id)| entry.id != *id)
        {
            return Some(self.retire_unrehydratable_receipt(receipt));
        }
        let mut carried = Vec::with_capacity(loaded.len());
        let mut satisfied: Vec<UndeliveredId> = Vec::new();
        for entry in loaded {
            if entry.status == ReportStatus::Presented {
                satisfied.push(entry.id);
            } else {
                carried.push(entry);
            }
        }
        Self::sweep_carried(&mut crate::lock_unpoison(&self.presentations), conn);
        let items = self.carry_items(live, conn, &carried).await?;
        let companion_key = receipt.companion.as_uuid().as_hyphenated().to_string();
        let receipt_live = self.with_presentation_state(live, |state| {
            if !satisfied.is_empty()
                && let Some(live_receipt) = state.receipts.get_mut(&companion_key)
                && live_receipt.id == receipt.id
            {
                live_receipt.selected.retain(|id| !satisfied.contains(id));
            }
            state
                .receipts
                .get(&companion_key)
                .is_some_and(|live| live.id == receipt.id)
        });
        match receipt_live {
            Some(true) => {}
            Some(false) => {
                self.forget_carried(conn, &items).await;
                return Some(UndeliveredResponse::StaleBaseView { current: None });
            }
            None => {
                self.forget_carried(conn, &items).await;
                return None;
            }
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

    fn retire_unrehydratable_receipt(&self, receipt: &Receipt) -> UndeliveredResponse {
        let mut state = crate::lock_unpoison(&self.presentations);
        let companion_key = receipt.companion.as_uuid().as_hyphenated().to_string();
        if state
            .receipts
            .get(&companion_key)
            .is_some_and(|live| live.id == receipt.id)
        {
            state.remove_receipt(&companion_key);
        }
        UndeliveredResponse::StaleBaseView { current: None }
    }

    /// Begins or continues one pass: fetches the longest fitting prefix of
    /// one bounded fetch, commits it, installs the receipt.
    ///
    /// Returns `None` when the pass installed nothing: the connection was
    /// superseded or closed before the commit, or the commit landed but the
    /// connection was superseded before the report refs were attached (see
    /// [`Self::commit_install`]).
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
        // The wire this pass continues from, if any: the request's own cursor
        // wire, or the one a stored continuation was minted under. It is
        // consumed (see `take_cursor`) so a cursor-less advance replaces the
        // predecessor wire instead of leaking it.
        let mut from_cursor = cursor.map(|cursor| cursor.0.clone());
        // A cursor supplied by this request is a continuation, not an explicit
        // head pass: only the stored cursor-less catch-up may fall through to a
        // head re-display when it drains.
        let from_request_cursor = cursor.is_some();
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
            let bound = match self.store.undelivered_pass_bound().await {
                Ok(bound) => bound,
                Err(_) => return Some(UndeliveredResponse::Unavailable),
            };
            let planned = self.with_presentation_state(live, |state| {
                let ckey = companion.as_raw().as_uuid().as_hyphenated().to_string();
                if let Some(receipt) = state.receipts.get(&ckey).cloned()
                    && (receipt.connection != conn || receipt.expired())
                {
                    state.remove_receipt(&ckey);
                }
                let sub = state.subs.entry(conn.to_string()).or_insert(Subscription {
                    companion: companion.as_raw(),
                    scan_floor: 0,
                    drained_once: false,
                    resume: None,
                });
                if sub.companion != companion.as_raw() {
                    sub.scan_floor = 0;
                    sub.drained_once = false;
                    sub.resume = None;
                }
                let continuation = (trigger != PassTrigger::Redisplay)
                    .then_some(sub.resume.as_ref())
                    .flatten()
                    .filter(|_| sub.companion == companion.as_raw())
                    .map(|(cursor, pending_only, saved, wire)| {
                        (*cursor, *pending_only, *saved, wire.clone())
                    });
                let arrivals = trigger != PassTrigger::Redisplay
                    && sub.drained_once
                    && sub.scan_floor < bound
                    && sub.companion == companion.as_raw();
                let plan = if let Some((cursor, pending_only, saved, wire)) = continuation {
                    from_cursor = Some(wire);
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
                Err(_) => return Some(UndeliveredResponse::Unavailable),
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
            if matches!(start, PlanStart::Continued { .. })
                && !from_request_cursor
                && trigger != PassTrigger::Push
            {
                // The continued cursor is consumed by this drained pass: the
                // recurrence below is an explicit head re-display under
                // `None`, so without this the stored cursor would survive
                // every repeat request and the map would grow with the page
                // count (take_cursor's contract).
                self.with_presentation_state(live, |state| {
                    Self::take_cursor(state, conn, from_cursor.as_deref());
                })?;
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
            if let Some(next) = fetched_next {
                let installed = self
                    .with_presentation_state(live, |state| {
                        Self::take_cursor(state, conn, from_cursor.as_deref());
                        summary.has_more = true;
                        let wire = Self::mint_cursor(
                            state,
                            conn,
                            StoredCursor::Undelivered {
                                companion: companion.as_raw(),
                                cursor: next,
                                pending_only,
                                limit: pass_limit,
                            },
                        );
                        if let Some(sub) = state.subs.get_mut(conn) {
                            sub.resume = Some((next, pending_only, pass_limit, wire.0.clone()));
                        }
                        summary.next_cursor = Some(wire);
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
            from_cursor.as_deref(),
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
    /// durable row (CCT §10.5).
    ///
    /// Returns `None` in two distinct cases: (a) the ownership section
    /// refused — no receipt, cursor, or subscription entry is created for a
    /// superseded connection and the attempt's carried refs are dropped; or
    /// (b) the section committed but the connection was superseded before
    /// [`Self::attach_reports`] installed the report refs, so the durable
    /// marks, round mapping, receipt, cursor, and subscription install all
    /// stand and are released by the supersession sweep.
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
                    if entry.status != ReportStatus::Pending {
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
                        Ok(ene_companion::ReportStatusTransition::HeldForErasure) => {
                            let mut withheld = item;
                            withheld.excerpt = String::new();
                            withheld.truncated = false;
                            selected.push(entry.id);
                            carried.push(withheld);
                        }
                        Ok(ene_companion::ReportStatusTransition::StaleSource) => {
                            dropped.push(item);
                        }
                        Ok(_) => {
                            dropped.push(item);
                        }
                        Err(_) => {
                            dropped.push(item);
                        }
                    }
                }
                for item in dropped {
                    state.carried.remove(&(conn.to_string(), item.reference.0));
                }
                let receipt_id = Uuid::new_v4().as_hyphenated().to_string();
                let ttl = state.receipt_ttl;
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
                Self::take_cursor(state, conn, from_cursor);
                if let Some(old) = state.receipts.insert(companion_key.clone(), receipt) {
                    state.receipt_ids.remove(&old.id);
                    state.retire(&old.id, &old.connection);
                }
                state.receipt_ids.insert(receipt_id.clone(), companion_key);
                let (next_cursor, drained) = match fetched_next {
                    Some(next) => {
                        let wire = Self::mint_cursor(
                            state,
                            conn,
                            StoredCursor::Undelivered {
                                companion: companion.as_raw(),
                                cursor: next,
                                pending_only,
                                limit,
                            },
                        );
                        if let Some(sub) = state.subs.get_mut(conn)
                            && sub.companion == companion.as_raw()
                        {
                            sub.resume = Some((next, pending_only, limit, wire.0.clone()));
                        }
                        (Some(wire), false)
                    }
                    None => {
                        if let Some(sub) = state.subs.get_mut(conn) {
                            drain_sub(sub, companion.as_raw(), upper);
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

    async fn carry_items(
        &self,
        live: &LiveInput,
        conn: &str,
        entries: &[UndeliveredRef],
    ) -> Option<Vec<UndeliveredItemView>> {
        let coverage = self.current_coverage().await;
        let mut loaded = Vec::with_capacity(entries.len());
        for entry in entries {
            let (excerpt, truncated) = match self
                .store
                .load_undelivered_excerpt(entry.source, EXCERPT_MAX_BYTES)
                .await
            {
                Ok(Some(page)) if coverage.covers(&page.text) => (String::new(), false),
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
        let serves_body = loaded
            .iter()
            .any(|(_, _, _, excerpt, _)| !excerpt.is_empty());
        if serves_body {
            if !self.note_client_body_delivery(live).await {
                for (_, _, _, excerpt, truncated) in &mut loaded {
                    excerpt.clear();
                    *truncated = false;
                }
            } else {
                let fresh = self.current_coverage().await;
                for (_, _, _, excerpt, truncated) in &mut loaded {
                    if !excerpt.is_empty() && fresh.covers(excerpt) {
                        excerpt.clear();
                        *truncated = false;
                    }
                }
            }
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

    async fn forget_carried(&self, conn: &str, items: &[UndeliveredItemView]) {
        let mut state = crate::lock_unpoison(&self.presentations);
        for item in items {
            state
                .carried
                .remove(&(conn.to_string(), item.reference.0.clone()));
        }
    }

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

    fn mark_drained(
        &self,
        live: &LiveInput,
        conn: &str,
        companion: CompanionId,
        upper: u64,
    ) -> bool {
        self.with_presentation_state(live, |state| {
            if let Some(sub) = state.subs.get_mut(conn) {
                drain_sub(sub, companion.as_raw(), upper);
            }
        })
        .is_some()
    }

    fn frame_budget(&self) -> usize {
        crate::lock_unpoison(&self.presentations).frame_budget
    }

    #[cfg(all(test, unix))]
    #[expect(dead_code, reason = "test observation probe")]
    pub(crate) fn set_receipt_ttl_for_test(&self, ttl: Duration) {
        crate::lock_unpoison(&self.presentations).receipt_ttl = ttl;
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

    fn empty_attributed(&self, attribution: &PresenceAttribution) -> UndeliveredSummary {
        // A receipt-less shell has nothing to ACK, so its round is never
        // resolved back: mint an opaque wire without registering a
        // process-lifetime `rounds` mapping.
        let round_wire = RoundWireId(RawId::new().as_uuid().to_string());
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
        let presentations = std::sync::Arc::clone(&self.presentations);
        let store = self.store.clone();
        let paired_client = live.paired_device.as_deref().map(device_client);
        let ack = ack.clone();
        let applied = self
            .with_current_connection_blocking(live, move || {
                let verdict = (|| {
                    let mut state = crate::lock_unpoison(&presentations);
                    let Some(companion_key) = state.receipt_ids.get(&ack.receipt.0).cloned() else {
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
                        state.remove_receipt(&companion_key);
                        Verdict::Refuse(UndeliveredAckOutcome::StalePresentation)
                    } else if receipt.connection != conn || receipt.incarnation != incarnation {
                        Verdict::Refuse(UndeliveredAckOutcome::StaleConnection)
                    } else if paired_client != Some(receipt.client) {
                        // The connection's client moved under the receipt.
                        Verdict::Refuse(UndeliveredAckOutcome::StaleConnection)
                    } else if round_view.as_deref() != Some(receipt.round_wire.as_str())
                        || generation_view != Some(receipt.generation)
                    {
                        Verdict::Refuse(UndeliveredAckOutcome::StalePresentation)
                    } else {
                        state.remove_receipt(&companion_key);
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
                        // Bounded to the carried ids; later arrivals are never
                        // touched by this ACK.
                        let mut presented = 0_u32;
                        let mut held = 0_u32;
                        let mut not_written = 0_u32;
                        let mut unavailable = false;
                        for id in &receipt.selected {
                            match store.compare_and_mark_reported_sync(
                                *id,
                                ReportStatus::PresentationUnknown,
                                mark,
                            ) {
                                Ok(ene_companion::ReportStatusTransition::PendingToPresented) => {
                                    presented += 1;
                                }
                                Ok(ene_companion::ReportStatusTransition::HeldForErasure) => {
                                    held += 1;
                                }
                                // `AlreadyPresented` writes nothing but is a
                                // true duplicate, so it is not "not written".
                                Ok(ene_companion::ReportStatusTransition::AlreadyPresented) => {}
                                // A stale/gone row or a rolled-back compare
                                // wrote nothing and stays re-presentable:
                                // never count it as already presented.
                                Ok(_) => not_written += 1,
                                // A store failure is not a domain answer: the
                                // ACK cannot claim any status was written.
                                Err(_) => unavailable = true,
                            }
                        }
                        if unavailable {
                            UndeliveredAckOutcome::Unavailable
                        } else if held > 0 && presented == 0 {
                            UndeliveredAckOutcome::HeldForErasure
                        } else if presented > 0 {
                            UndeliveredAckOutcome::Presented { presented }
                        } else if not_written == 0 {
                            // Every carried row was already presented (a parallel
                            // round observation got there first): no write.
                            UndeliveredAckOutcome::AlreadyPresented
                        } else {
                            // Some rows were not written but stay
                            // re-presentable: nothing durable changed.
                            UndeliveredAckOutcome::KeptUnknown
                        }
                    }
                    PresentationStatus::Failed => {
                        let mut count = 0_u32;
                        let mut unavailable = false;
                        for id in &receipt.selected {
                            match store.compare_and_mark_reported_sync(
                                *id,
                                ReportStatus::PresentationUnknown,
                                mark,
                            ) {
                                Ok(ene_companion::ReportStatusTransition::FailedToPending) => {
                                    count += 1;
                                }
                                Ok(_) => {}
                                Err(_) => unavailable = true,
                            }
                        }
                        if unavailable {
                            UndeliveredAckOutcome::Unavailable
                        } else {
                            UndeliveredAckOutcome::ReturnedToPending { count }
                        }
                    }
                    PresentationStatus::Unknown => UndeliveredAckOutcome::KeptUnknown,
                }
            })
            .await;
        applied.unwrap_or(UndeliveredAckOutcome::StaleConnection)
    }

    pub(crate) async fn list_tasks_wire(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        query: &ListTasks,
    ) -> Vec<WireFrame> {
        let Some(limit) = checked_limit(query.limit) else {
            return vec![field_reject(frame, live, "query limit must be 1..=50")];
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
        let headlines: Vec<TaskHeadline> = match self.store.list_tasks_after(after, limit).await {
            Ok(headlines) => headlines,
            Err(_) => {
                return vec![outgoing_frame(
                    frame,
                    live,
                    WirePayload::TaskListResponse(TaskListResponse::Unavailable),
                )];
            }
        };
        // Test-only race gate: pause after the durable read and before the
        // guarded mint.
        #[cfg(test)]
        if let Some(gate) = self.ref_mint_gate() {
            gate.pause().await;
        }
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

    pub(crate) async fn report_wire(
        &self,
        frame: &WireFrame,
        live: &LiveInput,
        query: &GetTaskReport,
    ) -> Vec<WireFrame> {
        let Some(limit) = checked_limit(query.limit) else {
            return vec![field_reject(frame, live, "query limit must be 1..=50")];
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
                    WirePayload::TaskReportResponse(TaskReportResponse::Unavailable),
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
                    WirePayload::TaskReportResponse(TaskReportResponse::Unavailable),
                )];
            }
        };
        #[cfg(test)]
        if let Some(gate) = self.ref_mint_gate() {
            gate.pause().await;
        }
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
                return vec![field_reject(
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
        let coverage = self.current_coverage().await;
        match self
            .store
            .load_report_source_bounded(source, query.cursor.unwrap_or(0), limit)
            .await
        {
            Ok(Some(page)) if !coverage.covers(&page.text) => {
                if !page.text.is_empty() && !self.note_client_body_delivery(live).await {
                    return vec![outgoing_frame(
                        frame,
                        live,
                        WirePayload::ReportSourceResponse(ReportSourceResponse::InputUnavailable),
                    )];
                }
                if !page.text.is_empty() {
                    let fresh = self.current_coverage().await;
                    if fresh.covers(&page.text) {
                        return vec![outgoing_frame(
                            frame,
                            live,
                            WirePayload::ReportSourceResponse(
                                ReportSourceResponse::InputUnavailable,
                            ),
                        )];
                    }
                }
                vec![outgoing_frame(
                    frame,
                    live,
                    WirePayload::ReportSourceResponse(ReportSourceResponse::Page(
                        ReportSourcePageView {
                            text: page.text,
                            total_bytes: page.total_bytes,
                            next: page.next,
                        },
                    )),
                )]
            }
            Ok(Some(_)) | Ok(None) | Err(_) => vec![outgoing_frame(
                frame,
                live,
                WirePayload::ReportSourceResponse(ReportSourceResponse::InputUnavailable),
            )],
        }
    }

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
            Ok(None) => {
                return vec![outgoing_frame(
                    frame,
                    live,
                    WirePayload::SelectTaskResponse(SelectTaskResponse::UnknownRef),
                )];
            }
            Err(_) => {
                return vec![outgoing_frame(
                    frame,
                    live,
                    WirePayload::SelectTaskResponse(SelectTaskResponse::Unavailable),
                )];
            }
        };
        let running = match self.store.ensure_running_companion().await {
            Ok(running) => running,
            Err(_) => {
                return vec![outgoing_frame(
                    frame,
                    live,
                    WirePayload::SelectTaskResponse(SelectTaskResponse::Unavailable),
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
        #[cfg(test)]
        if let Some(gate) = self.ref_mint_gate() {
            gate.pause().await;
        }
        // The selection commit runs under the ownership section: a
        // connection superseded while the Task was read selects nothing, and
        // the new connection must select again (IPC §9.3 replacement).
        // The report-row read is fallible, so it runs before the selection
        // commit: a store failure answers `Unavailable` (documented as
        // side-effect-free) while the projection is still untouched, never
        // after the connection already selected the Task.
        let details = if record.task.adopted_result.is_some() {
            true
        } else {
            match self.store.list_task_report_rows_after(task, None, 1).await {
                Ok(rows) => !rows.is_empty(),
                Err(_) => {
                    return vec![outgoing_frame(
                        frame,
                        live,
                        WirePayload::SelectTaskResponse(SelectTaskResponse::Unavailable),
                    )];
                }
            }
        };
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
            ResumeApply::Conflict => vec![outgoing_frame(
                frame,
                live,
                WirePayload::CommandReplayReject(CommandReplayRejectWire::CommandIdConflict {
                    command_id,
                }),
            )],
        }
    }

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
            Ok(None) => return ResumeTaskOutcomeWire::StaleConnection,
            Err(_) => return ResumeTaskOutcomeWire::Unavailable,
        };
        map_resume_outcome(&command.task, outcome)
    }

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
        let prepared = self
            .with_presentation_state(live, |state| {
                let sub = state.subs.entry(conn.clone()).or_insert(Subscription {
                    companion: companion.as_raw(),
                    scan_floor: 0,
                    drained_once: false,
                    resume: None,
                });
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
            state.remove_receipt(&key);
        }
    }

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
            state.remove_receipt(&key);
        }
    }

    pub(crate) async fn push_undelivered(
        &self,
        template: &WireFrame,
        live: &LiveInput,
    ) -> Option<WireFrame> {
        let _gate = self.presentation_gate().await;
        let conn = conn_key(&live.connection_id);
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

    #[must_use]
    pub(crate) fn undelivered_wakeup(&self) -> tokio::sync::watch::Receiver<u64> {
        self.store.undelivered_wakeup()
    }

    #[cfg(test)]
    #[expect(dead_code, reason = "test hook for receipt expiry")]
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
    pub(crate) async fn pause(&self) {
        self.entered.add_permits(1);
        let permit = self.release.acquire().await.expect("gate stays open");
        permit.forget();
    }

    #[expect(dead_code, reason = "test gate hook")]
    pub(crate) async fn wait_entered(&self) {
        let permit = self.entered.acquire().await.expect("gate is entered");
        permit.forget();
    }

    #[expect(dead_code, reason = "test gate hook")]
    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

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
