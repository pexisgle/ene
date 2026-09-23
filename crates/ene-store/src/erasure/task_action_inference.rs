use ene_preservation::{ErasureConditionRef, ParticipantOwnerRef};
use rusqlite::{Transaction, params};

use crate::Store;
use crate::codec::encode_id;

use super::{
    ErasurePageError, LocalErasureParticipant, PageOutcome, PageRequest, SweepCursor, bounded_step,
    redact_exact,
};

#[cfg(windows)]
pub(crate) const ERASED_LOCATOR: &str = r"C:\erased";
#[cfg(not(windows))]
pub(crate) const ERASED_LOCATOR: &str = "/erased";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErasureShape {
    Text,
    AbsolutePath,
}

#[derive(Debug, Clone, Copy)]
struct ErasureColumn {
    name: &'static str,
    shape: ErasureShape,
}

#[derive(Debug, Clone, Copy)]
struct ErasureStage {
    table: &'static str,
    columns: &'static [ErasureColumn],
}

const PURPOSE_TEXT: ErasureColumn = ErasureColumn {
    name: "purpose_text",
    shape: ErasureShape::Text,
};

const TASK_BODY: ErasureColumn = ErasureColumn {
    name: "body",
    shape: ErasureShape::Text,
};

const OBSERVATION_PATH: ErasureColumn = ErasureColumn {
    name: "path",
    shape: ErasureShape::AbsolutePath,
};

const WORKSPACE_FOLDER: ErasureColumn = ErasureColumn {
    name: "folder",
    shape: ErasureShape::Text,
};

const WORKSPACE_SAVE_TARGET: ErasureColumn = ErasureColumn {
    name: "save_target",
    shape: ErasureShape::Text,
};

const DELEGATION_SCOPE_FOLDER: ErasureColumn = ErasureColumn {
    name: "scope_folder",
    shape: ErasureShape::Text,
};

const DELEGATION_SCOPE_SAVE_TARGET: ErasureColumn = ErasureColumn {
    name: "scope_save_target",
    shape: ErasureShape::Text,
};

const ACTION_REAL_TARGET: ErasureColumn = ErasureColumn {
    name: "real_target",
    shape: ErasureShape::AbsolutePath,
};

const TASK_STAGES: &[ErasureStage] = &[
    ErasureStage {
        table: "task",
        columns: &[PURPOSE_TEXT],
    },
    ErasureStage {
        table: "task_revision",
        columns: &[PURPOSE_TEXT],
    },
    ErasureStage {
        table: "task_result",
        columns: &[TASK_BODY],
    },
    ErasureStage {
        table: "task_agent_observation",
        columns: &[OBSERVATION_PATH],
    },
    ErasureStage {
        table: "workspace_assoc",
        columns: &[WORKSPACE_FOLDER, WORKSPACE_SAVE_TARGET],
    },
    ErasureStage {
        table: "delegation",
        columns: &[DELEGATION_SCOPE_FOLDER, DELEGATION_SCOPE_SAVE_TARGET],
    },
];

const ACTION_STAGES: &[ErasureStage] = &[ErasureStage {
    table: "action_attempt",
    columns: &[ACTION_REAL_TARGET],
}];

const INFERENCE_STAGES: &[ErasureStage] = &[];

pub(crate) fn task_content_surface() -> impl Iterator<Item = (&'static str, &'static str)> {
    TASK_STAGES.iter().flat_map(|stage| {
        stage
            .columns
            .iter()
            .map(|column| (stage.table, column.name))
    })
}

pub(crate) fn action_content_surface() -> impl Iterator<Item = (&'static str, &'static str)> {
    ACTION_STAGES.iter().flat_map(|stage| {
        stage
            .columns
            .iter()
            .map(|column| (stage.table, column.name))
    })
}

struct ValueRedaction {
    column: &'static str,
    value: String,
    removed: u64,
}

struct RawErasureRow {
    rowid: i64,
    values: Vec<Option<String>>,
}

fn page_sql(stage: &ErasureStage) -> String {
    let columns = stage
        .columns
        .iter()
        .map(|column| column.name)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT rowid, {columns} FROM {} WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
        stage.table
    )
}

