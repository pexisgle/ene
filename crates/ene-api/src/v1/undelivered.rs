//! Undelivered presentation and first-party Task queries (IPC §13.3, §18.2).
//!
//! The Host derives every view here from canonical owner rows at request
//! time; nothing here is authority. References are Host-issued opaque values
//! scoped to the issuing connection's query: the Host resolves each back to
//! its domain identity, and anything unresolvable (a rotated projection, a
//! dropped map after restart) answers `UnknownRef`, never a guess.
//!
//! Limits ride the storage query (`1..=50`, default 50); an out-of-range
//! limit is a wire-shape refusal (`UnsupportedFieldValue`), never a silent
//! clamp. Cursors are bound to their query and Task; reuse across either
//! answers `StaleBaseView`.
//!
//! Debug redaction rule (IPC §23): refs, revisions, generations, and outcomes
//! stay visible; excerpts, instruction bodies, and report source text may
//! quote managed content and are redacted.

use serde::{Deserialize, Serialize};

use super::refs::{CompanionWireRef, RoundWireId};
use super::round::PresentationStatus;

macro_rules! string_wire_ref {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub String);
    };
}

string_wire_ref!(
    PresentationReceiptWireRef,
    "Opaque presentation receipt, Host-issued per Companion presentation. Echo only; one live receipt per Companion."
);
string_wire_ref!(
    UndeliveredWireRef,
    "Opaque undelivered-item reference, Host-issued per connection query. Echo only."
);
string_wire_ref!(
    TaskWireRef,
    "Opaque Task reference, Host-issued per connection query. Echo only; never a TaskId."
);
string_wire_ref!(
    ReportSourceWireRef,
    "Opaque report-source reference, Host-issued per connection query. Echo only."
);
string_wire_ref!(
    PageCursorWire,
    "Opaque page cursor, Host-issued and bound to its query and Task. Echo only; reuse across queries answers StaleBaseView."
);

/// Default page size when a query omits `limit` (IPC §13.3, §18.2).
pub const DEFAULT_PAGE_LIMIT: u32 = 50;

/// Maximum page size; the bound rides the storage query, never truncation.
pub const MAX_PAGE_LIMIT: u32 = 50;

/// Maximum excerpt bytes of one undelivered source body (UTF-8 boundary).
pub const EXCERPT_MAX_BYTES: u32 = 2048;

/// Byte-cursor page bounds for [`GetReportSource`] bodies.
pub const MIN_SOURCE_LIMIT_BYTES: u32 = 4;
pub const MAX_SOURCE_LIMIT_BYTES: u32 = 16384;
pub const DEFAULT_SOURCE_LIMIT_BYTES: u32 = 4096;

/// Display projection of one undelivered source correlation (CI §5.2).
///
/// Identity only: the kind plus the canonical subject and certainty the
/// owner fact carries. Bodies travel as bounded excerpts, never here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UndeliveredSourceView {
    /// Closed source kind (e.g. `history_message`, `task_revision`,
    /// `result_recorded`, `action_attempt`).
    pub kind: String,
    /// Opaque subject identity (canonical fact id rendering). Display only.
    pub subject: String,
    /// Closed certainty name when the source carries one, else [`None`].
    pub certainty: Option<String>,
}

/// One carried undelivered item: correlation plus a scrubbed bounded excerpt.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UndeliveredItemView {
    pub reference: UndeliveredWireRef,
    pub source: UndeliveredSourceView,
    /// Bounded excerpt of the source's canonical body (≤2 KiB, UTF-8
    /// boundary, scrubbed). Redacted from [`core::fmt::Debug`].
    pub excerpt: String,
    /// Whether the canonical body extends past [`Self::excerpt`].
    pub truncated: bool,
}

impl core::fmt::Debug for UndeliveredItemView {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("UndeliveredItemView")
            .field("reference", &self.reference)
            .field("source", &self.source)
            .field("excerpt", &"[redacted]")
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// One Task headline: current lifecycle facts only, never bodies or full
/// detail. The same Task's facts stay grouped under one headline.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskReportView {
    pub task: TaskWireRef,
    pub revision: u64,
    /// Closed progress name (`started`, `in_progress`, `completed`,
    /// `failed`, `cancelled`).
    pub progress: String,
    /// Whether a paged [`GetTaskReport`] would return detail rows.
    pub details_available: bool,
}

