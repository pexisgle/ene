//! Owner-local Targeted Deletion participants over this store's durable rows.
//!
//! `ene-preservation` owns the cross-cutting [`ErasureParticipant`] contract
//! and never depends on a concrete participant crate (lifecycle §9). The
//! Task, Action, and Inference owners keep their durable master in this
//! crate's single SQLite writer, so their bounded local-erasure
//! implementations live beside those tables, and the Host composition
//! (`apps/ene-core`) registers them with
//! `HostHandle::register_deletion_participant`.
//!
//! Every implementation is a mechanical, LLM-independent sweep over the
//! owner's body-bearing columns:
//!
//! * the exact target text is compared to every stored value (SQLite `instr`,
//!   no pattern grammar and no semantic matching);
//! * a match is redacted in place with a fixed marker, never by deleting the
//!   owner row, so the objective facts of the row (identity, revision,
//!   progress, adoption, certainty, grounds, ticket, provider, model, usage)
//!   stay exactly as committed;
//! * the work is bounded per demand by `ERASURE_SCAN_ROWS`; the fan-out
//!   re-demands until the participant reports `Verified`, and a crash or a
//!   restart re-drives the same sweep idempotently because a redaction is a
//!   no-op on an already-clean value;
//! * `Verified` is a second, complete pass over the same columns that found
//!   zero occurrences — never inferred from the erase pass alone.
//!
//! The reported `erased_count` / `remainder_count` describe the bounded work
//! this run observed; they are progress metadata, never the completion
//! decision (the durable `Verified` state plus the remainder pass is), and a
//! resumed run counts only what it redacts itself.
//!
//! The participant never decides deletion semantics: it receives the
//! protected exact-text material with the condition identity and reports the
//! fact. Covered source-correlation identities are not a second deletion
//! registry here: Task result collection of observation-derived paraphrases
//! reuses the durable `erasure_use_hold` association already published at
//! admission. Other owner rows still use the exact text as the mechanical
//! handle — and the same text is what the bounded remainder pass verifies.
//! A stale condition can only produce a fact the canonical store refuses
//! (`StaleSweep`), so an older generation never advances the current sweep;
//! a demand without protected material or with a target the owner cannot
//! represent is an explicit hold, never a silent success.
//!
//! Owner surfaces:
//!
//! * Task: the in-force purpose text, every revision snapshot purpose, the
//!   recorded result body, the observation occurrence path correlation, and
//!   the internal workspace / delegation scope path copies. A result body
//!   that does not contain the exact target is still collected when its
//!   owning delegation is associated with the demanded operation
//!   (`erasure_use_hold`): that hold is the observation→delegation
//!   correspondence published at admission for a discarded body-observed
//!   source, so a paraphrase of already-started work cannot survive as
//!   undeleted derived personal data. Correlation columns
//!   (`task_context_entry`, the observation identities, the delegation
//!   and association identities) are retained: they are body-free identities,
//!   not copies.
//! * Action: the attempt's resolved target path. Certainty and grounds are
//!   never rewritten; an already-observed external effect stays a fact, and
//!   only the stored target text is redacted.
//! * Inference: verification only. The current inference-owned surface
//!   (attempts, ordered `data_use` correlation, usage facts) stores no copy
//!   of a logical input or output body, so there is no body column to erase;
//!   the participant proves the absence over that empty set and never
//!   rewrites ticket, provider, model, usage, or pricing facts.

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

/// The Task owner's `(table, content column)` surface, in stage order. The
/// system-wide remainder probe derives its Task surface from these stages, so
/// a stage column cannot be swept without being probed.
pub(crate) fn task_content_surface() -> impl Iterator<Item = (&'static str, &'static str)> {
    TASK_STAGES.iter().flat_map(|stage| {
        stage
            .columns
            .iter()
            .map(|column| (stage.table, column.name))
    })
}

/// The Action owner's `(table, content column)` surface, in stage order. The
/// system-wide remainder probe derives its Action surface from these stages.
pub(crate) fn action_content_surface() -> impl Iterator<Item = (&'static str, &'static str)> {
    ACTION_STAGES.iter().flat_map(|stage| {
        stage
            .columns
            .iter()
            .map(|column| (stage.table, column.name))
    })
}

/// Redacted value planned for one stored column.
struct ValueRedaction {
    column: &'static str,
    value: String,
    removed: u64,
}

/// One stored row of one stage: its rowid cursor plus the stage's columns in
/// order. `None` is a stored NULL.
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

/// One page of one stage inside the demand's `Immediate` transaction: reads
/// the stage's rows from the rowid keyset, plans each row's redactions, and
/// applies them when the walk is erasing. A verification page only counts the
/// rows that still need redaction: the shared walk restarts the erase pass
/// from the head of that stage when it sees a match, so a verification page
/// never rewrites.
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

/// Whether one `task_result` row's owning delegation is associated with the
/// demanded operation (`erasure_use_hold`, `task_delegation`).
///
/// That hold is the observation→delegation correspondence published at
/// admission: a discarded body-observed source cannot be proven unrelated to
/// the target, so the already-stored result body is collected even when
/// mechanical search of the paraphrase misses. The execution seal, adoption
/// pointer, and Action certainty are facts and are not consulted here.
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

/// Plans the redactions of one row. The whole demand is applied inside one
/// `Immediate` transaction; an unrepresentable value returns before the
/// commit, so the demand rolls back atomically rather than committing
/// half-swept.
///
/// `provenance_linked` is the observation-hold path for `task_result`: the
/// whole body is replaced by the body-free marker because a paraphrase of a
/// discarded observation cannot be proven unrelated to the target by
/// mechanical search. An already-collected marker is left untouched only when
/// the marker itself is target-free; a target that overlaps the marker is
/// redacted out of it so the verify pass stays a clean pass. The execution
/// seal itself is the row's existence and is never rewritten.
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
            // The fixed marker is target-free only when the target is not a
            // substring of it. A target that overlaps the marker must go
            // through the same mechanical predicate as every other value
            // instead of being certified clean by identity.
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

/// Redacts one stored filesystem locator, keeping it a readable canonical
/// absolute path. A redaction that would break that shape (the match consumed
/// the path root) is replaced by the fixed [`ERASED_LOCATOR`] marker; if even
/// the marker contains the target, the value is unrepresentable and the sweep
/// fails closed.
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

/// The Inference owner has no body-bearing column, so the shared walk reaches
/// `Verified` over the empty stage list after the currentness check.
fn inference_step(
    tx: &Transaction<'_>,
    cursor: &mut SweepCursor,
    target: &str,
) -> Result<(), ErasurePageError> {
    bounded_step(tx, cursor, target, INFERENCE_STAGES.len(), |_, _, _| {
        Ok(PageOutcome::default())
    })
}

/// The Task owner's local-erasure registration.
#[must_use]
pub fn task_erasure_participant(store: Store) -> LocalErasureParticipant {
    LocalErasureParticipant::new(ParticipantOwnerRef::Task, task_step, store)
}

/// The Action owner's local-erasure registration.
#[must_use]
pub fn action_erasure_participant(store: Store) -> LocalErasureParticipant {
    LocalErasureParticipant::new(ParticipantOwnerRef::Action, action_step, store)
}

/// The Inference owner's verification registration.
#[must_use]
pub fn inference_erasure_participant(store: Store) -> LocalErasureParticipant {
    LocalErasureParticipant::new(ParticipantOwnerRef::Inference, inference_step, store)
}
