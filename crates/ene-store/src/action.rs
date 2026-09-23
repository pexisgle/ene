use std::path::Path;
use std::sync::Arc;

use ene_action::{
    ActionAttemptId, ActionAttemptRecord, ActionAttemptRepository, ActionCertainty,
    ActionStartOutcome, ActionTechnicalError, AttemptCommitPremise, CertaintyUpdateOutcome,
    EffectGrounds, OperationKind, RealTargetRef,
};
use ene_companion::{ActionCertaintyWire, TaskFact, UndeliveredSource};
use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::Store;
use crate::codec::{decode_id, decode_u64, encode_id, encode_u64, lock_shared};
use crate::run_blocking;

const SQL_INSERT_ATTEMPT: &str = "INSERT INTO action_attempt (attempt_id, task_id, task_revision, delegation_id, workspace_assoc_id, real_target, operation, relied_evaluation, certainty, grounds, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";

const SQL_SELECT_ATTEMPT: &str = "SELECT task_id, task_revision, delegation_id, workspace_assoc_id, real_target, operation, relied_evaluation, certainty, grounds, started_at FROM action_attempt WHERE attempt_id = ?1";

const SQL_SELECT_ATTEMPT_EXISTS: &str =
    "SELECT attempt_id FROM action_attempt WHERE attempt_id = ?1";

const SQL_SELECT_EVALUATION_EXISTS: &str =
    "SELECT attempt_id FROM action_attempt WHERE relied_evaluation = ?1";

const SQL_SELECT_CERTAINTY: &str =
    "SELECT certainty, task_id FROM action_attempt WHERE attempt_id = ?1";

const SQL_UPDATE_CERTAINTY: &str = "UPDATE action_attempt SET certainty = ?2, grounds = ?3 WHERE attempt_id = ?1 AND certainty = ?4";

const SQL_SELECT_TASK_ASSIGNEE: &str = "SELECT assignee FROM task WHERE task_id = ?1";

const SQL_SELECT_DELEGATION_PREMISE: &str =
    "SELECT task_id, task_revision, scope_assoc FROM delegation WHERE delegation_id = ?1";

const SQL_SELECT_TASK_STATE: &str = "SELECT revision, progress FROM task WHERE task_id = ?1";

const SQL_SELECT_DELEGATION_RESULT: &str =
    "SELECT result_id FROM task_result WHERE delegation_id = ?1";

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

fn register_action_attempt(
    tx: &rusqlite::Transaction<'_>,
    task_text: &str,
    attempt: RawId,
    certainty: ActionCertaintyWire,
) -> Result<(), ActionTechnicalError> {
    let assignee: Option<String> = tx
        .query_row(SQL_SELECT_TASK_ASSIGNEE, params![task_text], |row| {
            row.get(0)
        })
        .optional()
        .map_err(action_unavailable)?;
    let Some(assignee) = assignee else {
        return Err(action_unavailable(
            "task row missing for action attempt notification",
        ));
    };
    let task = decode_id(task_text).map_err(action_unavailable)?;
    let source = UndeliveredSource::TaskRecord {
        task,
        fact: TaskFact::ActionAttempt { attempt, certainty },
    };
    crate::companion::register_undelivered_tx(
        tx,
        &assignee,
        RawId::new(),
        &source,
        None,
        None,
        WallClockWithTz::now(),
    )
    .map_err(action_unavailable)
}

fn certainty_wire(certainty: ActionCertainty) -> ActionCertaintyWire {
    match certainty {
        ActionCertainty::ConfirmedSuccess => ActionCertaintyWire::ConfirmedSuccess,
        ActionCertainty::ConfirmedFailure => ActionCertaintyWire::ConfirmedFailure,
        ActionCertainty::Unknown => ActionCertaintyWire::Unknown,
    }
}

