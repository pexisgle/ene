//! Canonical Group J admission and unfinished lifecycle. All mutations share
//! SQLite's Immediate writer boundary with AU14 and Task resume; no I/O or
//! participant erase runs under the transaction. Reads are SELECT-only and
//! bound their validation to the rows they actually return.

use std::sync::Arc;

use ene_preservation::*;
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{
    Store,
    codec::{decode_id, encode_id, lock_shared},
    run_blocking,
};

fn storage(_: rusqlite::Error) -> PreservationTechnicalError {
    PreservationTechnicalError::StorageUnavailable
}

fn corrupt() -> PreservationTechnicalError {
    PreservationTechnicalError::CorruptState
}

/// One bounded, keyset-paged candidate page for both owner queries.
///
/// Candidates are unfinished operations plus torn orphan conditions with no
/// operation row at all: the union keeps the page bounded while an orphan
/// condition can never be omitted from a query result as a silent
/// authoritative empty set — it fails closed through `validate`.
fn candidate_page(
    tx: &rusqlite::Transaction<'_>,
    after: &str,
    limit: u32,
) -> Result<Vec<String>, PreservationTechnicalError> {
    let mut statement = tx
        .prepare(
            "SELECT operation_id FROM deletion_operation WHERE phase!='completed' AND operation_id>?1
         UNION
         SELECT c.operation_id FROM erasure_condition c
             LEFT JOIN deletion_operation o ON o.operation_id=c.operation_id
             WHERE o.operation_id IS NULL AND c.operation_id>?1
         ORDER BY operation_id LIMIT ?2",
        )
        .map_err(storage)?;
    statement
        .query_map(params![after, limit], |r| r.get(0))
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)
}

/// Structural integrity of one operation's canonical rows, scoped to the
/// operation a query actually touches: an inner join must never silently hide
/// an orphan correlation, an unfinished operation whose condition was closed
/// early, or a completed operation that still holds protected material.
///
/// Source-correlation invariant (current-sweep canonical): an unfinished
/// operation keeps `erasure_condition_source` rows only in its current sweep
/// (`NextSweep` copies forward then deletes the old sweep atomically);
/// historical `erasure_condition` rows remain but carry no source rows; a
/// completed operation keeps zero source rows (the A5 completion boundary
/// must delete them — A1 exposes no completion authority — and any remaining
/// row fails closed). No second copy exists for audit/history.
/// Bounded by the touching query's page, never a whole-store scan.
fn validate(conn: &Connection, operation: &str) -> Result<(), PreservationTechnicalError> {
    let broken: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM deletion_operation o
         LEFT JOIN erasure_condition c ON c.operation_id=o.operation_id AND c.sweep=o.sweep
         WHERE o.operation_id=?1 AND (o.sweep <= 0 OR o.phase NOT IN ('active','held','finalizing','completed')
         OR o.purpose NOT IN ('privacy','security')
         OR (o.phase='held') != (o.hold_reason IS NOT NULL)
         OR (o.hold_reason IS NOT NULL AND o.hold_reason NOT IN ('unavailable','generation_exhausted'))
         OR c.operation_id IS NULL
         OR (o.phase != 'completed' AND c.closed_at IS NOT NULL)
         OR (o.phase = 'completed' AND c.closed_at IS NULL)
         OR (o.phase IN ('active','held') AND NOT EXISTS
             (SELECT 1 FROM deletion_search_material m WHERE m.operation_id=o.operation_id AND length(m.exact_text)>0))
         OR (o.phase='completed' AND (EXISTS
             (SELECT 1 FROM deletion_search_material m WHERE m.operation_id=o.operation_id) OR EXISTS
             (SELECT 1 FROM deletion_semantic_hint h WHERE h.operation_id=o.operation_id)))))
         OR EXISTS(SELECT 1 FROM erasure_condition c LEFT JOIN deletion_operation o
             ON o.operation_id=c.operation_id
             WHERE c.operation_id=?1 AND (o.operation_id IS NULL OR c.sweep<=0 OR c.sweep>o.sweep))
         OR EXISTS(SELECT 1 FROM erasure_condition_source s LEFT JOIN erasure_condition c
             ON c.operation_id=s.operation_id AND c.sweep=s.sweep
             WHERE s.operation_id=?1 AND (c.operation_id IS NULL OR s.sweep<=0))
         OR EXISTS(SELECT 1 FROM erasure_condition_source s JOIN deletion_operation o
             ON o.operation_id=s.operation_id
             WHERE s.operation_id=?1 AND o.phase!='completed' AND s.sweep!=o.sweep)
         OR EXISTS(SELECT 1 FROM erasure_condition_source s JOIN deletion_operation o
             ON o.operation_id=s.operation_id
             WHERE s.operation_id=?1 AND o.phase='completed')",
            [operation],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if broken {
        return Err(corrupt());
    }
    Ok(())
}

