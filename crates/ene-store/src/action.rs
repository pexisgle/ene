//! `ActionAttemptRepository` over the `action_attempt` table group.
//!
//! The insert is AU5: one short `Immediate` transaction compares the
//! delegation correspondence, the relied task revision, the current task
//! revision, the current workspace association (exactly one, fail closed on
//! duplicates), and the delegation's copied scope association before writing
//! the attempt row. Missing rows and moved revisions/associations answer
//! `StalePremise` with zero writes; a premise that disagrees with the stored
//! delegation row, a duplicate association, an unknown stored operation, and
//! a duplicate attempt identity are technical errors and are never reduced to
//! stale.
//!
//! The certainty update is a per-row compare-and-set that accepts only
//! `expected = Unknown` and the closed-world `(certainty, grounds)` pairs. The
//! read composes a record only when the correlation and the certainty/grounds
//! pair are consistent; a malformed row is never guessed.

use std::path::Path;
use std::sync::Arc;

use ene_action::{
    ActionAttemptId, ActionAttemptRecord, ActionAttemptRepository, ActionCertainty,
    ActionStartOutcome, ActionTechnicalError, AttemptCommitPremise, CertaintyUpdateOutcome,
    EffectGrounds, OperationKind, RealTargetRef,
};
use ene_primitive::{RevisionInner, WallClockWithTz};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{decode_id, decode_u64, encode_id, encode_u64, lock_shared};
use crate::run_blocking;

const SQL_INSERT_ATTEMPT: &str = "INSERT INTO action_attempt (attempt_id, task_id, task_revision, delegation_id, workspace_assoc_id, real_target, operation, certainty, grounds, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)";

const SQL_SELECT_ATTEMPT: &str = "SELECT task_id, task_revision, delegation_id, workspace_assoc_id, real_target, operation, certainty, grounds, started_at FROM action_attempt WHERE attempt_id = ?1";

const SQL_SELECT_ATTEMPT_EXISTS: &str =
    "SELECT attempt_id FROM action_attempt WHERE attempt_id = ?1";

const SQL_SELECT_CERTAINTY: &str = "SELECT certainty FROM action_attempt WHERE attempt_id = ?1";

const SQL_UPDATE_CERTAINTY: &str = "UPDATE action_attempt SET certainty = ?2, grounds = ?3 WHERE attempt_id = ?1 AND certainty = ?4";

const SQL_SELECT_DELEGATION_PREMISE: &str =
    "SELECT task_id, task_revision, scope_assoc FROM delegation WHERE delegation_id = ?1";

const SQL_SELECT_TASK_REVISION_POINTER: &str = "SELECT revision FROM task WHERE task_id = ?1";

/// Probes two rows so a duplicate association is detected instead of being
/// silently reduced to the first one.
const SQL_SELECT_WORKSPACE_ASSOCS: &str =
    "SELECT assoc_id FROM workspace_assoc WHERE task_id = ?1 ORDER BY rowid LIMIT 2";

fn action_unavailable(reason: impl core::fmt::Display) -> ActionTechnicalError {
    ActionTechnicalError::StorageUnavailable {
        reason: reason.to_string(),
    }
}

fn decode_revision(raw: i64) -> Result<RevisionInner, ActionTechnicalError> {
    Ok(RevisionInner::from_u64(
        decode_u64(raw).map_err(action_unavailable)?,
    ))
}

