//! `TaskRepository` over the Task table group.
//!
//! Creation writes the D1 current row, its initial D2 revision snapshot, the
//! adopting context entry, and — when a workspace association was confirmed
//! — that association in one short `Immediate` transaction, so the commit is
//! the only visibility boundary and a crash mid-creation leaves no partial
//! AU2 unit. Steering compares the expected revision to the current row
//! inside the same short transaction and, on success, forwards the D2
//! snapshot, the new revision's adopted-purpose context entry, and the D1
//! pointer atomically. Reads compose the committed rows of the current
//! revision or answer `None`; a partial unit or a current row that disagrees
//! with its revision snapshot, purpose, adopted context entry, or assignee is
//! a technical error, never fabricated.

use std::sync::Arc;
use std::sync::Mutex;

use ene_primitive::WallClockWithTz;
use ene_task::{
    AssigneeRef, Task, TaskCommitOutcome, TaskCommitPremise, TaskContextEntry, TaskContextEntryId,
    TaskContextItem, TaskContextOrigin, TaskContextOriginKind, TaskCreationPremise, TaskId,
    TaskPurpose, TaskPurposeRef, TaskRecord, TaskRef, TaskRepository, TaskRevision,
    TaskRevisionRecord, TaskTechnicalError, WorkspaceAssocId, WorkspaceAssociation,
    WorkspaceFolderRef,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{decode_id, decode_u64, encode_id, encode_u64, lock_shared};
use crate::run_blocking;

const SQL_INSERT_TASK: &str = "INSERT INTO task (task_id, revision, purpose_adopted_revision, purpose_text, assignee) VALUES (?1, ?2, ?3, ?4, ?5)";

const SQL_INSERT_TASK_REVISION: &str = "INSERT INTO task_revision (task_id, revision, purpose_adopted_revision, purpose_text, assignee) VALUES (?1, ?2, ?3, ?4, ?5)";

const SQL_INSERT_TASK_CONTEXT_ENTRY: &str = "INSERT INTO task_context_entry (entry_id, task_id, revision, purpose_adopted_revision, origin_kind, origin_source, acquired_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

const SQL_INSERT_WORKSPACE_ASSOC: &str =
    "INSERT INTO workspace_assoc (assoc_id, task_id, folder, save_target) VALUES (?1, ?2, ?3, ?4)";

const SQL_SELECT_TASK: &str =
    "SELECT revision, purpose_adopted_revision, assignee FROM task WHERE task_id = ?1";

/// The D1 pointer moves only forward: revision, adopted-purpose identity, and
/// the in-force purpose text, all in one statement. The assignee is not part
/// of a steering commit and is deliberately not written.
const SQL_UPDATE_TASK: &str = "UPDATE task SET revision = ?2, purpose_adopted_revision = ?3, purpose_text = ?4 WHERE task_id = ?1";

const SQL_SELECT_TASK_REVISION: &str = "SELECT purpose_adopted_revision, purpose_text, assignee FROM task_revision WHERE task_id = ?1 AND revision = ?2";

const SQL_SELECT_TASK_CONTEXT: &str = "SELECT entry_id, purpose_adopted_revision, origin_kind, origin_source, acquired_at FROM task_context_entry WHERE task_id = ?1 AND revision = ?2 ORDER BY rowid";

/// The adopted-purpose entry in force at the current revision, used to carry
/// its provenance into the next revision when the purpose does not change.
const SQL_SELECT_ADOPTED_PURPOSE_ENTRY: &str = "SELECT origin_kind, origin_source, acquired_at FROM task_context_entry WHERE task_id = ?1 AND revision = ?2 AND purpose_adopted_revision = ?3 ORDER BY rowid LIMIT 1";

const SQL_SELECT_WORKSPACE_ASSOC: &str = "SELECT assoc_id, folder, save_target FROM workspace_assoc WHERE task_id = ?1 ORDER BY rowid LIMIT 1";

/// The context origin kinds this stage stores; an unknown stored value is an
/// unreadable row and is rejected on read.
const ORIGIN_KIND_OWNER_CONVERSATION: &str = "owner_conversation";
const ORIGIN_KIND_SPONTANEOUS: &str = "spontaneous";
const ORIGIN_KIND_SCHEDULE_OCCURRENCE: &str = "schedule_occurrence";

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
    // The adopted-purpose entry is the only context kind AU4 records. Its
    // identity comes from the premise in both branches: the repository only
    // stamps the post-CAS reference and, on a change, the adopted revision.
    // On a carry-forward the old adopted identity stays in `item`, while the
    // provenance and acquisition are copied from the entry in force.
    let (adopted_revision, purpose_text, origin_kind, origin_source, acquired_at) = match premise
        .new_purpose
    {
        Some(adoption) => (
            next_revision,
            adoption.purpose.text,
            encode_origin_kind(adoption.origin.kind).to_owned(),
            encode_id(adoption.origin.source),
            adoption.acquired_at.to_rfc3339(),
        ),
        None => {
            let predecessor: (String, String, String) = tx
                .query_row(
                    SQL_SELECT_ADOPTED_PURPOSE_ENTRY,
                    params![
                        task_text,
                        current.revision,
                        current.purpose_adopted_revision
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(task_unavailable)?
                .ok_or_else(|| {
                    task_unavailable("task adopted purpose entry missing for the current revision")
                })?;
            // Fail closed on unreadable stored provenance instead of copying
            // corruption into the new revision. The reads only validate; the
            // bytes stay as stored.
            decode_origin_kind(&predecessor.0)?;
            decode_id(&predecessor.1).map_err(task_unavailable)?;
            decode_clock(&predecessor.2)?;
            (
                current_adopted,
                snapshot.purpose_text,
                predecessor.0,
                predecessor.1,
                predecessor.2,
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
            adopted_raw,
            origin_kind,
            origin_source,
            acquired_at
        ],
    )
    .map_err(task_unavailable)?;
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
    purpose_adopted_revision: i64,
    origin_kind: String,
    origin_source: String,
    acquired_at: String,
}

struct RawWorkspaceAssoc {
    assoc: String,
    folder: String,
    save_target: Option<String>,
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
    revision_raw: i64,
    reference: TaskRef,
) -> Result<Vec<TaskContextEntry>, TaskTechnicalError> {
    let mut statement = conn
        .prepare(SQL_SELECT_TASK_CONTEXT)
        .map_err(task_unavailable)?;
    let rows = statement
        .query_map(params![task_text, revision_raw], |row| {
            Ok(RawContextEntry {
                entry: row.get(0)?,
                purpose_adopted_revision: row.get(1)?,
                origin_kind: row.get(2)?,
                origin_source: row.get(3)?,
                acquired_at: row.get(4)?,
            })
        })
        .map_err(task_unavailable)?;
    let mut entries = Vec::new();
    for row in rows {
        let raw = row.map_err(task_unavailable)?;
        entries.push(TaskContextEntry {
            entry: TaskContextEntryId::from_raw(decode_id(&raw.entry).map_err(task_unavailable)?),
            reference,
            item: TaskContextItem::AdoptedPurpose(TaskPurposeRef {
                task: reference.task,
                adopted_revision: decode_revision(raw.purpose_adopted_revision)?,
            }),
            origin: TaskContextOrigin {
                kind: decode_origin_kind(&raw.origin_kind)?,
                source: decode_id(&raw.origin_source).map_err(task_unavailable)?,
            },
            acquired_at: decode_clock(&raw.acquired_at)?,
        });
    }
    Ok(entries)
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
    let context = load_context_sync(&guard, &task_text, raw_task.revision, reference)?;
    if context.is_empty() {
        return Err(task_unavailable(
            "task context entries missing for the current revision",
        ));
    }
    // AU2 records the adopted purpose as the only item kind; a non-matching
    // adopted identity is an inconsistent unit. The first slice that adds
    // another item kind must extend this read rule with it.
    if !context.iter().all(|entry| {
        matches!(entry.item, TaskContextItem::AdoptedPurpose(adopted) if adopted == purpose)
    }) {
        return Err(task_unavailable(
            "task context adopted purpose does not match the current purpose",
        ));
    }
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
}