/// Bounded covering-candidate read for [`covering_condition`]: the single
/// current-sweep source correlation reachable through the existing
/// `idx_erasure_condition_source_source` index on `(source)`. Only an open
/// current condition of an unfinished operation qualifies, and unfinished
/// source rows exist only in the current sweep (old sweeps are deleted by
/// `NextSweep`, completed operations keep zero source rows), so completed
/// historical operations never match this join and the row count for one
/// source never grows with history. Exposed so tests can `EXPLAIN QUERY PLAN`
/// the exact production statement; boundedness itself is pinned by the
/// durable row invariant (source-row counts), not by the plan alone.
pub(crate) const COVERING_CANDIDATE_SQL: &str = "SELECT s.operation_id,c.sweep,c.opened_at
     FROM erasure_condition_source s
     JOIN erasure_condition c ON c.operation_id=s.operation_id AND c.sweep=s.sweep
     JOIN deletion_operation o ON o.operation_id=c.operation_id AND o.sweep=c.sweep
     WHERE s.source=?1 AND c.closed_at IS NULL AND o.phase!='completed' LIMIT 1";

/// Bounded torn-state probe for [`covering_condition`], scoped to the queried
/// source (`WHERE s.source=?1` in every branch, served by the same source
/// index). Each branch mirrors one way `validate` refuses torn current state
/// that the candidate join above would otherwise read as "not covering":
/// a source row with no parent condition, a source row with no operation row,
/// an unfinished operation whose current condition was closed early, a
/// completed operation with any source row still present, an unfinished
/// source row outside the operation's current sweep (old-sweep leftover that
/// was not inherited, or a future sweep), and an unfinished operation missing
/// its current condition row. Completed operations that follow the lifecycle
/// rules (closed current condition, protected material, hints, and all source
/// rows removed) match no branch. Every branch starts from the source index,
/// so the probe input is the source rows naming this source — never a scan
/// of historical operations.
pub(crate) const COVERING_TORN_PROBE_SQL: &str = "SELECT
     EXISTS(SELECT 1 FROM erasure_condition_source s
         LEFT JOIN erasure_condition c ON c.operation_id=s.operation_id AND c.sweep=s.sweep
         WHERE s.source=?1 AND c.operation_id IS NULL)
     OR EXISTS(SELECT 1 FROM erasure_condition_source s
         LEFT JOIN deletion_operation o ON o.operation_id=s.operation_id
         WHERE s.source=?1 AND o.operation_id IS NULL)
     OR EXISTS(SELECT 1 FROM erasure_condition_source s
         JOIN erasure_condition c ON c.operation_id=s.operation_id AND c.sweep=s.sweep
         JOIN deletion_operation o ON o.operation_id=c.operation_id AND o.sweep=c.sweep
         WHERE s.source=?1 AND o.phase!='completed' AND c.closed_at IS NOT NULL)
     OR EXISTS(SELECT 1 FROM erasure_condition_source s
         JOIN deletion_operation o ON o.operation_id=s.operation_id
         WHERE s.source=?1 AND o.phase='completed')
     OR EXISTS(SELECT 1 FROM erasure_condition_source s
         JOIN deletion_operation o ON o.operation_id=s.operation_id
         WHERE s.source=?1 AND o.phase!='completed' AND s.sweep!=o.sweep)
     OR EXISTS(SELECT 1 FROM erasure_condition_source s
         JOIN deletion_operation o ON o.operation_id=s.operation_id
         WHERE s.source=?1 AND o.phase!='completed'
         AND NOT EXISTS(SELECT 1 FROM erasure_condition c
             WHERE c.operation_id=o.operation_id AND c.sweep=o.sweep))";

