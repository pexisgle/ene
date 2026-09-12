//! `TaskRepository` over the Task table group.
//!
//! Creation writes the D1 current row, its initial D2 revision snapshot, the
//! adopting context entry, and — when a workspace association was confirmed
//! — that association in one short `Immediate` transaction, so the commit is
//! the only visibility boundary and a crash mid-creation leaves no partial
//! AU2 unit. Reads compose the committed rows of the current revision or
//! answer `None`; a partial unit or a current row that disagrees with its
//! revision snapshot, purpose, adopted context entry, or assignee is a
//! technical error, never fabricated.

use std::sync::Arc;
use std::sync::Mutex;

use ene_primitive::WallClockWithTz;
use ene_task::{
    AssigneeRef, Task, TaskContextEntry, TaskContextEntryId, TaskContextItem, TaskContextOrigin,
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

const SQL_INSERT_TASK_CONTEXT_ENTRY: &str = "INSERT INTO task_context_entry (entry_id, task_id, revision, purpose_adopted_revision, origin_kind, origin_source, acquired_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";

const SQL_INSERT_WORKSPACE_ASSOC: &str =
    "INSERT INTO workspace_assoc (assoc_id, task_id, folder, save_target) VALUES (?1, ?2, ?3, ?4)";

const SQL_SELECT_TASK: &str =
    "SELECT revision, purpose_adopted_revision, assignee FROM task WHERE task_id = ?1";

const SQL_SELECT_TASK_REVISION: &str = "SELECT purpose_adopted_revision, purpose_text, assignee FROM task_revision WHERE task_id = ?1 AND revision = ?2";

const SQL_SELECT_TASK_CONTEXT: &str = "SELECT entry_id, purpose_adopted_revision, origin_kind, origin_source, acquired_at FROM task_context_entry WHERE task_id = ?1 AND revision = ?2 ORDER BY rowid";

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

    async fn load_task(&self, task: TaskId) -> Result<Option<TaskRecord>, TaskTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_task_sync(&conn, task)).await
    }
}