fn stage_page(
    tx: &Transaction<'_>,
    stage: &ErasureStage,
    request: &PageRequest<'_>,
    condition: ErasureConditionRef,
) -> Result<PageOutcome, ErasurePageError> {
    let rows = {
        let mut statement = tx.prepare(&page_sql(stage))?;
        statement
            .query_map(params![request.after_ordinal, request.limit], |row| {
                let mut values = Vec::with_capacity(stage.columns.len());
                for index in 0..stage.columns.len() {
                    values.push(row.get::<_, Option<String>>(index + 1)?);
                }
                Ok(RawErasureRow {
                    rowid: row.get(0)?,
                    values,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut scanned = 0u32;
    let mut matched = 0u64;
    let mut deleted = 0u64;
    let mut last = None;
    for row in &rows {
        scanned += 1;
        last = Some((String::new(), row.rowid));
        let provenance_linked = stage.table == "task_result"
            && result_delegation_held_for_operation(tx, row.rowid, condition)?;
        let planned = plan_redactions(stage, row, request.target, provenance_linked)?;
        if planned.is_empty() {
            continue;
        }
        matched += 1;
        if !request.delete {
            continue;
        }
        for value in planned {
            let sql = format!(
                "UPDATE {} SET {} = ?1 WHERE rowid = ?2",
                stage.table, value.column
            );
            tx.execute(&sql, params![value.value, row.rowid])?;
            deleted += value.removed;
        }
    }
    Ok(PageOutcome {
        scanned,
        matched,
        deleted,
        last,
    })
}

fn result_delegation_held_for_operation(
    tx: &Transaction<'_>,
    rowid: i64,
    condition: ErasureConditionRef,
) -> Result<bool, ErasurePageError> {
    tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM task_result r
             JOIN erasure_use_hold h
               ON h.use_kind = 'task_delegation' AND h.use_id = r.delegation_id
             WHERE r.rowid = ?1 AND h.operation_id = ?2
         )",
        params![rowid, encode_id(condition.operation.as_raw())],
        |row| row.get(0),
    )
    .map_err(|_| ErasurePageError::Storage)
}

fn plan_redactions(
    stage: &ErasureStage,
    row: &RawErasureRow,
    target: &str,
    provenance_linked: bool,
) -> Result<Vec<ValueRedaction>, ErasurePageError> {
    let mut planned = Vec::new();
    for (column, value) in stage.columns.iter().zip(&row.values) {
        let Some(text) = value.as_deref() else {
            continue;
        };
        if provenance_linked && column.name == "body" {
            let replacement = redact_exact(super::ERASED_MARKER, target).map_or_else(
                || String::from(super::ERASED_MARKER),
                |(redacted, _)| redacted,
            );
            if replacement.as_str() != text {
                planned.push(ValueRedaction {
                    column: column.name,
                    value: replacement,
                    removed: 1,
                });
            }
            continue;
        }
        let erased = match column.shape {
            ErasureShape::Text => redact_exact(text, target),
            ErasureShape::AbsolutePath => erase_path(text, target)?,
        };
        if let Some((value, removed)) = erased {
            planned.push(ValueRedaction {
                column: column.name,
                value,
                removed,
            });
        }
    }
    Ok(planned)
}

fn erase_path(text: &str, target: &str) -> Result<Option<(String, u64)>, ErasurePageError> {
    let Some((redacted, removed)) = redact_exact(text, target) else {
        return Ok(None);
    };
    if !std::path::Path::new(&redacted).is_absolute() {
        if ERASED_LOCATOR.contains(target) {
            return Err(ErasurePageError::Unrepresentable);
        }
        return Ok(Some((String::from(ERASED_LOCATOR), removed)));
    }
    Ok(Some((redacted, removed)))
}

fn task_step(
    tx: &Transaction<'_>,
    cursor: &mut SweepCursor,
    target: &str,
) -> Result<(), ErasurePageError> {
    let condition = cursor.condition;
    bounded_step(
        tx,
        cursor,
        target,
        TASK_STAGES.len(),
        |tx, stage, request| stage_page(tx, &TASK_STAGES[stage], request, condition),
    )
}

fn action_step(
    tx: &Transaction<'_>,
    cursor: &mut SweepCursor,
    target: &str,
) -> Result<(), ErasurePageError> {
    let condition = cursor.condition;
    bounded_step(
        tx,
        cursor,
        target,
        ACTION_STAGES.len(),
        |tx, stage, request| stage_page(tx, &ACTION_STAGES[stage], request, condition),
    )
}

fn inference_step(
    tx: &Transaction<'_>,
    cursor: &mut SweepCursor,
    target: &str,
) -> Result<(), ErasurePageError> {
    bounded_step(tx, cursor, target, INFERENCE_STAGES.len(), |_, _, _| {
        Ok(PageOutcome::default())
    })
}

#[must_use]
pub fn task_erasure_participant(store: Store) -> LocalErasureParticipant {
    LocalErasureParticipant::new(ParticipantOwnerRef::Task, task_step, store)
}

#[must_use]
pub fn action_erasure_participant(store: Store) -> LocalErasureParticipant {
    LocalErasureParticipant::new(ParticipantOwnerRef::Action, action_step, store)
}

#[must_use]
pub fn inference_erasure_participant(store: Store) -> LocalErasureParticipant {
    LocalErasureParticipant::new(ParticipantOwnerRef::Inference, inference_step, store)
}
