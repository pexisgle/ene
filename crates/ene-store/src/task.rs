use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;

use ene_action::ActionCertainty;
use ene_companion::{HistoryRole, TaskFact, TerminalKindWire, UndeliveredSource};
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, ConversationTaskRepository, DelegatedWorkspace, DelegationCreationPremise,
    DelegationId, DelegationOutcome, DelegationRef, DelegationScope, OwnerMessageCurrentness,
    PAST_FACTS_ENTRY_CAP, PastExecutedFact, PastExecutedFactsPage, REPORT_PAGE_MAX,
    ResumeInstructionSource, Task, TaskAgentEphemeralId, TaskAgentObservationId,
    TaskAgentObservationPremise, TaskAgentOutput, TaskAgentResultArrival, TaskCancelOutcome,
    TaskCommitOutcome, TaskCommitPremise, TaskContextEntry, TaskContextEntryId, TaskContextItem,
    TaskContextOrigin, TaskContextOriginKind, TaskCreationOutcome, TaskCreationPremise,
    TaskFailureOutcome, TaskFailurePremise, TaskHeadline, TaskId, TaskProgress, TaskPurpose,
    TaskPurposeRef, TaskRecord, TaskRef, TaskReportRow, TaskReportRowCursor, TaskReportRowKind,
    TaskReportSourcePage, TaskReportSourceRef, TaskRepository, TaskResultAcceptance,
    TaskResultAdoptionClaim, TaskResultArrivalOutcome, TaskResultId, TaskResultRecord,
    TaskResumeCommitPremise, TaskResumeHold, TaskResumeOutcome, TaskRevision, TaskRevisionRecord,
    TaskTechnicalError, UnadoptedResultCursor, WorkspaceAssocId, WorkspaceAssociation,
    WorkspaceFolderRef,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{
    decode_id, decode_lifecycle, decode_role, decode_u64, encode_id, encode_u64, lock_shared,
};
use crate::companion::{
    ACTIVITY_KIND_RESUME_INSTRUCTION, SQL_SELECT_ACTIVITY_PREMISE, SQL_SELECT_COMPANION_LIFECYCLE,
    SQL_SELECT_HISTORY_PREMISE,
};
use crate::run_blocking;

const SQL_INSERT_TASK: &str = "INSERT INTO task (task_id, revision, purpose_adopted_revision, purpose_text, assignee, progress) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

const SQL_INSERT_TASK_REVISION: &str = "INSERT INTO task_revision (task_id, revision, purpose_adopted_revision, purpose_text, assignee) VALUES (?1, ?2, ?3, ?4, ?5)";

const SQL_INSERT_TASK_CONTEXT_ENTRY: &str = "INSERT INTO task_context_entry (entry_id, task_id, revision, item_kind, purpose_adopted_revision, origin_kind, origin_source, acquired_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

const SQL_INSERT_WORKSPACE_ASSOC: &str =
    "INSERT INTO workspace_assoc (assoc_id, task_id, folder, save_target) VALUES (?1, ?2, ?3, ?4)";

const SQL_SELECT_TASK: &str =
    "SELECT revision, purpose_adopted_revision, progress, assignee FROM task WHERE task_id = ?1";

const SQL_SELECT_TASK_ASSIGNEE: &str = "SELECT assignee FROM task WHERE task_id = ?1";

const SQL_MARK_TASK_IN_PROGRESS: &str = "UPDATE task SET progress = 'in_progress' WHERE task_id = ?1 AND progress IN ('started', 'in_progress')";

const SQL_COMPLETE_TASK: &str = "UPDATE task SET progress = 'completed' WHERE task_id = ?1 AND progress IN ('started', 'in_progress')";

const SQL_CANCEL_TASK: &str = "UPDATE task SET progress = 'cancelled' WHERE task_id = ?1 AND progress IN ('started', 'in_progress')";

const SQL_UPDATE_TASK: &str = "UPDATE task SET revision = ?2, purpose_adopted_revision = ?3, purpose_text = ?4 WHERE task_id = ?1";

const SQL_SELECT_TASK_REVISION: &str = "SELECT purpose_adopted_revision, purpose_text, assignee FROM task_revision WHERE task_id = ?1 AND revision = ?2";

const SQL_SELECT_TASK_CONTEXT: &str = "SELECT entry_id, revision, item_kind, purpose_adopted_revision, origin_kind, origin_source, acquired_at FROM task_context_entry WHERE task_id = ?1 ORDER BY revision, entry_id";

const SQL_SELECT_ADOPTED_PURPOSE_ENTRY: &str = "SELECT purpose_adopted_revision, origin_kind, origin_source, acquired_at FROM task_context_entry WHERE task_id = ?1 AND revision = ?2 AND item_kind = ?3 ORDER BY rowid LIMIT 2";

const SQL_SELECT_WORKSPACE_ASSOCS: &str = "SELECT assoc_id, folder, save_target FROM workspace_assoc WHERE task_id = ?1 ORDER BY rowid LIMIT 2";

const SQL_INSERT_DELEGATION: &str = "INSERT INTO delegation (delegation_id, task_id, task_revision, delegator, agent, scope_assoc, scope_folder, scope_save_target) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)";

const SQL_SELECT_DELEGATION: &str = "SELECT task_id, task_revision, delegator, agent, scope_assoc, scope_folder, scope_save_target FROM delegation WHERE delegation_id = ?1";

const SQL_SELECT_DELEGATION_CORRESPONDENCE: &str =
    "SELECT task_id, task_revision FROM delegation WHERE delegation_id = ?1";

const SQL_SELECT_RESULT_BY_ID: &str = "SELECT task_id, task_revision, delegation_id, body, adopted_revision, recorded_at FROM task_result WHERE result_id = ?1";

const SQL_SELECT_RESULT_BY_DELEGATION: &str = "SELECT result_id, task_id, task_revision, body, adopted_revision, recorded_at FROM task_result WHERE delegation_id = ?1";

const SQL_DELEGATION_HAS_STARTED_WORK: &str = "SELECT EXISTS(SELECT 1 FROM inference_attempt WHERE delegation_id = ?1 UNION ALL SELECT 1 FROM action_attempt WHERE delegation_id = ?1)";

const SQL_SELECT_OBSERVATION: &str = "SELECT delegation_id, task_id, task_revision, workspace_assoc_id, action_attempt_id, path, body_observed, observed_at FROM task_agent_observation WHERE observation_id = ?1";

const SQL_INSERT_OBSERVATION: &str = "INSERT INTO task_agent_observation (observation_id, delegation_id, task_id, task_revision, workspace_assoc_id, action_attempt_id, path, body_observed, observed_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)";

const SQL_SELECT_OBSERVATION_ATTEMPT: &str = "SELECT delegation_id, task_id, task_revision, workspace_assoc_id, real_target, operation FROM action_attempt WHERE attempt_id = ?1";

const SQL_INSERT_RESULT: &str = "INSERT INTO task_result (result_id, task_id, task_revision, delegation_id, body, adopted_revision, recorded_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

const SQL_FAIL_TASK: &str = "UPDATE task SET progress = 'failed' WHERE task_id = ?1 AND revision = ?2 AND progress IN ('started', 'in_progress')";

const SQL_SELECT_RESULT_CORRESPONDENCE: &str =
    "SELECT task_id, task_revision, delegation_id FROM task_result WHERE result_id = ?1";

const SQL_LIST_UNADOPTED_FIRST: &str = "SELECT r.result_id, r.recorded_at FROM task_result r JOIN task t ON t.task_id = r.task_id WHERE r.adopted_revision IS NULL AND t.progress IN ('started', 'in_progress') AND t.revision = r.task_revision ORDER BY r.recorded_at, r.result_id LIMIT ?1";

const SQL_LIST_UNADOPTED_AFTER: &str = "SELECT r.result_id, r.recorded_at FROM task_result r JOIN task t ON t.task_id = r.task_id WHERE r.adopted_revision IS NULL AND t.progress IN ('started', 'in_progress') AND t.revision = r.task_revision AND (r.recorded_at > ?1 OR (r.recorded_at = ?1 AND r.result_id > ?2)) ORDER BY r.recorded_at, r.result_id LIMIT ?3";

const SQL_SELECT_RESULT_ATTEMPTS: &str =
    "SELECT attempt_id FROM task_result_attempt WHERE result_id = ?1 ORDER BY attempt_id";

const SQL_INSERT_RESULT_ATTEMPT: &str =
    "INSERT INTO task_result_attempt (result_id, attempt_id) VALUES (?1, ?2)";

const SQL_SELECT_DELEGATION_ATTEMPTS: &str = "SELECT attempt_id, task_id, task_revision, certainty FROM action_attempt WHERE delegation_id = ?1 ORDER BY attempt_id";

const SQL_SELECT_TASK_ATTEMPTS: &str = "SELECT a.attempt_id, a.task_id, a.task_revision, a.delegation_id, d.task_id, d.task_revision, a.certainty FROM action_attempt a LEFT JOIN delegation d ON d.delegation_id = a.delegation_id WHERE d.task_id = ?1 OR a.task_id = ?1 ORDER BY a.attempt_id";

const SQL_SELECT_ATTEMPT_CORRESPONDENCE: &str =
    "SELECT delegation_id, task_id, task_revision FROM action_attempt WHERE attempt_id = ?1";

const SQL_MARK_RESULT_ADOPTED: &str = "UPDATE task_result SET adopted_revision = ?2 WHERE result_id = ?1 AND adopted_revision IS NULL";

const SQL_SELECT_ADOPTED_RESULT: &str = "SELECT result_id, task_revision, adopted_revision FROM task_result WHERE task_id = ?1 AND adopted_revision IS NOT NULL ORDER BY rowid LIMIT 2";

const ORIGIN_KIND_OWNER_CONVERSATION: &str = "owner_conversation";
const ORIGIN_KIND_SPONTANEOUS: &str = "spontaneous";
const ORIGIN_KIND_SCHEDULE_OCCURRENCE: &str = "schedule_occurrence";

/// The stored `item_kind` discriminators. The kind decides which payload is
/// required: an adopted purpose carries the adopted revision, an adopted
/// instruction carries no payload because the entry identity is the adoption
/// identity.
const ITEM_KIND_ADOPTED_PURPOSE: &str = "adopted_purpose";
const ITEM_KIND_ADOPTED_INSTRUCTION: &str = "adopted_instruction";

const ORIGIN_KIND_OWNER_MANAGEMENT: &str = "owner_management";

/// The current-revision sealed-but-unadopted results for the resume
/// availability check: `adopted_revision IS NULL` on the relied revision is
/// the durable "may still adopt" marker. Bodies are never read here.
const SQL_UNADOPTED_RESULTS_AT_REVISION: &str = "SELECT delegation_id FROM task_result WHERE task_id = ?1 AND task_revision = ?2 AND adopted_revision IS NULL ORDER BY result_id";

const SQL_PAST_FACT_ROWS: &str = "SELECT row_kind, row_id FROM (SELECT 'action_attempt' AS row_kind, attempt_id AS row_id, 0 AS rank, rowid AS seq FROM action_attempt WHERE task_id = ?1 UNION ALL SELECT 'task_result', result_id, 1, rowid FROM task_result WHERE task_id = ?1) ORDER BY rank, seq LIMIT ?2";

const SQL_PAST_ATTEMPT_FACT: &str =
    "SELECT operation, real_target, certainty FROM action_attempt WHERE attempt_id = ?1";

const SQL_PAST_RESULT_FACT: &str =
    "SELECT task_revision, adopted_revision FROM task_result WHERE result_id = ?1";
const SQL_LIST_TASKS_FIRST: &str = "SELECT task_id, revision, purpose_adopted_revision, progress, assignee, EXISTS(SELECT 1 FROM task_result r WHERE r.task_id = task.task_id AND r.adopted_revision = task.revision) FROM task ORDER BY task_id LIMIT ?1";

const SQL_LIST_TASKS_AFTER: &str = "SELECT task_id, revision, purpose_adopted_revision, progress, assignee, EXISTS(SELECT 1 FROM task_result r WHERE r.task_id = task.task_id AND r.adopted_revision = task.revision) FROM task WHERE task_id > ?1 ORDER BY task_id LIMIT ?2";

const SQL_LIST_REPORT_ROWS_FIRST: &str = "SELECT row_kind, row_id, adopted FROM (SELECT 'action_attempt' AS row_kind, attempt_id AS row_id, NULL AS adopted, 0 AS rank FROM action_attempt WHERE task_id = ?1 UNION ALL SELECT 'task_result', result_id, adopted_revision, 1 FROM task_result WHERE task_id = ?1) ORDER BY rank, row_id LIMIT ?2";

const SQL_LIST_REPORT_ROWS_AFTER: &str = "SELECT row_kind, row_id, adopted FROM (SELECT 'action_attempt' AS row_kind, attempt_id AS row_id, NULL AS adopted, 0 AS rank FROM action_attempt WHERE task_id = ?1 UNION ALL SELECT 'task_result', result_id, adopted_revision, 1 FROM task_result WHERE task_id = ?1) WHERE rank > ?2 OR (rank = ?2 AND row_id > ?3) ORDER BY rank, row_id LIMIT ?4";

const SQL_REPORT_REVISION_PAGE: &str = "SELECT length(CAST(purpose_text AS BLOB)), substr(CAST(purpose_text AS BLOB), ?3, ?4) FROM task_revision WHERE task_id = ?1 AND revision = ?2";

const SQL_REPORT_RESULT_PAGE: &str = "SELECT length(CAST(body AS BLOB)), substr(CAST(body AS BLOB), ?2, ?3) FROM task_result WHERE result_id = ?1";

fn task_unavailable(reason: impl core::fmt::Display) -> TaskTechnicalError {
    TaskTechnicalError::StorageUnavailable {
        reason: reason.to_string(),
    }
}