/// One presented page: the actually-carried prefix plus its receipt.
///
/// The receipt covers exactly [`Self::items`]; the ACK for it presents only
/// those ids. Nothing here claims the operator read full bodies: the rest
/// pages through [`GetTaskReport`] / [`GetReportSource`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndeliveredSummary {
    pub receipt: PresentationReceiptWireRef,
    pub round: RoundWireId,
    pub presence_generation: u64,
    /// Actually-carried items only, at most 50.
    pub items: Vec<UndeliveredItemView>,
    /// Headlines for the distinct Tasks behind [`Self::items`].
    pub reports: Vec<TaskReportView>,
    /// Whether unpresented rows remain (later in this pass or newly arrived).
    pub has_more: bool,
    /// Continuation of this pass; [`None`] means the pass reached its
    /// captured bound. Bound to this companion's pass: reuse elsewhere
    /// answers `StaleBaseView`.
    #[serde(default)]
    pub next_cursor: Option<PageCursorWire>,
}

/// Subscription / paging request for the undelivered backlog.
///
/// [`None`] cursor catches up: a new-arrival pass first, else an explicit
/// head pass. `redisplay` forces the explicit head pass (Pending + Unknown)
/// even while new arrivals wait; without it a drained subscription serves
/// arrivals first and never auto-resends failed rows on a new-arrival pass.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UndeliveredRequest {
    /// Which companion's backlog; [`None`] means the running companion.
    #[serde(default)]
    pub companion: Option<CompanionWireRef>,
    /// Continue this pass, else catch up.
    #[serde(default)]
    pub cursor: Option<PageCursorWire>,
    /// Page bound (`1..=50`, default 50). Out of range answers
    /// `UnsupportedFieldValue`.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Force an explicit head re-display.
    #[serde(default)]
    pub redisplay: bool,
}

/// Outcome of one [`UndeliveredRequest`]: a page or a typed domain refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UndeliveredResponse {
    Summary(UndeliveredSummary),
    /// Not even one item fits the agreed frame cap. Nothing was mutated:
    /// no rows, no cursor, no receipt.
    FrameTooLarge,
    /// No formal presence backs a Companion summary. Management views stay
    /// readable; presence needs a fresh summon.
    NoCurrentPresence,
    /// The companion projection is unknown or rotated.
    UnknownCompanion,
    /// The base view is stale: the cursor belongs to another query, Task,
    /// or connection, or a live receipt's selection can no longer be exactly
    /// rehydrated. Re-query from the head for a fresh page and receipt.
    StaleBaseView {
        current: Option<PageCursorWire>,
    },
}

/// Presentation observation for one receipt: the batch the operator actually
/// painted. A partial batch reports `Unknown` / `Failed`, and the Host never
/// marks more than the carried ids.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UndeliveredAck {
    pub receipt: PresentationReceiptWireRef,
    pub status: PresentationStatus,
}

/// Outcome of one [`UndeliveredAck`]: an Ok-side domain outcome, never an
/// error. Only carried ids move; later arrivals and other receipts are
/// untouched.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UndeliveredAckOutcome {
    /// Carried ids confirmed presented.
    Presented { presented: u32 },
    /// Every carried id was already presented; nothing was written.
    AlreadyPresented,
    /// A `Failed` batch returned its carried rows to `Pending`.
    ReturnedToPending { count: u32 },
    /// An `Unknown` batch kept its rows re-presentable; nothing was written.
    KeptUnknown,
    /// The receipt is unknown on this connection (never issued or lost to a
    /// restart: re-query for a new receipt).
    UnknownRef,
    /// The receipt is superseded or expired; its rows stay as they are.
    StalePresentation,
    /// The ACK arrived on a different connection than the receipt's; ACKs
    /// never migrate across connections.
    StaleConnection,
    /// A current erasure condition covers the carried rows' source bodies:
    /// no status was written and the items are not confirmed presented. The
    /// Client re-queries after the deletion settles; distinct from a stale
    /// receipt (nothing about the receipt itself was wrong).
    HeldForErasure,
}

/// First-party Task list query: canonical TaskId byte order, bounded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ListTasks {
    #[serde(default)]
    pub cursor: Option<PageCursorWire>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// One Task list entry: stored lifecycle plus the current in-memory
