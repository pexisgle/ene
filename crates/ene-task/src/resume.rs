//! Explicit resume of an interrupted Task (H-A.1 / AU17).
//!
//! Resume is a new Owner instruction to continue the remaining work of a
//! Task whose execution was lost (Host restart, dropped runner) without
//! terminal state, unknown effects, or an adoptable sealed result in the
//! way. The Task owner accepts it by moving the same [`TaskId`] to `r+1`
//! with the purpose carried over, recording only the resume instruction as
//! a new adopted-instruction entry (no body is copied; the body stays
//! canonical in History or in the first-party activity record), and creating
//! one new delegation and agent with the scope frozen from the current
//! workspace association. The old delegation is untouched: its late results
//! stay attributable to the original revision through the existing AU15
//! paths.
//!
//! The refusal priority is
//! `MissingTask → TaskTerminal → StalePremise → Superseded → AlreadyRunning
//! → HeldByUnknownEffects → ResultAvailable → NeedsRevalidation →
//! RevisionExhausted`. Every refusal is an `Ok`-side domain outcome with
//! zero Task writes; malformed rows, foreign sources, and forged references
//! are technical errors or ingress refusals, never an empty premise.

use ene_primitive::{RawId, WallClockWithTz};

use crate::context::TaskContextEntryId;
use crate::delegation::{DelegationId, DelegationRef, TaskAgentEphemeralId};
use crate::repository::{OwnerMessageCurrentness, TaskRepository, TaskTechnicalError};
use crate::result::TaskResultId;
use crate::task::{SteeringPremiseRef, TaskId, TaskProgress, TaskRef};

/// One explicit resume request: the relied-on revision and purpose plus the
/// new Owner instruction that continues the work.
///
/// The caller never mints identities and never copies the instruction body:
/// [`orchestrate_resume`](crate::orchestrate_resume) mints the new
/// revision's entry identities and the delegation/agent identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeTaskCommand {
    /// The relied-on revision and purpose, compared against durable state.
    pub premise: SteeringPremiseRef,
    /// The new Owner instruction that continues the work. Provenance only.
    pub instruction: ResumeInstructionSource,
}

/// Where the resume instruction's body canonically lives.
///
/// The body is never copied into Task state: the resume commit records the
/// reference as the new adopted-instruction entry's origin, and the Task
/// Agent turn resolves it through the [`TaskInstructionSource`](crate::TaskInstructionSource)
/// port like any other adopted instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResumeInstructionSource {
    /// An Owner conversation message. `currentness` is the same comparison
    /// material the conversation-sourced commits carry: the turn only
    /// commits while `message` is still the newest accepted Owner input.
    OwnerHistory {
        message: RawId,
        currentness: OwnerMessageCurrentness,
    },
    /// A first-party Owner management activity record, owned by the
    /// companion domain and read by primary key. The activity carries the
    /// explicitly selected Task ref and purpose, the resume instruction
    /// body, and the acceptance time.
    OwnerManagement { activity: RawId },
}

/// The Task owner's domain result for one explicit resume (H-A.1).
///
/// `Resumed` means the revision forward and the new delegation committed
/// durably; it is not provider transmission or external-effect success. A
/// launcher refusal or technical failure after the commit is reported
/// separately and never rewrites this outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskResumeOutcome {
    /// The Task moved to `r+1` and one new delegation committed.
    Resumed {
        task: TaskRef,
        delegation: DelegationRef,
    },
    /// The relied-on revision or purpose does not match the durable current
    /// state; nothing was changed and the caller re-evaluates.
    StalePremise { current: TaskRef },
    /// A newer accepted Owner input superseded the relied utterance;
    /// nothing was changed. Only the conversation-sourced guarded commit
    /// answers this.
    Superseded,
    /// The Task is terminal (`Completed` / `Failed` / `Cancelled`); nothing
    /// was changed. Absorbing, so it is distinct from revision staleness.
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
    /// The same Task already has a launch reservation or a running
    /// registration in this process; nothing was changed.
    AlreadyRunning { task: TaskId },
    /// At least one `Unknown` Action attempt remains under the Task (every
    /// revision and delegation); the blockers are read through the existing
    /// bounded report query, not carried here. Nothing was changed.
    HeldByUnknownEffects { task: TaskId },
    /// The current revision carries a sealed result that adoption may still
    /// complete; no new work was created. The orchestrator routes the result
    /// through the existing re-evaluation before answering.
    ResultAvailable { task: TaskId },
    /// A re-checkable premise is not currently available; nothing was
    /// changed and no provider I/O or Action started.
    NeedsRevalidation(TaskResumeHold),
    /// The premise names a Task with no durable state; nothing was changed.
    MissingTask { task: TaskId },
    /// No representable next revision can be durably committed; nothing was
    /// changed.
    RevisionExhausted { task: TaskId },
}