fn register_task_undelivered(
    tx: &rusqlite::Transaction<'_>,
    companion_text: &str,
    task: RawId,
    fact: TaskFact,
) -> Result<(), TaskTechnicalError> {
    let source = UndeliveredSource::TaskRecord { task, fact };
    crate::companion::register_undelivered_tx(
        tx,
        companion_text,
        RawId::new(),
        &source,
        None,
        None,
        WallClockWithTz::now(),
    )
    .map_err(task_unavailable)
}

fn decode_revision(raw: i64) -> Result<TaskRevision, TaskTechnicalError> {
    Ok(TaskRevision::from_u64(
        decode_u64(raw).map_err(task_unavailable)?,
    ))
}

/// Decodes the stored progress, closed world: an unknown name or a stored
/// NULL (nullable in the schema) is an unreadable row, never a default.
fn decode_progress(raw: Option<&str>) -> Result<TaskProgress, TaskTechnicalError> {
    let text = raw.ok_or_else(|| task_unavailable("task progress is missing"))?;
    TaskProgress::from_name(text).ok_or_else(|| task_unavailable("unknown task progress"))
}

fn decode_certainty(raw: &str) -> Result<ActionCertainty, TaskTechnicalError> {
    ActionCertainty::from_name(raw).ok_or_else(|| task_unavailable("unknown action certainty"))
}

fn encode_origin_kind(kind: TaskContextOriginKind) -> &'static str {
    match kind {
        TaskContextOriginKind::OwnerConversation => ORIGIN_KIND_OWNER_CONVERSATION,
        TaskContextOriginKind::OwnerManagement => ORIGIN_KIND_OWNER_MANAGEMENT,
        TaskContextOriginKind::Spontaneous => ORIGIN_KIND_SPONTANEOUS,
        TaskContextOriginKind::ScheduleOccurrence => ORIGIN_KIND_SCHEDULE_OCCURRENCE,
    }
}

fn decode_origin_kind(text: &str) -> Result<TaskContextOriginKind, TaskTechnicalError> {
    match text {
        ORIGIN_KIND_OWNER_CONVERSATION => Ok(TaskContextOriginKind::OwnerConversation),
        ORIGIN_KIND_OWNER_MANAGEMENT => Ok(TaskContextOriginKind::OwnerManagement),
        ORIGIN_KIND_SPONTANEOUS => Ok(TaskContextOriginKind::Spontaneous),
        ORIGIN_KIND_SCHEDULE_OCCURRENCE => Ok(TaskContextOriginKind::ScheduleOccurrence),
        _ => Err(task_unavailable("unknown task context origin kind")),
    }
}

fn decode_context_item(
    task: TaskId,
    item_kind: &str,
    purpose_adopted_revision: Option<i64>,
) -> Result<TaskContextItem, TaskTechnicalError> {
    match item_kind {
        ITEM_KIND_ADOPTED_PURPOSE => {
            let raw = purpose_adopted_revision.ok_or_else(|| {
                task_unavailable("task adopted purpose entry missing its adopted revision")
            })?;
            Ok(TaskContextItem::AdoptedPurpose(TaskPurposeRef {
                task,
                adopted_revision: decode_revision(raw)?,
            }))
        }
        ITEM_KIND_ADOPTED_INSTRUCTION => {
            if purpose_adopted_revision.is_some() {
                return Err(task_unavailable(
                    "task adopted instruction entry carries an adopted revision",
                ));
            }
            Ok(TaskContextItem::AdoptedInstruction)
        }
        _ => Err(task_unavailable("unknown task context item kind")),
    }
}

fn decode_assignee(text: &str) -> Result<AssigneeRef, TaskTechnicalError> {
    Ok(AssigneeRef {
        companion: decode_id(text).map_err(task_unavailable)?,
    })
}

fn decode_clock(text: &str) -> Result<WallClockWithTz, TaskTechnicalError> {
    WallClockWithTz::parse_rfc3339(text).map_err(task_unavailable)
}

