//! `TaskRepository` over the Task table group.
//!
//! Creation writes the D1 current row (progress `started`), its initial D2
//! revision snapshot, the adopting context entry, and — when a workspace
//! association was confirmed — that association in one short `Immediate`
//! transaction, so the commit is the only visibility boundary and a crash
//! mid-creation leaves no partial AU2 unit. Steering compares terminal
//! progress and the expected revision to the current row inside the same
//! short transaction and, on success, forwards the D2 snapshot, the new
//! revision's adopted-purpose context entry, the adopted instruction entry
//! when the premise carries one, and the D1 pointer atomically. Delegation
//! (AU3) compares the expected revision, terminal progress, and the
//! same-revision snapshot's assignee in one short transaction before
//! inserting the correlation row and advancing `started -> in_progress`.
//!
//! The Task result group (AU15a/AU15b) lives in the same repository: the
//! arrival commit writes the one `task_result` row (which itself is the
//! execution seal) with the relied `(task, revision)` copied from the
//! delegation, and the adoption commit enumerates the result-local Action
//! attempts, requires an exact claim match, evaluates the Task-wide
//! completion barrier, and only then stamps the correlation, the adopted
//! revision, and the progress CAS. Reads compose the committed rows or answer
//! `None`: the current revision's adopted-purpose entry first, then every
//! adopted instruction entry up to the current revision, plus the single
//! adopted result when one exists. A partial unit, a current row that
//! disagrees with its revision snapshot, purpose, adopted-purpose entry,
//! assignee, adopted result, or progress, an unknown item kind, and a
//! kind/payload disagreement are technical errors, never fabricated.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;

