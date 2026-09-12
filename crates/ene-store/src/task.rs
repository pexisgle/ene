//! `TaskRepository` over the Task table group.
//!
//! Creation writes the D1 current row, its initial D2 revision snapshot, the
//! adopting context entry, and — when a workspace association was confirmed
//! — that association in one short `Immediate` transaction, so the commit is
//! the only visibility boundary and a crash mid-creation leaves no partial
//! AU2 unit. Steering compares the expected revision to the current row
//! inside the same short transaction and, on success, forwards the D2
//! snapshot, the new revision's adopted-purpose context entry, the adopted
//! instruction entry when the premise carries one, and the D1 pointer
//! atomically. Delegation (AU3) compares the expected revision and the
//! same-revision snapshot's assignee in one short transaction before
//! inserting the correlation row. Reads compose the committed rows or answer
//! `None`: the current revision's adopted-purpose entry first, then every
//! adopted instruction entry up to the current revision. A partial unit, a current row that
//! disagrees with its revision snapshot, purpose, adopted-purpose entry, or
//! assignee, an unknown item kind, and a kind/payload disagreement are
//! technical errors, never fabricated.

use std::sync::Arc;
use std::sync::Mutex;

use ene_primitive::WallClockWithTz;
use ene_task::{
    AssigneeRef, DelegatedWorkspace, DelegationCreationPremise, DelegationId, DelegationOutcome,
    DelegationRef, DelegationScope, Task, TaskAgentEphemeralId, TaskCommitOutcome,
    TaskCommitPremise, TaskContextEntry, TaskContextEntryId, TaskContextItem, TaskContextOrigin,
    TaskContextOriginKind, TaskCreationPremise, TaskId, TaskPurpose, TaskPurposeRef, TaskRecord,
    TaskRef, TaskRepository, TaskRevision, TaskRevisionRecord, TaskTechnicalError,
    WorkspaceAssocId, WorkspaceAssociation, WorkspaceFolderRef,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{decode_id, decode_u64, encode_id, encode_u64, lock_shared};
use crate::run_blocking;

const SQL_INSERT_TASK: &str = "INSERT INTO task (task_id, revision, purpose_adopted_revision, purpose_text, assignee) VALUES (?1, ?2, ?3, ?4, ?5)";

const SQL_INSERT_TASK_REVISION: &str = "INSERT INTO task_revision (task_id, revision, purpose_adopted_revision, purpose_text, assignee) VALUES (?1, ?2, ?3, ?4, ?5)";

const SQL_INSERT_TASK_CONTEXT_ENTRY: &str = "INSERT INTO task_context_entry (entry_id, task_id, revision, item_kind, purpose_adopted_revision, origin_kind, origin_source, acquired_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

const SQL_INSERT_WORKSPACE_ASSOC: &str =
    "INSERT INTO workspace_assoc (assoc_id, task_id, folder, save_target) VALUES (?1, ?2, ?3, ?4)";

const SQL_SELECT_TASK: &str =
    "SELECT revision, purpose_adopted_revision, assignee FROM task WHERE task_id = ?1";

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

const SQL_SELECT_WORKSPACE_ASSOC: &str = "SELECT assoc_id, folder, save_target FROM workspace_assoc WHERE task_id = ?1 ORDER BY rowid LIMIT 1";

const SQL_INSERT_DELEGATION: &str = "INSERT INTO delegation (delegation_id, task_id, task_revision, delegator, agent, scope_assoc, scope_folder, scope_save_target) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)";

const SQL_SELECT_DELEGATION: &str = "SELECT task_id, task_revision, delegator, agent, scope_assoc, scope_folder, scope_save_target FROM delegation WHERE delegation_id = ?1";

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
    if snapshot.assignee != current.assignee {
        return Err(task_unavailable(
            "task revision assignee does not match the current assignee",
        ));
    }
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
        assignee: row.get(2)?,
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
    let revision = TaskRevisionRecord {
        reference,
        purpose: revision_purpose,
        purpose_text: TaskPurpose {
            text: snapshot.purpose_text,
        },
        assignee: revision_assignee,
    };
    let context = load_context_sync(&guard, &task_text, reference, purpose)?;
    let workspace = guard
        .query_row(SQL_SELECT_WORKSPACE_ASSOC, params![task_text], |row| {
            Ok(RawWorkspaceAssoc {
                assoc: row.get(0)?,
                folder: row.get(1)?,
                save_target: row.get(2)?,
            })
        })
        .optional()
        .map_err(task_unavailable)?
        .map(|raw| decode_workspace(raw, task))
        .transpose()?;
    Ok(Some(TaskRecord {
        task: Task {
            reference,
            purpose,
            assignee: task_assignee,
        },
        revision,
        context,
        workspace,
    }))
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
}