/// Which re-checkable premise holds a resume back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskResumeHold {
    /// The assignee companion is not `Running`.
    CompanionUnavailable,
    /// The Task has no current workspace association to freeze the new
    /// delegation's scope from.
    WorkspaceUnavailable,
    /// The resume instruction's canonical source cannot serve as an Owner
    /// instruction (absent row).
    InstructionUnavailable,
    /// The final permission / cap judgement for a new execution is not
    /// available; the AU14/AU5 gates still decide each use.
    PermissionUnavailable,
    /// A canonical source of the resumed input is covered by a current
    /// erasure condition.
    DataUseHeld,
    /// The Host cannot launch the new delegation (no runner available).
    ExecutionUnavailable,
}

/// The Host-known readiness premises one resume commit compares.
///
/// The companion lifecycle, workspace association, instruction source, and
/// erasure coverage are re-read from durable state inside the commit; only
/// the judgements the store cannot make travel here. Every flag is
/// comparison material for the commit's single transaction, never
/// authority: a stale `true` still loses to the durable facts the commit
/// reads (terminal, staleness, unknown effects, adoptable results).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskResumeReadiness {
    /// The final permission / cap judgement for a new execution is
    /// available. `false` answers
    /// [`TaskResumeHold::PermissionUnavailable`].
    pub permission_available: bool,
    /// No launch reservation or running registration covers the Task in
    /// this process. `false` answers [`TaskResumeOutcome::AlreadyRunning`].
    /// The Host evaluates this under the launch commit scope so the check,
    /// the commit, and the new reservation are one critical section.
    pub execution_free: bool,
    /// The Host can launch the new delegation. `false` answers
    /// [`TaskResumeHold::ExecutionUnavailable`].
    pub launch_possible: bool,
}

/// The premise for one resume commit (AU17).
///
/// The Task owner mints every identity before the repository call; the
/// repository stamps only the post-CAS `(task, revision)` references. The
/// purpose is always carried over: no purpose text travels here because no
/// purpose change is adopted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskResumeCommitPremise {
    /// The resume request with its relied-on revision, purpose, and
    /// instruction provenance.
    pub command: ResumeTaskCommand,
    /// The Host-known readiness premises compared in the commit.
    pub readiness: TaskResumeReadiness,
    /// Identity of the context entry carrying the purpose forward.
    pub adopted_purpose_entry: TaskContextEntryId,
    /// Identity of the context entry adopting the resume instruction.
    pub adopted_instruction_entry: TaskContextEntryId,
    /// Identity of the new delegation correspondence.
    pub delegation: DelegationId,
    /// Identity of the new ephemeral agent.
    pub agent: TaskAgentEphemeralId,
    /// When the resume was accepted for ordering.
    pub accepted_at: WallClockWithTz,
}

/// Mints the AU17 commit premise for one resume request.
///
/// [`orchestrate_resume`] uses this for the ordinary async commit; a Host
/// that must run the commit under a connection-ownership section (CCT
/// §10.4) mints the same premise and calls the repository's synchronous
/// commit itself. Identity minting is pure and never depends on connection
/// state, so both paths produce the same shape.
#[must_use]
pub fn resume_commit_premise(
    command: ResumeTaskCommand,
    readiness: TaskResumeReadiness,
) -> TaskResumeCommitPremise {
    TaskResumeCommitPremise {
        command,
        readiness,
        adopted_purpose_entry: TaskContextEntryId::generate(),
        adopted_instruction_entry: TaskContextEntryId::generate(),
        delegation: DelegationId::generate(),
        agent: TaskAgentEphemeralId::generate(),
        accepted_at: WallClockWithTz::now(),
    }
}