fn insert_attempt_sync(
    conn: &std::sync::Mutex<rusqlite::Connection>,
    premise: AttemptCommitPremise,
) -> Result<ActionStartOutcome, ActionTechnicalError> {
    let attempt_text = encode_id(premise.attempt.as_raw());
    let task_text = encode_id(premise.task);
    let delegation_text = encode_id(premise.delegation);
    let workspace_text = encode_id(premise.workspace);
    let evaluation_text = encode_id(premise.relied_evaluation);
    let revision_raw = encode_u64(premise.task_revision.as_u64()).map_err(action_unavailable)?;
    let mut guard = lock_shared(conn);
    let tx = guard
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(action_unavailable)?;
    let existing: Option<String> = tx
        .query_row(SQL_SELECT_ATTEMPT_EXISTS, params![attempt_text], |row| {
            row.get(0)
        })
        .optional()
        .map_err(action_unavailable)?;
    if existing.is_some() {
        return Err(action_unavailable("duplicate action attempt id"));
    }
    let used_evaluation: Option<String> = tx
        .query_row(
            SQL_SELECT_EVALUATION_EXISTS,
            params![evaluation_text],
            |row| row.get(0),
        )
        .optional()
        .map_err(action_unavailable)?;
    if used_evaluation.is_some() {
        return Err(action_unavailable("action evaluation already used"));
    }
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
    let current: Option<(i64, Option<String>)> = tx
        .query_row(SQL_SELECT_TASK_STATE, params![task_text], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .map_err(action_unavailable)?;
    let Some((current_raw, progress_raw)) = current else {
        return Ok(ActionStartOutcome::StalePremise);
    };
    if decode_u64(current_raw).map_err(action_unavailable)? != premise.task_revision.as_u64() {
        return Ok(ActionStartOutcome::StalePremise);
    }
    let progress_text =
        progress_raw.ok_or_else(|| action_unavailable("task progress is missing"))?;
    let progress = ene_task::TaskProgress::from_name(&progress_text)
        .ok_or_else(|| action_unavailable("unknown task progress"))?;
    if progress.is_terminal() {
        return Ok(ActionStartOutcome::TaskTerminal);
    }
    let sealed: Option<String> = tx
        .query_row(
            SQL_SELECT_DELEGATION_RESULT,
            params![delegation_text],
            |row| row.get(0),
        )
        .optional()
        .map_err(action_unavailable)?;
    if sealed.is_some() {
        return Ok(ActionStartOutcome::ExecutionSealed);
    }
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
    if crate::preservation::covering_text(&tx, premise.real_target.as_path())
        .map_err(|error| action_unavailable(error.to_string()))?
        .is_some()
        || crate::preservation::held_use(
            &tx,
            crate::preservation::USE_KIND_TASK_DELEGATION,
            premise.delegation,
        )
        .map_err(|error| action_unavailable(error.to_string()))?
    {
        tx.commit().map_err(action_unavailable)?;
        return Ok(ActionStartOutcome::HeldForErasure);
    }
    tx.execute(
        SQL_INSERT_ATTEMPT,
        params![
            attempt_text,
            task_text,
            revision_raw,
            delegation_text,
            workspace_text,
            premise.real_target.as_path(),
            premise.operation.as_str(),
            evaluation_text,
            ActionCertainty::Unknown.as_str(),
            Option::<String>::None,
            started_at,
        ],
    )
    .map_err(action_unavailable)?;
    register_action_attempt(
        &tx,
        &task_text,
        premise.attempt.as_raw(),
        ActionCertaintyWire::Unknown,
    )?;
    if matches!(premise.operation, OperationKind::Read | OperationKind::List) {
        crate::preservation::hold_body_observing_delegation(&tx, premise.delegation, &started_at)
            .map_err(|error| action_unavailable(error.to_string()))?;
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
    let current: Option<(String, String)> = tx
        .query_row(SQL_SELECT_CERTAINTY, params![attempt_text], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()
        .map_err(action_unavailable)?;
    let Some((current_text, task_text)) = current else {
        return Ok(CertaintyUpdateOutcome::MissingAttempt);
    };
    let current = ActionCertainty::from_name(&current_text)
        .ok_or_else(|| action_unavailable("unknown stored action certainty"))?;
    if current != expected {
        return Ok(CertaintyUpdateOutcome::StaleCurrent { current });
    }
    tx.execute(
        SQL_UPDATE_CERTAINTY,
        params![
            attempt_text,
            new.as_str(),
            grounds.as_str(),
            expected.as_str()
        ],
    )
    .map_err(action_unavailable)?;
    register_action_attempt(&tx, &task_text, attempt.as_raw(), certainty_wire(new))?;
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
    relied_evaluation: String,
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
        relied_evaluation: row.get(6)?,
        certainty: row.get(7)?,
        grounds: row.get(8)?,
        started_at: row.get(9)?,
    })
}

fn grounds_match(certainty: ActionCertainty, grounds: Option<EffectGrounds>) -> bool {
    match grounds {
        None => certainty == ActionCertainty::Unknown,
        Some(grounds) => ene_action::certainty_grounds_pair_is_valid(certainty, grounds),
    }
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
        relied_evaluation: decode_id(&raw.relied_evaluation).map_err(action_unavailable)?,
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
        self.hint_after_commit(run_blocking(move || insert_attempt_sync(&conn, premise)).await)
    }

    async fn compare_and_set_certainty(
        &self,
        attempt: ActionAttemptId,
        expected: ActionCertainty,
        new: ActionCertainty,
        grounds: EffectGrounds,
    ) -> Result<CertaintyUpdateOutcome, ActionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        self.hint_after_commit(
            run_blocking(move || compare_and_set_sync(&conn, attempt, expected, new, grounds))
                .await,
        )
    }

    async fn load_attempt(
        &self,
        attempt: ActionAttemptId,
    ) -> Result<Option<ActionAttemptRecord>, ActionTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || load_attempt_sync(&conn, attempt)).await
    }
}