fn insert_attempt_sync(
    conn: &std::sync::Mutex<rusqlite::Connection>,
    premise: AttemptCommitPremise,
) -> Result<ActionStartOutcome, ActionTechnicalError> {
    let attempt_text = encode_id(premise.attempt.as_raw());
    let task_text = encode_id(premise.task);
    let delegation_text = encode_id(premise.delegation);
    let workspace_text = encode_id(premise.workspace);
    let revision_raw = encode_u64(premise.task_revision.as_u64()).map_err(action_unavailable)?;
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(action_unavailable)?;
    // Attempt identities are single-use. An explicit pre-check answers a
    // duplicate deterministically; the primary-key constraint below only
    // covers a writer that committed between this read and the insert.
    let existing: Option<String> = tx
        .query_row(SQL_SELECT_ATTEMPT_EXISTS, params![attempt_text], |row| {
            row.get(0)
        })
        .optional()
        .map_err(action_unavailable)?;
    if existing.is_some() {
        return Err(action_unavailable("duplicate action attempt id"));
    }
    // (1) The delegation row exists and its relied revision equals the
    // premise. A disagreement is an inconsistent correlation, never a
    // fabricated stale; a missing row is a domain stale with zero writes.
    let stored: Option<(String, i64, Option<String>)> = tx
        .query_row(
            SQL_SELECT_DELEGATION_PREMISE,
            params![delegation_text],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(action_unavailable)?;
    let Some((delegation_task_text, delegation_revision_raw, scope_assoc_text)) = stored else {
        return Ok(ActionStartOutcome::StalePremise);
    };
    let delegation_task = decode_id(&delegation_task_text).map_err(action_unavailable)?;
    let delegation_revision = decode_u64(delegation_revision_raw).map_err(action_unavailable)?;
    if delegation_task != premise.task || delegation_revision != premise.task_revision.as_u64() {
        return Err(action_unavailable(
            "delegation correlation disagrees with the attempt premise",
        ));
    }
    // (2) The current Task row is still at the relied revision.
    let current: Option<i64> = tx
        .query_row(
            SQL_SELECT_TASK_REVISION_POINTER,
            params![task_text],
            |row| row.get(0),
        )
        .optional()
        .map_err(action_unavailable)?;
    let Some(current_raw) = current else {
        return Ok(ActionStartOutcome::StalePremise);
    };
    if decode_u64(current_raw).map_err(action_unavailable)? != premise.task_revision.as_u64() {
        return Ok(ActionStartOutcome::StalePremise);
    }
    // (3) Exactly one current workspace association exists, it is the premise
    // association, and the delegation's copied scope relied on the same
    // boundary. The copy is provenance: a mismatch means the delegation did
    // not witness the current boundary, so no widening is allowed.
    let associations = {
        let mut statement = tx
            .prepare(SQL_SELECT_WORKSPACE_ASSOCS)
            .map_err(action_unavailable)?;
        statement
            .query_map(params![task_text], |row| row.get::<_, String>(0))
            .map_err(action_unavailable)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(action_unavailable)?
    };
    if associations.len() > 1 {
        return Err(action_unavailable(
            "multiple workspace associations for the task",
        ));
    }
    let Some(current_assoc_text) = associations.first() else {
        return Ok(ActionStartOutcome::StalePremise);
    };
    let current_assoc = decode_id(current_assoc_text).map_err(action_unavailable)?;
    if current_assoc != premise.workspace {
        return Ok(ActionStartOutcome::StalePremise);
    }
    let Some(scope_assoc_text) = scope_assoc_text else {
        return Ok(ActionStartOutcome::StalePremise);
    };
    if decode_id(&scope_assoc_text).map_err(action_unavailable)? != current_assoc {
        return Ok(ActionStartOutcome::StalePremise);
    }
    let started_at = WallClockWithTz::now().to_rfc3339();
    match tx.execute(
        SQL_INSERT_ATTEMPT,
        params![
            attempt_text,
            task_text,
            revision_raw,
            delegation_text,
            workspace_text,
            premise.real_target.as_path(),
            premise.operation.as_str(),
            ActionCertainty::Unknown.as_str(),
            Option::<String>::None,
            started_at,
        ],
    ) {
        Ok(_) => {}
        Err(error)
            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) =>
        {
            // The explicit pre-check answered a duplicate; this only covers a
            // writer that committed between that read and this insert.
            return Err(action_unavailable("duplicate action attempt id"));
        }
        Err(error) => return Err(action_unavailable(error)),
    }
    tx.commit().map_err(action_unavailable)?;
    Ok(ActionStartOutcome::Started)
}

fn compare_and_set_sync(
    conn: &std::sync::Mutex<rusqlite::Connection>,
    attempt: ActionAttemptId,
    expected: ActionCertainty,
    new: ActionCertainty,
    grounds: EffectGrounds,
) -> Result<CertaintyUpdateOutcome, ActionTechnicalError> {
    // Only the started value may be compare-and-set, and only the closed-world
    // evidence pair may be written. A confirmed value is never rewritten.
    if expected != ActionCertainty::Unknown {
        return Err(action_unavailable(
            "certainty compare-and-set expected value must be unknown",
        ));
    }
    if !ene_action::certainty_grounds_pair_is_valid(new, grounds) {
        return Err(action_unavailable(
            "certainty update carries an invalid grounds pairing",
        ));
    }
    let attempt_text = encode_id(attempt.as_raw());
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(action_unavailable)?;
    let current: Option<String> = tx
        .query_row(SQL_SELECT_CERTAINTY, params![attempt_text], |row| {
            row.get(0)
        })
        .optional()
        .map_err(action_unavailable)?;
    let Some(current_text) = current else {
        return Ok(CertaintyUpdateOutcome::MissingAttempt);
    };
    let current = ActionCertainty::from_name(&current_text)
        .ok_or_else(|| action_unavailable("unknown stored action certainty"))?;
    if current != expected {
        return Ok(CertaintyUpdateOutcome::StaleCurrent { current });
    }
    let updated = tx
        .execute(
            SQL_UPDATE_CERTAINTY,
            params![
                attempt_text,
                new.as_str(),
                grounds.as_str(),
                expected.as_str()
            ],
        )
        .map_err(action_unavailable)?;
    if updated != 1 {
        return Err(action_unavailable(
            "attempt certainty update did not apply exactly once",
        ));
    }
    tx.commit().map_err(action_unavailable)?;
    Ok(CertaintyUpdateOutcome::Updated)
}