/// One shared closure-aware coverage read for inference and Task resume.
/// Historical `erasure_condition` rows stay inside the operation interval as
/// lifecycle/history, but only the current sweep carries source rows and
/// participates in current coverage; a generation advance copies the current
/// sweep's cumulative sources forward and then deletes the old sweep's source
/// rows in the same transaction, never closing the interval.
///
/// Bounded by construction: SQL narrows to the single current-sweep
/// candidate through the source index before any Rust-side validation, so
/// completed historical operations (zero source rows by invariant) are never
/// enumerated or validated — validation runs only on the one candidate, if
/// any, and the per-operation check never walks other operations' history.
/// When there is no candidate, the torn-state probe above still fails
/// closed on torn current state relevant to this source instead of reading
/// it as "not covering". A genuine absence of coverage is the authoritative
/// empty set (`Ok(None)`): there is no second registry and no cached
/// NoDeletion sentinel.
pub(crate) fn covering_condition(
    conn: &Connection,
    source: &str,
) -> Result<Option<ErasureConditionRef>, PreservationTechnicalError> {
    let candidate: Option<(String, i64, String)> = conn
        .query_row(COVERING_CANDIDATE_SQL, [source], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()
        .map_err(storage)?;
    if let Some((id, sweep, opened)) = candidate {
        validate(conn, &id)?;
        parse_time(&opened)?;
        return Ok(Some(decode_ref(&id, sweep)?.condition()));
    }
    let torn: bool = conn
        .query_row(COVERING_TORN_PROBE_SQL, [source], |row| row.get(0))
        .map_err(storage)?;
    if torn {
        return Err(corrupt());
    }
    Ok(None)
}

fn decode_ref(id: &str, sweep: i64) -> Result<DeletionOperationRef, PreservationTechnicalError> {
    if sweep <= 0 {
        return Err(corrupt());
    }
    Ok(DeletionOperationRef {
        operation: DeletionOperationId::from_raw(decode_id(id).map_err(|_| corrupt())?),
        sweep: DeletionSweepGeneration::from_u64(sweep as u64),
    })
}

fn parse_time(raw: &str) -> Result<WallClockWithTz, PreservationTechnicalError> {
    WallClockWithTz::parse_rfc3339(raw).map_err(|_| corrupt())
}

type RawOperation = (String, i64, String, String, String, Option<String>);
fn raw_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawOperation> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}
fn decode_operation(
    raw: RawOperation,
) -> Result<DeletionOperationRecord, PreservationTechnicalError> {
    let phase = match raw.2.as_str() {
        "active" => DeletionOperationPhase::Active,
        "held" => DeletionOperationPhase::Held,
        "finalizing" => DeletionOperationPhase::Finalizing,
        "completed" => DeletionOperationPhase::Completed,
        _ => return Err(corrupt()),
    };
    let purpose = match raw.3.as_str() {
        "privacy" => DeletionPurpose::Privacy,
        "security" => DeletionPurpose::Security,
        _ => return Err(corrupt()),
    };
    let hold = match raw.5.as_deref() {
        None => None,
        Some("unavailable") => Some(DeletionHoldReason::Unavailable),
        Some("generation_exhausted") => Some(DeletionHoldReason::GenerationExhausted),
        _ => return Err(corrupt()),
    };
    Ok(DeletionOperationRecord {
        current: decode_ref(&raw.0, raw.1)?,
        phase,
        purpose,
        started_at: parse_time(&raw.4)?,
        hold,
    })
}

