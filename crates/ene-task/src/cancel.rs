//! Task cancel admission (AU16).
//!
//! Accepting a cancel request and stopping running work are separate facts.
//! The only durable fact this module defines is the admission: one
//! [`TaskRepository::cancel_task`](crate::TaskRepository::cancel_task)
//! compare-and-set that moves a non-terminal `task.progress` to
//! [`TaskProgress::Cancelled`](crate::TaskProgress::Cancelled). In-memory
//! cancellation tokens, dropped futures, and stop signals are not authority;
//! they can be lost on restart and never prove that a provider call or an
//! external effect stopped.
//!
//! There is deliberately no `orchestrate_cancel` wrapper and no cancel-specific
//! gate, row, flag, or outcome variant on the other admission boundaries:
//! `Cancelled` is a terminal progress value, so the existing non-terminal
//! gates (delegation AU3, steering AU4, inference claim AU14, Action start
//! AU5, result adoption AU15b) refuse new work without a second condition.
//! Already-started activity keeps its durable facts (inference attempts,
//! `data_use` correlation, Action certainty, result arrival and seal).
//! A cancelled Task is never resumed in place: re-execution creates a new
//! Task and a new delegation under it.

use crate::task::{TaskId, TaskProgress};

/// The cancel request accepted at the Task boundary (AU16).
///
/// The request carries the target Task only. It has no reason body: the
/// conversation or the first-party management path keeps the reason and the
/// command references it; the Task row never duplicates that text. The
/// request carries no revision premise because cancel is a Task-level
/// operation: it is not made stale by a concurrent steering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancelTaskCommand {
    pub task: TaskId,
}

/// The Task owner's domain result for one cancel request (AU16).
///
/// Every variant is an `Ok`-side domain answer. `CancelAccepted` means exactly
/// that the request was accepted and durably recorded; it never means that
/// running provider I/O or an external effect stopped, or that an unknown
/// outcome was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCancelOutcome {
    /// The current progress (`Started` / `InProgress`) was moved to
    /// `Cancelled` in one atomic compare-and-set. The admission is durable and
    /// survives restart.
    CancelAccepted,
    /// The Task is already `Cancelled`; the idempotent re-request wrote
    /// nothing (admission happens exactly once).
    AlreadyCancelled,
    /// The Task is terminal for another reason (`Completed` / `Failed`) and
    /// cannot be cancelled; nothing was written.
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
    /// The premise names a Task with no durable state; nothing was written.
    MissingTask { task: TaskId },
}