/// Orchestrates one explicit resume against the repository (H-A.1 / AU17).
///
/// The orchestration mints the new revision's context entry identities and
/// the delegation/agent identities, then calls
/// [`TaskRepository::commit_task_resume`]. That commit is the only
/// currentness guarantee: it compares the expected revision and purpose,
/// the non-terminal progress, the readiness premises, the Task-wide
/// `Unknown` barrier, and the adoptable sealed results inside its single
/// short transaction, in the documented refusal priority. A competing
/// winner between any earlier read and the commit is still refused there.
///
/// When the commit answers
/// [`TaskResumeOutcome::ResultAvailable`], the same orchestration routes
/// the current revision's sealed-but-unadopted results through the existing
/// [`reevaluate_result_adoption`](crate::reevaluate_result_adoption) gate
/// before answering, so a result that became adoptable does not wait for
/// another pass. The routing re-runs adoption only; it never starts
/// provider I/O, an Action, or an execution. The answered outcome stays
/// `ResultAvailable`.
pub async fn orchestrate_resume(
    repository: &impl TaskRepository,
    command: ResumeTaskCommand,
    readiness: TaskResumeReadiness,
) -> Result<TaskResumeOutcome, TaskTechnicalError> {
    let outcome = repository
        .commit_task_resume(resume_commit_premise(command, readiness))
        .await?;
    if let TaskResumeOutcome::ResultAvailable { task } = outcome {
        route_available_result(repository, task).await?;
    }
    Ok(outcome)
}

/// Orchestrates one conversation-sourced explicit resume.
///
/// Identical to [`orchestrate_resume`] except that the commit additionally
/// requires the relied Owner input to still be the newest accepted one: a
/// newer Owner input supersedes the turn and answers
/// [`TaskResumeOutcome::Superseded`] with zero writes.
pub async fn orchestrate_resume_current(
    repository: &impl crate::ConversationTaskRepository,
    command: ResumeTaskCommand,
    readiness: TaskResumeReadiness,
    currentness: OwnerMessageCurrentness,
) -> Result<TaskResumeOutcome, TaskTechnicalError> {
    let outcome = repository
        .commit_task_resume_from_conversation(
            resume_commit_premise(command, readiness),
            currentness,
        )
        .await?;
    if let TaskResumeOutcome::ResultAvailable { task } = outcome {
        route_available_result(repository, task).await?;
    }
    Ok(outcome)
}

/// Re-evaluates the current revision's sealed-but-unadopted results once.
///
/// The walk is Task-scoped and bounded: it pages the Task's own report
/// detail rows and re-runs the existing adoption gate only for results the
/// Task still relies on. A result that can only answer
/// `RecordedToOriginalOnly` is left to its history; a technical failure
/// fails closed instead of being rounded to availability.
///
/// Public so a Host that committed a resume through the repository's
/// synchronous boundary can run the same post-commit routing the async
/// orchestrators run.
pub async fn route_available_result(
    repository: &impl TaskRepository,
    task: TaskId,
) -> Result<(), TaskTechnicalError> {
    use crate::report::TaskReportRowKind;

    let Some(record) = repository.load_task(task).await? else {
        return Ok(());
    };
    let current = record.task.reference;
    let mut after = None;
    for _ in 0..4 {
        let page = repository
            .list_task_report_rows_after(task, after, crate::report::REPORT_PAGE_MAX)
            .await?;
        let full_page = page.len() as u32 == crate::report::REPORT_PAGE_MAX;
        after = page.last().map(|row| crate::report::TaskReportRowCursor {
            kind: row.kind,
            id: row.id,
        });
        for row in &page {
            if row.kind != TaskReportRowKind::TaskResult {
                continue;
            }
            let result = TaskResultId::from_raw(row.id);
            let Some(stored) = repository.load_task_result(result).await? else {
                continue;
            };
            if stored.adopted_revision.is_some() || stored.task != current {
                continue;
            }
            crate::reevaluate_result_adoption(repository, result).await?;
        }
        if !full_page {
            break;
        }
    }
    Ok(())
}
