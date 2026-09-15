//! Bounded report reads over the Task table group (PR §4.6, IPC §18).
//!
//! These queries feed reconnect reporting, the management view, and the
//! conversational report: they return stored facts only, apply their bound in
//! SQL, and mutate nothing. The saved lifecycle they expose is separate from
//! the Host's in-memory execution registration — there is no durable
//! "currently executing" flag, and a read never starts, repairs, re-evaluates,
//! or registers a runner.

use ene_primitive::RawId;

use crate::result::TaskResultId;
use crate::task::{TaskId, TaskProgress, TaskRevision};

/// Maximum rows one report page returns (IPC §18: query limits are 1..=50).
pub const REPORT_PAGE_MAX: u32 = 50;

/// One Task lifecycle headline: the stored current facts, without bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskHeadline {
    pub task: TaskId,
    pub revision: TaskRevision,
    pub progress: TaskProgress,
    /// The Task's assignee, the companion the work belongs to.
    pub assignee: RawId,
    /// Whether the current revision carries an adopted result (the durable
    /// completion marker). The result identity itself is a report detail row.
    pub adopted_result: bool,
}

/// Which owner row one report detail entry names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskReportRowKind {
    /// An `action_attempt` row, listed before results.
    ActionAttempt,
    /// A `task_result` row.
    TaskResult,
}

/// One report detail row identity, without its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskReportRow {
    pub kind: TaskReportRowKind,
    /// The Action attempt id or the Task result id.
    pub id: RawId,
    /// Result rows only: the revision the result was adopted at, when one was
    /// stamped. `None` is a recorded result that never became the completion.
    pub adopted_revision: Option<TaskRevision>,
}

/// Keyset position of one report detail row.
///
/// The order is `(kind, canonical id byte order)`: Action attempts first,
/// then results. The cursor is storage traversal order only and carries no
/// currentness meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskReportRowCursor {
    pub kind: TaskReportRowKind,
    pub id: RawId,
}

/// One task-owned textual report source to read in bounded byte pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskReportSourceRef {
    /// The purpose text snapshot of one Task revision.
    RevisionPurpose {
        task: TaskId,
        revision: TaskRevision,
    },
    /// The recorded body of one final result.
    ResultBody(TaskResultId),
}

/// One byte-bounded page of a report source body.
///
/// `text` is cut on a UTF-8 character boundary; `next` names the exact byte
/// cursor of the following page so a caller can page a body larger than one
/// frame without decoding it whole. `total_bytes` is the full body length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskReportSourcePage {
    pub text: String,
    pub total_bytes: u64,
    pub next: Option<u64>,
}