/// execution-registration flag, kept separate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskListItem {
    pub task: TaskWireRef,
    pub revision: u64,
    pub progress: String,
    /// Whether this Host process currently holds a launch reservation or a
    /// running registration for the Task. Never durability, never a start.
    pub running: bool,
    /// Opaque purpose identity (`{task}:{adopted_revision}`): the revision
    /// that adopted the purpose in force, which is not necessarily
    /// [`Self::revision`] after a purpose-preserving steering or resume.
    /// Echo it back in a resume premise as-is.
    pub purpose: String,
}

/// One Task list page: a single read transaction's current values, not a
/// multi-page snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskListPage {
    pub tasks: Vec<TaskListItem>,
    pub next_cursor: Option<PageCursorWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskListResponse {
    Page(TaskListPage),
    StaleBaseView { current: Option<PageCursorWire> },
}

/// Paged Task report query: attempt rows before result rows, canonical id
/// order. Read-only: no startup, repair, re-evaluation, or presented-update.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GetTaskReport {
    pub task: TaskWireRef,
    #[serde(default)]
    pub cursor: Option<PageCursorWire>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// One report detail row identity: no bodies. `source` names the bounded
/// body page for result rows (and is [`None`] for attempt rows, which carry
/// no body); `purpose_source` on the page names the revision-purpose body.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskReportRowView {
    /// `action_attempt` or `task_result`.
    pub kind: String,
    /// Canonical row identity rendering. Display only.
    pub id: String,
    /// Result rows only: the stamped adoption revision, when adopted.
    pub adopted_revision: Option<u64>,
    pub source: Option<ReportSourceWireRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskReportPage {
    pub task: TaskWireRef,
    pub revision: u64,
    pub progress: String,
    /// Opaque purpose identity (`{task}:{adopted_revision}`), as in
    /// [`TaskListItem::purpose`].
    pub purpose: String,
    /// Bounded body source of the adopting revision's purpose text.
    pub purpose_source: ReportSourceWireRef,
    pub rows: Vec<TaskReportRowView>,
    pub next_cursor: Option<PageCursorWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskReportResponse {
    Page(TaskReportPage),
    UnknownRef,
    StaleBaseView { current: Option<PageCursorWire> },
}

/// Bounded body page of one report source, bound to that source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GetReportSource {
    pub source: ReportSourceWireRef,
    /// Byte cursor into the body; [`None`] starts at zero.
    #[serde(default)]
    pub cursor: Option<u64>,
    /// `4..=16384`, default 4096. Out of range answers
    /// `UnsupportedFieldValue`.
    #[serde(default)]
    pub limit_bytes: Option<u32>,
}

/// One byte-bounded body page, cut on a UTF-8 boundary.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReportSourcePageView {
    /// Redacted from [`core::fmt::Debug`].
    pub text: String,
    pub total_bytes: u64,
    pub next: Option<u64>,
}

impl core::fmt::Debug for ReportSourcePageView {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ReportSourcePageView")
            .field("text", &"[redacted]")
            .field("total_bytes", &self.total_bytes)
            .field("next", &self.next)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReportSourceResponse {
    Page(ReportSourcePageView),
    UnknownRef,
    /// The row exists but no scrubbed bounded view can be built (or it is
    /// gone): no body is sent, and nothing is marked presented.
    InputUnavailable,
}

/// First-party Task selection for the conversation: in-memory display
/// selection only. No durable mutation, no execution start; a restart or
/// reconnect resets to unselected. Never sourced from model output.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SelectTask {
    pub task: TaskWireRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskSelected {
    pub task: TaskWireRef,
    pub revision: u64,
    pub progress: String,
    /// Opaque purpose identity (`{task}:{adopted_revision}`), as in
    /// [`TaskListItem::purpose`].
    pub purpose: String,
    pub details_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SelectTaskResponse {
    Selected(TaskSelected),
    UnknownRef,
}

/// Explicit first-party resume: the relied premise plus the new Owner
/// instruction. Maps onto the existing owner `ResumeTaskCommand`; the
/// envelope `command_id` keys the retry epoch (same epoch + id +
/// fingerprint replays `InFlight` / the original outcome; a new epoch never
/// auto-resends). No resume receipt table exists: restart safety rests on
/// the durable revision compare.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResumeTask {
    pub task: TaskWireRef,
    pub expected_revision: u64,
    /// Opaque purpose identity echoed from selection or listing:
    /// `{task}:{adopted_revision}`, the stored adopting revision, never the
    /// current Task revision.
    pub expected_purpose: String,
    /// The new Owner instruction body. Redacted from [`core::fmt::Debug`].
    pub instruction: String,
}

impl core::fmt::Debug for ResumeTask {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ResumeTask")
            .field("task", &self.task)
            .field("expected_revision", &self.expected_revision)
            .field("expected_purpose", &self.expected_purpose)
            .field("instruction", &"[redacted]")
            .finish()
    }
}