use ene_action::ActionCertainty;
use ene_primitive::{RawId, WallClockWithTz};
use ene_task::{
    AssigneeRef, DelegatedWorkspace, DelegationCreationPremise, DelegationId, DelegationOutcome,
    DelegationRef, DelegationScope, Task, TaskAgentEphemeralId, TaskAgentOutput,
    TaskAgentResultArrival, TaskCommitOutcome, TaskCommitPremise, TaskContextEntry,
    TaskContextEntryId, TaskContextItem, TaskContextOrigin, TaskContextOriginKind,
    TaskCreationPremise, TaskId, TaskProgress, TaskPurpose, TaskPurposeRef, TaskRecord, TaskRef,
    TaskRepository, TaskResultAcceptance, TaskResultAdoptionClaim, TaskResultId, TaskResultRecord,
    TaskRevision, TaskRevisionRecord, TaskTechnicalError, WorkspaceAssocId, WorkspaceAssociation,
    WorkspaceFolderRef,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{decode_id, decode_u64, encode_id, encode_u64, lock_shared};
use crate::run_blocking;

const SQL_INSERT_TASK: &str = "INSERT INTO task (task_id, revision, purpose_adopted_revision, purpose_text, assignee, progress) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

const SQL_INSERT_TASK_REVISION: &str = "INSERT INTO task_revision (task_id, revision, purpose_adopted_revision, purpose_text, assignee) VALUES (?1, ?2, ?3, ?4, ?5)";

const SQL_INSERT_TASK_CONTEXT_ENTRY: &str = "INSERT INTO task_context_entry (entry_id, task_id, revision, item_kind, purpose_adopted_revision, origin_kind, origin_source, acquired_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

const SQL_INSERT_WORKSPACE_ASSOC: &str =
    "INSERT INTO workspace_assoc (assoc_id, task_id, folder, save_target) VALUES (?1, ?2, ?3, ?4)";

const SQL_SELECT_TASK: &str =
    "SELECT revision, purpose_adopted_revision, progress, assignee FROM task WHERE task_id = ?1";

/// Advances the progress of a non-terminal Task to `in_progress` inside the
/// AU3 transaction. The terminal check is the transaction's own read; the
/// guard makes a concurrent writer that somehow produced a terminal value
/// fail the insert instead of being overwritten.
const SQL_MARK_TASK_IN_PROGRESS: &str = "UPDATE task SET progress = 'in_progress' WHERE task_id = ?1 AND progress IN ('started', 'in_progress')";

/// The completed CAS of the adoption commit: terminal values are never
/// rewritten, so the update must apply exactly once from a non-terminal
/// value in the same transaction that verified the barrier.
const SQL_COMPLETE_TASK: &str = "UPDATE task SET progress = 'completed' WHERE task_id = ?1 AND progress IN ('started', 'in_progress')";

/// The D1 pointer moves only forward: revision, adopted-purpose identity, and
/// the in-force purpose text, all in one statement. The assignee is not part
/// of a steering commit and is deliberately not written.
const SQL_UPDATE_TASK: &str = "UPDATE task SET revision = ?2, purpose_adopted_revision = ?3, purpose_text = ?4 WHERE task_id = ?1";

const SQL_SELECT_TASK_REVISION: &str = "SELECT purpose_adopted_revision, purpose_text, assignee FROM task_revision WHERE task_id = ?1 AND revision = ?2";

const SQL_SELECT_TASK_CONTEXT: &str = "SELECT entry_id, revision, item_kind, purpose_adopted_revision, origin_kind, origin_source, acquired_at FROM task_context_entry WHERE task_id = ?1 ORDER BY revision, entry_id";

/// The adopted-purpose entry in force at the current revision, used to
/// validate the unit before a steering forward writes and to carry its
/// provenance into the next revision when the purpose does not change. The
/// item kind is part of the filter so an instruction row is never read as the
/// predecessor purpose. The payload is not filtered and the probe is not
/// narrowed to one row, so a missing, duplicated, or pointer-mismatched entry
/// stays detectable instead of being normalized.
const SQL_SELECT_ADOPTED_PURPOSE_ENTRY: &str = "SELECT purpose_adopted_revision, origin_kind, origin_source, acquired_at FROM task_context_entry WHERE task_id = ?1 AND revision = ?2 AND item_kind = ?3 ORDER BY rowid LIMIT 2";

/// Probes two rows so a duplicate association (a violated 0..1 invariant) is
/// detected at the storage boundary instead of silently reduced to the first
/// row.
const SQL_SELECT_WORKSPACE_ASSOCS: &str = "SELECT assoc_id, folder, save_target FROM workspace_assoc WHERE task_id = ?1 ORDER BY rowid LIMIT 2";

const SQL_INSERT_DELEGATION: &str = "INSERT INTO delegation (delegation_id, task_id, task_revision, delegator, agent, scope_assoc, scope_folder, scope_save_target) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)";

const SQL_SELECT_DELEGATION: &str = "SELECT task_id, task_revision, delegator, agent, scope_assoc, scope_folder, scope_save_target FROM delegation WHERE delegation_id = ?1";

/// The delegation correspondence alone, used by the result paths where the
/// scope copy is not part of the premise (a malformed scope must not make a
/// result arrival unreadable when the correspondence it needs is intact).
const SQL_SELECT_DELEGATION_CORRESPONDENCE: &str =
    "SELECT task_id, task_revision FROM delegation WHERE delegation_id = ?1";

const SQL_SELECT_RESULT_BY_ID: &str = "SELECT task_id, task_revision, delegation_id, body, adopted_revision, recorded_at FROM task_result WHERE result_id = ?1";

const SQL_SELECT_RESULT_BY_DELEGATION: &str = "SELECT result_id, task_id, task_revision, body, adopted_revision, recorded_at FROM task_result WHERE delegation_id = ?1";

const SQL_INSERT_RESULT: &str = "INSERT INTO task_result (result_id, task_id, task_revision, delegation_id, body, adopted_revision, recorded_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

const SQL_SELECT_RESULT_ATTEMPTS: &str =
    "SELECT attempt_id FROM task_result_attempt WHERE result_id = ?1 ORDER BY attempt_id";

/// Strict by construction: the verified result-local set is fixed at the seal,
/// so the first evaluation inserts it and every later evaluation must find the
/// stored set already exactly equal — a missing row is never refilled.
const SQL_INSERT_RESULT_ATTEMPT: &str =
    "INSERT INTO task_result_attempt (result_id, attempt_id) VALUES (?1, ?2)";

/// The result-local authoritative set enumeration: all Action attempts of the
/// sealed delegation (execution lifetime). The correspondence columns are
/// read back so a row that disagrees with the result's copied
/// `(task, revision)` fails closed instead of being trusted.
const SQL_SELECT_DELEGATION_ATTEMPTS: &str = "SELECT attempt_id, task_id, task_revision, certainty FROM action_attempt WHERE delegation_id = ?1 ORDER BY attempt_id";

/// The Task-wide completion barrier enumeration: every revision and every
/// delegation of the Task. A row is read from either direction — under a
/// delegation of the Task, or with a copied `task_id` naming the Task — so a
/// corrupted copy cannot fall out of the barrier. The delegation columns are
/// joined in and read back so each row's copied correlation is verified, not
/// trusted; timestamps, liveness, and the caller's claim are never inputs.
const SQL_SELECT_TASK_ATTEMPTS: &str = "SELECT a.attempt_id, a.task_id, a.task_revision, a.delegation_id, d.task_id, d.task_revision, a.certainty FROM action_attempt a LEFT JOIN delegation d ON d.delegation_id = a.delegation_id WHERE d.task_id = ?1 OR a.task_id = ?1 ORDER BY a.attempt_id";

/// The copied correlation of one Action attempt, used to verify a stamped
/// result-local row before it is trusted.
const SQL_SELECT_ATTEMPT_CORRESPONDENCE: &str =
    "SELECT delegation_id, task_id, task_revision FROM action_attempt WHERE attempt_id = ?1";

const SQL_MARK_RESULT_ADOPTED: &str = "UPDATE task_result SET adopted_revision = ?2 WHERE result_id = ?1 AND adopted_revision IS NULL";

/// The single adopted result of one Task, if any. The current Task row is
/// the master; this bounded probe only resolves the [`Task`] field, and more
/// than one adopted row is a violated 0..1 invariant, so the probe reads two.
const SQL_SELECT_ADOPTED_RESULT: &str = "SELECT result_id, task_revision, adopted_revision FROM task_result WHERE task_id = ?1 AND adopted_revision IS NOT NULL ORDER BY rowid LIMIT 2";

/// The context origin kinds this stage stores; an unknown stored value is an
/// unreadable row and is rejected on read.
const ORIGIN_KIND_OWNER_CONVERSATION: &str = "owner_conversation";
const ORIGIN_KIND_SPONTANEOUS: &str = "spontaneous";
const ORIGIN_KIND_SCHEDULE_OCCURRENCE: &str = "schedule_occurrence";

/// The stored `item_kind` discriminators. The kind decides which payload is
/// required: an adopted purpose carries the adopted revision, an adopted
/// instruction carries no payload because the entry identity is the adoption
/// identity.
const ITEM_KIND_ADOPTED_PURPOSE: &str = "adopted_purpose";
const ITEM_KIND_ADOPTED_INSTRUCTION: &str = "adopted_instruction";

fn task_unavailable(reason: impl core::fmt::Display) -> TaskTechnicalError {
    TaskTechnicalError::StorageUnavailable {
        reason: reason.to_string(),
    }
}

fn decode_revision(raw: i64) -> Result<TaskRevision, TaskTechnicalError> {
    Ok(TaskRevision::from_u64(
        decode_u64(raw).map_err(task_unavailable)?,
    ))
}

/// Decodes the stored progress, closed world: an unknown name or a stored
/// NULL (the migration adds the column nullable) is an unreadable row, never
/// a default.
fn decode_progress(raw: Option<&str>) -> Result<TaskProgress, TaskTechnicalError> {
    let text = raw.ok_or_else(|| task_unavailable("task progress is missing"))?;
    TaskProgress::from_name(text).ok_or_else(|| task_unavailable("unknown task progress"))
}

/// Decodes the stored Action certainty, closed world. Task never writes it;
/// an unknown stored name is an unreadable row for the barrier.
fn decode_certainty(raw: &str) -> Result<ActionCertainty, TaskTechnicalError> {
    ActionCertainty::from_name(raw).ok_or_else(|| task_unavailable("unknown action certainty"))
}

fn encode_origin_kind(kind: TaskContextOriginKind) -> &'static str {
    match kind {
        TaskContextOriginKind::OwnerConversation => ORIGIN_KIND_OWNER_CONVERSATION,
        TaskContextOriginKind::Spontaneous => ORIGIN_KIND_SPONTANEOUS,
        TaskContextOriginKind::ScheduleOccurrence => ORIGIN_KIND_SCHEDULE_OCCURRENCE,
    }
}

