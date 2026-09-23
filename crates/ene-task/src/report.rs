use ene_primitive::RawId;

use crate::result::TaskResultId;
use crate::task::{TaskId, TaskProgress, TaskPurposeRef, TaskRevision};

pub const REPORT_PAGE_MAX: u32 = 50;

pub const PAST_FACTS_ENTRY_CAP: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastExecutedFact {
    pub source: RawId,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastExecutedFactsPage {
    pub facts: Vec<PastExecutedFact>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskHeadline {
    pub task: TaskId,
    pub revision: TaskRevision,
    pub purpose: TaskPurposeRef,
    pub progress: TaskProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskReportRowKind {
    ActionAttempt,
    TaskResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskReportRow {
    pub kind: TaskReportRowKind,
    pub id: RawId,
    pub adopted_revision: Option<TaskRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskReportRowCursor {
    pub kind: TaskReportRowKind,
    pub id: RawId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskReportSourceRef {
    RevisionPurpose {
        task: TaskId,
        revision: TaskRevision,
    },
    ResultBody(TaskResultId),
}

/// One byte-bounded page of a report source body.
///
/// `text` is cut on a UTF-8 character boundary; `next` names the exact byte
/// cursor of the following page so a caller can page a body larger than one
/// frame without decoding it whole. `total_bytes` is the full body length.
#[derive(Clone, PartialEq, Eq)]
pub struct TaskReportSourcePage {
    pub text: String,
    pub total_bytes: u64,
    pub next: Option<u64>,
}

/// Diagnostics see lengths and cursors, never the body text.
impl core::fmt::Debug for TaskReportSourcePage {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TaskReportSourcePage")
            .field("text", &"[redacted]")
            .field("total_bytes", &self.total_bytes)
            .field("next", &self.next)
            .finish()
    }
}