/// Wire projection of the owner `TaskResumeOutcome` (IB H-A.1) plus the
/// protocol's retry-epoch answers. Every refusal is Ok-side with zero Task
/// writes; `Unavailable` is the only technical answer (retryable).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResumeTaskOutcomeWire {
    Resumed {
        task: TaskWireRef,
        revision: u64,
        delegation: String,
    },
    StalePremise {
        current_revision: u64,
    },
    Superseded,
    TaskTerminal {
        progress: String,
    },
    AlreadyRunning,
    HeldByUnknownEffects,
    ResultAvailable,
    NeedsRevalidation {
        hold: String,
    },
    MissingTask,
    RevisionExhausted,
    /// The same epoch + command is still being processed; no second commit.
    InFlight,
    /// The task or purpose reference resolves to nothing on this connection.
    UnknownRef,
    /// The command id was first seen under a different sender epoch; old
    /// commands never auto-resend across epochs.
    StaleConnection,
    /// The store could not answer; nothing was decided, retry is safe.
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::super::refs::RoundWireId;
    use super::{
        DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT, ReportSourcePageView, ResumeTask, TaskWireRef,
        UndeliveredItemView, UndeliveredSourceView,
    };

    #[test]
    fn page_bounds_match_the_wire_contract() {
        assert_eq!(DEFAULT_PAGE_LIMIT, 50);
        assert_eq!(MAX_PAGE_LIMIT, 50);
        assert_eq!(super::EXCERPT_MAX_BYTES, 2048);
        assert_eq!(super::MIN_SOURCE_LIMIT_BYTES, 4);
        assert_eq!(super::MAX_SOURCE_LIMIT_BYTES, 16384);
        assert_eq!(super::DEFAULT_SOURCE_LIMIT_BYTES, 4096);
    }

    #[test]
    fn item_debug_redacts_the_excerpt_but_keeps_refs() {
        let item = UndeliveredItemView {
            reference: super::UndeliveredWireRef(String::from("und-1")),
            source: UndeliveredSourceView {
                kind: String::from("task_revision"),
                subject: String::from("subject-9"),
                certainty: None,
            },
            excerpt: String::from("private managed words"),
            truncated: true,
        };
        let rendered = format!("{item:?}");
        assert!(
            !rendered.contains("private managed words"),
            "excerpt redacted: {rendered}"
        );
        assert!(
            rendered.contains("und-1") && rendered.contains("subject-9"),
            "refs stay visible: {rendered}"
        );
    }

    #[test]
    fn source_page_debug_redacts_text_but_keeps_accounting() {
        let page = ReportSourcePageView {
            text: String::from("private body bytes"),
            total_bytes: 9000,
            next: Some(4096),
        };
        let rendered = format!("{page:?}");
        assert!(
            !rendered.contains("private body bytes"),
            "body redacted: {rendered}"
        );
        assert!(
            rendered.contains("9000"),
            "accounting stays visible: {rendered}"
        );
    }

    #[test]
    fn resume_debug_redacts_the_instruction() {
        let command = ResumeTask {
            task: TaskWireRef(String::from("task-1")),
            expected_revision: 3,
            expected_purpose: String::from("task-1:3"),
            instruction: String::from("continue the remaining work please"),
        };
        let rendered = format!("{command:?}");
        assert!(
            !rendered.contains("continue the remaining work"),
            "instruction redacted: {rendered}"
        );
        assert!(rendered.contains("task-1"), "refs stay visible: {rendered}");
        let _ = RoundWireId(String::from("round-1"));
    }
}