fn decode_origin_kind(text: &str) -> Result<TaskContextOriginKind, TaskTechnicalError> {
    match text {
        ORIGIN_KIND_OWNER_CONVERSATION => Ok(TaskContextOriginKind::OwnerConversation),
        ORIGIN_KIND_SPONTANEOUS => Ok(TaskContextOriginKind::Spontaneous),
        ORIGIN_KIND_SCHEDULE_OCCURRENCE => Ok(TaskContextOriginKind::ScheduleOccurrence),
        _ => Err(task_unavailable("unknown task context origin kind")),
    }
}

/// Decodes one context row's kind and payload together. The kind is resolved
/// first, and a payload that the kind does not allow is one unreadable row:
/// an unknown kind, a purpose without its revision, and an instruction with a
/// purpose revision are all technical errors, never skipped or reinterpreted.
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

fn create_task_sync(
    conn: &Mutex<Connection>,
    premise: TaskCreationPremise,
) -> Result<TaskRef, TaskTechnicalError> {
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
    tx.execute(
        SQL_INSERT_TASK,
        params![
            task_text,
            revision_raw,
            revision_raw,
            premise.purpose.text,
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
            premise.purpose.text,
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
        tx.execute(
            SQL_INSERT_WORKSPACE_ASSOC,
            params![
                encode_id(workspace.assoc.as_raw()),
                task_text,
                workspace.need.folder.path,
                workspace
                    .need
                    .save_target
                    .as_ref()
                    .map(|target| target.path.as_str()),
            ],
        )
        .map_err(task_unavailable)?;
    }
    tx.commit().map_err(task_unavailable)?;
    Ok(reference)
}

/// The adopted-purpose entry read for validation before a steering commit.
struct RawAdoptedPurpose {
    purpose_adopted_revision: Option<i64>,
    origin_kind: String,
    origin_source: String,
    acquired_at: String,
}

/// Reads and validates the adopted-purpose entry in force at `revision`.
///
/// The steering forward must not normalize a unit that every read rejects:
/// exactly one purpose entry may exist at the current revision and its
/// adopted revision must agree with the D1 pointer. Zero rows, two or more
/// rows, a missing payload, and a pointer disagreement are technical errors.
/// The `LIMIT 2` probe keeps the duplicate check at the storage boundary.
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

/// Commits one steering forward (AU4).
///
/// The current row is read inside the `Immediate` transaction, so the compare
/// and every write share one serialization boundary: concurrent steering on
/// the same Task cannot both win, and the stale loser writes nothing. Errors
/// and the stale path return without committing, so the transaction rolls
/// back to the pre-steering state.
fn forward_steering_sync(
    conn: &Mutex<Connection>,
    premise: TaskCommitPremise,
) -> Result<TaskCommitOutcome, TaskTechnicalError> {
    let task = premise.expected.task;
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
        // No Task deletion path exists in this slice; the premise names
        // state that was never committed. Missing is a domain outcome, not a
        // storage failure, and there is no current revision to compare.
        return Ok(TaskCommitOutcome::MissingTask { task });
    };
    // Terminal progress is absorbing and is checked before the revision
    // compare: no steering can ever commit again, so reporting revision
    // staleness alone would invite a retry that can never succeed. Returning
    // without committing drops the transaction, so nothing is written.
    let current_progress = decode_progress(current.progress.as_deref())?;
    if current_progress.is_terminal() {
        return Ok(TaskCommitOutcome::TaskTerminal {
            task,
            progress: current_progress,
        });
    }
    let current_revision = decode_revision(current.revision)?;
    if current_revision != premise.expected.revision {
        // Returning without committing drops the transaction: the stale loser
        // leaves the winner's durable state exactly as it found it.
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
        // The revision column is a signed integer; the first successor it
        // cannot hold exhausts the durable sequence rather than aliasing.
        return Ok(TaskCommitOutcome::RevisionExhausted { task });
    };
    let current_adopted = decode_revision(current.purpose_adopted_revision)?;
    // The forward must never normalize a D1/D2 pair that every read rejects,
    // so the pair the commit depends on is validated before any write.
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
    // The adopted-purpose entry every read resolves must be exactly one row
    // for the current revision and agree with the D1 pointer. Validating it
    // before the first write keeps the forward from normalizing a unit the
    // reads reject; the validated row also supplies the carry-forward
    // provenance when the purpose does not change.
    let current_purpose_entry =
        validated_adopted_purpose_entry(&tx, &task_text, current.revision, current_adopted)?;
    // The adopted-purpose entry's identity comes from the premise in both
    // branches: the repository only stamps the post-CAS reference and, on a
    // change, the adopted revision. On a carry-forward the old adopted
    // identity stays in `item`, while the provenance and acquisition are
    // copied from the validated entry in force.
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
                // Fail closed on unreadable stored provenance instead of copying
                // corruption into the new revision. The reads only validate; the
                // bytes stay as stored.
                decode_origin_kind(&current_purpose_entry.origin_kind)?;
                decode_id(&current_purpose_entry.origin_source).map_err(task_unavailable)?;
                decode_clock(&current_purpose_entry.acquired_at)?;
                (
                    current_adopted,
                    snapshot.purpose_text,
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
        // The instruction entry is written once at its adoption revision: its
        // identity is the entry itself, so the payload column stays NULL. A
        // failure here rolls back the whole forward, purpose entry included.
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
    tx.commit().map_err(task_unavailable)?;
    Ok(TaskCommitOutcome::CommittedAs(TaskRef {
        task,
        revision: next_revision,
    }))
}

/// Commits one delegation correlation row (AU3).
///
/// The current Task row and the snapshot it points at are read inside one
/// `Immediate` transaction, so the revision compare and the insert share a
/// serialization boundary: a concurrent steering forward either precedes the
/// delegation (which then answers stale) or follows it (the delegation stays
/// bound to the revision it compared). Missing and stale return `Ok` without
/// committing, so neither leaves a row; a missing snapshot or an assignee
/// disagreement is a technical error and never a synthesized correspondence.
/// The delegator is copied from the same row the compare read. The row's
/// existence is a correlation, not proof that the agent is running or alive.
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
        // No Task deletion path exists in this slice; the premise names state
        // that was never committed. Missing is a domain outcome, not a storage
        // failure, and the uncommitted transaction leaves zero rows.
        return Ok(DelegationOutcome::MissingTask { task: task.task });
    };
    let current_revision = decode_revision(current.revision)?;
    if current_revision != task.revision {
        // The stale loser writes nothing: returning drops the transaction, so
        // the winner's durable state is exactly as found.
        return Ok(DelegationOutcome::StaleTaskRevision {
            current: TaskRef {
                task: task.task,
                revision: current_revision,
            },
        });
    }
    // Terminal progress refuses the whole creation: no delegation row and no
    // revision advance. The task revision and progress are read in the same
    // snapshot as the compare above.
    let current_progress = decode_progress(current.progress.as_deref())?;
    if current_progress.is_terminal() {
        return Ok(DelegationOutcome::TaskTerminal {
            task: task.task,
            progress: current_progress,
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
    // Validate the stored identity text before copying it as the delegator: a
    // malformed stored identity is an unreadable row, not a new value. Decode
    // both rows so this check agrees with the other read paths instead of
    // comparing one string form against another.
    let current_assignee = decode_assignee(&current.assignee)?;
    let snapshot_assignee = decode_assignee(&snapshot.assignee)?;
    if snapshot_assignee != current_assignee {
        return Err(task_unavailable(
            "task revision assignee does not match the current assignee",
        ));
    }
    let delegator = current_assignee;
    // The scope is a copy of the boundary the delegator relied on, written
    // verbatim; it records the boundary, not a permission.
    let (scope_assoc, scope_folder, scope_save_target) = match &scope_copy.workspace {
        None => (None, None, None),
        Some(workspace) => (
            Some(encode_id(workspace.assoc.as_raw())),
            Some(workspace.folder.path.as_str()),
            workspace
                .save_target
                .as_ref()
                .map(|target| target.path.as_str()),
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
    // The first delegation advances `started -> in_progress`; later
    // delegations keep `in_progress` in place. Exactly one row must move, or
    // the transaction rolls back and no delegation becomes visible.
    let advanced = tx
        .execute(SQL_MARK_TASK_IN_PROGRESS, params![task_text])
        .map_err(task_unavailable)?;
    if advanced != 1 {
        return Err(task_unavailable(
            "task progress did not advance with delegation creation",
        ));
    }
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
    /// Nullable only for the V20 add-column migration; reads fail closed on
    /// NULL and new writes always name a value.
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

/// Decodes the copied scope boundary. `NULL` association means no workspace,
/// and then both path columns must be `NULL`; an association without its
/// folder is as unreadable as a folder without an association.
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

/// Reads the context of one Task at `current`.
///
/// The result is the adopted-purpose entry in force first, then every adopted
/// instruction entry up to the current revision in `(revision, entry_id)`
/// order. The query is not bounded by kind or revision in SQL: an unknown
/// kind, a kind/payload disagreement, or any row beyond the current revision
/// is corruption the read detects instead of silently excluding. Purpose
/// entries of past revisions are retained D2 history, not context.
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
        // Resolve the kind before reading any payload so a row is never
        // classified by its payload when the two disagree.
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
    // The snapshot is keyed by the D1 current revision, so a row can only be
    // the record of that revision; a missing row is an incomplete unit.
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
    // The D1 current row and its D2 snapshot must describe the same purpose
    // and assignee. A mismatch is an inconsistent unit, never a TaskRecord.
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
    // The association is 0..1 for this stage: two rows are a violated
    // invariant and a technical error, never a silent first-row pick.
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

/// Resolves the single adopted result of one Task from the bounded
/// `task_result` probe, verifying it against the current unit.
///
/// The adopted row is not a second master: `task_result.adopted_revision` is
/// the durable stamp, and this read only composes the [`Task`] field. The
/// adoption commit can only run while the current revision equals the relied
/// revision and the Task is non-terminal, so an adopted row on a moved
/// revision, on a non-completed Task, or more than one adopted row is an
/// inconsistent unit and fails closed. A completed Task must name the result
/// it was completed by.
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

/// Reads one delegation correlation row by decoding every stored column.
///
/// `None` means the identity has no stored delegation. A malformed identity
/// or a scope copy that does not describe a boundary is a technical error,
/// never a fabricated [`DelegationRef`]. Row existence is not liveness: it
/// says only that the delegation was created against the stored revision.
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

/// Reads the result-local verified correlation of one result, in attempt-id
/// order. The set is stamped once at adoption evaluation and never grows or
/// shrinks: an empty read means no evaluation has stamped it yet.
fn load_result_attempts(
    conn: &Connection,
    result: TaskResultId,
) -> Result<Vec<RawId>, TaskTechnicalError> {
    let mut statement = conn
        .prepare(SQL_SELECT_RESULT_ATTEMPTS)
        .map_err(task_unavailable)?;
    let rows = statement
        .query_map(params![encode_id(result.as_raw())], |row| {
            row.get::<_, String>(0)
        })
        .map_err(task_unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(task_unavailable)?;
    rows.into_iter()
        .map(|text| decode_id(&text).map_err(task_unavailable))
        .collect()
}

/// Reads one result's bounded durable correlation unit and composes the
/// record, or fails closed.
///
/// Before the record is built: the sealed delegation must exist and its stored
/// `(task_id, task_revision)` must equal the result's copied correlation, and
/// every stamped result-local attempt must be an `action_attempt` row whose
/// copied delegation/task/revision equal the result's. A missing delegation,
/// a missing attempt, or any disagreement is an inconsistent unit and a
/// technical error, never a silently recomposed record. The remembered
/// certainty values stay with their Action owner and are not duplicated here.
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
    let attempt_refs = load_result_attempts(conn, result)?;
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
    Ok(TaskResultRecord {
        result,
        task: TaskRef {
            task: TaskId::from_raw(decode_id(&task).map_err(task_unavailable)?),
            revision: decode_revision(task_revision)?,
        },
        delegation: DelegationId::from_raw(decode_id(&delegation).map_err(task_unavailable)?),
        body: TaskAgentOutput::new(body),
        attempt_refs,
        adopted_revision: adopted_revision.map(decode_revision).transpose()?,
        recorded_at: decode_clock(&recorded_at)?,
    })
}

/// Records one final result arrival and seals its delegation (AU15a).
///
/// Every check lives in the one short `Immediate` transaction: the
/// delegation correspondence must exist and decode, a same-identity retry
/// must match the stored body/delegation/revision exactly (idempotent, one
/// body row), and a different identity for an already-sealed delegation is a
/// fail-closed technical error. Currentness, certainty, terminal state, and
/// completion are deliberately not judged here.
fn record_task_result_arrival_sync(
    conn: &Mutex<Connection>,
    arrival: TaskAgentResultArrival,
) -> Result<TaskResultRecord, TaskTechnicalError> {
    let delegation_text = encode_id(arrival.delegation.as_raw());
    let result_text = encode_id(arrival.result.as_raw());
    let body_text = arrival.body.text().to_owned();
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(task_unavailable)?;
    let correspondence: Option<(String, i64)> = tx
        .query_row(
            SQL_SELECT_DELEGATION_CORRESPONDENCE,
            params![delegation_text],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(task_unavailable)?;
    let Some((task_text, revision_raw)) = correspondence else {
        // The finalization premise names a correspondence that is not
        // durable: an inconsistent unit, never a domain "missing" answer
        // (AU15b reports missing identities for adoption).
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
        return Ok(record);
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
        // Durable invariant: one delegation has at most one final result.
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
    tx.commit().map_err(task_unavailable)?;
    Ok(TaskResultRecord {
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
    })
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

/// Enumerates the result-local authoritative set from the sealed delegation.
///
/// Every row's `(task, revision)` is verified against the result's copied
/// correspondence; a disagreement is an inconsistent unit and fails closed.
/// `action_attempt` stores only the closed-world certainty vocabulary, so an
/// unknown name is unreadable.
fn enumerate_delegation_attempts(
    tx: &rusqlite::Transaction<'_>,
    delegation: DelegationId,
    task: TaskId,
    relied_revision: TaskRevision,
) -> Result<Vec<(RawId, ActionCertainty)>, TaskTechnicalError> {
    let mut statement = tx
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

/// Enumerates the Task-wide barrier input: every attempt under the Task, in
/// attempt-id order.
///
/// The enumeration is correspondence-aware. It reads attempts from both
/// directions — those under a delegation of the Task and those whose copied
/// `task_id` names the Task — and verifies each row's copied
/// `(task_id, task_revision)` against its delegation's stored correspondence.
/// A row whose delegation is missing, or whose copied correlation disagrees
/// with the delegation (or with the target Task), is an inconsistent unit:
/// it fails closed instead of being read as a certainty fact or dropped from
/// the barrier. Scope stays Task-wide: all revisions and all delegations.
fn enumerate_task_attempts(
    tx: &rusqlite::Transaction<'_>,
    task_id: TaskId,
) -> Result<Vec<(RawId, ActionCertainty)>, TaskTechnicalError> {
    let task_text = encode_id(task_id.as_raw());
    let mut statement = tx
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

/// Requires the caller's claim to equal the authoritative set exactly:
/// missing, extra, and duplicate refs are all inconsistent units and fail
/// closed instead of being rounded to stale or withheld.
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

/// Verifies and stamps the result-local fixed set.
///
/// An empty stored set is the unstamped first evaluation: the authoritative
/// set is inserted whole. Once any row is stamped the set is durable and
/// fixed, so the stored set must equal the authoritative set exactly — a
/// missing, extra, or undecodable row is an inconsistent unit and fails
/// closed. There is no silent refill: a stored non-empty set is never
/// extended or repaired.
fn stamp_result_attempts(
    tx: &rusqlite::Transaction<'_>,
    result_text: &str,
    attempts: &[RawId],
) -> Result<(), TaskTechnicalError> {
    let mut statement = tx
        .prepare(SQL_SELECT_RESULT_ATTEMPTS)
        .map_err(task_unavailable)?;
    let stored = statement
        .query_map(params![result_text], |row| row.get::<_, String>(0))
        .map_err(task_unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(task_unavailable)?;
    drop(statement);
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
    let stored: HashSet<RawId> = stored
        .into_iter()
        .map(|text| decode_id(&text).map_err(task_unavailable))
        .collect::<Result<_, _>>()?;
    let authoritative: HashSet<RawId> = attempts.iter().copied().collect();
    if stored != authoritative {
        return Err(task_unavailable(
            "stored task result correlation is not exactly the authoritative attempt set",
        ));
    }
    Ok(())
}

/// Attempts one adoption commit (AU15b) in a single short transaction.
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
    // The authoritative set is resolved from the execution lifetime, and the
    // caller's claim must match it exactly before any write is considered.
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
    if let Some(adopted_raw) = raw.adopted_revision {
        // This result already committed its adoption. The retry is idempotent
        // only while the durable current unit still agrees with the completed
        // state: the adopted stamp equals the relied revision, the current
        // Task is still at that revision and completed, and exactly this
        // result is the Task's adopted result. The same checks `load_task`
        // applies, so a corrupted current unit fails closed here too instead
        // of answering success from a stale stamp.
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
        // The result-local set was stamped in the same transaction as the
        // adopted stamp, so a retry must find it already exactly equal.
        stamp_result_attempts(&tx, &result_text, &local)?;
        tx.commit().map_err(task_unavailable)?;
        return Ok(TaskResultAcceptance::AdoptedAsCompletion(TaskRef {
            task: task_id,
            revision: relied_revision,
        }));
    }
    // The purpose identity is the relied revision's snapshot correspondence,
    // never a text comparison. A missing snapshot is an inconsistent unit.
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
    if current_revision != relied_revision
        || relied_purpose != current_purpose
        || current_progress.is_terminal()
    {
        stamp_result_attempts(&tx, &result_text, &local)?;
        tx.commit().map_err(task_unavailable)?;
        return Ok(TaskResultAcceptance::RecordedToOriginalOnly);
    }
    // Blockers: relied attempts that are not `ConfirmedSuccess`, union every
    // `Unknown` under the same Task across all revisions and delegations.
    // Cross-delegation / old-revision confirmed facts never block by
    // themselves, and barrier attempts are never stamped as dependencies.
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
    tx.commit().map_err(task_unavailable)?;
    Ok(TaskResultAcceptance::AdoptedAsCompletion(TaskRef {
        task: task_id,
        revision: relied_revision,
    }))
}

impl TaskRepository for Store {
    async fn create_task(
        &self,
        premise: TaskCreationPremise,
    ) -> Result<TaskRef, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || create_task_sync(&conn, premise)).await
    }

    async fn forward_steering(
        &self,
        premise: TaskCommitPremise,
    ) -> Result<TaskCommitOutcome, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || forward_steering_sync(&conn, premise)).await
    }

    async fn load_task(&self, task: TaskId) -> Result<Option<TaskRecord>, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_task_sync(&conn, task)).await
    }

    async fn create_delegation(
        &self,
        premise: DelegationCreationPremise,
    ) -> Result<DelegationOutcome, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || create_delegation_sync(&conn, premise)).await
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
    ) -> Result<TaskResultRecord, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || record_task_result_arrival_sync(&conn, arrival)).await
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

    async fn adopt_result(
        &self,
        claim: TaskResultAdoptionClaim,
    ) -> Result<TaskResultAcceptance, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || adopt_result_sync(&conn, claim)).await
    }
}