impl PreservationRepository for Store {
    async fn start_targeted_deletion(
        &self,
        command: StartTargetedDeletionCommand,
    ) -> Result<StartTargetedDeletionOutcome, PreservationTechnicalError> {
        let MechanicalDeletionTarget::ExactText(material) = &command.target().mechanical;
        if material.expose_for_erasure().trim().is_empty() {
            return Ok(StartTargetedDeletionOutcome::NeedsClarification);
        }
        if !command.is_confirmed() {
            return Ok(StartTargetedDeletionOutcome::ConfirmationRequired);
        }
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let MechanicalDeletionTarget::ExactText(material) = &command.target().mechanical;
            let existing: Option<(RawOperation, String)> = tx
                .query_row(
                    "SELECT o.operation_id,o.sweep,o.phase,o.purpose,o.started_at,o.hold_reason
                 FROM deletion_operation o JOIN deletion_search_material m ON m.operation_id=o.operation_id
                 WHERE o.phase!='completed' AND m.exact_text=?1 ORDER BY o.operation_id LIMIT 1",
                    [material.expose_for_erasure()],
                    |row| {
                        let id: String = row.get(0)?;
                        Ok((raw_operation(row)?, id))
                    },
                )
                .optional()
                .map_err(storage)?;
            if let Some((raw, existing_id)) = existing {
                validate(&tx, &existing_id)?;
                let record = decode_operation(raw)?;
                // Same mechanical target is an idempotent request only if its
                // known scope and exploration aids are already covered. Do not
                // silently widen a confirmed operation on a duplicate command.
                let mut covered = true;
                for source in command.known_sources() {
                    let found: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM erasure_condition_source WHERE operation_id=?1 AND sweep=?2 AND source=?3)",
                            params![
                                encode_id(record.current.operation.as_raw()),
                                record.current.sweep.as_u64() as i64,
                                encode_id(*source)
                            ],
                            |r| r.get(0),
                        )
                        .map_err(storage)?;
                    covered &= found;
                }
                for hint in &command.target().semantic_hints {
                    let found: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM deletion_semantic_hint WHERE operation_id=?1 AND material=?2)",
                            params![
                                encode_id(record.current.operation.as_raw()),
                                hint.expose_for_erasure()
                            ],
                            |r| r.get(0),
                        )
                        .map_err(storage)?;
                    covered &= found;
                }
                return Ok(if covered {
                    StartTargetedDeletionOutcome::AlreadyCoveredBy(record.current)
                } else {
                    StartTargetedDeletionOutcome::HeldByOperation(record.current)
                });
            }
            let current = DeletionOperationRef {
                operation: DeletionOperationId::from_raw(RawId::new()),
                sweep: DeletionSweepGeneration::from_u64(1),
            };
            let id = encode_id(current.operation.as_raw());
            let at = command.requested_at().to_rfc3339();
            let purpose = match command.purpose() {
                DeletionPurpose::Privacy => "privacy",
                DeletionPurpose::Security => "security",
            };
            tx.execute(
                "INSERT INTO deletion_operation (operation_id,sweep,phase,purpose,started_at) VALUES (?1,1,'active',?2,?3)",
                params![id, purpose, at],
            )
            .map_err(storage)?;
            tx.execute(
                "INSERT INTO deletion_search_material (operation_id,exact_text) VALUES (?1,?2)",
                params![id, material.expose_for_erasure()],
            )
            .map_err(storage)?;
            for (ordinal, hint) in command.target().semantic_hints.iter().enumerate() {
                let ordinal = i64::try_from(ordinal).map_err(|_| corrupt())?;
                tx.execute(
                    "INSERT INTO deletion_semantic_hint (operation_id,ordinal,material) VALUES (?1,?2,?3)",
                    params![id, ordinal, hint.expose_for_erasure()],
                )
                .map_err(storage)?;
            }
            tx.execute(
                "INSERT INTO erasure_condition (operation_id,sweep,opened_at) VALUES (?1,1,?2)",
                params![id, at],
            )
            .map_err(storage)?;
            for source in command.known_sources() {
                tx.execute(
                    "INSERT OR IGNORE INTO erasure_condition_source (operation_id,sweep,source) VALUES (?1,1,?2)",
                    params![id, encode_id(*source)],
                )
                .map_err(storage)?;
            }
            // The admission commit may not publish a structurally impossible
            // operation; the post-insert check keeps the producer honest even
            // against future code that assembles rows differently.
            validate(&tx, &id)?;
            tx.commit().map_err(storage)?;
            Ok(StartTargetedDeletionOutcome::Started(current))
        })
        .await
    }

    async fn change_deletion_lifecycle(
        &self,
        expected: DeletionOperationRef,
        change: DeletionLifecycleChange,
    ) -> Result<DeletionLifecycleOutcome, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(expected.operation.as_raw());
            validate(&tx, &id)?;
            let raw = tx
                .query_row(
                    "SELECT operation_id,sweep,phase,purpose,started_at,hold_reason FROM deletion_operation WHERE operation_id=?1",
                    [&id],
                    raw_operation,
                )
                .optional()
                .map_err(storage)?;
            let Some(raw) = raw else {
                return Ok(DeletionLifecycleOutcome::Missing);
            };
            let record = decode_operation(raw)?;
            if record.current != expected {
                return Ok(DeletionLifecycleOutcome::StaleSweep);
            }
            if record.phase == DeletionOperationPhase::Completed {
                return Ok(DeletionLifecycleOutcome::Completed);
            }
            if record.hold == Some(DeletionHoldReason::GenerationExhausted) {
                return Ok(DeletionLifecycleOutcome::Held(
                    DeletionHoldReason::GenerationExhausted,
                ));
            }
            let mut current = expected;
            match change {
                DeletionLifecycleChange::Hold => {
                    // Do not erase finalizing markers/authority with a generic hold.
                    if record.phase == DeletionOperationPhase::Finalizing {
                        return Ok(DeletionLifecycleOutcome::Finalizing);
                    }
                    tx.execute(
                        "UPDATE deletion_operation SET phase='held',hold_reason='unavailable' WHERE operation_id=?1",
                        [&id],
                    )
                    .map_err(storage)?;
                }
                DeletionLifecycleChange::Resume => {
                    if record.phase == DeletionOperationPhase::Finalizing {
                        return Ok(DeletionLifecycleOutcome::Finalizing);
                    }
                    tx.execute(
                        "UPDATE deletion_operation SET phase='active',hold_reason=NULL WHERE operation_id=?1",
                        [&id],
                    )
                    .map_err(storage)?;
                }
                DeletionLifecycleChange::NextSweep => {
                    // A Held(Unavailable) operation keeps its current condition
                    // active (§5.1) until an explicit Resume decides recovery:
                    // advancing the generation underneath the hold would publish
                    // a new current condition without a recovery decision and
                    // silently flip the phase that A4/A5 participant sweep
                    // tracking observes. Reject like the GenerationExhausted
                    // hold below — no writes, no generation advance — so an
                    // explicit Resume stays the only path back to Active.
                    if record.phase == DeletionOperationPhase::Held
                        && record.hold == Some(DeletionHoldReason::Unavailable)
                    {
                        return Ok(DeletionLifecycleOutcome::Held(
                            DeletionHoldReason::Unavailable,
                        ));
                    }
                    let sweep =
                        i64::try_from(expected.sweep.as_u64()).map_err(|_| corrupt())?;
                    let Some(next) = sweep.checked_add(1) else {
                        tx.execute(
                            "UPDATE deletion_operation SET phase='held',hold_reason='generation_exhausted' WHERE operation_id=?1",
                            [&id],
                        )
                        .map_err(storage)?;
                        tx.commit().map_err(storage)?;
                        return Ok(DeletionLifecycleOutcome::Held(
                            DeletionHoldReason::GenerationExhausted,
                        ));
                    };
                    // A finalizing operation whose material has already been
                    // destroyed cannot be restarted by a generic lifecycle call.
                    let material: bool = tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM deletion_search_material WHERE operation_id=?1)",
                            [&id],
                            |r| r.get(0),
                        )
                        .map_err(storage)?;
                    if !material {
                        return Ok(DeletionLifecycleOutcome::Finalizing);
                    }
                    tx.execute(
                        "INSERT INTO erasure_condition (operation_id,sweep,opened_at) VALUES (?1,?2,?3)",
                        params![id, next, WallClockWithTz::now().to_rfc3339()],
                    )
                    .map_err(storage)?;
                    // Cumulative source coverage is inherited by copying the
                    // current sweep forward; the old sweep's source rows are
                    // then deleted in the same transaction so an unfinished
                    // operation keeps source correlations only in its current
                    // sweep. Historical erasure_condition rows remain as
                    // lifecycle/history. The copy-then-delete order with a
                    // single commit keeps a crash from publishing a current
                    // sweep that silently drops coverage.
                    tx.execute(
                        "INSERT INTO erasure_condition_source (operation_id,sweep,source) SELECT operation_id,?2,source FROM erasure_condition_source WHERE operation_id=?1 AND sweep=?3",
                        params![id, next, sweep],
                    )
                    .map_err(storage)?;
                    tx.execute(
                        "DELETE FROM erasure_condition_source WHERE operation_id=?1 AND sweep=?2",
                        params![id, sweep],
                    )
                    .map_err(storage)?;
                    tx.execute(
                        "UPDATE deletion_operation SET sweep=?2,phase='active',hold_reason=NULL WHERE operation_id=?1",
                        params![id, next],
                    )
                    .map_err(storage)?;
                    current.sweep = DeletionSweepGeneration::from_u64(next as u64);
                }
            }
            validate(&tx, &id)?;
            tx.commit().map_err(storage)?;
            Ok(DeletionLifecycleOutcome::Applied(current))
        })
        .await
    }

    async fn unfinished_deletions(
        &self,
        after: Option<DeletionOperationId>,
        limit: u32,
    ) -> Result<Vec<DeletionOperationRecord>, PreservationTechnicalError> {
        if !(1..=100).contains(&limit) {
            return Err(PreservationTechnicalError::InvalidLimit);
        }
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            // A read transaction gives validation and the bounded page the same snapshot.
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let after = after.map(|id| encode_id(id.as_raw())).unwrap_or_default();
            let ids = candidate_page(&tx, &after, limit)?;
            ids.into_iter()
                .map(|id| {
                    validate(&tx, &id)?;
                    let raw = tx
                        .query_row(
                            "SELECT operation_id,sweep,phase,purpose,started_at,hold_reason FROM deletion_operation WHERE operation_id=?1",
                            [&id],
                            raw_operation,
                        )
                        .optional()
                        .map_err(storage)?
                        // An orphan condition candidate has no operation row:
                        // that is exactly the torn state `validate` refuses.
                        .ok_or_else(corrupt)?;
                    decode_operation(raw)
                })
                .collect()
        })
        .await
    }

    async fn current_erasure_conditions(
        &self,
        after: Option<DeletionOperationId>,
        limit: u32,
    ) -> Result<Vec<CurrentErasureCondition>, PreservationTechnicalError> {
        if !(1..=100).contains(&limit) {
            return Err(PreservationTechnicalError::InvalidLimit);
        }
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let after = after.map(|id| encode_id(id.as_raw())).unwrap_or_default();
            let ids = candidate_page(&tx, &after, limit)?;
            // Paging over unfinished operations (not over open conditions)
            // keeps validation total for the page: an unfinished operation
            // whose current condition was closed early never silently drops
            // out of the result; it fails closed through `validate`.
            ids.iter()
                .map(|id| {
                    validate(&tx, id)?;
                    // Only the current sweep is the current condition (§7):
                    // the operation's stated sweep joins the condition, so a
                    // historical sweep row can never be returned after a
                    // generation advance.
                    let (sweep, opened): (i64, String) = tx
                        .query_row(
                            "SELECT c.sweep,c.opened_at FROM erasure_condition c
                         JOIN deletion_operation o ON o.operation_id=c.operation_id AND o.sweep=c.sweep
                         WHERE c.operation_id=?1 AND c.closed_at IS NULL AND o.phase!='completed'",
                            [id],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .optional()
                        .map_err(storage)?
                        .ok_or_else(corrupt)?;
                    let current = decode_ref(id, sweep)?;
                    Ok(CurrentErasureCondition {
                        condition: current.condition(),
                        scope: current.operation,
                        opened_at: parse_time(&opened)?,
                    })
                })
                .collect()
        })
        .await
    }
}
