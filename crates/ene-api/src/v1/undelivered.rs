use serde::{Deserialize, Serialize};

use super::refs::{CompanionWireRef, RoundWireId, string_wire_ref};
use super::round::PresentationStatus;

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
    /// Continuation of this pass; [`None`] means the pass reached its
    /// captured bound. Bound to this companion's pass: reuse elsewhere
    /// answers `StaleBaseView`.
    pub next_cursor: Option<PageCursorWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UndeliveredRequest {
    /// Which companion's backlog; [`None`] means the running companion.
    pub companion: Option<CompanionWireRef>,
    /// Continue this pass, else catch up.
    pub cursor: Option<PageCursorWire>,
    /// Page bound (`1..=50`, default 50). Out of range answers
    /// `UnsupportedFieldValue`.
    pub limit: Option<u32>,
    #[serde(default)]
    pub redisplay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UndeliveredResponse {
    Summary(UndeliveredSummary),
    /// The store could not answer; nothing was read or changed.
    Unavailable,
    /// Not even one item fits the agreed frame cap. Nothing was mutated:
    /// no rows, no cursor, no receipt.
    FrameTooLarge,
    NoCurrentPresence,
    UnknownCompanion,
    StaleBaseView {
        current: Option<PageCursorWire>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UndeliveredAck {
    pub receipt: PresentationReceiptWireRef,
    pub status: PresentationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UndeliveredAckOutcome {
    Presented {
        presented: u32,
    },
    AlreadyPresented,
    ReturnedToPending {
        count: u32,
    },
    KeptUnknown,
    UnknownRef,
    StalePresentation,
    StaleConnection,
    HeldForErasure,
    /// The store could not answer; no status was written for the carried ids.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ListTasks {
    pub cursor: Option<PageCursorWire>,
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
    StaleBaseView {
        current: Option<PageCursorWire>,
    },
    /// The store could not answer; nothing was read.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GetTaskReport {
    pub task: TaskWireRef,
    pub cursor: Option<PageCursorWire>,
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
    StaleBaseView {
        current: Option<PageCursorWire>,
    },
    /// The store could not answer; nothing was read.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GetReportSource {
    pub source: ReportSourceWireRef,
    /// Byte cursor into the body; [`None`] starts at zero.
    pub cursor: Option<u64>,
    /// `4..=16384`, default 4096. Out of range answers
    /// `UnsupportedFieldValue`.
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
    /// The store could not answer; nothing was changed.
    Unavailable,
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

#[cfg(test)]
mod tests {
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
    }
}