struct RawAttempt {
    task: String,
    task_revision: i64,
    delegation: String,
    workspace: String,
    real_target: String,
    operation: String,
    certainty: String,
    grounds: Option<String>,
    started_at: String,
}

fn raw_attempt_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawAttempt> {
    Ok(RawAttempt {
        task: row.get(0)?,
        task_revision: row.get(1)?,
        delegation: row.get(2)?,
        workspace: row.get(3)?,
        real_target: row.get(4)?,
        operation: row.get(5)?,
        certainty: row.get(6)?,
        grounds: row.get(7)?,
        started_at: row.get(8)?,
    })
}

/// True when the stored certainty and grounds form one allowed state.
///
/// The started attempt has no grounds; every reported pair must agree. A
/// confirmed certainty without its matching ground is corruption.
fn grounds_match(certainty: ActionCertainty, grounds: Option<EffectGrounds>) -> bool {
    matches!(
        (certainty, grounds),
        (ActionCertainty::Unknown, None)
            | (
                ActionCertainty::Unknown,
                Some(EffectGrounds::OutcomeUnverified)
            )
            | (
                ActionCertainty::ConfirmedSuccess,
                Some(EffectGrounds::ObservedAtTarget)
            )
            | (
                ActionCertainty::ConfirmedFailure,
                Some(EffectGrounds::RefusedBeforeEffect)
            )
    )
}

fn decode_attempt_record(
    attempt: ActionAttemptId,
    raw: RawAttempt,
) -> Result<ActionAttemptRecord, ActionTechnicalError> {
    let operation = OperationKind::from_name(&raw.operation)
        .ok_or_else(|| action_unavailable("unknown action operation in attempt row"))?;
    let certainty = ActionCertainty::from_name(&raw.certainty)
        .ok_or_else(|| action_unavailable("unknown action certainty in attempt row"))?;
    let grounds = match &raw.grounds {
        Some(text) => Some(
            EffectGrounds::from_name(text)
                .ok_or_else(|| action_unavailable("unknown action grounds in attempt row"))?,
        ),
        None => None,
    };
    if !grounds_match(certainty, grounds) {
        return Err(action_unavailable(
            "action certainty and grounds disagree in attempt row",
        ));
    }
    if raw.real_target.is_empty() || !Path::new(&raw.real_target).is_absolute() {
        return Err(action_unavailable(
            "action attempt real target is not a canonical absolute path",
        ));
    }
    Ok(ActionAttemptRecord {
        attempt,
        delegation: decode_id(&raw.delegation).map_err(action_unavailable)?,
        task: decode_id(&raw.task).map_err(action_unavailable)?,
        task_revision: decode_revision(raw.task_revision)?,
        workspace: decode_id(&raw.workspace).map_err(action_unavailable)?,
        real_target: RealTargetRef::from_canonical_path(raw.real_target),
        operation,
        certainty,
        grounds,
        started_at: WallClockWithTz::parse_rfc3339(&raw.started_at).map_err(action_unavailable)?,
    })
}

fn load_attempt_sync(
    conn: &std::sync::Mutex<rusqlite::Connection>,
    attempt: ActionAttemptId,
) -> Result<Option<ActionAttemptRecord>, ActionTechnicalError> {
    let guard = lock_shared(conn);
    let found: Option<RawAttempt> = guard
        .query_row(
            SQL_SELECT_ATTEMPT,
            params![encode_id(attempt.as_raw())],
            raw_attempt_row,
        )
        .optional()
        .map_err(action_unavailable)?;
    found
        .map(|raw| decode_attempt_record(attempt, raw))
        .transpose()
}

impl ActionAttemptRepository for Store {
    async fn insert_attempt_if_current(
        &self,
        premise: AttemptCommitPremise,
    ) -> Result<ActionStartOutcome, ActionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || insert_attempt_sync(&conn, premise)).await
    }

    async fn compare_and_set_certainty(
        &self,
        attempt: ActionAttemptId,
        expected: ActionCertainty,
        new: ActionCertainty,
        grounds: EffectGrounds,
    ) -> Result<CertaintyUpdateOutcome, ActionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || compare_and_set_sync(&conn, attempt, expected, new, grounds)).await
    }

    async fn load_attempt(
        &self,
        attempt: ActionAttemptId,
    ) -> Result<Option<ActionAttemptRecord>, ActionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_attempt_sync(&conn, attempt)).await
    }
}
