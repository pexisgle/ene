use serde::{Deserialize, Serialize};

use super::refs::{CompanionWireRef, RoundWireId};
use super::round::PresentationStatus;

macro_rules! string_wire_ref {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub String);
    };
}

string_wire_ref!(PresentationReceiptWireRef);
string_wire_ref!(UndeliveredWireRef);
string_wire_ref!(TaskWireRef);
string_wire_ref!(ReportSourceWireRef);
string_wire_ref!(PageCursorWire);

pub const DEFAULT_PAGE_LIMIT: u32 = 50;
pub const MAX_PAGE_LIMIT: u32 = 50;
pub const EXCERPT_MAX_BYTES: u32 = 2048;
pub const MIN_SOURCE_LIMIT_BYTES: u32 = 4;
pub const MAX_SOURCE_LIMIT_BYTES: u32 = 16384;
pub const DEFAULT_SOURCE_LIMIT_BYTES: u32 = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UndeliveredSourceView {
    pub kind: String,
    pub subject: String,
    pub certainty: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UndeliveredItemView {
    pub reference: UndeliveredWireRef,
    pub source: UndeliveredSourceView,
    pub excerpt: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskReportView {
    pub task: TaskWireRef,
    pub revision: u64,
    pub progress: String,
    pub details_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndeliveredSummary {
    pub receipt: PresentationReceiptWireRef,
    pub round: RoundWireId,
    pub presence_generation: u64,
    pub items: Vec<UndeliveredItemView>,
    pub reports: Vec<TaskReportView>,
    pub has_more: bool,
    #[serde(default)]
    pub next_cursor: Option<PageCursorWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UndeliveredRequest {
    #[serde(default)]
    pub companion: Option<CompanionWireRef>,
    #[serde(default)]
    pub cursor: Option<PageCursorWire>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub redisplay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UndeliveredResponse {
    Summary(UndeliveredSummary),
    FrameTooLarge,
    NoCurrentPresence,
    UnknownCompanion,
    StaleBaseView { current: Option<PageCursorWire> },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UndeliveredAck {
    pub receipt: PresentationReceiptWireRef,
    pub status: PresentationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UndeliveredAckOutcome {
    Presented { presented: u32 },
    AlreadyPresented,
    ReturnedToPending { count: u32 },
    KeptUnknown,
    UnknownRef,
    StalePresentation,
    StaleConnection,
    HeldForErasure,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ListTasks {
    #[serde(default)]
    pub cursor: Option<PageCursorWire>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskListItem {
    pub task: TaskWireRef,
    pub revision: u64,
    pub progress: String,
    pub running: bool,
    pub purpose: String,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GetTaskReport {
    pub task: TaskWireRef,
    #[serde(default)]
    pub cursor: Option<PageCursorWire>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskReportRowView {
    pub kind: String,
    pub id: String,
    pub adopted_revision: Option<u64>,
    pub source: Option<ReportSourceWireRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskReportPage {
    pub task: TaskWireRef,
    pub revision: u64,
    pub progress: String,
    pub purpose: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GetReportSource {
    pub source: ReportSourceWireRef,
    #[serde(default)]
    pub cursor: Option<u64>,
    #[serde(default)]
    pub limit_bytes: Option<u32>,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReportSourcePageView {
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
    InputUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SelectTask {
    pub task: TaskWireRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskSelected {
    pub task: TaskWireRef,
    pub revision: u64,
    pub progress: String,
    pub purpose: String,
    pub details_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SelectTaskResponse {
    Selected(TaskSelected),
    UnknownRef,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResumeTask {
    pub task: TaskWireRef,
    pub expected_revision: u64,
    pub expected_purpose: String,
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
    InFlight,
    UnknownRef,
    StaleConnection,
    Unavailable,
}