fn owner_message_is_current(
    tx: &rusqlite::Transaction<'_>,
    currentness: &OwnerMessageCurrentness,
) -> Result<bool, TaskTechnicalError> {
    let expected_rowid: Option<i64> = tx
        .query_row(
            crate::companion::SQL_SELECT_OWNER_ROWID,
            params![encode_id(currentness.message)],
            |row| row.get(0),
        )
        .optional()
        .map_err(task_unavailable)?;
    let Some(expected_rowid) = expected_rowid else {
        return Ok(false);
    };
    let newer = tx
        .query_row(
            crate::companion::SQL_EXISTS_NEWER_OWNER,
            params![
                encode_id(currentness.companion),
                crate::codec::encode_role(ene_companion::HistoryRole::Owner),
                expected_rowid
            ],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(task_unavailable)?;
    Ok(newer.is_none())
}

fn create_task_sync(
    conn: &Mutex<Connection>,
    premise: TaskCreationPremise,
    currentness: Option<OwnerMessageCurrentness>,
) -> Result<TaskCreationOutcome, TaskTechnicalError> {
    let revision = TaskRevision::initial();
    let reference = TaskRef {
        task: premise.task,
        revision,
    };
    let task_text = encode_id(premise.task.as_raw());
    let revision_raw = encode_u64(revision.as_u64()).map_err(task_unavailable)?;
    let assignee_text = encode_id(premise.assignee.companion);
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(task_unavailable)?;
    if let Some(currentness) = currentness
        && !owner_message_is_current(&tx, &currentness)?
    {
        return Ok(TaskCreationOutcome::Superseded);
    }
    let purpose_text = crate::preservation::redact_covered_text(&tx, &premise.purpose.text)
        .map_err(|error| task_unavailable(error.to_string()))?;
    let workspace_paths = match &premise.workspace {
        None => (None, None),
        Some(workspace) => (
            Some(
                crate::preservation::redact_covered_text(&tx, &workspace.need.folder.path)
                    .map_err(|error| task_unavailable(error.to_string()))?,
            ),
            workspace
                .need
                .save_target
                .as_ref()
                .map(|target| crate::preservation::redact_covered_text(&tx, &target.path))
                .transpose()
                .map_err(|error| task_unavailable(error.to_string()))?,
        ),
    };
    tx.execute(
        SQL_INSERT_TASK,
        params![
            task_text,
            revision_raw,
            revision_raw,
            purpose_text,
            assignee_text,
            TaskProgress::Started.as_str(),
        ],
    )
    .map_err(task_unavailable)?;
    tx.execute(
        SQL_INSERT_TASK_REVISION,
        params![
            task_text,
            revision_raw,
            revision_raw,
            purpose_text,
            assignee_text,
        ],
    )
    .map_err(task_unavailable)?;
    tx.execute(
        SQL_INSERT_TASK_CONTEXT_ENTRY,
        params![
            encode_id(premise.entry.as_raw()),
            task_text,
            revision_raw,
            ITEM_KIND_ADOPTED_PURPOSE,
            revision_raw,
            encode_origin_kind(premise.origin.kind),
            encode_id(premise.origin.source),
            premise.acquired_at.to_rfc3339(),
        ],
    )
    .map_err(task_unavailable)?;
    if let Some(workspace) = &premise.workspace {
        let (folder, save_target) = &workspace_paths;
        tx.execute(
            SQL_INSERT_WORKSPACE_ASSOC,
            params![
                encode_id(workspace.assoc.as_raw()),
                task_text,
                folder.as_deref(),
                save_target.as_deref(),
            ],
        )
        .map_err(task_unavailable)?;
    }
    register_task_undelivered(
        &tx,
        &assignee_text,
        premise.task.as_raw(),
        TaskFact::TaskRevision {
            task: premise.task.as_raw(),
            revision: revision.as_u64(),
        },
    )?;
    tx.commit().map_err(task_unavailable)?;
    Ok(TaskCreationOutcome::Created(reference))
}

struct RawAdoptedPurpose {
    purpose_adopted_revision: Option<i64>,
    origin_kind: String,
    origin_source: String,
    acquired_at: String,
}

fn validated_adopted_purpose_entry(
    conn: &Connection,
    task_text: &str,
    revision: i64,
    expected_adopted_revision: TaskRevision,
) -> Result<RawAdoptedPurpose, TaskTechnicalError> {
    let mut statement = conn
        .prepare(SQL_SELECT_ADOPTED_PURPOSE_ENTRY)
        .map_err(task_unavailable)?;
    let rows = statement
        .query_map(
            params![task_text, revision, ITEM_KIND_ADOPTED_PURPOSE],
            |row| {
                Ok(RawAdoptedPurpose {
                    purpose_adopted_revision: row.get(0)?,
                    origin_kind: row.get(1)?,
                    origin_source: row.get(2)?,
                    acquired_at: row.get(3)?,
                })
            },
        )
        .map_err(task_unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(task_unavailable)?;
    let mut rows = rows.into_iter();
    let entry = rows.next().ok_or_else(|| {
        task_unavailable("task adopted purpose entry missing for the current revision")
    })?;
    if rows.next().is_some() {
        return Err(task_unavailable(
            "multiple adopted purpose entries for the current revision",
        ));
    }
    let adopted = entry.purpose_adopted_revision.ok_or_else(|| {
        task_unavailable("task adopted purpose entry missing its adopted revision")
    })?;
    if decode_revision(adopted)? != expected_adopted_revision {
        return Err(task_unavailable(
            "task context adopted purpose does not match the current purpose",
        ));
    }
    Ok(entry)
}

/// Decodes the provenance identity of one validated adopted-purpose entry with
/// the same checks the Task read applies to every context row: an unknown
/// origin kind, an undecodable source, or a malformed acquisition time is an
/// unreadable row. The steering carry-forward and the resume carry-forward
/// share this, so neither re-stamps provenance the Task reads would reject.
fn validate_adopted_purpose_provenance(
    entry: &RawAdoptedPurpose,
) -> Result<(), TaskTechnicalError> {
    decode_origin_kind(&entry.origin_kind)?;
    decode_id(&entry.origin_source).map_err(task_unavailable)?;
    decode_clock(&entry.acquired_at)?;
    Ok(())
}

fn forward_steering_sync(
    conn: &Mutex<Connection>,
    premise: TaskCommitPremise,
    currentness: Option<OwnerMessageCurrentness>,
) -> Result<TaskCommitOutcome, TaskTechnicalError> {
    let task = premise.expected.task;
    let task_text = encode_id(task.as_raw());
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(task_unavailable)?;
    if let Some(currentness) = currentness
        && !owner_message_is_current(&tx, &currentness)?
    {
        return Ok(TaskCommitOutcome::Superseded);
    }
    let current: Option<RawTask> = tx
        .query_row(SQL_SELECT_TASK, params![task_text], raw_task_row)
        .optional()
        .map_err(task_unavailable)?;
    let Some(current) = current else {
        return Ok(TaskCommitOutcome::MissingTask { task });
    };
    let current_progress = decode_progress(current.progress.as_deref())?;
    if current_progress.is_terminal() {
        return Ok(TaskCommitOutcome::TaskTerminal {
            task,
            progress: current_progress,
        });
    }
    let current_revision = decode_revision(current.revision)?;
    if current_revision != premise.expected.revision {
        return Ok(TaskCommitOutcome::StaleExpected {
            current: TaskRef {
                task,
                revision: current_revision,
            },
        });
    }
    let Some(next_revision) = current_revision.checked_next() else {
        return Ok(TaskCommitOutcome::RevisionExhausted { task });
    };
    let Ok(next_raw) = encode_u64(next_revision.as_u64()) else {
        return Ok(TaskCommitOutcome::RevisionExhausted { task });
    };
    let current_adopted = decode_revision(current.purpose_adopted_revision)?;
    let snapshot: RawTaskRevision = tx
        .query_row(
            SQL_SELECT_TASK_REVISION,
            params![task_text, current.revision],
            raw_revision_row,
        )
        .optional()
        .map_err(task_unavailable)?
        .ok_or_else(|| {
            task_unavailable("task revision snapshot missing for the current revision")
        })?;
    if decode_revision(snapshot.purpose_adopted_revision)? != current_adopted {
        return Err(task_unavailable(
            "task revision purpose does not match the current purpose",
        ));
    }
    if decode_assignee(&snapshot.assignee)? != decode_assignee(&current.assignee)? {
        return Err(task_unavailable(
            "task revision assignee does not match the current assignee",
        ));
    }
    let current_purpose_entry =
        validated_adopted_purpose_entry(&tx, &task_text, current.revision, current_adopted)?;
    if let Some(instruction) = &premise.adopted_instruction
        && crate::preservation::covering_condition(&tx, &encode_id(instruction.origin.source))
            .map_err(|error| task_unavailable(error.to_string()))?
            .is_some()
    {
        return Ok(TaskCommitOutcome::HeldForErasure);
    }
    if let Some(adoption) = &premise.new_purpose
        && (crate::preservation::covering_text(&tx, &adoption.purpose.text)
            .map_err(|error| task_unavailable(error.to_string()))?
            .is_some()
            || crate::preservation::covering_condition(&tx, &encode_id(adoption.origin.source))
                .map_err(|error| task_unavailable(error.to_string()))?
                .is_some())
    {
        return Ok(TaskCommitOutcome::HeldForErasure);
    }
    let (adopted_revision, purpose_text, origin_kind, origin_source, acquired_at) =
        match premise.new_purpose {
            Some(adoption) => (
                next_revision,
                adoption.purpose.text,
                encode_origin_kind(adoption.origin.kind).to_owned(),
                encode_id(adoption.origin.source),
                adoption.acquired_at.to_rfc3339(),
            ),
            None => {
                validate_adopted_purpose_provenance(&current_purpose_entry)?;
                (
                    current_adopted,
                    crate::preservation::redact_covered_text(&tx, &snapshot.purpose_text)
                        .map_err(|error| task_unavailable(error.to_string()))?,
                    current_purpose_entry.origin_kind,
                    current_purpose_entry.origin_source,
                    current_purpose_entry.acquired_at,
                )
            }
        };
    let adopted_raw = encode_u64(adopted_revision.as_u64()).map_err(task_unavailable)?;
    tx.execute(
        SQL_INSERT_TASK_REVISION,
        params![
            task_text,
            next_raw,
            adopted_raw,
            purpose_text,
            current.assignee
        ],
    )
    .map_err(task_unavailable)?;
    tx.execute(
        SQL_INSERT_TASK_CONTEXT_ENTRY,
        params![
            encode_id(premise.adopted_purpose_entry.as_raw()),
            task_text,
            next_raw,
            ITEM_KIND_ADOPTED_PURPOSE,
            adopted_raw,
            origin_kind,
            origin_source,
            acquired_at
        ],
    )
    .map_err(task_unavailable)?;
    if let Some(instruction) = premise.adopted_instruction {
        tx.execute(
            SQL_INSERT_TASK_CONTEXT_ENTRY,
            params![
                encode_id(instruction.entry.as_raw()),
                task_text,
                next_raw,
                ITEM_KIND_ADOPTED_INSTRUCTION,
                Option::<i64>::None,
                encode_origin_kind(instruction.origin.kind),
                encode_id(instruction.origin.source),
                instruction.acquired_at.to_rfc3339(),
            ],
        )
        .map_err(task_unavailable)?;
    }
    tx.execute(
        SQL_UPDATE_TASK,
        params![task_text, next_raw, adopted_raw, purpose_text],
    )
    .map_err(task_unavailable)?;
    register_task_undelivered(
        &tx,
        &current.assignee,
        task.as_raw(),
        TaskFact::TaskRevision {
            task: task.as_raw(),
            revision: next_revision.as_u64(),
        },
    )?;
    tx.commit().map_err(task_unavailable)?;
    Ok(TaskCommitOutcome::CommittedAs(TaskRef {
        task,
        revision: next_revision,
    }))
}

fn cancel_task_sync(
    conn: &Mutex<Connection>,
    task: TaskId,
    currentness: Option<OwnerMessageCurrentness>,
) -> Result<TaskCancelOutcome, TaskTechnicalError> {
    let task_text = encode_id(task.as_raw());
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(task_unavailable)?;
    if let Some(currentness) = currentness
        && !owner_message_is_current(&tx, &currentness)?
    {
        return Ok(TaskCancelOutcome::Superseded);
    }
    let current: Option<RawTask> = tx
        .query_row(SQL_SELECT_TASK, params![task_text], raw_task_row)
        .optional()
        .map_err(task_unavailable)?;
    let Some(current) = current else {
        return Ok(TaskCancelOutcome::MissingTask { task });
    };
    let current_progress = decode_progress(current.progress.as_deref())?;
    if current_progress == TaskProgress::Cancelled {
        return Ok(TaskCancelOutcome::AlreadyCancelled);
    }
    if current_progress.is_terminal() {
        return Ok(TaskCancelOutcome::TaskTerminal {
            task,
            progress: current_progress,
        });
    }
    let moved = tx
        .execute(SQL_CANCEL_TASK, params![task_text])
        .map_err(task_unavailable)?;
    if moved != 1 {
        return Err(task_unavailable(
            "cancel CAS did not move exactly the current non-terminal task",
        ));
    }
    register_task_undelivered(
        &tx,
        &current.assignee,
        task.as_raw(),
        TaskFact::Terminal {
            task: task.as_raw(),
            progress: TerminalKindWire::Cancelled,
        },
    )?;
    tx.commit().map_err(task_unavailable)?;
    Ok(TaskCancelOutcome::CancelAccepted)
}

fn fail_task_sync(
    conn: &Mutex<Connection>,
    premise: TaskFailurePremise,
) -> Result<TaskFailureOutcome, TaskTechnicalError> {
    let task = premise.task.task;
    let task_text = encode_id(task.as_raw());
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(task_unavailable)?;
    let current: Option<RawTask> = tx
        .query_row(SQL_SELECT_TASK, params![task_text], raw_task_row)
        .optional()
        .map_err(task_unavailable)?;
    let Some(current) = current else {
        return Ok(TaskFailureOutcome::MissingTask { task });
    };
    let current_revision = decode_revision(current.revision)?;
    if let Some(delegation) = premise.delegation {
        let correspondence: Option<(String, i64)> = tx
            .query_row(
                SQL_SELECT_DELEGATION_CORRESPONDENCE,
                params![encode_id(delegation.as_raw())],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(task_unavailable)?;
        let Some((delegation_task, delegation_revision)) = correspondence else {
            return Ok(TaskFailureOutcome::MissingDelegation { delegation });
        };
        if decode_id(&delegation_task).map_err(task_unavailable)? != task.as_raw() {
            return Err(task_unavailable(
                "failure delegation does not belong to the premise's task",
            ));
        }
        if decode_revision(delegation_revision)? != premise.task.revision {
            return Ok(TaskFailureOutcome::StalePremise {
                current: TaskRef {
                    task,
                    revision: current_revision,
                },
            });
        }
    }
    let current_progress = decode_progress(current.progress.as_deref())?;
    if current_progress == TaskProgress::Failed {
        return Ok(TaskFailureOutcome::AlreadyFailed { task });
    }
    if current_progress.is_terminal() {
        return Ok(TaskFailureOutcome::TaskTerminal {
            task,
            progress: current_progress,
        });
    }
    if current_revision != premise.task.revision {
        return Ok(TaskFailureOutcome::StalePremise {
            current: TaskRef {
                task,
                revision: current_revision,
            },
        });
    }
    let moved = tx
        .execute(SQL_FAIL_TASK, params![task_text, current.revision])
        .map_err(task_unavailable)?;
    if moved != 1 {
        return Err(task_unavailable(
            "failure CAS did not move exactly the relied non-terminal task",
        ));
    }
    register_task_undelivered(
        &tx,
        &current.assignee,
        task.as_raw(),
        TaskFact::Terminal {
            task: task.as_raw(),
            progress: TerminalKindWire::Failed,
        },
    )?;
    tx.commit().map_err(task_unavailable)?;
    Ok(TaskFailureOutcome::FailedAs(TaskRef {
        task,
        revision: current_revision,
    }))
}

fn create_delegation_sync(
    conn: &Mutex<Connection>,
    premise: DelegationCreationPremise,
) -> Result<DelegationOutcome, TaskTechnicalError> {
    let DelegationCreationPremise {
        delegation,
        task,
        agent,
        scope_copy,
    } = premise;
    let task_text = encode_id(task.task.as_raw());
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(task_unavailable)?;
    let current: Option<RawTask> = tx
        .query_row(SQL_SELECT_TASK, params![task_text], raw_task_row)
        .optional()
        .map_err(task_unavailable)?;
    let Some(current) = current else {
        return Ok(DelegationOutcome::MissingTask { task: task.task });
    };
    // Terminal progress refuses the whole creation: no delegation row and no
    // revision advance. The task revision and progress are read in the same
    // snapshot as the compare below. The terminal check precedes the revision
    // compare so a terminal Task at a moved revision answers `TaskTerminal`,
    // never `StaleTaskRevision`: terminal has its own outcome and is never
    // folded into `Stale*` (the steering and agent admissions order it the
    // same way).
    let current_progress = decode_progress(current.progress.as_deref())?;
    if current_progress.is_terminal() {
        return Ok(DelegationOutcome::TaskTerminal {
            task: task.task,
            progress: current_progress,
        });
    }
    let current_revision = decode_revision(current.revision)?;
    if current_revision != task.revision {
        return Ok(DelegationOutcome::StaleTaskRevision {
            current: TaskRef {
                task: task.task,
                revision: current_revision,
            },
        });
    }
    // Fail-closed D1/D2 check: the delegation relies on this revision's
    // snapshot, and the delegator copied below must be the assignee that
    // snapshot records. A missing snapshot or a disagreement is an
    // inconsistent unit; no correspondence is synthesized from either side.
    let snapshot: RawTaskRevision = tx
        .query_row(
            SQL_SELECT_TASK_REVISION,
            params![task_text, current.revision],
            raw_revision_row,
        )
        .optional()
        .map_err(task_unavailable)?
        .ok_or_else(|| {
            task_unavailable("task revision snapshot missing for the delegated revision")
        })?;
    let current_assignee = decode_assignee(&current.assignee)?;
    let snapshot_assignee = decode_assignee(&snapshot.assignee)?;
    if snapshot_assignee != current_assignee {
        return Err(task_unavailable(
            "task revision assignee does not match the current assignee",
        ));
    }
    let delegator = current_assignee;
    let (scope_assoc, scope_folder, scope_save_target) = match &scope_copy.workspace {
        None => (None, None, None),
        Some(workspace) => (
            Some(encode_id(workspace.assoc.as_raw())),
            Some(
                crate::preservation::redact_covered_text(&tx, &workspace.folder.path)
                    .map_err(|error| task_unavailable(error.to_string()))?,
            ),
            workspace
                .save_target
                .as_ref()
                .map(|target| crate::preservation::redact_covered_text(&tx, &target.path))
                .transpose()
                .map_err(|error| task_unavailable(error.to_string()))?,
        ),
    };
    tx.execute(
        SQL_INSERT_DELEGATION,
        params![
            encode_id(delegation.as_raw()),
            task_text,
            current.revision,
            current.assignee,
            encode_id(agent.as_raw()),
            scope_assoc,
            scope_folder,
            scope_save_target,
        ],
    )
    .map_err(task_unavailable)?;
    let advanced = tx
        .execute(SQL_MARK_TASK_IN_PROGRESS, params![task_text])
        .map_err(task_unavailable)?;
    if advanced != 1 {
        return Err(task_unavailable(
            "task progress did not advance with delegation creation",
        ));
    }
    register_task_undelivered(
        &tx,
        &current.assignee,
        task.task.as_raw(),
        TaskFact::Delegation(delegation.as_raw()),
    )?;
    tx.commit().map_err(task_unavailable)?;
    Ok(DelegationOutcome::Delegated(DelegationRef {
        delegation,
        task,
        delegator,
        agent,
        scope: scope_copy,
    }))
}

struct RawTask {
    revision: i64,
    purpose_adopted_revision: i64,
    /// Nullable in the schema; reads fail closed on NULL and new writes
    /// always name a value.
    progress: Option<String>,
    assignee: String,
}

struct RawTaskRevision {
    purpose_adopted_revision: i64,
    purpose_text: String,
    assignee: String,
}

struct RawContextEntry {
    entry: String,
    revision: i64,
    item_kind: String,
    purpose_adopted_revision: Option<i64>,
    origin_kind: String,
    origin_source: String,
    acquired_at: String,
}

struct RawWorkspaceAssoc {
    assoc: String,
    folder: String,
    save_target: Option<String>,
}

struct RawDelegation {
    task: String,
    task_revision: i64,
    delegator: String,
    agent: String,
    scope_assoc: Option<String>,
    scope_folder: Option<String>,
    scope_save_target: Option<String>,
}

fn raw_task_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawTask> {
    Ok(RawTask {
        revision: row.get(0)?,
        purpose_adopted_revision: row.get(1)?,
        progress: row.get(2)?,
        assignee: row.get(3)?,
    })
}

fn raw_revision_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawTaskRevision> {
    Ok(RawTaskRevision {
        purpose_adopted_revision: row.get(0)?,
        purpose_text: row.get(1)?,
        assignee: row.get(2)?,
    })
}

fn raw_delegation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawDelegation> {
    Ok(RawDelegation {
        task: row.get(0)?,
        task_revision: row.get(1)?,
        delegator: row.get(2)?,
        agent: row.get(3)?,
        scope_assoc: row.get(4)?,
        scope_folder: row.get(5)?,
        scope_save_target: row.get(6)?,
    })
}

fn decode_delegation_scope(
    scope_assoc: Option<String>,
    scope_folder: Option<String>,
    scope_save_target: Option<String>,
) -> Result<DelegationScope, TaskTechnicalError> {
    match (scope_assoc, scope_folder, scope_save_target) {
        (None, None, None) => Ok(DelegationScope { workspace: None }),
        (Some(assoc), Some(folder), save_target) => Ok(DelegationScope {
            workspace: Some(DelegatedWorkspace {
                assoc: WorkspaceAssocId::from_raw(decode_id(&assoc).map_err(task_unavailable)?),
                folder: WorkspaceFolderRef { path: folder },
                save_target: save_target.map(|path| WorkspaceFolderRef { path }),
            }),
        }),
        _ => Err(task_unavailable("inconsistent delegation scope copy")),
    }
}

fn decode_workspace(
    raw: RawWorkspaceAssoc,
    task: TaskId,
) -> Result<WorkspaceAssociation, TaskTechnicalError> {
    Ok(WorkspaceAssociation {
        assoc: WorkspaceAssocId::from_raw(decode_id(&raw.assoc).map_err(task_unavailable)?),
        task,
        folder: WorkspaceFolderRef { path: raw.folder },
        save_target: raw.save_target.map(|path| WorkspaceFolderRef { path }),
    })
}

fn load_context_sync(
    conn: &Connection,
    task_text: &str,
    current: TaskRef,
    purpose: TaskPurposeRef,
) -> Result<Vec<TaskContextEntry>, TaskTechnicalError> {
    let mut statement = conn
        .prepare(SQL_SELECT_TASK_CONTEXT)
        .map_err(task_unavailable)?;
    let rows = statement
        .query_map(params![task_text], |row| {
            Ok(RawContextEntry {
                entry: row.get(0)?,
                revision: row.get(1)?,
                item_kind: row.get(2)?,
                purpose_adopted_revision: row.get(3)?,
                origin_kind: row.get(4)?,
                origin_source: row.get(5)?,
                acquired_at: row.get(6)?,
            })
        })
        .map_err(task_unavailable)?;
    let mut purpose_entry = None;
    let mut instructions = Vec::new();
    for row in rows {
        let raw = row.map_err(task_unavailable)?;
        let revision = decode_revision(raw.revision)?;
        if revision > current.revision {
            return Err(task_unavailable(
                "task context entry revision is beyond the current revision",
            ));
        }
        let item = decode_context_item(current.task, &raw.item_kind, raw.purpose_adopted_revision)?;
        let entry = TaskContextEntry {
            entry: TaskContextEntryId::from_raw(decode_id(&raw.entry).map_err(task_unavailable)?),
            reference: TaskRef {
                task: current.task,
                revision,
            },
            item,
            origin: TaskContextOrigin {
                kind: decode_origin_kind(&raw.origin_kind)?,
                source: decode_id(&raw.origin_source).map_err(task_unavailable)?,
            },
            acquired_at: decode_clock(&raw.acquired_at)?,
        };
        match entry.item {
            TaskContextItem::AdoptedPurpose(adopted) if revision == current.revision => {
                if adopted != purpose {
                    return Err(task_unavailable(
                        "task context adopted purpose does not match the current purpose",
                    ));
                }
                if purpose_entry.is_some() {
                    return Err(task_unavailable(
                        "multiple adopted purpose entries for the current revision",
                    ));
                }
                purpose_entry = Some(entry);
            }
            TaskContextItem::AdoptedPurpose(_) => {}
            TaskContextItem::AdoptedInstruction => instructions.push(entry),
        }
    }
    let Some(purpose_entry) = purpose_entry else {
        return Err(task_unavailable(
            "task context adopted purpose entry missing for the current revision",
        ));
    };
    let mut context = Vec::with_capacity(instructions.len() + 1);
    context.push(purpose_entry);
    context.extend(instructions);
    Ok(context)
}

fn load_task_sync(
    conn: &Mutex<Connection>,
    task: TaskId,
) -> Result<Option<TaskRecord>, TaskTechnicalError> {
    let task_text = encode_id(task.as_raw());
    let guard = lock_shared(conn);
    let current: Option<RawTask> = guard
        .query_row(SQL_SELECT_TASK, params![task_text], raw_task_row)
        .optional()
        .map_err(task_unavailable)?;
    let Some(raw_task) = current else {
        return Ok(None);
    };
    let reference = TaskRef {
        task,
        revision: decode_revision(raw_task.revision)?,
    };
    let purpose = TaskPurposeRef {
        task,
        adopted_revision: decode_revision(raw_task.purpose_adopted_revision)?,
    };
    let snapshot: RawTaskRevision = guard
        .query_row(
            SQL_SELECT_TASK_REVISION,
            params![task_text, raw_task.revision],
            raw_revision_row,
        )
        .optional()
        .map_err(task_unavailable)?
        .ok_or_else(|| {
            task_unavailable("task revision snapshot missing for the current revision")
        })?;
    let revision_purpose = TaskPurposeRef {
        task,
        adopted_revision: decode_revision(snapshot.purpose_adopted_revision)?,
    };
    if revision_purpose != purpose {
        return Err(task_unavailable(
            "task revision purpose does not match the current purpose",
        ));
    }
    let task_assignee = decode_assignee(&raw_task.assignee)?;
    let revision_assignee = decode_assignee(&snapshot.assignee)?;
    if revision_assignee != task_assignee {
        return Err(task_unavailable(
            "task revision assignee does not match the current assignee",
        ));
    }
    let progress = decode_progress(raw_task.progress.as_deref())?;
    let adopted_result = load_adopted_result(&guard, &task_text, reference.revision, progress)?;
    let revision = TaskRevisionRecord {
        reference,
        purpose: revision_purpose,
        purpose_text: TaskPurpose {
            text: snapshot.purpose_text,
        },
        assignee: revision_assignee,
    };
    let context = load_context_sync(&guard, &task_text, reference, purpose)?;
    let workspace_rows = {
        let mut statement = guard
            .prepare(SQL_SELECT_WORKSPACE_ASSOCS)
            .map_err(task_unavailable)?;
        statement
            .query_map(params![task_text], |row| {
                Ok(RawWorkspaceAssoc {
                    assoc: row.get(0)?,
                    folder: row.get(1)?,
                    save_target: row.get(2)?,
                })
            })
            .map_err(task_unavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(task_unavailable)?
    };
    if workspace_rows.len() > 1 {
        return Err(task_unavailable(
            "multiple workspace associations for the task",
        ));
    }
    let workspace = workspace_rows
        .into_iter()
        .next()
        .map(|raw| decode_workspace(raw, task))
        .transpose()?;
    Ok(Some(TaskRecord {
        task: Task {
            reference,
            purpose,
            assignee: task_assignee,
            progress,
            adopted_result,
        },
        revision,
        context,
        workspace,
    }))
}

fn load_adopted_result(
    conn: &Connection,
    task_text: &str,
    current_revision: TaskRevision,
    progress: TaskProgress,
) -> Result<Option<TaskResultId>, TaskTechnicalError> {
    let mut statement = conn
        .prepare(SQL_SELECT_ADOPTED_RESULT)
        .map_err(task_unavailable)?;
    let rows = statement
        .query_map(params![task_text], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(task_unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(task_unavailable)?;
    if rows.len() > 1 {
        return Err(task_unavailable(
            "multiple adopted task results for the task",
        ));
    }
    let Some((result, task_revision, adopted_revision)) = rows.into_iter().next() else {
        if progress == TaskProgress::Completed {
            return Err(task_unavailable("completed task has no adopted result"));
        }
        return Ok(None);
    };
    if decode_revision(task_revision)? != current_revision
        || decode_revision(adopted_revision)? != current_revision
    {
        return Err(task_unavailable(
            "adopted task result does not match the current revision",
        ));
    }
    if progress != TaskProgress::Completed {
        return Err(task_unavailable(
            "adopted task result without a completed task",
        ));
    }
    Ok(Some(TaskResultId::from_raw(
        decode_id(&result).map_err(task_unavailable)?,
    )))
}

fn require_adopted_result_current_unit(
    conn: &Connection,
    result: TaskResultId,
    task_text: &str,
    relied_revision: TaskRevision,
    adopted_revision: TaskRevision,
) -> Result<(), TaskTechnicalError> {
    if adopted_revision != relied_revision {
        return Err(task_unavailable(
            "adopted task result does not match its relied revision",
        ));
    }
    let current: Option<RawTask> = conn
        .query_row(SQL_SELECT_TASK, params![task_text], raw_task_row)
        .optional()
        .map_err(task_unavailable)?;
    let Some(current) = current else {
        return Err(task_unavailable("adopted task result has no current task"));
    };
    let current_revision = decode_revision(current.revision)?;
    let current_progress = decode_progress(current.progress.as_deref())?;
    if current_revision != relied_revision || current_progress != TaskProgress::Completed {
        return Err(task_unavailable(
            "adopted task result does not match the completed current task",
        ));
    }
    let current_purpose = decode_revision(current.purpose_adopted_revision)?;
    let snapshot: RawTaskRevision = conn
        .query_row(
            SQL_SELECT_TASK_REVISION,
            params![task_text, current.revision],
            raw_revision_row,
        )
        .optional()
        .map_err(task_unavailable)?
        .ok_or_else(|| {
            task_unavailable("task revision snapshot missing for the adopted relied revision")
        })?;
    if decode_revision(snapshot.purpose_adopted_revision)? != current_purpose {
        return Err(task_unavailable(
            "task revision purpose does not match the current purpose",
        ));
    }
    if load_adopted_result(conn, task_text, current_revision, current_progress)? != Some(result) {
        return Err(task_unavailable(
            "adopted task result is not the task's single adopted result",
        ));
    }
    Ok(())
}

fn load_delegation_sync(
    conn: &Mutex<Connection>,
    delegation: DelegationId,
) -> Result<Option<DelegationRef>, TaskTechnicalError> {
    let guard = lock_shared(conn);
    let stored: Option<RawDelegation> = guard
        .query_row(
            SQL_SELECT_DELEGATION,
            params![encode_id(delegation.as_raw())],
            raw_delegation_row,
        )
        .optional()
        .map_err(task_unavailable)?;
    let Some(raw) = stored else {
        return Ok(None);
    };
    Ok(Some(DelegationRef {
        delegation,
        task: TaskRef {
            task: TaskId::from_raw(decode_id(&raw.task).map_err(task_unavailable)?),
            revision: decode_revision(raw.task_revision)?,
        },
        delegator: decode_assignee(&raw.delegator)?,
        agent: TaskAgentEphemeralId::from_raw(decode_id(&raw.agent).map_err(task_unavailable)?),
        scope: decode_delegation_scope(raw.scope_assoc, raw.scope_folder, raw.scope_save_target)?,
    }))
}

struct RawResultById {
    task: String,
    task_revision: i64,
    delegation: String,
    body: String,
    adopted_revision: Option<i64>,
    recorded_at: String,
}

struct RawResultByDelegation {
    result: String,
    task: String,
    task_revision: i64,
    body: String,
    adopted_revision: Option<i64>,
    recorded_at: String,
}

fn raw_result_by_id_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawResultById> {
    Ok(RawResultById {
        task: row.get(0)?,
        task_revision: row.get(1)?,
        delegation: row.get(2)?,
        body: row.get(3)?,
        adopted_revision: row.get(4)?,
        recorded_at: row.get(5)?,
    })
}

fn raw_result_by_delegation_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RawResultByDelegation> {
    Ok(RawResultByDelegation {
        result: row.get(0)?,
        task: row.get(1)?,
        task_revision: row.get(2)?,
        body: row.get(3)?,
        adopted_revision: row.get(4)?,
        recorded_at: row.get(5)?,
    })
}

fn load_result_attempts(
    conn: &Connection,
    result_text: &str,
) -> Result<Vec<RawId>, TaskTechnicalError> {
    let mut statement = conn
        .prepare(SQL_SELECT_RESULT_ATTEMPTS)
        .map_err(task_unavailable)?;
    let rows = statement
        .query_map(params![result_text], |row| row.get::<_, String>(0))
        .map_err(task_unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(task_unavailable)?;
    rows.into_iter()
        .map(|text| decode_id(&text).map_err(task_unavailable))
        .collect()
}

#[expect(
    clippy::too_many_arguments,
    reason = "one decoded row's columns; a struct would restate the SQL row"
)]
fn compose_result(
    conn: &Connection,
    result: TaskResultId,
    task: String,
    task_revision: i64,
    delegation: String,
    body: String,
    adopted_revision: Option<i64>,
    recorded_at: String,
) -> Result<TaskResultRecord, TaskTechnicalError> {
    let correspondence: Option<(String, i64)> = conn
        .query_row(
            SQL_SELECT_DELEGATION_CORRESPONDENCE,
            params![delegation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(task_unavailable)?;
    let Some((delegation_task, delegation_revision)) = correspondence else {
        return Err(task_unavailable(
            "result delegation correspondence is missing",
        ));
    };
    if delegation_task != task || delegation_revision != task_revision {
        return Err(task_unavailable(
            "result delegation correspondence disagrees with the recorded result",
        ));
    }
    let attempt_refs = load_result_attempts(conn, &encode_id(result.as_raw()))?;
    for attempt in &attempt_refs {
        let stamped: Option<(String, String, i64)> = conn
            .query_row(
                SQL_SELECT_ATTEMPT_CORRESPONDENCE,
                params![encode_id(*attempt)],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(task_unavailable)?;
        let Some((attempt_delegation, attempt_task, attempt_revision)) = stamped else {
            return Err(task_unavailable(
                "stamped task result attempt is missing from the action attempts",
            ));
        };
        if attempt_delegation != delegation
            || attempt_task != task
            || attempt_revision != task_revision
        {
            return Err(task_unavailable(
                "stamped task result attempt disagrees with the recorded result correlation",
            ));
        }
    }
    let task_id = TaskId::from_raw(decode_id(&task).map_err(task_unavailable)?);
    let delegation_id = DelegationId::from_raw(decode_id(&delegation).map_err(task_unavailable)?);
    let relied_revision = decode_revision(task_revision)?;
    // Re-derive the authoritative set from the sealed delegation for the exact
    // comparison. Only a stored non-empty set (or an adopted stamp) is
    // compared, so the documented ambiguity of an all-empty unstamped first
    // evaluation stays untouched.
    let authoritative = || -> Result<Vec<RawId>, TaskTechnicalError> {
        let attempts =
            enumerate_delegation_attempts(conn, delegation_id, task_id, relied_revision)?;
        Ok(attempts.into_iter().map(|(attempt, _)| attempt).collect())
    };
    if let Some(adopted_stamp) = adopted_revision {
        // The adopted stamp only means the completed unit is readable while
        // the current Task still agrees with it; the same checks the retry and
        // `load_task` apply, so a bounded read never answers success from a
        // stale stamp after an adopted-unit corruption.
        require_adopted_result_current_unit(
            conn,
            result,
            &task,
            relied_revision,
            decode_revision(adopted_stamp)?,
        )?;
        // An adopted result's stamp committed in the same transaction as its
        // adoption, so `adopted_revision = Some` proves the set is not an
        // unstamped first evaluation. A fully wiped set (here and in the retry
        // path) is durable corruption, never an initial empty evaluation, and
        // an extra row is equally unreadable.
        require_stamped_attempts_exact(&attempt_refs, &authoritative()?)?;
    } else if !attempt_refs.is_empty() {
        // A non-adopted acceptance stamps the result-local set to the
        // authoritative set, so a stored non-empty set that no longer matches
        // is a truncated or extended durable set, never an empty first
        // evaluation.
        require_stamped_attempts_exact(&attempt_refs, &authoritative()?)?;
    }
    Ok(TaskResultRecord {
        result,
        task: TaskRef {
            task: task_id,
            revision: relied_revision,
        },
        delegation: delegation_id,
        body: TaskAgentOutput::new(body),
        attempt_refs,
        adopted_revision: adopted_revision.map(decode_revision).transpose()?,
        recorded_at: decode_clock(&recorded_at)?,
    })
}

fn record_task_result_arrival_sync(
    conn: &Mutex<Connection>,
    arrival: TaskAgentResultArrival,
) -> Result<TaskResultArrivalOutcome, TaskTechnicalError> {
    let delegation_text = encode_id(arrival.delegation.as_raw());
    let result_text = encode_id(arrival.result.as_raw());
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(task_unavailable)?;
    let current = crate::credential::current_set_revision(&tx)
        .map_err(|error| task_unavailable(error.to_string()))?;
    if current != arrival.body.credential_set() {
        return Ok(TaskResultArrivalOutcome::StaleCredentialSet { current });
    }
    let held = crate::preservation::held_use(
        &tx,
        crate::preservation::USE_KIND_TASK_DELEGATION,
        arrival.delegation.as_raw(),
    )
    .map_err(|error| task_unavailable(error.to_string()))?;
    let body_text = if held {
        String::from(crate::erasure::ERASED_MARKER)
    } else {
        crate::preservation::redact_covered_text(&tx, arrival.body.body())
            .map_err(|error| task_unavailable(error.to_string()))?
    };
    let correspondence: Option<(String, i64)> = tx
        .query_row(
            SQL_SELECT_DELEGATION_CORRESPONDENCE,
            params![delegation_text],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(task_unavailable)?;
    let Some((task_text, revision_raw)) = correspondence else {
        return Err(task_unavailable(
            "delegation correspondence missing for final result arrival",
        ));
    };
    let delegation_task = decode_id(&task_text).map_err(task_unavailable)?;
    let delegation_revision = decode_revision(revision_raw)?;
    let existing: Option<RawResultById> = tx
        .query_row(
            SQL_SELECT_RESULT_BY_ID,
            params![result_text],
            raw_result_by_id_row,
        )
        .optional()
        .map_err(task_unavailable)?;
    if let Some(raw) = existing {
        let same_fingerprint = decode_id(&raw.task).map_err(task_unavailable)? == delegation_task
            && decode_revision(raw.task_revision)? == delegation_revision
            && decode_id(&raw.delegation).map_err(task_unavailable)? == arrival.delegation.as_raw()
            && raw.body == body_text;
        if !same_fingerprint {
            return Err(task_unavailable(
                "task result identity reused with different content",
            ));
        }
        let record = compose_result(
            &tx,
            arrival.result,
            raw.task,
            raw.task_revision,
            raw.delegation,
            raw.body,
            raw.adopted_revision,
            raw.recorded_at,
        )?;
        tx.commit().map_err(task_unavailable)?;
        return Ok(TaskResultArrivalOutcome::Recorded(record));
    }
    let sealed: Option<String> = tx
        .query_row(
            SQL_SELECT_RESULT_BY_DELEGATION,
            params![delegation_text],
            |row| row.get(0),
        )
        .optional()
        .map_err(task_unavailable)?;
    if sealed.is_some() {
        return Err(task_unavailable(
            "delegation already sealed by another final result",
        ));
    }
    let recorded_at = WallClockWithTz::now();
    tx.execute(
        SQL_INSERT_RESULT,
        params![
            result_text,
            task_text,
            revision_raw,
            delegation_text,
            body_text,
            Option::<i64>::None,
            recorded_at.to_rfc3339(),
        ],
    )
    .map_err(task_unavailable)?;
    let assignee: Option<String> = tx
        .query_row(SQL_SELECT_TASK_ASSIGNEE, params![task_text], |row| {
            row.get(0)
        })
        .optional()
        .map_err(task_unavailable)?;
    let Some(assignee) = assignee else {
        return Err(task_unavailable(
            "task row missing for final result arrival",
        ));
    };
    register_task_undelivered(
        &tx,
        &assignee,
        delegation_task,
        TaskFact::ResultRecorded(arrival.result.as_raw()),
    )?;
    tx.commit().map_err(task_unavailable)?;
    Ok(TaskResultArrivalOutcome::Recorded(TaskResultRecord {
        result: arrival.result,
        task: TaskRef {
            task: TaskId::from_raw(delegation_task),
            revision: delegation_revision,
        },
        delegation: arrival.delegation,
        body: TaskAgentOutput::new(body_text),
        attempt_refs: Vec::new(),
        adopted_revision: None,
        recorded_at,
    }))
}

fn load_task_result_sync(
    conn: &Mutex<Connection>,
    result: TaskResultId,
) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
    let guard = lock_shared(conn);
    let found: Option<RawResultById> = guard
        .query_row(
            SQL_SELECT_RESULT_BY_ID,
            params![encode_id(result.as_raw())],
            raw_result_by_id_row,
        )
        .optional()
        .map_err(task_unavailable)?;
    found
        .map(|raw| {
            compose_result(
                &guard,
                result,
                raw.task,
                raw.task_revision,
                raw.delegation,
                raw.body,
                raw.adopted_revision,
                raw.recorded_at,
            )
        })
        .transpose()
}

fn record_task_agent_observation_sync(
    conn: &Mutex<Connection>,
    premise: TaskAgentObservationPremise,
) -> Result<TaskAgentObservationId, TaskTechnicalError> {
    let observation_text = encode_id(premise.observation.as_raw());
    let delegation_text = encode_id(premise.delegation.as_raw());
    let observed_at = premise.observed_at.to_rfc3339();
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(task_unavailable)?;
    let correlation = observation_correlation(&tx, &premise)?;
    let existing: Option<RawObservation> = tx
        .query_row(
            SQL_SELECT_OBSERVATION,
            params![observation_text],
            raw_observation_row,
        )
        .optional()
        .map_err(task_unavailable)?;
    if let Some(raw) = existing {
        let same = raw.delegation == delegation_text
            && raw.task == correlation.task
            && raw.task_revision == correlation.task_revision
            && raw.workspace == correlation.workspace
            && raw.attempt == correlation.attempt
            && raw.path == correlation.path
            && raw.body_observed == correlation.body_observed
            && raw.observed_at == observed_at;
        if !same {
            return Err(task_unavailable(
                "task observation identity reused with different correlation",
            ));
        }
        tx.commit().map_err(task_unavailable)?;
        return Ok(premise.observation);
    }
    tx.execute(
        SQL_INSERT_OBSERVATION,
        params![
            observation_text,
            delegation_text,
            correlation.task,
            correlation.task_revision,
            correlation.workspace,
            correlation.attempt,
            correlation.path,
            correlation.body_observed,
            observed_at,
        ],
    )
    .map_err(task_unavailable)?;
    let covered = match &premise.observed {
        Some(observed) => crate::preservation::covering_text_condition(&tx, observed)
            .map_err(|error| task_unavailable(error.to_string()))?,
        None => None,
    };
    let covered = match covered {
        Some(hit) => Some(hit),
        None => match correlation.path.as_deref() {
            Some(path) => crate::preservation::covering_text_condition(&tx, path)
                .map_err(|error| task_unavailable(error.to_string()))?,
            None => None,
        },
    };
    if let Some((condition, _coverage)) = covered {
        crate::preservation::publish_observation_source(
            &tx,
            condition,
            premise.observation.as_raw(),
        )
        .map_err(|error| task_unavailable(error.to_string()))?;
        crate::preservation::hold_delegation(
            &tx,
            condition,
            premise.delegation.as_raw(),
            &observed_at,
        )
        .map_err(|error| task_unavailable(error.to_string()))?;
    }
    tx.commit().map_err(task_unavailable)?;
    Ok(premise.observation)
}

struct ObservationCorrelation {
    task: String,
    task_revision: i64,
    workspace: Option<String>,
    attempt: Option<String>,
    path: Option<String>,
    body_observed: bool,
}

fn observation_correlation(
    tx: &rusqlite::Transaction<'_>,
    premise: &TaskAgentObservationPremise,
) -> Result<ObservationCorrelation, TaskTechnicalError> {
    let delegation_text = encode_id(premise.delegation.as_raw());
    let correspondence: Option<(String, i64, Option<String>)> = tx
        .query_row(SQL_SELECT_DELEGATION, params![delegation_text], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(4)?))
        })
        .optional()
        .map_err(task_unavailable)?;
    let Some((task, task_revision, scope_assoc)) = correspondence else {
        return Err(task_unavailable(
            "delegation correspondence missing for observation record",
        ));
    };
    let Some(attempt) = &premise.attempt else {
        if premise.observed.is_some() {
            return Err(task_unavailable(
                "refusal observation carries an observed body",
            ));
        }
        return Ok(ObservationCorrelation {
            task,
            task_revision,
            workspace: None,
            attempt: None,
            path: None,
            body_observed: false,
        });
    };
    let attempt_text = encode_id(*attempt);
    let stored: Option<(String, String, i64, String, String, String)> = tx
        .query_row(
            SQL_SELECT_OBSERVATION_ATTEMPT,
            params![attempt_text],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()
        .map_err(task_unavailable)?;
    let Some((delegation, stored_task, stored_revision, workspace, real_target, operation)) =
        stored
    else {
        return Err(task_unavailable(
            "producing action attempt missing for observation record",
        ));
    };
    if delegation != delegation_text || stored_task != task || stored_revision != task_revision {
        return Err(task_unavailable(
            "producing action attempt disagrees with the delegation correspondence",
        ));
    }
    // A valid attempt can never carry a NULL delegation scope: the start path
    // refuses it, so a missing or diverging scope is torn state, not a
    // reason to copy a foreign workspace into the observation.
    let scope = scope_assoc
        .ok_or_else(|| task_unavailable("delegation scope missing for observation record"))?;
    if decode_id(&scope).map_err(task_unavailable)?
        != decode_id(&workspace).map_err(task_unavailable)?
    {
        return Err(task_unavailable(
            "producing action attempt workspace disagrees with the delegation scope",
        ));
    }
    let body_observed = premise.observed.is_some();
    let operation = ene_action::OperationKind::from_name(&operation)
        .ok_or_else(|| task_unavailable("unknown action operation in observation attempt"))?;
    if body_observed
        && !matches!(
            operation,
            ene_action::OperationKind::Read | ene_action::OperationKind::List
        )
    {
        return Err(task_unavailable(
            "observed body names a non-body action operation",
        ));
    }
    Ok(ObservationCorrelation {
        task,
        task_revision,
        workspace: Some(workspace),
        attempt: Some(attempt_text),
        path: Some(real_target),
        body_observed,
    })
}

struct RawObservation {
    delegation: String,
    task: String,
    task_revision: i64,
    workspace: Option<String>,
    attempt: Option<String>,
    path: Option<String>,
    body_observed: bool,
    observed_at: String,
}

fn raw_observation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawObservation> {
    Ok(RawObservation {
        delegation: row.get(0)?,
        task: row.get(1)?,
        task_revision: row.get(2)?,
        workspace: row.get(3)?,
        attempt: row.get(4)?,
        path: row.get(5)?,
        body_observed: row.get(6)?,
        observed_at: row.get(7)?,
    })
}

fn delegation_has_started_work_sync(
    conn: &Mutex<Connection>,
    delegation: DelegationId,
) -> Result<bool, TaskTechnicalError> {
    let guard = lock_shared(conn);
    guard
        .query_row(
            SQL_DELEGATION_HAS_STARTED_WORK,
            params![encode_id(delegation.as_raw())],
            |row| row.get::<_, bool>(0),
        )
        .map_err(task_unavailable)
}

fn load_result_adoption_claim_sync(
    conn: &Mutex<Connection>,
    result: TaskResultId,
) -> Result<Option<TaskResultAdoptionClaim>, TaskTechnicalError> {
    let result_text = encode_id(result.as_raw());
    let guard = lock_shared(conn);
    let found: Option<(String, i64, String)> = guard
        .query_row(
            SQL_SELECT_RESULT_CORRESPONDENCE,
            params![result_text],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(task_unavailable)?;
    let Some((task, revision, delegation)) = found else {
        return Ok(None);
    };
    let task_id = TaskId::from_raw(decode_id(&task).map_err(task_unavailable)?);
    let relied_revision = decode_revision(revision)?;
    let delegation_id = DelegationId::from_raw(decode_id(&delegation).map_err(task_unavailable)?);
    let delegation_correspondence: Option<(String, i64)> = guard
        .query_row(
            SQL_SELECT_DELEGATION_CORRESPONDENCE,
            params![delegation],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(task_unavailable)?;
    if let Some((delegation_task, delegation_revision)) = delegation_correspondence {
        let agrees = decode_id(&delegation_task).map_err(task_unavailable)? == task_id.as_raw()
            && decode_revision(delegation_revision)? == relied_revision;
        if !agrees {
            return Err(task_unavailable(
                "result delegation correspondence disagrees with the recorded result",
            ));
        }
    }
    let attempt_refs =
        enumerate_delegation_attempts(&guard, delegation_id, task_id, relied_revision)?
            .into_iter()
            .map(|(attempt, _)| attempt)
            .collect();
    Ok(Some(TaskResultAdoptionClaim {
        result,
        attempt_refs,
    }))
}

fn list_unadopted_results_after_sync(
    conn: &Mutex<Connection>,
    after: Option<UnadoptedResultCursor>,
    limit: u64,
) -> Result<Vec<UnadoptedResultCursor>, TaskTechnicalError> {
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let guard = lock_shared(conn);
    let rows: Vec<(String, String)> = match after {
        None => {
            let mut statement = guard
                .prepare(SQL_LIST_UNADOPTED_FIRST)
                .map_err(task_unavailable)?;
            statement
                .query_map(params![limit], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(task_unavailable)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(task_unavailable)?
        }
        Some(cursor) => {
            let mut statement = guard
                .prepare(SQL_LIST_UNADOPTED_AFTER)
                .map_err(task_unavailable)?;
            statement
                .query_map(
                    params![
                        cursor.recorded_at.to_rfc3339(),
                        encode_id(cursor.result.as_raw()),
                        limit
                    ],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(task_unavailable)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(task_unavailable)?
        }
    };
    rows.into_iter()
        .map(|(result, recorded_at)| {
            Ok(UnadoptedResultCursor {
                recorded_at: decode_clock(&recorded_at)?,
                result: TaskResultId::from_raw(decode_id(&result).map_err(task_unavailable)?),
            })
        })
        .collect()
}

fn load_task_action_attempts_sync(
    conn: &Mutex<Connection>,
    task: TaskId,
) -> Result<Vec<RawId>, TaskTechnicalError> {
    let guard = lock_shared(conn);
    Ok(enumerate_task_attempts(&guard, task)?
        .into_iter()
        .map(|(attempt, _)| attempt)
        .collect())
}

fn list_tasks_after_sync(
    conn: &Mutex<Connection>,
    after: Option<TaskId>,
    limit: u32,
) -> Result<Vec<TaskHeadline>, TaskTechnicalError> {
    let cap = i64::from(limit.clamp(1, REPORT_PAGE_MAX));
    let guard = lock_shared(conn);
    let rows: Vec<RawHeadline> = match after {
        None => {
            let mut statement = guard
                .prepare(SQL_LIST_TASKS_FIRST)
                .map_err(task_unavailable)?;
            statement
                .query_map(params![cap], raw_headline_row)
                .map_err(task_unavailable)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(task_unavailable)?
        }
        Some(after) => {
            let mut statement = guard
                .prepare(SQL_LIST_TASKS_AFTER)
                .map_err(task_unavailable)?;
            statement
                .query_map(params![encode_id(after.as_raw()), cap], raw_headline_row)
                .map_err(task_unavailable)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(task_unavailable)?
        }
    };
    rows.into_iter().map(decode_headline).collect()
}

type RawHeadline = (String, i64, Option<i64>, Option<String>, String, bool);

fn raw_headline_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawHeadline> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn decode_headline(raw: RawHeadline) -> Result<TaskHeadline, TaskTechnicalError> {
    let (task, revision, purpose_revision, progress, assignee, adopted_result) = raw;
    let task = TaskId::from_raw(decode_id(&task).map_err(task_unavailable)?);
    Ok(TaskHeadline {
        task,
        revision: decode_revision(revision)?,
        purpose: TaskPurposeRef {
            task,
            adopted_revision: decode_revision(
                purpose_revision
                    .ok_or_else(|| task_unavailable("task purpose revision is missing"))?,
            )?,
        },
        progress: decode_progress(progress.as_deref())?,
        assignee: decode_id(&assignee).map_err(task_unavailable)?,
        adopted_result,
    })
}

fn list_task_report_rows_after_sync(
    conn: &Mutex<Connection>,
    task: TaskId,
    after: Option<TaskReportRowCursor>,
    limit: u32,
) -> Result<Vec<TaskReportRow>, TaskTechnicalError> {
    let cap = i64::from(limit.clamp(1, REPORT_PAGE_MAX));
    let task_text = encode_id(task.as_raw());
    let guard = lock_shared(conn);
    let rows: Vec<(String, String, Option<i64>)> = match after {
        None => {
            let mut statement = guard
                .prepare(SQL_LIST_REPORT_ROWS_FIRST)
                .map_err(task_unavailable)?;
            statement
                .query_map(params![task_text, cap], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(task_unavailable)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(task_unavailable)?
        }
        Some(cursor) => {
            let rank: i64 = match cursor.kind {
                TaskReportRowKind::ActionAttempt => 0,
                TaskReportRowKind::TaskResult => 1,
            };
            let mut statement = guard
                .prepare(SQL_LIST_REPORT_ROWS_AFTER)
                .map_err(task_unavailable)?;
            statement
                .query_map(params![task_text, rank, encode_id(cursor.id), cap], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(task_unavailable)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(task_unavailable)?
        }
    };
    rows.into_iter()
        .map(|(kind_text, id_text, adopted)| {
            let kind = match kind_text.as_str() {
                "action_attempt" => TaskReportRowKind::ActionAttempt,
                "task_result" => TaskReportRowKind::TaskResult,
                _ => return Err(task_unavailable("unknown task report row kind")),
            };
            if kind == TaskReportRowKind::ActionAttempt && adopted.is_some() {
                return Err(task_unavailable(
                    "action attempt report row carries an adoption marker",
                ));
            }
            Ok(TaskReportRow {
                kind,
                id: decode_id(&id_text).map_err(task_unavailable)?,
                adopted_revision: adopted.map(decode_revision).transpose()?,
            })
        })
        .collect()
}

fn load_report_source_bounded_sync(
    conn: &Mutex<Connection>,
    source: TaskReportSourceRef,
    cursor_bytes: u64,
    limit_bytes: u32,
) -> Result<Option<TaskReportSourcePage>, TaskTechnicalError> {
    // 4 is the maximum UTF-8 code-unit length, so any page starting on a
    // character boundary contains at least one whole character and `next`
    // strictly advances even at the smallest requested limit.
    let cap = i64::from(limit_bytes.max(4));
    // SQLite `substr` is 1-based; a cursor at or past the end reads empty and
    // reports no next page.
    let start = i64::try_from(cursor_bytes)
        .map_err(|_| task_unavailable("report source cursor out of range"))?
        .saturating_add(1);
    let guard = lock_shared(conn);
    let found: Option<(i64, Vec<u8>)> = match source {
        TaskReportSourceRef::RevisionPurpose { task, revision } => guard
            .query_row(
                SQL_REPORT_REVISION_PAGE,
                params![
                    encode_id(task.as_raw()),
                    encode_u64(revision.as_u64()).map_err(task_unavailable)?,
                    start,
                    cap
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(task_unavailable)?,
        TaskReportSourceRef::ResultBody(result) => guard
            .query_row(
                SQL_REPORT_RESULT_PAGE,
                params![encode_id(result.as_raw()), start, cap],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(task_unavailable)?,
    };
    let Some((total_raw, bytes)) = found else {
        return Ok(None);
    };
    let total_bytes = decode_u64(total_raw).map_err(task_unavailable)?;
    let text = crate::codec::utf8_prefix(&bytes)
        .map_err(task_unavailable)?
        .to_owned();
    let end = cursor_bytes.saturating_add(text.len() as u64);
    Ok(Some(TaskReportSourcePage {
        text,
        total_bytes,
        next: (end < total_bytes).then_some(end),
    }))
}

fn load_delegation_result_sync(
    conn: &Mutex<Connection>,
    delegation: DelegationId,
) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
    let guard = lock_shared(conn);
    let found: Option<RawResultByDelegation> = guard
        .query_row(
            SQL_SELECT_RESULT_BY_DELEGATION,
            params![encode_id(delegation.as_raw())],
            raw_result_by_delegation_row,
        )
        .optional()
        .map_err(task_unavailable)?;
    found
        .map(|raw| {
            let result = TaskResultId::from_raw(decode_id(&raw.result).map_err(task_unavailable)?);
            compose_result(
                &guard,
                result,
                raw.task,
                raw.task_revision,
                encode_id(delegation.as_raw()),
                raw.body,
                raw.adopted_revision,
                raw.recorded_at,
            )
        })
        .transpose()
}

fn enumerate_delegation_attempts(
    conn: &Connection,
    delegation: DelegationId,
    task: TaskId,
    relied_revision: TaskRevision,
) -> Result<Vec<(RawId, ActionCertainty)>, TaskTechnicalError> {
    let mut statement = conn
        .prepare(SQL_SELECT_DELEGATION_ATTEMPTS)
        .map_err(task_unavailable)?;
    let rows = statement
        .query_map(params![encode_id(delegation.as_raw())], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(task_unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(task_unavailable)?;
    let mut attempts = Vec::with_capacity(rows.len());
    for (attempt, row_task, row_revision, certainty) in rows {
        if decode_id(&row_task).map_err(task_unavailable)? != task.as_raw()
            || decode_revision(row_revision)? != relied_revision
        {
            return Err(task_unavailable(
                "action attempt correspondence disagrees with the delegated execution",
            ));
        }
        attempts.push((
            decode_id(&attempt).map_err(task_unavailable)?,
            decode_certainty(&certainty)?,
        ));
    }
    Ok(attempts)
}

fn enumerate_task_attempts(
    conn: &Connection,
    task_id: TaskId,
) -> Result<Vec<(RawId, ActionCertainty)>, TaskTechnicalError> {
    let task_text = encode_id(task_id.as_raw());
    let mut statement = conn
        .prepare(SQL_SELECT_TASK_ATTEMPTS)
        .map_err(task_unavailable)?;
    let rows = statement
        .query_map(params![task_text], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(task_unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(task_unavailable)?;
    let mut attempts = Vec::with_capacity(rows.len());
    for (
        attempt,
        row_task,
        row_revision,
        row_delegation,
        delegation_task,
        delegation_revision,
        certainty,
    ) in rows
    {
        decode_id(&row_delegation).map_err(task_unavailable)?;
        let Some(delegation_task) = delegation_task else {
            return Err(task_unavailable(
                "action attempt delegation is missing from the task-wide barrier",
            ));
        };
        let Some(delegation_revision) = delegation_revision else {
            return Err(task_unavailable(
                "action attempt delegation correspondence is incomplete",
            ));
        };
        if decode_id(&row_task).map_err(task_unavailable)? != task_id.as_raw()
            || decode_id(&delegation_task).map_err(task_unavailable)? != task_id.as_raw()
            || decode_revision(row_revision)? != decode_revision(delegation_revision)?
        {
            return Err(task_unavailable(
                "action attempt correspondence disagrees with the task-wide barrier",
            ));
        }
        attempts.push((
            decode_id(&attempt).map_err(task_unavailable)?,
            decode_certainty(&certainty)?,
        ));
    }
    Ok(attempts)
}

fn claim_matches_authoritative(
    claim: &[RawId],
    authoritative: &[(RawId, ActionCertainty)],
) -> Result<(), TaskTechnicalError> {
    let mut claimed = HashSet::with_capacity(claim.len());
    for attempt in claim {
        if !claimed.insert(*attempt) {
            return Err(task_unavailable(
                "task result adoption claim repeats an attempt",
            ));
        }
    }
    if claimed.len() != authoritative.len()
        || authoritative
            .iter()
            .any(|(attempt, _)| !claimed.contains(attempt))
    {
        return Err(task_unavailable(
            "task result adoption claim does not match the authoritative attempt set",
        ));
    }
    Ok(())
}

fn require_stamped_attempts_exact(
    stored: &[RawId],
    authoritative: &[RawId],
) -> Result<(), TaskTechnicalError> {
    let stored: HashSet<RawId> = stored.iter().copied().collect();
    let authoritative: HashSet<RawId> = authoritative.iter().copied().collect();
    if stored != authoritative {
        return Err(task_unavailable(
            "stored task result correlation is not exactly the authoritative attempt set",
        ));
    }
    Ok(())
}

/// Verifies and stamps the result-local fixed set.
///
/// A stored non-empty set is durable and fixed: it must equal the
/// authoritative set exactly and is never extended or repaired. An empty
/// stored set is inserted whole as a first evaluation — for a non-adopted
/// result (`adopted_revision = None`) a fully wiped set is indistinguishable
/// from an unstamped first evaluation, so this insert may refill it; that
/// residual ambiguity is out of this slice's guarantee. An
/// adopted result never reaches the empty branch: its retry and the bounded
/// reads use [`require_stamped_attempts_exact`] and fail closed on a wipe.
fn stamp_result_attempts(
    tx: &rusqlite::Transaction<'_>,
    result_text: &str,
    attempts: &[RawId],
) -> Result<(), TaskTechnicalError> {
    let stored = load_result_attempts(tx, result_text)?;
    if stored.is_empty() {
        for attempt in attempts {
            tx.execute(
                SQL_INSERT_RESULT_ATTEMPT,
                params![result_text, encode_id(*attempt)],
            )
            .map_err(task_unavailable)?;
        }
        return Ok(());
    }
    require_stamped_attempts_exact(&stored, attempts)
}

fn adopt_result_sync(
    conn: &Mutex<Connection>,
    claim: TaskResultAdoptionClaim,
) -> Result<TaskResultAcceptance, TaskTechnicalError> {
    let result_text = encode_id(claim.result.as_raw());
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(task_unavailable)?;
    let raw: Option<RawResultById> = tx
        .query_row(
            SQL_SELECT_RESULT_BY_ID,
            params![result_text],
            raw_result_by_id_row,
        )
        .optional()
        .map_err(task_unavailable)?;
    let Some(raw) = raw else {
        return Ok(TaskResultAcceptance::MissingResult {
            result: claim.result,
        });
    };
    let task_id = TaskId::from_raw(decode_id(&raw.task).map_err(task_unavailable)?);
    let relied_revision = decode_revision(raw.task_revision)?;
    let delegation = DelegationId::from_raw(decode_id(&raw.delegation).map_err(task_unavailable)?);
    let correspondence: Option<(String, i64)> = tx
        .query_row(
            SQL_SELECT_DELEGATION_CORRESPONDENCE,
            params![encode_id(delegation.as_raw())],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(task_unavailable)?;
    let Some((delegation_task, delegation_revision)) = correspondence else {
        return Ok(TaskResultAcceptance::MissingDelegation { delegation });
    };
    if decode_id(&delegation_task).map_err(task_unavailable)? != task_id.as_raw()
        || decode_revision(delegation_revision)? != relied_revision
    {
        return Err(task_unavailable(
            "result delegation correspondence disagrees with the recorded result",
        ));
    }
    let authoritative = enumerate_delegation_attempts(&tx, delegation, task_id, relied_revision)?;
    claim_matches_authoritative(&claim.attempt_refs, &authoritative)?;
    let local: Vec<RawId> = authoritative.iter().map(|(attempt, _)| *attempt).collect();
    let current: Option<RawTask> = tx
        .query_row(SQL_SELECT_TASK, params![raw.task], raw_task_row)
        .optional()
        .map_err(task_unavailable)?;
    let Some(current) = current else {
        return Ok(TaskResultAcceptance::MissingTask { task: task_id });
    };
    let current_revision = decode_revision(current.revision)?;
    let current_purpose = decode_revision(current.purpose_adopted_revision)?;
    let current_progress = decode_progress(current.progress.as_deref())?;
    let snapshot: Option<RawTaskRevision> = tx
        .query_row(
            SQL_SELECT_TASK_REVISION,
            params![raw.task, raw.task_revision],
            raw_revision_row,
        )
        .optional()
        .map_err(task_unavailable)?;
    let Some(snapshot) = snapshot else {
        return Err(task_unavailable(
            "task revision snapshot missing for the relied revision",
        ));
    };
    let relied_purpose = decode_revision(snapshot.purpose_adopted_revision)?;
    // The relied revision's purpose snapshot and the current Task's purpose
    // identity are two halves of one D1/D2 unit. A same-revision disagreement
    // is a corrupted unit, not a reason to record the result as original-only.
    if current_revision == relied_revision && relied_purpose != current_purpose {
        return Err(task_unavailable(
            "task revision purpose does not match the current purpose",
        ));
    }
    if let Some(adopted_raw) = raw.adopted_revision {
        if decode_revision(adopted_raw)? != relied_revision {
            return Err(task_unavailable(
                "adopted task result does not match its relied revision",
            ));
        }
        if current_revision != relied_revision || current_progress != TaskProgress::Completed {
            return Err(task_unavailable(
                "adopted task result does not match the completed current task",
            ));
        }
        if load_adopted_result(&tx, &raw.task, current_revision, current_progress)?
            != Some(claim.result)
        {
            return Err(task_unavailable(
                "adopted task result is not the task's single adopted result",
            ));
        }
        require_stamped_attempts_exact(&load_result_attempts(&tx, &result_text)?, &local)?;
        tx.commit().map_err(task_unavailable)?;
        return Ok(TaskResultAcceptance::AdoptedAsCompletion(TaskRef {
            task: task_id,
            revision: relied_revision,
        }));
    }
    if current_revision != relied_revision || current_progress.is_terminal() {
        stamp_result_attempts(&tx, &result_text, &local)?;
        tx.commit().map_err(task_unavailable)?;
        return Ok(TaskResultAcceptance::RecordedToOriginalOnly);
    }
    let mut blockers: Vec<RawId> = Vec::new();
    let mut seen: HashSet<RawId> = HashSet::with_capacity(authoritative.len());
    for (attempt, certainty) in &authoritative {
        if *certainty != ActionCertainty::ConfirmedSuccess && seen.insert(*attempt) {
            blockers.push(*attempt);
        }
    }
    for (attempt, certainty) in enumerate_task_attempts(&tx, task_id)? {
        if certainty == ActionCertainty::Unknown && seen.insert(attempt) {
            blockers.push(attempt);
        }
    }
    if !blockers.is_empty() {
        stamp_result_attempts(&tx, &result_text, &local)?;
        tx.commit().map_err(task_unavailable)?;
        return Ok(TaskResultAcceptance::WithheldByEffectFacts { attempts: blockers });
    }
    stamp_result_attempts(&tx, &result_text, &local)?;
    let adopted = tx
        .execute(
            SQL_MARK_RESULT_ADOPTED,
            params![
                result_text,
                encode_u64(relied_revision.as_u64()).map_err(task_unavailable)?
            ],
        )
        .map_err(task_unavailable)?;
    if adopted != 1 {
        return Err(task_unavailable(
            "task result adoption did not apply exactly once",
        ));
    }
    let completed = tx
        .execute(SQL_COMPLETE_TASK, params![raw.task])
        .map_err(task_unavailable)?;
    if completed != 1 {
        return Err(task_unavailable(
            "task completion did not apply exactly once",
        ));
    }
    register_task_undelivered(
        &tx,
        &current.assignee,
        task_id.as_raw(),
        TaskFact::ResultAdopted(claim.result.as_raw()),
    )?;
    tx.commit().map_err(task_unavailable)?;
    Ok(TaskResultAcceptance::AdoptedAsCompletion(TaskRef {
        task: task_id,
        revision: relied_revision,
    }))
}

enum ResumeSourceVerdict {
    Usable,
    Superseded,
    Hold,
}

struct StoredActivityPremise {
    companion_text: String,
    kind_text: String,
    task_text: Option<String>,
    task_revision: Option<i64>,
    purpose_revision: Option<i64>,
}

fn validate_resume_source(
    tx: &rusqlite::Transaction<'_>,
    instruction: &ResumeInstructionSource,
    task: TaskId,
    expected: TaskRef,
    purpose: TaskPurposeRef,
    assignee: AssigneeRef,
    guarded: bool,
) -> Result<ResumeSourceVerdict, TaskTechnicalError> {
    match *instruction {
        ResumeInstructionSource::OwnerHistory {
            message,
            currentness,
        } => {
            let found: Option<(String, String)> = tx
                .query_row(
                    SQL_SELECT_HISTORY_PREMISE,
                    params![encode_id(message)],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(task_unavailable)?;
            let Some((role_text, companion_text)) = found else {
                return Ok(ResumeSourceVerdict::Hold);
            };
            let role = decode_role(&role_text).map_err(task_unavailable)?;
            if role != HistoryRole::Owner {
                return Ok(ResumeSourceVerdict::Hold);
            }
            if decode_id(&companion_text).map_err(task_unavailable)? != assignee.companion {
                return Ok(ResumeSourceVerdict::Hold);
            }
            let premise = OwnerMessageCurrentness {
                companion: assignee.companion,
                message,
            };
            if !owner_message_is_current(tx, &premise)? {
                if guarded && currentness.message == message {
                    return Ok(ResumeSourceVerdict::Superseded);
                }
                return Ok(ResumeSourceVerdict::Hold);
            }
            Ok(ResumeSourceVerdict::Usable)
        }
        ResumeInstructionSource::OwnerManagement { activity } => {
            let found: Option<StoredActivityPremise> = tx
                .query_row(
                    SQL_SELECT_ACTIVITY_PREMISE,
                    params![encode_id(activity)],
                    |row| {
                        Ok(StoredActivityPremise {
                            companion_text: row.get(0)?,
                            kind_text: row.get(1)?,
                            task_text: row.get(2)?,
                            task_revision: row.get(3)?,
                            purpose_revision: row.get(4)?,
                        })
                    },
                )
                .optional()
                .map_err(task_unavailable)?;
            let Some(premise_row) = found else {
                return Ok(ResumeSourceVerdict::Hold);
            };
            let (companion_text, kind_text, task_text, task_revision, purpose_revision) = (
                premise_row.companion_text,
                premise_row.kind_text,
                premise_row.task_text,
                premise_row.task_revision,
                premise_row.purpose_revision,
            );
            if kind_text != ACTIVITY_KIND_RESUME_INSTRUCTION {
                return Err(task_unavailable("unknown activity record kind"));
            }
            if decode_id(&companion_text).map_err(task_unavailable)? != assignee.companion {
                return Ok(ResumeSourceVerdict::Hold);
            }
            let (Some(task_text), Some(task_revision), Some(purpose_revision)) =
                (task_text, task_revision, purpose_revision)
            else {
                return Err(task_unavailable(
                    "resume activity record missing its selected task",
                ));
            };
            if decode_id(&task_text).map_err(task_unavailable)? != task.as_raw()
                || decode_revision(task_revision)? != expected.revision
                || decode_revision(purpose_revision)? != purpose.adopted_revision
            {
                return Err(task_unavailable(
                    "resume activity does not name the resumed task premise",
                ));
            }
            Ok(ResumeSourceVerdict::Usable)
        }
    }
}

fn source_covered_by_erasure(
    tx: &rusqlite::Transaction<'_>,
    source: RawId,
) -> Result<bool, TaskTechnicalError> {
    crate::preservation::covering_condition(tx, &encode_id(source))
        .map(|condition| condition.is_some())
        .map_err(task_unavailable)
}

fn has_adoptable_sealed_result(
    tx: &rusqlite::Transaction<'_>,
    task: TaskId,
    current_revision: TaskRevision,
) -> Result<bool, TaskTechnicalError> {
    let current_raw = encode_u64(current_revision.as_u64()).map_err(task_unavailable)?;
    let mut statement = tx
        .prepare(SQL_UNADOPTED_RESULTS_AT_REVISION)
        .map_err(task_unavailable)?;
    let rows = statement
        .query_map(params![encode_id(task.as_raw()), current_raw], |row| {
            row.get::<_, String>(0)
        })
        .map_err(task_unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(task_unavailable)?;
    for delegation_text in rows {
        let delegation =
            DelegationId::from_raw(decode_id(&delegation_text).map_err(task_unavailable)?);
        let authoritative = enumerate_delegation_attempts(tx, delegation, task, current_revision)?;
        if authoritative
            .iter()
            .all(|(_, certainty)| *certainty == ActionCertainty::ConfirmedSuccess)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn commit_task_resume_sync(
    conn: &Mutex<Connection>,
    premise: TaskResumeCommitPremise,
    currentness: Option<OwnerMessageCurrentness>,
) -> Result<TaskResumeOutcome, TaskTechnicalError> {
    let task = premise.command.premise.expected.task;
    let task_text = encode_id(task.as_raw());
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(task_unavailable)?;
    let current: Option<RawTask> = tx
        .query_row(SQL_SELECT_TASK, params![task_text], raw_task_row)
        .optional()
        .map_err(task_unavailable)?;
    let Some(current) = current else {
        return Ok(TaskResumeOutcome::MissingTask { task });
    };
    let current_progress = decode_progress(current.progress.as_deref())?;
    if current_progress.is_terminal() {
        return Ok(TaskResumeOutcome::TaskTerminal {
            task,
            progress: current_progress,
        });
    }
    let current_revision = decode_revision(current.revision)?;
    let current_purpose = decode_revision(current.purpose_adopted_revision)?;
    if current_revision != premise.command.premise.expected.revision
        || current_purpose != premise.command.premise.purpose.adopted_revision
    {
        return Ok(TaskResumeOutcome::StalePremise {
            current: TaskRef {
                task,
                revision: current_revision,
            },
        });
    }
    if let Some(currentness) = currentness
        && !owner_message_is_current(&tx, &currentness)?
    {
        return Ok(TaskResumeOutcome::Superseded);
    }
    let assignee = decode_assignee(&current.assignee)?;
    let source = validate_resume_source(
        &tx,
        &premise.command.instruction,
        task,
        premise.command.premise.expected,
        TaskPurposeRef {
            task,
            adopted_revision: current_purpose,
        },
        assignee,
        currentness.is_some(),
    )?;
    if matches!(source, ResumeSourceVerdict::Superseded) {
        return Ok(TaskResumeOutcome::Superseded);
    }
    if !premise.readiness.execution_free {
        return Ok(TaskResumeOutcome::AlreadyRunning { task });
    }
    if enumerate_task_attempts(&tx, task)?
        .iter()
        .any(|(_, certainty)| *certainty == ActionCertainty::Unknown)
    {
        return Ok(TaskResumeOutcome::HeldByUnknownEffects { task });
    }
    if has_adoptable_sealed_result(&tx, task, current_revision)? {
        return Ok(TaskResumeOutcome::ResultAvailable { task });
    }
    let lifecycle: Option<String> = tx
        .query_row(
            SQL_SELECT_COMPANION_LIFECYCLE,
            params![encode_id(assignee.companion)],
            |row| row.get(0),
        )
        .optional()
        .map_err(task_unavailable)?;
    let Some(lifecycle) = lifecycle else {
        return Err(task_unavailable(
            "resume task assignee has no companion row",
        ));
    };
    let running = decode_lifecycle(&lifecycle).map_err(task_unavailable)?
        == ene_companion::CompanionLifecycle::Running;
    let mut associations = {
        let mut statement = tx
            .prepare(SQL_SELECT_WORKSPACE_ASSOCS)
            .map_err(task_unavailable)?;
        statement
            .query_map(params![task_text], |row| {
                Ok(RawWorkspaceAssoc {
                    assoc: row.get(0)?,
                    folder: row.get(1)?,
                    save_target: row.get(2)?,
                })
            })
            .map_err(task_unavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(task_unavailable)?
    };
    if associations.len() > 1 {
        return Err(task_unavailable(
            "multiple workspace associations for the task",
        ));
    }
    let association = associations.pop();
    let current_purpose_entry =
        validated_adopted_purpose_entry(&tx, &task_text, current.revision, current_purpose)?;
    validate_adopted_purpose_provenance(&current_purpose_entry)?;
    let purpose_source =
        decode_id(&current_purpose_entry.origin_source).map_err(task_unavailable)?;
    let instruction_source = match premise.command.instruction {
        ResumeInstructionSource::OwnerHistory { message, .. } => message,
        ResumeInstructionSource::OwnerManagement { activity } => activity,
    };
    let hold = if !running {
        Some(TaskResumeHold::CompanionUnavailable)
    } else if association.is_none() {
        Some(TaskResumeHold::WorkspaceUnavailable)
    } else if matches!(source, ResumeSourceVerdict::Hold) {
        Some(TaskResumeHold::InstructionUnavailable)
    } else if !premise.readiness.permission_available {
        Some(TaskResumeHold::PermissionUnavailable)
    } else if source_covered_by_erasure(&tx, purpose_source)?
        || source_covered_by_erasure(&tx, instruction_source)?
    {
        Some(TaskResumeHold::DataUseHeld)
    } else if !premise.readiness.launch_possible {
        Some(TaskResumeHold::ExecutionUnavailable)
    } else {
        None
    };
    if let Some(hold) = hold {
        return Ok(TaskResumeOutcome::NeedsRevalidation(hold));
    }
    let Some(next_revision) = current_revision.checked_next() else {
        return Ok(TaskResumeOutcome::RevisionExhausted { task });
    };
    let Ok(next_raw) = encode_u64(next_revision.as_u64()) else {
        return Ok(TaskResumeOutcome::RevisionExhausted { task });
    };
    let Some(association) = association else {
        return Err(task_unavailable(
            "workspace association missing after the hold",
        ));
    };
    let snapshot: RawTaskRevision = tx
        .query_row(
            SQL_SELECT_TASK_REVISION,
            params![task_text, current.revision],
            raw_revision_row,
        )
        .optional()
        .map_err(task_unavailable)?
        .ok_or_else(|| {
            task_unavailable("task revision snapshot missing for the current revision")
        })?;
    // A revision forward resolves its new rows from the transaction's current
    // rows, so an inconsistent durable snapshot is a technical error, never a
    // normalized unit.
    if decode_revision(snapshot.purpose_adopted_revision)? != current_purpose {
        return Err(task_unavailable(
            "task revision purpose does not match the current purpose",
        ));
    }
    if decode_assignee(&snapshot.assignee)? != decode_assignee(&current.assignee)? {
        return Err(task_unavailable(
            "task revision assignee does not match the current assignee",
        ));
    }
    // The purpose is carried over: the adopted identity stays, the text
    // stays the snapshot's, and only the entry identity is new. A purpose
    // under a canonical current condition is materialized body-free instead
    // of being re-adopted verbatim.
    let purpose_text = crate::preservation::redact_covered_text(&tx, &snapshot.purpose_text)
        .map_err(|error| task_unavailable(error.to_string()))?;
    tx.execute(
        SQL_INSERT_TASK_REVISION,
        params![
            task_text,
            next_raw,
            current.purpose_adopted_revision,
            purpose_text,
            current.assignee
        ],
    )
    .map_err(task_unavailable)?;
    tx.execute(
        SQL_INSERT_TASK_CONTEXT_ENTRY,
        params![
            encode_id(premise.adopted_purpose_entry.as_raw()),
            task_text,
            next_raw,
            ITEM_KIND_ADOPTED_PURPOSE,
            current.purpose_adopted_revision,
            current_purpose_entry.origin_kind,
            current_purpose_entry.origin_source,
            current_purpose_entry.acquired_at
        ],
    )
    .map_err(task_unavailable)?;
    let (origin_kind, origin_source) = match premise.command.instruction {
        ResumeInstructionSource::OwnerHistory { message, .. } => {
            (ORIGIN_KIND_OWNER_CONVERSATION, encode_id(message))
        }
        ResumeInstructionSource::OwnerManagement { activity } => {
            (ORIGIN_KIND_OWNER_MANAGEMENT, encode_id(activity))
        }
    };
    tx.execute(
        SQL_INSERT_TASK_CONTEXT_ENTRY,
        params![
            encode_id(premise.adopted_instruction_entry.as_raw()),
            task_text,
            next_raw,
            ITEM_KIND_ADOPTED_INSTRUCTION,
            Option::<i64>::None,
            origin_kind,
            origin_source,
            premise.accepted_at.to_rfc3339(),
        ],
    )
    .map_err(task_unavailable)?;
    tx.execute(
        SQL_UPDATE_TASK,
        params![
            task_text,
            next_raw,
            current.purpose_adopted_revision,
            purpose_text
        ],
    )
    .map_err(task_unavailable)?;
    let advanced = tx
        .execute(SQL_MARK_TASK_IN_PROGRESS, params![task_text])
        .map_err(task_unavailable)?;
    if advanced != 1 {
        return Err(task_unavailable(
            "task progress did not advance with resume delegation creation",
        ));
    }
    let workspace = decode_workspace(association, task)?;
    let scope = DelegationScope {
        workspace: Some(DelegatedWorkspace {
            assoc: workspace.assoc,
            folder: workspace.folder,
            save_target: workspace.save_target,
        }),
    };
    let (scope_assoc, scope_folder, scope_save_target) = match &scope.workspace {
        None => (None, None, None),
        Some(workspace) => (
            Some(encode_id(workspace.assoc.as_raw())),
            Some(
                crate::preservation::redact_covered_text(&tx, &workspace.folder.path)
                    .map_err(|error| task_unavailable(error.to_string()))?,
            ),
            workspace
                .save_target
                .as_ref()
                .map(|target| crate::preservation::redact_covered_text(&tx, &target.path))
                .transpose()
                .map_err(|error| task_unavailable(error.to_string()))?,
        ),
    };
    tx.execute(
        SQL_INSERT_DELEGATION,
        params![
            encode_id(premise.delegation.as_raw()),
            task_text,
            next_raw,
            current.assignee,
            encode_id(premise.agent.as_raw()),
            scope_assoc,
            scope_folder,
            scope_save_target,
        ],
    )
    .map_err(task_unavailable)?;
    register_task_undelivered(
        &tx,
        &current.assignee,
        task.as_raw(),
        TaskFact::TaskRevision {
            task: task.as_raw(),
            revision: next_revision.as_u64(),
        },
    )?;
    register_task_undelivered(
        &tx,
        &current.assignee,
        task.as_raw(),
        TaskFact::Delegation(premise.delegation.as_raw()),
    )?;
    tx.commit().map_err(task_unavailable)?;
    Ok(TaskResumeOutcome::Resumed {
        task: TaskRef {
            task,
            revision: next_revision,
        },
        delegation: DelegationRef {
            delegation: premise.delegation,
            task: TaskRef {
                task,
                revision: next_revision,
            },
            delegator: assignee,
            agent: premise.agent,
            scope,
        },
    })
}

/// Prompt-only rendering: a raw newline in a path would break the fixed
/// one-fact-per-line framing documented on `PastExecutedFact::line`.
fn prompt_safe_field(text: &str) -> String {
    text.replace('\n', "\\n").replace('\r', "\\r")
}

/// Reads the Task's past-executed facts for the every-turn prompt block.
///
/// The walk is Task-scoped and bounded: attempts first, then results, each
/// oldest first, capped one past
/// [`PAST_FACTS_ENTRY_CAP`](ene_task::PAST_FACTS_ENTRY_CAP) so truncation
/// is reported instead of reasoned from. Each row contributes attribution
/// only — never a body. SELECT-only.
fn load_past_executed_facts_sync(
    conn: &Mutex<Connection>,
    task: TaskId,
) -> Result<PastExecutedFactsPage, TaskTechnicalError> {
    let cap = i64::try_from(PAST_FACTS_ENTRY_CAP).unwrap_or(i64::MAX);
    let guard = lock_shared(conn);
    let mut statement = guard
        .prepare(SQL_PAST_FACT_ROWS)
        .map_err(task_unavailable)?;
    let rows = statement
        .query_map(
            params![encode_id(task.as_raw()), cap.saturating_add(1)],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(task_unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(task_unavailable)?;
    let has_more = rows.len() as i64 > cap;
    let mut facts = Vec::with_capacity(rows.len().min(PAST_FACTS_ENTRY_CAP));
    for (kind_text, id_text) in rows.into_iter().take(PAST_FACTS_ENTRY_CAP) {
        let id = decode_id(&id_text).map_err(task_unavailable)?;
        match kind_text.as_str() {
            "action_attempt" => {
                let found: Option<(String, String, String)> = guard
                    .query_row(SQL_PAST_ATTEMPT_FACT, params![id_text], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })
                    .optional()
                    .map_err(task_unavailable)?;
                let Some((operation, target, certainty)) = found else {
                    return Err(task_unavailable(
                        "past executed action attempt is not readable",
                    ));
                };
                let operation = ene_action::OperationKind::from_name(&operation)
                    .ok_or_else(|| task_unavailable("unknown past action operation"))?;
                if target.is_empty() {
                    return Err(task_unavailable("past action target is missing"));
                }
                let certainty = decode_certainty(&certainty)?;
                facts.push(PastExecutedFact {
                    source: id,
                    line: format!(
                        "action {} {} {} {}",
                        id.as_uuid(),
                        operation.as_str(),
                        prompt_safe_field(&target),
                        certainty.as_str()
                    ),
                });
            }
            "task_result" => {
                let found: Option<(i64, Option<i64>)> = guard
                    .query_row(SQL_PAST_RESULT_FACT, params![id_text], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })
                    .optional()
                    .map_err(task_unavailable)?;
                let Some((relied_revision, adopted_revision)) = found else {
                    return Err(task_unavailable(
                        "past executed task result is not readable",
                    ));
                };
                let adopted = adopted_revision
                    .map(decode_revision)
                    .transpose()?
                    .map(|revision| revision.as_u64().to_string())
                    .unwrap_or_else(|| String::from("none"));
                facts.push(PastExecutedFact {
                    source: id,
                    line: format!(
                        "result {} rev={} adopted={}",
                        id.as_uuid(),
                        decode_revision(relied_revision)?.as_u64(),
                        adopted
                    ),
                });
            }
            _ => {
                return Err(task_unavailable("unknown past executed fact row kind"));
            }
        }
    }
    Ok(PastExecutedFactsPage { facts, has_more })
}

impl Store {
    pub fn commit_task_resume_sync(
        &self,
        premise: TaskResumeCommitPremise,
    ) -> Result<TaskResumeOutcome, TaskTechnicalError> {
        self.hint_after_commit(commit_task_resume_sync(&self.conn, premise, None))
    }
}

impl TaskRepository for Store {
    async fn create_task(
        &self,
        premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        match self
            .hint_after_commit(run_blocking(move || create_task_sync(&conn, premise, None)).await)?
        {
            TaskCreationOutcome::Created(reference) => Ok(reference),
            TaskCreationOutcome::Superseded => Err(task_unavailable(
                "unguarded task creation cannot answer supersession",
            )),
        }
    }

    async fn forward_steering(
        &self,
        premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        self.hint_after_commit(
            run_blocking(move || forward_steering_sync(&conn, premise, None)).await,
        )
    }

    async fn load_task(&self, task: TaskId) -> Result<Option<TaskRecord>, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_task_sync(&conn, task)).await
    }

    async fn cancel_task(&self, task: TaskId) -> Result<TaskCancelOutcome, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        self.hint_after_commit(run_blocking(move || cancel_task_sync(&conn, task, None)).await)
    }

    async fn fail_task(
        &self,
        premise: TaskFailurePremise,
    ) -> Result<TaskFailureOutcome, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        self.hint_after_commit(run_blocking(move || fail_task_sync(&conn, premise)).await)
    }

    async fn create_delegation(
        &self,
        premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        self.hint_after_commit(run_blocking(move || create_delegation_sync(&conn, premise)).await)
    }

    async fn load_delegation(
        &self,
        delegation: DelegationId,
    ) -> Result<Option<DelegationRef>, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_delegation_sync(&conn, delegation)).await
    }

    async fn record_task_result_arrival(
        &self,
        arrival: TaskAgentResultArrival,
    ) -> Result<TaskResultArrivalOutcome, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        self.hint_after_commit(
            run_blocking(move || record_task_result_arrival_sync(&conn, arrival)).await,
        )
    }

    async fn load_task_result(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_task_result_sync(&conn, result)).await
    }

    async fn load_delegation_result(
        &self,
        delegation: DelegationId,
    ) -> Result<Option<TaskResultRecord>, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_delegation_result_sync(&conn, delegation)).await
    }

    async fn delegation_has_started_work(
        &self,
        delegation: DelegationId,
    ) -> Result<bool, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || delegation_has_started_work_sync(&conn, delegation)).await
    }

    async fn record_task_agent_observation(
        &self,
        premise: TaskAgentObservationPremise,
    ) -> Result<TaskAgentObservationId, TaskTechnicalError> {
        #[cfg(any(test, feature = "test-support"))]
        self.test_parks.observation_write.pause_if_armed().await;
        let conn = Arc::clone(&self.conn);
        run_blocking(move || record_task_agent_observation_sync(&conn, premise)).await
    }

    async fn adopt_result(
        &self,
        claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        self.hint_after_commit(run_blocking(move || adopt_result_sync(&conn, claim)).await)
    }

    async fn load_result_adoption_claim(
        &self,
        result: TaskResultId,
    ) -> Result<Option<TaskResultAdoptionClaim>, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_result_adoption_claim_sync(&conn, result)).await
    }

    async fn list_unadopted_results_after(
        &self,
        after: Option<UnadoptedResultCursor>,
        limit: u64,
    ) -> Result<Vec<UnadoptedResultCursor>, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || list_unadopted_results_after_sync(&conn, after, limit)).await
    }

    async fn load_task_action_attempts(
        &self,
        task: TaskId,
    ) -> Result<Vec<RawId>, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_task_action_attempts_sync(&conn, task)).await
    }

    async fn list_tasks_after(
        &self,
        after: Option<TaskId>,
        limit: u32,
    ) -> Result<Vec<TaskHeadline>, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || list_tasks_after_sync(&conn, after, limit)).await
    }

    async fn list_task_report_rows_after(
        &self,
        task: TaskId,
        after: Option<TaskReportRowCursor>,
        limit: u32,
    ) -> Result<Vec<TaskReportRow>, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || list_task_report_rows_after_sync(&conn, task, after, limit)).await
    }

    async fn load_report_source_bounded(
        &self,
        source: TaskReportSourceRef,
        cursor_bytes: u64,
        limit_bytes: u32,
    ) -> Result<Option<TaskReportSourcePage>, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            load_report_source_bounded_sync(&conn, source, cursor_bytes, limit_bytes)
        })
        .await
    }

    async fn commit_task_resume(
        &self,
        premise: TaskResumeCommitPremise,
    ) -> Result<TaskResumeOutcome, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        self.hint_after_commit(
            run_blocking(move || commit_task_resume_sync(&conn, premise, None)).await,
        )
    }

    async fn load_past_executed_facts(
        &self,
        task: TaskId,
    ) -> Result<PastExecutedFactsPage, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_past_executed_facts_sync(&conn, task)).await
    }
}

impl ConversationTaskRepository for Store {
    async fn create_task_from_conversation(
        &self,
        premise: TaskCreationPremise,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCreationOutcome, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        self.hint_after_commit(
            run_blocking(move || create_task_sync(&conn, premise, Some(currentness))).await,
        )
    }

    async fn forward_steering_from_conversation(
        &self,
        premise: TaskCommitPremise,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        self.hint_after_commit(
            run_blocking(move || forward_steering_sync(&conn, premise, Some(currentness))).await,
        )
    }

    async fn cancel_task_from_conversation(
        &self,
        task: TaskId,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskCancelOutcome, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        self.hint_after_commit(
            run_blocking(move || cancel_task_sync(&conn, task, Some(currentness))).await,
        )
    }

    async fn commit_task_resume_from_conversation(
        &self,
        premise: TaskResumeCommitPremise,
        currentness: OwnerMessageCurrentness,
    ) -> Result<TaskResumeOutcome, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        self.hint_after_commit(
            run_blocking(move || commit_task_resume_sync(&conn, premise, Some(currentness))).await,
        )
    }
}
