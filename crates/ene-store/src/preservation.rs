//! Canonical Group J admission and unfinished lifecycle. All mutations share
//! SQLite's Immediate writer boundary with AU14 and Task resume; no I/O or
//! participant erase runs under the transaction. The `PreservationRepository`
//! read methods are SELECT-only and bound their validation to the rows they
//! actually return; the Store coverage predicates fill the durable
//! `erasure_use_hold` correspondence through `held_use` and are therefore not
//! side-effect free.

use std::sync::Arc;

use ene_preservation::*;
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, params_from_iter};

use crate::{
    Store,
    codec::{
        decode_id, encode_id, lock_shared, preservation_corrupt as corrupt,
        preservation_storage as storage,
    },
    run_blocking, run_deletion_blocking,
};

pub(crate) fn check_page_limit(limit: u32) -> Result<(), PreservationTechnicalError> {
    if !(1..=100).contains(&limit) {
        return Err(PreservationTechnicalError::InvalidLimit);
    }
    Ok(())
}

/// Whether `condition` is the operation's current, unfinished condition
/// (lifecycle §6-§7).
///
/// A participant-local erasure pass must run this check inside the same
/// transaction as its mutation: a sweep the operation has moved past or a
/// completed operation must never erase local state. Completion ends the
/// text's meaning as a deletion target (§7: a completed operation is not a
/// permanent keyword ban), so a late duplicate from a superseded run must be
/// refused instead of erasing text the Owner provided afterwards. A condition
/// that cannot be encoded, or that has no matching operation row, is not
/// current.
pub(crate) fn condition_is_current(
    conn: &Connection,
    condition: ErasureConditionRef,
) -> rusqlite::Result<bool> {
    let Ok(sweep) = i64::try_from(condition.sweep.as_u64()) else {
        return Ok(false);
    };
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM deletion_operation
         WHERE operation_id=?1 AND sweep=?2 AND phase!='completed')",
        params![encode_id(condition.operation.as_raw()), sweep],
        |row| row.get(0),
    )
}

pub(crate) fn source_is_covered(
    conn: &Connection,
    condition: ErasureConditionRef,
    source: RawId,
) -> Result<bool, PreservationTechnicalError> {
    let sweep = i64::try_from(condition.sweep.as_u64()).map_err(|_| corrupt())?;
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM erasure_condition_source
         WHERE operation_id=?1 AND sweep=?2 AND source=?3)",
        params![
            encode_id(condition.operation.as_raw()),
            sweep,
            encode_id(source)
        ],
        |row| row.get(0),
    )
    .map_err(storage)
}

fn candidate_page(
    tx: &rusqlite::Transaction<'_>,
    after: &str,
    limit: u32,
    include_completed: bool,
) -> Result<Vec<String>, PreservationTechnicalError> {
    let sql = if include_completed {
        "SELECT operation_id FROM deletion_operation WHERE operation_id>?1
         UNION
         SELECT c.operation_id FROM erasure_condition c
             LEFT JOIN deletion_operation o ON o.operation_id=c.operation_id
             WHERE o.operation_id IS NULL AND c.operation_id>?1
         ORDER BY operation_id LIMIT ?2"
    } else {
        "SELECT operation_id FROM deletion_operation WHERE phase!='completed' AND operation_id>?1
         UNION
         SELECT c.operation_id FROM erasure_condition c
             LEFT JOIN deletion_operation o ON o.operation_id=c.operation_id
             WHERE o.operation_id IS NULL AND c.operation_id>?1
         ORDER BY operation_id LIMIT ?2"
    };
    let mut statement = tx.prepare(sql).map_err(storage)?;
    statement
        .query_map(params![after, limit], |r| r.get(0))
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)
}

/// The durable reconciliation cursor rows for one operation, as
/// `(identity_table, sweep, complete)`.
fn reconciliation_rows(
    conn: &Connection,
    operation: &str,
) -> Result<Vec<(String, i64, i64)>, PreservationTechnicalError> {
    let mut statement = conn
        .prepare(
            "SELECT identity_table,sweep,complete FROM deletion_reconciliation
             WHERE operation_id=?1",
        )
        .map_err(storage)?;
    statement
        .query_map([operation], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)
}

/// Structural integrity of one operation's reconciliation cursor rows,
/// scoped to the operation a query actually touches.
///
/// Invariant: an unfinished operation carries exactly one row per known
/// identity table for its current sweep; a `Finalizing` operation has every
/// row complete (the completion premise was re-read before the marker was
/// taken); a completed operation keeps zero rows. Any other shape — a missing
/// table, a foreign sweep, an unknown table name, or leftover rows after
/// completion — is torn canonical state that fails closed, never a silently
/// incomplete walk. Rows for an operation that does not exist are torn state
/// for the same reason a condition without its operation is.
fn validate_reconciliation(
    conn: &Connection,
    operation: &str,
) -> Result<(), PreservationTechnicalError> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT phase,sweep FROM deletion_operation WHERE operation_id=?1",
            [operation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    let rows = reconciliation_rows(conn, operation)?;
    let Some((phase, sweep)) = row else {
        return if rows.is_empty() {
            Ok(())
        } else {
            Err(corrupt())
        };
    };
    if phase == "completed" {
        return if rows.is_empty() {
            Ok(())
        } else {
            Err(corrupt())
        };
    }
    if rows.len() != KNOWN_SOURCE_IDENTITIES.len() {
        return Err(corrupt());
    }
    for (table, row_sweep, complete) in &rows {
        if *row_sweep != sweep || !(0..=1).contains(complete) {
            return Err(corrupt());
        }
        if !KNOWN_SOURCE_IDENTITIES
            .iter()
            .any(|identity| identity.table == table)
        {
            return Err(corrupt());
        }
        if phase == "finalizing" && *complete != 1 {
            return Err(corrupt());
        }
    }
    Ok(())
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
///
/// Participant invariant: every operation carries a non-empty required
/// participant snapshot from admission; every row tracks the operation's
/// current sweep (`NextSweep` resets all progress to pending atomically); a
/// `Finalizing` operation has every row `verified` (verification is terminal
/// for its sweep) and a completed operation has every row `verified` for that
/// sweep. A participant row for an unknown owner, a foreign sweep, or a
/// missing snapshot is torn canonical state and fails closed, never a silently
/// incomplete set.
///
/// Completion invariant: the audit row exists exactly when the operation is
/// `completed`, matches the operation's purpose, start time, and final sweep,
/// names every required participant exactly once with `verified` as its final
/// status and the participant row's erased count, and a completed request's
/// staged `exact_text` is wiped. Because the wipe, the audit, the condition
/// closure, and the phase change share one commit, no partial shape (closed
/// condition without completion, audit without closure, wiped material without
/// audit) is ever readable.
///
/// Request provenance (A1b): an operation admitted from a staged request keeps
/// that request's purpose for its whole life, and — while its protected
/// material exists — the same exact mechanical text. A completed operation's
/// material is gone by design, so only the purpose link is checked there.
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
         OR (o.request_id IS NOT NULL AND NOT EXISTS
             (SELECT 1 FROM deletion_request r
              WHERE r.request_id=o.request_id AND r.purpose=o.purpose))
         OR (o.request_id IS NOT NULL AND o.phase != 'completed' AND NOT EXISTS
             (SELECT 1 FROM deletion_request r JOIN deletion_search_material m
                 ON m.operation_id=o.operation_id
              WHERE r.request_id=o.request_id AND r.exact_text=m.exact_text))
         OR (o.phase IN ('active','held') AND NOT EXISTS
             (SELECT 1 FROM deletion_search_material m WHERE m.operation_id=o.operation_id AND length(m.exact_text)>0))
         OR (o.phase='completed' AND (EXISTS
             (SELECT 1 FROM deletion_search_material m WHERE m.operation_id=o.operation_id) OR EXISTS
             (SELECT 1 FROM deletion_semantic_hint h WHERE h.operation_id=o.operation_id)))
         OR NOT EXISTS
             (SELECT 1 FROM deletion_participant p WHERE p.operation_id=o.operation_id)
         OR EXISTS
             (SELECT 1 FROM deletion_participant p WHERE p.operation_id=o.operation_id AND p.sweep != o.sweep)
         OR (o.phase='finalizing' AND EXISTS
             (SELECT 1 FROM deletion_participant p WHERE p.operation_id=o.operation_id AND p.state != 'verified'))
         OR (o.phase='completed' AND EXISTS
             (SELECT 1 FROM deletion_participant p WHERE p.operation_id=o.operation_id AND p.state != 'verified'))
         OR ((o.phase='completed') != EXISTS
             (SELECT 1 FROM deletion_completion_audit a WHERE a.operation_id=o.operation_id))
         OR EXISTS
             (SELECT 1 FROM deletion_completion_audit a WHERE a.operation_id=o.operation_id
              AND (a.purpose != o.purpose OR a.started_at != o.started_at OR a.sweep_count != o.sweep
                   OR a.participant_count <= 0 OR a.verified_count != a.participant_count
                   OR a.participant_count !=
                       (SELECT COUNT(*) FROM deletion_participant p WHERE p.operation_id=o.operation_id)))
         OR EXISTS
             (SELECT 1 FROM deletion_audit_participant ap
              LEFT JOIN deletion_participant p ON p.operation_id=ap.operation_id AND p.participant_owner=ap.participant_owner
              WHERE ap.operation_id=o.operation_id
                AND (p.participant_owner IS NULL OR ap.final_state != 'verified'
                     OR ap.erased_count != p.erased_count))
         OR (o.phase='completed' AND EXISTS
             (SELECT 1 FROM deletion_participant p WHERE p.operation_id=o.operation_id AND NOT EXISTS
                 (SELECT 1 FROM deletion_audit_participant ap
                  WHERE ap.operation_id=p.operation_id AND ap.participant_owner=p.participant_owner)))
         OR (o.phase='completed' AND o.request_id IS NOT NULL AND EXISTS
             (SELECT 1 FROM deletion_request r WHERE r.request_id=o.request_id AND r.exact_text IS NOT NULL))))
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
    validate_reconciliation(conn, operation)
}

pub(crate) const COVERING_CANDIDATE_SQL: &str = "SELECT s.operation_id,c.sweep,c.opened_at
     FROM erasure_condition_source s
     JOIN erasure_condition c ON c.operation_id=s.operation_id AND c.sweep=s.sweep
     JOIN deletion_operation o ON o.operation_id=c.operation_id AND o.sweep=c.sweep
     WHERE s.source=?1 AND c.closed_at IS NULL AND o.phase!='completed' LIMIT 1";

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

/// The unfinished operations whose current-sweep reconciliation is still
/// walking, as `(operation_id, sweep)` in `operation_id` order.
///
/// The published current-sweep correlation is the fast path; while a sweep is
/// incomplete, a covered identity may legitimately not be published yet. The
/// direct-fallback probes share this candidate set so the fast-skip probe and
/// the enumeration cannot drift. An empty result means no active/held
/// operation is mid-reconciliation, so the published correlation set is
/// already exhaustive.
fn unreconciled_operations(
    conn: &Connection,
) -> Result<Vec<(String, i64)>, PreservationTechnicalError> {
    let mut statement = conn
        .prepare(
            "SELECT o.operation_id,o.sweep FROM deletion_operation o
             WHERE o.phase IN ('active','held') AND EXISTS
                 (SELECT 1 FROM deletion_reconciliation r
                  WHERE r.operation_id=o.operation_id AND r.complete=0)
             ORDER BY o.operation_id",
        )
        .map_err(storage)?;
    let candidates: Vec<(String, i64)> = statement
        .query_map((), |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    drop(statement);
    Ok(candidates)
}

/// The protected exact target of one operation, `None` when the material row
/// is absent (completion wipe or missing row).
fn load_exact_target(
    conn: &Connection,
    operation: &str,
) -> Result<Option<String>, PreservationTechnicalError> {
    conn.query_row(
        "SELECT exact_text FROM deletion_search_material WHERE operation_id=?1",
        [operation],
        |row| row.get(0),
    )
    .optional()
    .map_err(storage)
}

/// The unreconciled operations' protected targets, as
/// `(operation_id, sweep, exact_text)` in `operation_id` order.
///
/// Active/held operations always keep protected material (`validate`); a
/// missing row is torn state, never a "checked everything" default.
fn unreconciled_targets(
    conn: &Connection,
) -> Result<Vec<(String, i64, String)>, PreservationTechnicalError> {
    let mut targets = Vec::new();
    for (id, sweep) in unreconciled_operations(conn)? {
        validate(conn, &id)?;
        let target = load_exact_target(conn, &id)?;
        targets.push((id, sweep, target.ok_or_else(corrupt)?));
    }
    Ok(targets)
}

/// Direct mechanical coverage of one source identity by an operation whose
/// current-sweep reconciliation is still walking.
///
/// The published current-sweep correlation is the fast path; while a sweep is
/// incomplete, a covered identity may legitimately not be published yet. The
/// identity's own stored body is durable evidence independent of any page
/// bound, so it is compared directly against each unreconciled operation's
/// protected target. This keeps a new send or adoption from starting on a
/// covered source between the condition commit and the end of the walk; after
/// the walk the published correlation answers the same way.
fn directly_covered_source(
    conn: &Connection,
    source: &str,
) -> Result<Option<ErasureConditionRef>, PreservationTechnicalError> {
    for (id, sweep, target) in unreconciled_targets(conn)? {
        if source_identity_carries(conn, source, &target)? {
            return Ok(Some(decode_ref(&id, sweep)?.condition()));
        }
    }
    Ok(None)
}

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
    directly_covered_source(conn, source)
}

/// One traversal of the canonical current conditions with their protected
/// mechanical targets (lifecycle §7/§11).
///
/// Returns `(condition, target)` in `operation_id` order; `target` is `None`
/// when the operation's protected material row is absent. Every unfinished
/// operation is Owner-confirmed and validated here, so an operation whose
/// structural rows are torn fails the read closed instead of being read as
/// "not covering".
fn current_operation_targets(
    conn: &Connection,
) -> Result<Vec<(ErasureConditionRef, Option<String>)>, PreservationTechnicalError> {
    let orphan: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM erasure_condition c
                 LEFT JOIN deletion_operation o ON o.operation_id=c.operation_id
                 WHERE o.operation_id IS NULL)",
            (),
            |row| row.get(0),
        )
        .map_err(storage)?;
    if orphan {
        return Err(corrupt());
    }
    let mut statement = conn
        .prepare(
            "SELECT operation_id, sweep FROM deletion_operation
             WHERE phase!='completed' ORDER BY operation_id",
        )
        .map_err(storage)?;
    let ids: Vec<(String, i64)> = statement
        .query_map((), |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    drop(statement);
    let mut targets = Vec::with_capacity(ids.len());
    for (id, sweep) in ids {
        validate(conn, &id)?;
        let target = load_exact_target(conn, &id)?;
        targets.push((decode_ref(&id, sweep)?.condition(), target));
    }
    Ok(targets)
}

/// One mechanical text verdict against the canonical current conditions
/// (lifecycle §7/§11).
///
/// [`Self::Target`] means a current condition's protected exact target occurs
/// in the text; the target itself is never copied out of the store or rendered
/// into a log, an outcome, a completion fact, or an audit row. [`Self::Unreadable`]
/// is a current condition with no readable protected target: production never
/// observes this shape (the completion wipe runs in the same transaction that
/// commits `completed`), so it is a defensive branch for torn state. Should a
/// wipe ever be observable before the completed commit, the condition still
/// covers, but no target remains to redact mechanically, so callers fail
/// closed (refuse; a collecting owner stores a body-free marker) rather than
/// treating an unreadable target as "not covering".
pub(crate) enum TextCoverage {
    Target,
    Unreadable,
}

/// The exact-target premise of one bounded read pass (lifecycle §7/§11).
///
/// This is the page-shaped companion of [`covering_text`]: one canonical read
/// of the unfinished operations' protected mechanical targets, reused for
/// every row of one read. The premise is read from the canonical store inside
/// the caller's transaction, so an empty target set across every current
/// condition is the authoritative "not covered" (no sentinel, no cached
/// verdict). A completed operation is excluded by the canonical
/// `phase`/`closed_at` invariant: its condition stopped covering text (§7:
/// completion is not a permanent keyword ban). Unfinished operations are the
/// bounded candidate set: they are Owner-confirmed and validated here, so an
/// operation whose structural rows are torn fails the read closed instead of
/// being read as "not covering".
///
/// [`Self::covers`] is the same mechanical predicate the A3 owner sweeps apply
/// — an exact substring match of a protected target. A current condition with
/// no readable target at all (defensive: should a wipe ever be observable
/// before the completed commit) leaves nothing to compare, so the premise is
/// unreadable and every body is covered (fail closed) rather than served as
/// uncovered.
pub(crate) struct TextCoveragePremise {
    targets: Option<Vec<String>>,
}

impl TextCoveragePremise {
    pub(crate) fn read(conn: &Connection) -> Result<Self, PreservationTechnicalError> {
        let mut targets = Vec::new();
        for (_, target) in current_operation_targets(conn)? {
            let Some(target) = target else {
                return Ok(Self { targets: None });
            };
            if !target.is_empty() {
                targets.push(target);
            }
        }
        Ok(Self {
            targets: Some(targets),
        })
    }

    pub(crate) fn covers(&self, text: &str) -> bool {
        let Some(targets) = self.targets.as_ref() else {
            return true;
        };
        targets.iter().any(|target| text.contains(target.as_str()))
    }
}

pub(crate) fn covering_text(
    conn: &Connection,
    text: &str,
) -> Result<Option<TextCoverage>, PreservationTechnicalError> {
    let premise = TextCoveragePremise::read(conn)?;
    let Some(targets) = premise.targets.as_ref() else {
        return Ok(Some(TextCoverage::Unreadable));
    };
    Ok(targets
        .iter()
        .find(|target| text.contains(target.as_str()))
        .map(|_| TextCoverage::Target))
}

pub(crate) fn covering_text_condition(
    conn: &Connection,
    text: &str,
) -> Result<Option<(ErasureConditionRef, TextCoverage)>, PreservationTechnicalError> {
    for (condition, target) in current_operation_targets(conn)? {
        match target {
            // Defensive: production commits the completed phase and the wipe
            // in one transaction, so an unfinished condition keeps its
            // material. Should a wipe ever be observable first, the body is
            // covered with no readable target.
            None => return Ok(Some((condition, TextCoverage::Unreadable))),
            Some(target) if !target.is_empty() && text.contains(&target) => {
                return Ok(Some((condition, TextCoverage::Target)));
            }
            Some(_) => {}
        }
    }
    Ok(None)
}

pub(crate) fn covering_sources(
    conn: &Connection,
    sources: &[RawId],
) -> Result<Option<ErasureConditionRef>, PreservationTechnicalError> {
    for source in sources {
        if let Some(condition) = covering_condition(conn, &encode_id(*source))? {
            return Ok(Some(condition));
        }
    }
    Ok(None)
}

/// Collects one covered body instead of persisting it: redacts every current
/// condition's mechanical target out of the text and returns the body-free
/// result, or [`crate::erasure::ERASED_MARKER`] when a current condition's
/// protected target is unreadable (defensive: see [`TextCoverage::Unreadable`])
/// so no mechanical comparison is possible at all.
///
/// This is the "erase collection" side of the A4 boundary contract for bodies
/// whose objective fact must survive (a Task result arrival seals its
/// delegation): the fact is committed, the body is not. A marker inserted for
/// one target can carry another (`"a"` and `"e"` both occur in the marker), so a
/// later pass can re-create an earlier target and the marker phase has no
/// fixpoint; it is therefore bounded at one pass per target. Residual coverage
/// falls back to removal, which strictly shortens the value (a joined
/// occurrence is removed on a later iteration) and so always reaches a
/// body-free fixpoint instead of growing without limit.
pub(crate) fn redact_covered_text(
    conn: &Connection,
    text: &str,
) -> Result<String, PreservationTechnicalError> {
    let premise = TextCoveragePremise::read(conn)?;
    let Some(targets) = premise.targets.as_ref() else {
        return Ok(String::from(crate::erasure::ERASED_MARKER));
    };
    let mut current = text.to_owned();
    for _ in 0..=targets.len() {
        let Some(target) = targets
            .iter()
            .find(|target| current.contains(target.as_str()))
        else {
            return Ok(current);
        };
        match crate::erasure::redact_exact(&current, target) {
            Some((redacted, _)) => current = redacted,
            // The premise matched the target, so a missing occurrence would be
            // an inconsistent mechanical predicate; fail closed rather than
            // storing a body that is still covered.
            None => return Err(corrupt()),
        }
    }
    loop {
        let Some(target) = targets
            .iter()
            .find(|target| current.contains(target.as_str()))
        else {
            return Ok(current);
        };
        current = current.replace(target.as_str(), "");
    }
}

pub(crate) fn covered_undelivered_source(
    conn: &Connection,
    kind: &str,
    id: &str,
    phase: &str,
) -> Result<bool, PreservationTechnicalError> {
    let source =
        crate::codec::decode_undelivered_source(conn, kind, id, phase).map_err(|_| corrupt())?;
    let raw = crate::codec::decode_id(id).map_err(|_| corrupt())?;
    if covering_condition(conn, &encode_id(raw))?.is_some() {
        return Ok(true);
    }
    use ene_companion::{TaskFact, UndeliveredSource};
    let body: Option<String> = match source {
        UndeliveredSource::HistoryMessage(message) => conn
            .query_row(
                "SELECT body FROM history_message WHERE message_id=?1",
                [encode_id(message)],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?,
        UndeliveredSource::TaskRecord {
            fact: TaskFact::TaskRevision { task, revision },
            ..
        } => conn
            .query_row(
                "SELECT purpose_text FROM task_revision WHERE task_id=?1 AND revision=?2",
                params![
                    encode_id(task),
                    i64::try_from(revision).map_err(|_| corrupt())?
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?,
        UndeliveredSource::TaskRecord {
            fact: TaskFact::ResultRecorded(result) | TaskFact::ResultAdopted(result),
            ..
        } => conn
            .query_row(
                "SELECT body FROM task_result WHERE result_id=?1",
                [encode_id(result)],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?,
        UndeliveredSource::TaskRecord {
            fact: TaskFact::ActionAttempt { attempt, .. },
            ..
        } => conn
            .query_row(
                "SELECT real_target FROM action_attempt WHERE attempt_id=?1",
                [encode_id(attempt)],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?,
        UndeliveredSource::ActivityRecord(activity) => conn
            .query_row(
                "SELECT body FROM activity_record WHERE activity_id=?1",
                [encode_id(activity)],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?,
        UndeliveredSource::TaskRecord {
            fact: TaskFact::Delegation(_) | TaskFact::Terminal { .. },
            ..
        } => None,
    };
    let Some(body) = body else {
        return Ok(false);
    };
    Ok(covering_text(conn, &body)?.is_some())
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
    let phase = DeletionOperationPhase::from_name(raw.2.as_str()).ok_or_else(corrupt)?;
    let purpose = decode_purpose(&raw.3)?;
    let hold = match raw.5.as_deref() {
        None => None,
        Some(name) => Some(DeletionHoldReason::from_name(name).ok_or_else(corrupt)?),
    };
    Ok(DeletionOperationRecord {
        current: decode_ref(&raw.0, raw.1)?,
        phase,
        purpose,
        started_at: parse_time(&raw.4)?,
        hold,
    })
}

macro_rules! operation_columns {
    () => {
        "operation_id,sweep,phase,purpose,started_at,hold_reason"
    };
}

fn load_operation(
    conn: &Connection,
    id: &str,
) -> Result<Option<RawOperation>, PreservationTechnicalError> {
    conn.query_row(
        concat!(
            "SELECT ",
            operation_columns!(),
            " FROM deletion_operation WHERE operation_id=?1"
        ),
        [id],
        raw_operation,
    )
    .optional()
    .map_err(storage)
}

fn load_operation_state(
    conn: &Connection,
    id: &str,
) -> Result<Option<(i64, String)>, PreservationTechnicalError> {
    conn.query_row(
        "SELECT sweep,phase FROM deletion_operation WHERE operation_id=?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(storage)
}

fn require_operation(conn: &Connection, id: &str) -> Result<(), PreservationTechnicalError> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM deletion_operation WHERE operation_id=?1)",
            [id],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if exists {
        Ok(())
    } else {
        Err(PreservationTechnicalError::UnknownOperation)
    }
}

type RawParticipant = (String, String, i64, Option<String>, Option<String>);

fn raw_participant(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawParticipant> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

/// One participant row is composed only when it is internally consistent: a
/// known owner and state name, a positive sweep, a hold class exactly when
/// held, and a parseable report time. Anything else is an unreadable row,
/// never a guessed progress.
fn decode_participant(
    operation: DeletionOperationId,
    raw: RawParticipant,
) -> Result<DeletionParticipantRecord, PreservationTechnicalError> {
    let owner = ParticipantOwnerRef::from_storage_name(&raw.0).ok_or_else(corrupt)?;
    if raw.2 <= 0 {
        return Err(corrupt());
    }
    let sweep = DeletionSweepGeneration::from_u64(raw.2 as u64);
    let hold = match raw.3.as_deref() {
        None => None,
        Some(name) => Some(ParticipantHoldClass::from_name(name).ok_or_else(corrupt)?),
    };
    let progress =
        ParticipantProgress::from_state_name(raw.1.as_str(), sweep, hold).ok_or_else(corrupt)?;
    // The report time is not part of the record, but a stored value that does
    // not parse is torn state and still fails the read closed.
    if let Some(text) = raw.4.as_deref() {
        parse_time(text)?;
    }
    Ok(DeletionParticipantRecord {
        participant: DeletionParticipantRef { operation, owner },
        progress,
    })
}

fn decode_purpose(raw: &str) -> Result<DeletionPurpose, PreservationTechnicalError> {
    DeletionPurpose::from_name(raw).ok_or_else(corrupt)
}

type RawRequest = (String, String, Option<String>, String);

fn raw_request(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRequest> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

fn decode_request(raw: RawRequest) -> Result<TargetedDeletionRequest, PreservationTechnicalError> {
    let Some(exact_text) = raw.2 else {
        return Err(corrupt());
    };
    if exact_text.is_empty() {
        return Err(corrupt());
    }
    // The stored request time is journal metadata with no reader; parsing it
    // still fails closed on an unreadable column.
    let _ = parse_time(&raw.3)?;
    Ok(TargetedDeletionRequest::from_durable(
        DeletionRequestId::from_raw(decode_id(&raw.0).map_err(|_| corrupt())?),
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                exact_text,
            )),
            semantic_hints: Vec::new(),
        },
        decode_purpose(&raw.1)?,
    ))
}

fn decode_request_id(raw: &str) -> Result<DeletionRequestId, PreservationTechnicalError> {
    Ok(DeletionRequestId::from_raw(
        decode_id(raw).map_err(|_| corrupt())?,
    ))
}

fn unfinished_by_exact_text(
    tx: &rusqlite::Transaction<'_>,
    text: &str,
) -> Result<Option<DeletionOperationRecord>, PreservationTechnicalError> {
    let existing: Option<(RawOperation, String)> = tx
        .query_row(
            "SELECT o.operation_id,o.sweep,o.phase,o.purpose,o.started_at,o.hold_reason
             FROM deletion_operation o JOIN deletion_search_material m ON m.operation_id=o.operation_id
             WHERE o.phase!='completed' AND m.exact_text=?1 ORDER BY o.operation_id LIMIT 1",
            [text],
            |row| {
                let id: String = row.get(0)?;
                Ok((raw_operation(row)?, id))
            },
        )
        .optional()
        .map_err(storage)?;
    match existing {
        None => Ok(None),
        Some((raw, id)) => {
            validate(tx, &id)?;
            Ok(Some(decode_operation(raw)?))
        }
    }
}

/// Whether one unfinished operation already covers the whole scope of a
/// duplicate request: every semantic hint and every required participant owner
/// must already belong to it. Same mechanical target is an idempotent request
/// only if its scope is covered; a duplicate never silently widens a confirmed
/// operation. The participant snapshot is part of the operation's scope, so a
/// duplicate whose set needs an owner outside the snapshot is a
/// live-operation conflict, not a silent widening.
fn scope_covered(
    tx: &rusqlite::Transaction<'_>,
    current: DeletionOperationRef,
    hints: &[DeletionSearchMaterial],
    participants: &[ParticipantOwnerRef],
) -> Result<bool, PreservationTechnicalError> {
    let mut covered = true;
    for hint in hints {
        let found: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM deletion_semantic_hint WHERE operation_id=?1 AND material=?2)",
                params![
                    encode_id(current.operation.as_raw()),
                    hint.expose_for_erasure()
                ],
                |r| r.get(0),
            )
            .map_err(storage)?;
        covered &= found;
    }
    for owner in participants {
        let found: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM deletion_participant WHERE operation_id=?1 AND participant_owner=?2)",
                params![
                    encode_id(current.operation.as_raw()),
                    owner.storage_name()
                ],
                |r| r.get(0),
            )
            .map_err(storage)?;
        covered &= found;
    }
    Ok(covered)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct KnownSourceIdentity {
    pub(crate) table: &'static str,
    key: &'static str,
    body: &'static str,
}

pub(crate) fn known_source_page_sql(identity: KnownSourceIdentity) -> String {
    format!(
        "SELECT {} FROM {} WHERE {} > ?1 AND instr({}, ?2) > 0 ORDER BY {} LIMIT ?3",
        identity.key, identity.table, identity.key, identity.body, identity.key
    )
}

pub(crate) const KNOWN_SOURCE_IDENTITIES: &[KnownSourceIdentity] = &[
    KnownSourceIdentity {
        table: "history_message",
        key: "message_id",
        body: "body",
    },
    KnownSourceIdentity {
        table: "activity_record",
        key: "activity_id",
        body: "body",
    },
    KnownSourceIdentity {
        table: "learning_summary",
        key: "summary_id",
        body: "content",
    },
    KnownSourceIdentity {
        table: "learning_memory",
        key: "memory_id",
        body: "content",
    },
    KnownSourceIdentity {
        table: "action_attempt",
        key: "attempt_id",
        body: "real_target",
    },
    KnownSourceIdentity {
        table: "task_result",
        key: "result_id",
        body: "body",
    },
];

/// Initializes the durable reconciliation cursors for one operation + sweep:
/// exactly one row per known identity table, incomplete.
///
/// The confirmed admission path publishes its first bounded pages in the same
/// transaction and the durable cursors carry the walk to its end; a row set
/// that does not match the known table set is torn state and fails closed
/// through [`validate`].
fn insert_reconciliation_rows(
    tx: &rusqlite::Transaction<'_>,
    operation: &str,
    sweep: i64,
) -> Result<(), PreservationTechnicalError> {
    for identity in KNOWN_SOURCE_IDENTITIES {
        tx.execute(
            "INSERT INTO deletion_reconciliation (operation_id,sweep,identity_table,cursor,complete) VALUES (?1,?2,?3,'',0)",
            params![operation, sweep, identity.table],
        )
        .map_err(storage)?;
    }
    Ok(())
}

fn next_incomplete_identity(
    conn: &Connection,
    operation: &str,
    sweep: i64,
) -> Result<Option<KnownSourceIdentity>, PreservationTechnicalError> {
    let rows = reconciliation_rows(conn, operation)?;
    Ok(KNOWN_SOURCE_IDENTITIES.iter().copied().find(|identity| {
        rows.iter().any(|(table, row_sweep, complete)| {
            table == identity.table && *row_sweep == sweep && *complete == 0
        })
    }))
}

fn reconciliation_is_complete(
    conn: &Connection,
    operation: &str,
    sweep: i64,
) -> Result<bool, PreservationTechnicalError> {
    let rows = reconciliation_rows(conn, operation)?;
    if rows.len() != KNOWN_SOURCE_IDENTITIES.len() {
        return Ok(false);
    }
    Ok(KNOWN_SOURCE_IDENTITIES.iter().all(|identity| {
        rows.iter().any(|(table, row_sweep, complete)| {
            table == identity.table && *row_sweep == sweep && *complete == 1
        })
    }))
}

fn reconcile_table_page(
    tx: &rusqlite::Transaction<'_>,
    operation: &str,
    sweep: i64,
    target: &str,
    identity: KnownSourceIdentity,
    page_size: u32,
) -> Result<bool, PreservationTechnicalError> {
    let (cursor, complete): (String, i64) = tx
        .query_row(
            "SELECT cursor,complete FROM deletion_reconciliation
             WHERE operation_id=?1 AND sweep=?2 AND identity_table=?3",
            params![operation, sweep, identity.table],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(corrupt)?;
    if complete == 1 {
        return Ok(true);
    }
    let sql = known_source_page_sql(identity);
    let mut statement = tx.prepare(&sql).map_err(storage)?;
    let keys: Vec<String> = statement
        .query_map(params![cursor, target, i64::from(page_size)], |row| {
            row.get(0)
        })
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    drop(statement);
    let held_at = WallClockWithTz::now().to_rfc3339();
    for key in &keys {
        let raw = decode_id(key).map_err(|_| corrupt())?;
        tx.execute(
            "INSERT OR IGNORE INTO erasure_condition_source (operation_id,sweep,source) VALUES (?1,?2,?3)",
            params![operation, sweep, encode_id(raw)],
        )
        .map_err(storage)?;
    }
    if !keys.is_empty() {
        associate_reconciled_page(tx, operation, &keys, &held_at)?;
    }
    let table_complete = u32::try_from(keys.len()).map_err(|_| corrupt())? < page_size;
    let next_cursor = keys.last().map_or(cursor, Clone::clone);
    tx.execute(
        "UPDATE deletion_reconciliation SET cursor=?4,complete=?5
         WHERE operation_id=?1 AND sweep=?2 AND identity_table=?3",
        params![
            operation,
            sweep,
            identity.table,
            next_cursor,
            i64::from(table_complete)
        ],
    )
    .map_err(storage)?;
    Ok(table_complete)
}

fn reconcile_admission_pages(
    tx: &rusqlite::Transaction<'_>,
    operation: &str,
    sweep: i64,
    target: &str,
    page_size: u32,
) -> Result<(), PreservationTechnicalError> {
    for identity in KNOWN_SOURCE_IDENTITIES {
        reconcile_table_page(tx, operation, sweep, target, *identity, page_size)?;
    }
    Ok(())
}

fn associate_reconciled_page(
    tx: &rusqlite::Transaction<'_>,
    operation: &str,
    sources: &[String],
    held_at: &str,
) -> Result<(), PreservationTechnicalError> {
    if sources.is_empty() {
        return Ok(());
    }
    let placeholders = (0..sources.len())
        .map(|index| format!("?{}", index + 3))
        .collect::<Vec<_>>()
        .join(",");
    let bind = || {
        [operation, held_at]
            .into_iter()
            .chain(sources.iter().map(String::as_str))
    };
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
             SELECT '{USE_KIND_INFERENCE_ATTEMPT}', a.ticket, ?1, ?2
             FROM inference_attempt_data_use u
             JOIN inference_attempt a ON a.ticket=u.ticket
             WHERE u.source IN ({placeholders})
             GROUP BY a.ticket"
        ),
        params_from_iter(bind()),
    )
    .map_err(storage)?;
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
             SELECT '{USE_KIND_TASK_DELEGATION}', d.delegation_id, ?1, ?2
             FROM delegation d
             WHERE NOT EXISTS(SELECT 1 FROM task_result r WHERE r.delegation_id=d.delegation_id)
               AND EXISTS(SELECT 1 FROM inference_attempt_data_use u
                          JOIN inference_attempt a ON a.ticket=u.ticket
                          WHERE u.source IN ({placeholders}) AND a.delegation_id=d.delegation_id)"
        ),
        params_from_iter(bind()),
    )
    .map_err(storage)?;
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
             SELECT '{USE_KIND_TASK_DELEGATION}', d.delegation_id, ?1, ?2
             FROM delegation d
             JOIN task_context_entry ce ON ce.task_id=d.task_id
             WHERE NOT EXISTS(SELECT 1 FROM task_result r WHERE r.delegation_id=d.delegation_id)
               AND ce.origin_source IN ({placeholders})"
        ),
        params_from_iter(bind()),
    )
    .map_err(storage)?;
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
             SELECT '{USE_KIND_LEARNING_FORMATION}', fs.formation_id, ?1, ?2
             FROM learning_formation_source fs
             WHERE fs.source IN ({placeholders})
             GROUP BY fs.formation_id"
        ),
        params_from_iter(bind()),
    )
    .map_err(storage)?;
    Ok(())
}

pub(crate) const USE_KIND_INFERENCE_ATTEMPT: &str = "inference_attempt";
pub(crate) const USE_KIND_TASK_DELEGATION: &str = "task_delegation";
pub(crate) const USE_KIND_LEARNING_FORMATION: &str = "learning_formation";

fn source_identity_carries(
    conn: &Connection,
    source: &str,
    target: &str,
) -> Result<bool, PreservationTechnicalError> {
    for identity in KNOWN_SOURCE_IDENTITIES {
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM {} WHERE {}=?1 AND instr({},?2)>0)",
            identity.table, identity.key, identity.body
        );
        let found: bool = conn
            .query_row(&sql, params![source, target], |row| row.get(0))
            .map_err(storage)?;
        if found {
            return Ok(true);
        }
    }
    Ok(false)
}

fn claimed_attempt_covers(
    conn: &Connection,
    ticket: &str,
    target: &str,
) -> Result<bool, PreservationTechnicalError> {
    let mut statement = conn
        .prepare("SELECT source FROM inference_attempt_data_use WHERE ticket=?1 ORDER BY ordinal")
        .map_err(storage)?;
    let sources: Vec<String> = statement
        .query_map([ticket], |row| row.get(0))
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    for source in sources {
        if source_identity_carries(conn, &source, target)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn claimed_delegation_covers(
    conn: &Connection,
    delegation: &str,
    target: &str,
) -> Result<bool, PreservationTechnicalError> {
    let row: Option<(String, i64, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT task_id,task_revision,scope_folder,scope_save_target
             FROM delegation WHERE delegation_id=?1",
            [delegation],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(storage)?;
    let Some((task, revision, folder, save_target)) = row else {
        return Ok(false);
    };
    let sealed: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM task_result WHERE delegation_id=?1)",
            [delegation],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if sealed {
        return Ok(false);
    }
    let mut statement = conn
        .prepare("SELECT ticket FROM inference_attempt WHERE delegation_id=?1")
        .map_err(storage)?;
    let tickets: Vec<String> = statement
        .query_map([delegation], |row| row.get(0))
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    for ticket in tickets {
        if claimed_attempt_covers(conn, &ticket, target)? {
            return Ok(true);
        }
    }
    let carries = |text: Option<String>| text.is_some_and(|text| text.contains(target));
    if carries(
        conn.query_row(
            "SELECT purpose_text FROM task WHERE task_id=?1",
            [&task],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?,
    ) {
        return Ok(true);
    }
    if carries(
        conn.query_row(
            "SELECT purpose_text FROM task_revision WHERE task_id=?1 AND revision=?2",
            params![task, revision],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?,
    ) {
        return Ok(true);
    }
    if carries(folder) || carries(save_target) {
        return Ok(true);
    }
    let mut statement = conn
        .prepare("SELECT origin_source FROM task_context_entry WHERE task_id=?1")
        .map_err(storage)?;
    let origins: Vec<String> = statement
        .query_map([&task], |row| row.get(0))
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    for origin in origins {
        if source_identity_carries(conn, &origin, target)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn source_identity_exists(
    conn: &Connection,
    source: &str,
) -> Result<bool, PreservationTechnicalError> {
    for identity in KNOWN_SOURCE_IDENTITIES {
        let sql = format!(
            "SELECT EXISTS(SELECT 1 FROM {} WHERE {}=?1)",
            identity.table, identity.key
        );
        let found: bool = conn
            .query_row(&sql, params![source], |row| row.get(0))
            .map_err(storage)?;
        if found {
            return Ok(true);
        }
    }
    Ok(false)
}

fn claimed_formation_covers(
    conn: &Connection,
    formation: &str,
    target: &str,
) -> Result<bool, PreservationTechnicalError> {
    let mut statement = conn
        .prepare("SELECT source FROM learning_formation_source WHERE formation_id=?1")
        .map_err(storage)?;
    let sources: Vec<String> = statement
        .query_map([formation], |row| row.get(0))
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    if sources.is_empty() {
        return Ok(true);
    }
    for source in sources {
        if source_identity_carries(conn, &source, target)? {
            return Ok(true);
        }
        if !source_identity_exists(conn, &source)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn directly_covered_uses(
    conn: &Connection,
    use_kind: &str,
    use_id: RawId,
) -> Result<Vec<String>, PreservationTechnicalError> {
    let mut matched = Vec::new();
    for (id, _sweep, target) in unreconciled_targets(conn)? {
        let covered = match use_kind {
            USE_KIND_INFERENCE_ATTEMPT => {
                claimed_attempt_covers(conn, &encode_id(use_id), &target)?
            }
            USE_KIND_TASK_DELEGATION => {
                claimed_delegation_covers(conn, &encode_id(use_id), &target)?
            }
            USE_KIND_LEARNING_FORMATION => {
                claimed_formation_covers(conn, &encode_id(use_id), &target)?
            }
            _ => return Err(corrupt()),
        };
        if covered {
            matched.push(id);
        }
    }
    Ok(matched)
}

pub(crate) fn held_use(
    conn: &Connection,
    use_kind: &str,
    use_id: RawId,
) -> Result<bool, PreservationTechnicalError> {
    let held: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM erasure_use_hold WHERE use_kind=?1 AND use_id=?2)",
            params![use_kind, encode_id(use_id)],
            |row| row.get(0),
        )
        .map_err(storage)?;
    let operations = directly_covered_uses(conn, use_kind, use_id)?;
    let held_at = WallClockWithTz::now().to_rfc3339();
    for operation in &operations {
        conn.execute(
            "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at) VALUES (?1,?2,?3,?4)",
            params![use_kind, encode_id(use_id), operation, held_at],
        )
        .map_err(storage)?;
    }
    Ok(held || !operations.is_empty())
}

fn publish_learning_formation(
    tx: &rusqlite::Transaction<'_>,
    companion: RawId,
    sources: &[RawId],
) -> Result<RawId, PreservationTechnicalError> {
    let formation = RawId::new();
    let formation_text = encode_id(formation);
    let started_at = WallClockWithTz::now().to_rfc3339();
    tx.execute(
        "INSERT INTO learning_formation (formation_id,companion_id,started_at) VALUES (?1,?2,?3)",
        params![formation_text, encode_id(companion), started_at],
    )
    .map_err(storage)?;
    for source in sources {
        tx.execute(
            "INSERT OR IGNORE INTO learning_formation_source (formation_id,source) VALUES (?1,?2)",
            params![formation_text, encode_id(*source)],
        )
        .map_err(storage)?;
    }
    if sources.is_empty() {
        tx.execute(
            "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
             SELECT ?1, ?2, operation_id, ?3
             FROM deletion_operation
             WHERE phase IN ('active', 'held', 'finalizing')",
            params![USE_KIND_LEARNING_FORMATION, formation_text, started_at],
        )
        .map_err(storage)?;
    } else {
        tx.execute(
            "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
             SELECT ?1, ?2, o.operation_id, ?3
             FROM deletion_operation o
             JOIN erasure_condition_source s
               ON s.operation_id = o.operation_id AND s.sweep = o.sweep
             JOIN learning_formation_source fs
               ON fs.source = s.source AND fs.formation_id = ?2
             WHERE o.phase IN ('active', 'held', 'finalizing')
             GROUP BY o.operation_id",
            params![USE_KIND_LEARNING_FORMATION, formation_text, started_at],
        )
        .map_err(storage)?;
    }
    let _ = held_use(tx, USE_KIND_LEARNING_FORMATION, formation)?;
    Ok(formation)
}

fn settle_learning_formation_sync(
    tx: &rusqlite::Transaction<'_>,
    formation: RawId,
) -> Result<(), PreservationTechnicalError> {
    let formation_text = encode_id(formation);
    tx.execute(
        "DELETE FROM learning_formation_source WHERE formation_id=?1",
        params![formation_text],
    )
    .map_err(storage)?;
    tx.execute(
        "DELETE FROM learning_formation WHERE formation_id=?1",
        params![formation_text],
    )
    .map_err(storage)?;
    Ok(())
}

fn formation_sources_missing(
    conn: &Connection,
    formation: RawId,
) -> Result<bool, PreservationTechnicalError> {
    let mut statement = conn
        .prepare("SELECT source FROM learning_formation_source WHERE formation_id=?1")
        .map_err(storage)?;
    let sources: Vec<String> = statement
        .query_map([encode_id(formation)], |row| row.get(0))
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    for source in sources {
        if !source_identity_exists(conn, &source)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn learning_formation_must_refuse_sync(
    conn: &Connection,
    formation: RawId,
) -> Result<bool, PreservationTechnicalError> {
    if held_use(conn, USE_KIND_LEARNING_FORMATION, formation)? {
        return Ok(true);
    }
    formation_sources_missing(conn, formation)
}

pub(crate) fn inflight_learning_formation_held(
    conn: &Connection,
    sources: &[String],
) -> Result<bool, PreservationTechnicalError> {
    if sources.is_empty() {
        return Ok(false);
    }
    let placeholders = (0..sources.len())
        .map(|index| format!("?{}", index + 2))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT EXISTS(
             SELECT 1 FROM erasure_use_hold h
             JOIN learning_formation_source fs ON fs.formation_id = h.use_id
             WHERE h.use_kind = ?1 AND fs.source IN ({placeholders})
         )"
    );
    let bind =
        std::iter::once(USE_KIND_LEARNING_FORMATION).chain(sources.iter().map(String::as_str));
    conn.query_row(&sql, params_from_iter(bind), |row| row.get(0))
        .map_err(storage)
}

impl Store {
    pub async fn inference_claim_held(
        &self,
        claim: RawId,
    ) -> Result<bool, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            held_use(&guard, USE_KIND_INFERENCE_ATTEMPT, claim)
        })
        .await
    }

    pub fn inference_claim_held_sync(
        &self,
        claim: RawId,
    ) -> Result<bool, PreservationTechnicalError> {
        let guard = lock_shared(&self.conn);
        held_use(&guard, USE_KIND_INFERENCE_ATTEMPT, claim)
    }

    pub async fn task_delegation_held(
        &self,
        delegation: RawId,
    ) -> Result<bool, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            held_use(&guard, USE_KIND_TASK_DELEGATION, delegation)
        })
        .await
    }

    pub async fn erasure_condition_is_current(
        &self,
        condition: ErasureConditionRef,
    ) -> Result<bool, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            condition_is_current(&guard, condition).map_err(storage)
        })
        .await
    }

    pub async fn erasure_sources_covered(
        &self,
        condition: ErasureConditionRef,
        candidates: Vec<RawId>,
    ) -> Result<Vec<bool>, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let guard = lock_shared(&conn);
            let mut flags = Vec::with_capacity(candidates.len());
            for source in candidates {
                flags.push(source_is_covered(&guard, condition, source)?);
            }
            Ok(flags)
        })
        .await
    }

    pub async fn begin_learning_formation(
        &self,
        companion: RawId,
        sources: Vec<RawId>,
    ) -> Result<RawId, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let formation = publish_learning_formation(&tx, companion, &sources)?;
            tx.commit().map_err(storage)?;
            Ok(formation)
        })
        .await
    }

    pub async fn settle_learning_formation(
        &self,
        formation: RawId,
    ) -> Result<(), PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            settle_learning_formation_sync(&tx, formation)?;
            tx.commit().map_err(storage)?;
            Ok(())
        })
        .await
    }

    pub async fn learning_formation_must_refuse(
        &self,
        formation: RawId,
    ) -> Result<bool, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            learning_formation_must_refuse_sync(&guard, formation)
        })
        .await
    }

    pub async fn note_host_transient_learning_arrival(
        &self,
        operations: Vec<DeletionOperationId>,
    ) -> Result<HostTransientArrivalOutcome, PreservationTechnicalError> {
        #[cfg(any(test, feature = "test-support"))]
        {
            self.test_parks
                .host_transient_arrival_attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let sticky = self
                .test_parks
                .fail_host_transient_arrival_sticky
                .load(std::sync::atomic::Ordering::SeqCst);
            if sticky {
                return Err(PreservationTechnicalError::StorageUnavailable);
            }
        }
        if operations.len() as u32 > HOST_TRANSIENT_ARRIVAL_PAGE {
            return Err(PreservationTechnicalError::InvalidLimit);
        }
        if operations.is_empty() {
            return Ok(HostTransientArrivalOutcome::Unchanged);
        }
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let outcome = note_host_transient_learning_arrival_sync(&tx, &operations)?;
            tx.commit().map_err(storage)?;
            Ok(outcome)
        })
        .await
    }
}

pub const HOST_TRANSIENT_ARRIVAL_PAGE: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostTransientArrivalOutcome {
    Unchanged,
    SweepOpened,
}

fn note_host_transient_learning_arrival_sync(
    tx: &rusqlite::Transaction<'_>,
    operations: &[DeletionOperationId],
) -> Result<HostTransientArrivalOutcome, PreservationTechnicalError> {
    let mut opened = false;
    for operation in operations {
        let id = encode_id(operation.as_raw());
        let row: Option<(i64, String, Option<String>)> = tx
            .query_row(
                "SELECT o.sweep, o.phase, p.state
                 FROM deletion_operation o
                 LEFT JOIN deletion_participant p
                   ON p.operation_id = o.operation_id
                  AND p.participant_owner = 'host_transient'
                 WHERE o.operation_id = ?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(storage)?;
        let Some((sweep, phase, state)) = row else {
            continue;
        };
        if phase == "completed" {
            continue;
        }
        if phase != "finalizing" && state.as_deref() != Some("verified") {
            continue;
        }
        match open_next_sweep(tx, &id, sweep)? {
            Some(_) => {
                validate(tx, &id)?;
                opened = true;
            }
            None => validate(tx, &id)?,
        }
    }
    Ok(if opened {
        HostTransientArrivalOutcome::SweepOpened
    } else {
        HostTransientArrivalOutcome::Unchanged
    })
}

pub(crate) const ASSOCIATE_ATTEMPTS_SQL: &str =
    "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
     SELECT ?3, a.ticket, ?1, ?4
     FROM erasure_condition_source s
     JOIN inference_attempt_data_use u ON u.source = s.source
     JOIN inference_attempt a ON a.ticket = u.ticket
     WHERE s.operation_id = ?1 AND s.sweep = ?2
     GROUP BY a.ticket";

pub(crate) const ASSOCIATE_FORMATIONS_SQL: &str =
    "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
     SELECT ?3, fs.formation_id, ?1, ?4
     FROM erasure_condition_source s
     JOIN learning_formation_source fs ON fs.source = s.source
     WHERE s.operation_id = ?1 AND s.sweep = ?2
     GROUP BY fs.formation_id";

fn mark_inflight_uses(
    tx: &rusqlite::Transaction<'_>,
    operation: &str,
    sweep: i64,
    target: &str,
    held_at: &str,
) -> Result<(), PreservationTechnicalError> {
    tx.execute(
        ASSOCIATE_ATTEMPTS_SQL,
        params![operation, sweep, USE_KIND_INFERENCE_ATTEMPT, held_at],
    )
    .map_err(storage)?;
    tx.execute(
        "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
         SELECT ?3, d.delegation_id, ?1, ?5
         FROM delegation d
         WHERE NOT EXISTS
             (SELECT 1 FROM task_result r WHERE r.delegation_id = d.delegation_id)
           AND (
             EXISTS
                 (SELECT 1 FROM erasure_use_hold h
                  JOIN inference_attempt a ON a.ticket = h.use_id
                  WHERE h.use_kind = ?6 AND h.operation_id = ?1
                    AND a.delegation_id = d.delegation_id)
             OR EXISTS
                 (SELECT 1 FROM task t
                  WHERE t.task_id = d.task_id AND instr(t.purpose_text, ?4) > 0)
             OR EXISTS
                 (SELECT 1 FROM task_revision tr
                  WHERE tr.task_id = d.task_id AND tr.revision = d.task_revision
                    AND instr(tr.purpose_text, ?4) > 0)
             OR EXISTS
                 (SELECT 1 FROM task_context_entry ce
                  WHERE ce.task_id = d.task_id
                    AND ce.origin_source IN
                        (SELECT source FROM erasure_condition_source
                         WHERE operation_id = ?1 AND sweep = ?2))
             OR instr(COALESCE(d.scope_folder, ''), ?4) > 0
             OR instr(COALESCE(d.scope_save_target, ''), ?4) > 0
           )",
        params![
            operation,
            sweep,
            USE_KIND_TASK_DELEGATION,
            target,
            held_at,
            USE_KIND_INFERENCE_ATTEMPT
        ],
    )
    .map_err(storage)?;
    tx.execute(
        ASSOCIATE_OBSERVATION_DELEGATIONS_SQL,
        params![operation, sweep, USE_KIND_TASK_DELEGATION, held_at],
    )
    .map_err(storage)?;
    tx.execute(
        ASSOCIATE_INFLIGHT_BODY_OBSERVING_DELEGATIONS_SQL,
        params![operation, USE_KIND_TASK_DELEGATION, held_at],
    )
    .map_err(storage)?;
    tx.execute(
        ASSOCIATE_FORMATIONS_SQL,
        params![operation, sweep, USE_KIND_LEARNING_FORMATION, held_at],
    )
    .map_err(storage)?;
    Ok(())
}

pub(crate) const ASSOCIATE_OBSERVATION_DELEGATIONS_SQL: &str =
    "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
     SELECT ?3, o.delegation_id, ?1, ?4
     FROM erasure_condition_source s
     JOIN task_agent_observation o ON o.observation_id = s.source
     WHERE s.operation_id = ?1 AND s.sweep = ?2
     GROUP BY o.delegation_id";

pub(crate) const ASSOCIATE_INFLIGHT_BODY_OBSERVING_DELEGATIONS_SQL: &str =
    "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
     SELECT ?2, d.delegation_id, ?1, ?3
     FROM delegation d
     WHERE NOT EXISTS
         (SELECT 1 FROM task_result r WHERE r.delegation_id = d.delegation_id)
       AND EXISTS
         (SELECT 1 FROM action_attempt a
          WHERE a.delegation_id = d.delegation_id
            AND a.operation IN ('read', 'list')
            AND NOT EXISTS
                (SELECT 1 FROM task_agent_observation o
                 WHERE o.action_attempt_id = a.attempt_id))";

pub(crate) const HOLD_OBSERVING_DELEGATIONS_SQL: &str =
    "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
     SELECT ?2, d.delegation_id, ?1, ?3
     FROM delegation d
     WHERE EXISTS
         (SELECT 1 FROM task_agent_observation o WHERE o.delegation_id = d.delegation_id)";

pub(crate) const TASK_OBSERVATION_SURVEY_LIMIT: u32 = 256;

struct SurveyedObservation {
    observation: RawId,
    delegation: String,
    task: String,
    task_revision: i64,
    workspace: Option<String>,
    attempt: Option<String>,
    path: Option<String>,
    body_observed: bool,
}

type RawSurveyedObservation = (
    String,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    Option<String>,
);

fn survey_task_observation_sources(
    tx: &rusqlite::Transaction<'_>,
    target: &str,
) -> Result<(Vec<RawId>, bool), PreservationTechnicalError> {
    let limit = i64::from(TASK_OBSERVATION_SURVEY_LIMIT) + 1;
    let mut statement = tx
        .prepare(
            "SELECT o.observation_id, o.delegation_id, o.task_id, o.task_revision,
                    o.workspace_assoc_id, o.action_attempt_id, o.path, o.body_observed,
                    d.delegation_id
             FROM task_agent_observation o
             LEFT JOIN delegation d ON d.delegation_id = o.delegation_id
             ORDER BY o.observation_id LIMIT ?1",
        )
        .map_err(storage)?;
    let rows: Vec<RawSurveyedObservation> = statement
        .query_map(params![limit], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        })
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    drop(statement);
    let overflow = rows.len() as u32 > TASK_OBSERVATION_SURVEY_LIMIT;
    if overflow {
        return Ok((Vec::new(), true));
    }
    let mut covered = Vec::new();
    for row in rows {
        let (
            observation,
            delegation,
            task,
            task_revision,
            workspace,
            attempt,
            path,
            body_observed,
            joined,
        ) = row;
        let Some(joined) = joined else {
            return Err(corrupt());
        };
        if joined != delegation {
            return Err(corrupt());
        }
        let observation = SurveyedObservation {
            observation: decode_id(&observation).map_err(|_| corrupt())?,
            delegation,
            task,
            task_revision,
            workspace,
            attempt,
            path,
            body_observed,
        };
        if observation_source_covered(tx, &observation, target)? {
            covered.push(observation.observation);
        }
    }
    Ok((covered, false))
}

fn observation_source_covered(
    tx: &rusqlite::Transaction<'_>,
    observation: &SurveyedObservation,
    target: &str,
) -> Result<bool, PreservationTechnicalError> {
    let Some(attempt_text) = &observation.attempt else {
        return Ok(observation.body_observed);
    };
    let attempt: Option<(String, String, i64, String, String)> = tx
        .query_row(
            "SELECT delegation_id, task_id, task_revision, workspace_assoc_id, real_target
             FROM action_attempt WHERE attempt_id = ?1",
            params![attempt_text],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage)?;
    let Some((delegation, task, task_revision, workspace, real_target)) = attempt else {
        return Ok(true);
    };
    if delegation != observation.delegation
        || task != observation.task
        || task_revision != observation.task_revision
        || Some(workspace) != observation.workspace
        || Some(real_target.clone()) != observation.path
    {
        return Ok(true);
    }
    if real_target.contains(target) {
        return Ok(true);
    }
    if !observation.body_observed {
        return Ok(false);
    }
    Ok(true)
}

pub(crate) fn publish_observation_source(
    tx: &rusqlite::Transaction<'_>,
    condition: ErasureConditionRef,
    observation: RawId,
) -> Result<(), PreservationTechnicalError> {
    if !condition_is_current(tx, condition).map_err(storage)? {
        return Err(corrupt());
    }
    tx.execute(
        "INSERT OR IGNORE INTO erasure_condition_source (operation_id,sweep,source) VALUES (?1,?2,?3)",
        params![
            encode_id(condition.operation.as_raw()),
            i64::try_from(condition.sweep.as_u64()).map_err(|_| corrupt())?,
            encode_id(observation)
        ],
    )
    .map_err(storage)?;
    Ok(())
}

pub(crate) fn hold_delegation(
    tx: &rusqlite::Transaction<'_>,
    condition: ErasureConditionRef,
    delegation: RawId,
    held_at: &str,
) -> Result<(), PreservationTechnicalError> {
    if !condition_is_current(tx, condition).map_err(storage)? {
        return Err(corrupt());
    }
    tx.execute(
        "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at) VALUES (?1,?2,?3,?4)",
        params![
            USE_KIND_TASK_DELEGATION,
            encode_id(delegation),
            encode_id(condition.operation.as_raw()),
            held_at
        ],
    )
    .map_err(storage)?;
    Ok(())
}

pub(crate) fn hold_body_observing_delegation(
    tx: &rusqlite::Transaction<'_>,
    delegation: RawId,
    held_at: &str,
) -> Result<(), PreservationTechnicalError> {
    tx.execute(
        "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
         SELECT ?1, ?2, operation_id, ?3
         FROM deletion_operation
         WHERE phase IN ('active', 'held', 'finalizing')",
        params![USE_KIND_TASK_DELEGATION, encode_id(delegation), held_at],
    )
    .map_err(storage)?;
    Ok(())
}

fn durable_client_incarnations(
    tx: &rusqlite::Transaction<'_>,
) -> Result<Vec<RawId>, PreservationTechnicalError> {
    let mut statement = tx
        .prepare("SELECT incarnation_id FROM client_delivery_evidence ORDER BY incarnation_id")
        .map_err(storage)?;
    let rows: Vec<String> = statement
        .query_map((), |row| row.get(0))
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    rows.into_iter()
        .map(|text| decode_id(&text).map_err(|_| corrupt()))
        .collect()
}

/// The canonical admission body for a durably confirmed staged request:
/// duplicate detection plus the durable-before-enforce insert of operation,
/// protected material, initial condition, and source correlations
/// (lifecycle §4.1).
///
/// `request` is the Host-local request whose Owner confirmation is already
/// durable; `deletion_operation.request_id` is `UNIQUE`, so one request can
/// never start two operations. Admission enumerates the covered source
/// correlations already durable in this store and publishes them with the
/// operation (§4.1 point 4): the implementation walks the owner's durable
/// identity rows whose stored text carries the confirmed exact target, in
/// bounded pages. A Client, model output, or caller never names a source.
fn admit_deletion(
    tx: &rusqlite::Transaction<'_>,
    command: &StartTargetedDeletionCommand,
    request: DeletionRequestId,
) -> Result<StartTargetedDeletionOutcome, PreservationTechnicalError> {
    if command.required_participants().is_empty()
        || command
            .required_participants()
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != command.required_participants().len()
    {
        return Err(PreservationTechnicalError::InvalidParticipantSet);
    }
    let mut participants = command.required_participants().to_vec();
    for incarnation in durable_client_incarnations(tx)? {
        let owner = ParticipantOwnerRef::ClientIncarnation(incarnation);
        if !participants.contains(&owner) {
            participants.push(owner);
        }
    }
    let MechanicalDeletionTarget::ExactText(material) = &command.target().mechanical;
    if let Some(record) = unfinished_by_exact_text(tx, material.expose_for_erasure())? {
        let covered = scope_covered(
            tx,
            record.current,
            &command.target().semantic_hints,
            &participants,
        )?;
        return Ok(if covered {
            StartTargetedDeletionOutcome::AlreadyCoveredBy(record.current)
        } else {
            StartTargetedDeletionOutcome::HeldByOperation(record.current)
        });
    }
    let (covered_observations, observation_overflow) =
        survey_task_observation_sources(tx, material.expose_for_erasure())?;
    let current = DeletionOperationRef {
        operation: DeletionOperationId::from_raw(RawId::new()),
        sweep: DeletionSweepGeneration::from_u64(1),
    };
    let id = encode_id(current.operation.as_raw());
    let at = command.requested_at().to_rfc3339();
    let request_text = encode_id(request.as_raw());
    tx.execute(
        "INSERT INTO deletion_operation (operation_id,request_id,sweep,phase,purpose,started_at) VALUES (?1,?2,1,'active',?3,?4)",
        params![id, request_text, command.purpose().as_str(), at],
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
    // §4.1 point 4: admission initializes the durable reconciliation cursors
    // and publishes the first bounded page of every known identity table in
    // this same transaction as the operation, the protected material, and the
    // initial condition. The page is a work bound, not a correctness bound:
    // the durable cursors carry the continuation, and completion refuses while
    // any table is still incomplete.
    insert_reconciliation_rows(tx, &id, 1)?;
    reconcile_admission_pages(
        tx,
        &id,
        1,
        material.expose_for_erasure(),
        DELETION_RECONCILIATION_PAGE_SIZE,
    )?;
    // The surveyed covered occurrence identities are published in the same
    // transaction: the occurrence identity is the durable name of the
    // observed source, never its body.
    for observation in &covered_observations {
        tx.execute(
            "INSERT OR IGNORE INTO erasure_condition_source (operation_id,sweep,source) VALUES (?1,1,?2)",
            params![id, encode_id(*observation)],
        )
        .map_err(storage)?;
    }
    mark_inflight_uses(tx, &id, 1, material.expose_for_erasure(), &at)?;
    if observation_overflow {
        tx.execute(
            HOLD_OBSERVING_DELEGATIONS_SQL,
            params![id, USE_KIND_TASK_DELEGATION, at],
        )
        .map_err(storage)?;
    }
    for owner in &participants {
        tx.execute(
            "INSERT INTO deletion_participant (operation_id,participant_owner,state,sweep,erased_count,remainder_count) VALUES (?1,?2,'pending',1,0,0)",
            params![id, owner.storage_name()],
        )
        .map_err(storage)?;
    }
    validate(tx, &id)?;
    Ok(StartTargetedDeletionOutcome::Started(current))
}

fn completion_summary(
    conn: &Connection,
    operation: &str,
    sweep: i64,
) -> Result<DeletionCompletionSummary, PreservationTechnicalError> {
    let sweep = u64::try_from(sweep).map_err(|_| corrupt())?;
    if sweep == 0 {
        return Err(corrupt());
    }
    let (required, verified): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(state='verified'),0)
             FROM deletion_participant WHERE operation_id=?1 AND sweep=?2",
            params![operation, i64::try_from(sweep).map_err(|_| corrupt())?],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(storage)?;
    let count = |value: i64| u64::try_from(value).map_err(|_| corrupt());
    Ok(DeletionCompletionSummary {
        operation: DeletionOperationId::from_raw(decode_id(operation).map_err(|_| corrupt())?),
        sweep: DeletionSweepGeneration::from_u64(sweep),
        required: count(required)?,
        verified: count(verified)?,
    })
}

fn open_next_sweep(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    sweep: i64,
) -> Result<Option<DeletionOperationRef>, PreservationTechnicalError> {
    if sweep <= 0 {
        return Err(corrupt());
    }
    let Some(next) = sweep.checked_add(1) else {
        tx.execute(
            "UPDATE deletion_operation SET phase='held',hold_reason='generation_exhausted' WHERE operation_id=?1",
            [id],
        )
        .map_err(storage)?;
        return Ok(None);
    };
    tx.execute(
        "UPDATE deletion_operation
         SET erased_total=erased_total+COALESCE
             ((SELECT SUM(erased_count) FROM deletion_participant WHERE operation_id=?1),0)
         WHERE operation_id=?1",
        [id],
    )
    .map_err(storage)?;
    tx.execute(
        "INSERT INTO erasure_condition (operation_id,sweep,opened_at) VALUES (?1,?2,?3)",
        params![id, next, WallClockWithTz::now().to_rfc3339()],
    )
    .map_err(storage)?;
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
        "DELETE FROM deletion_reconciliation WHERE operation_id=?1",
        [id],
    )
    .map_err(storage)?;
    insert_reconciliation_rows(tx, id, next)?;
    tx.execute(
        "UPDATE deletion_operation SET sweep=?2,phase='active',hold_reason=NULL WHERE operation_id=?1",
        params![id, next],
    )
    .map_err(storage)?;
    tx.execute(
        "UPDATE deletion_participant SET state='pending',sweep=?2,hold_class=NULL,erased_count=0,remainder_count=0,reported_at=NULL WHERE operation_id=?1",
        params![id, next],
    )
    .map_err(storage)?;
    Ok(Some(DeletionOperationRef {
        operation: DeletionOperationId::from_raw(decode_id(id).map_err(|_| corrupt())?),
        sweep: DeletionSweepGeneration::from_u64(u64::try_from(next).map_err(|_| corrupt())?),
    }))
}

fn system_remainder(
    tx: &rusqlite::Transaction<'_>,
    target: &str,
) -> Result<u64, PreservationTechnicalError> {
    crate::erasure::system_remainder(tx, target).map_err(storage)
}

fn return_to_active_for_remainder(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
    sweep: i64,
) -> Result<DeletionFinalizationOutcome, PreservationTechnicalError> {
    match open_next_sweep(tx, id, sweep)? {
        Some(next) => {
            validate(tx, id)?;
            Ok(DeletionFinalizationOutcome::RemainderCollected(next))
        }
        None => {
            validate(tx, id)?;
            Ok(DeletionFinalizationOutcome::Held(
                DeletionHoldReason::GenerationExhausted,
            ))
        }
    }
}

impl PreservationRepository for Store {
    async fn stage_targeted_deletion(
        &self,
        command: StageTargetedDeletionRequestCommand,
    ) -> Result<StageTargetedDeletionRequestOutcome, PreservationTechnicalError> {
        let MechanicalDeletionTarget::ExactText(material) = &command.target().mechanical;
        if material.expose_for_erasure().trim().is_empty() {
            return Ok(StageTargetedDeletionRequestOutcome::NeedsClarification);
        }
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let MechanicalDeletionTarget::ExactText(material) = &command.target().mechanical;
            let text = material.expose_for_erasure();
            if let Some(record) = unfinished_by_exact_text(&tx, text)? {
                let covered =
                    scope_covered(&tx, record.current, &command.target().semantic_hints, &[])?;
                return Ok(if covered {
                    StageTargetedDeletionRequestOutcome::AlreadyCoveredBy(record.current)
                } else {
                    StageTargetedDeletionRequestOutcome::HeldByOperation(record.current)
                });
            }
            let existing: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT r.request_id,(SELECT c.confirmed_at FROM deletion_confirmation c
                         WHERE c.request_id=r.request_id)
                     FROM deletion_request r WHERE r.exact_text=?1 AND r.purpose=?2
                     ORDER BY r.rowid LIMIT 1",
                    params![text, command.purpose().as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            if let Some((request, confirmed)) = existing {
                let request = decode_request_id(&request)?;
                return Ok(if confirmed.is_some() {
                    StageTargetedDeletionRequestOutcome::Confirmed(request)
                } else {
                    StageTargetedDeletionRequestOutcome::AlreadyStaged(request)
                });
            }
            let request = DeletionRequestId::from_raw(RawId::new());
            tx.execute(
                "INSERT INTO deletion_request (request_id,purpose,exact_text,requested_at) VALUES (?1,?2,?3,?4)",
                params![
                    encode_id(request.as_raw()),
                    command.purpose().as_str(),
                    text,
                    command.requested_at().to_rfc3339()
                ],
            )
            .map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(StageTargetedDeletionRequestOutcome::Staged(request))
        })
        .await
    }

    async fn confirm_targeted_deletion(
        &self,
        request: DeletionRequestId,
        required_participants: Vec<ParticipantOwnerRef>,
    ) -> Result<ConfirmTargetedDeletionOutcome, PreservationTechnicalError> {
        let id = encode_id(request.as_raw());
        let conn = Arc::clone(&self.conn);
        let request_exists = run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let staged: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM deletion_request WHERE request_id=?1)",
                    [&id],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            if !staged {
                return Ok(false);
            }
            tx.execute(
                "INSERT OR IGNORE INTO deletion_confirmation (request_id,confirmed_at) VALUES (?1,?2)",
                params![id, WallClockWithTz::now().to_rfc3339()],
            )
            .map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(true)
        })
        .await?;
        if !request_exists {
            return Ok(ConfirmTargetedDeletionOutcome::Missing);
        }
        match self
            .start_confirmed_targeted_deletion(request, required_participants)
            .await?
        {
            StartTargetedDeletionOutcome::Started(current) => {
                Ok(ConfirmTargetedDeletionOutcome::Started(current))
            }
            StartTargetedDeletionOutcome::AlreadyCoveredBy(current) => {
                Ok(ConfirmTargetedDeletionOutcome::AlreadyCoveredBy(current))
            }
            StartTargetedDeletionOutcome::HeldByOperation(current) => {
                Ok(ConfirmTargetedDeletionOutcome::HeldByOperation(current))
            }
            StartTargetedDeletionOutcome::NeedsClarification => {
                Ok(ConfirmTargetedDeletionOutcome::NeedsClarification)
            }
            StartTargetedDeletionOutcome::ConfirmationRequired => Err(corrupt()),
        }
    }

    async fn start_confirmed_targeted_deletion(
        &self,
        request: DeletionRequestId,
        required_participants: Vec<ParticipantOwnerRef>,
    ) -> Result<StartTargetedDeletionOutcome, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(request.as_raw());
            let started: Option<RawOperation> = tx
                .query_row(
                    "SELECT operation_id,sweep,phase,purpose,started_at,hold_reason
                     FROM deletion_operation WHERE request_id=?1",
                    [&id],
                    raw_operation,
                )
                .optional()
                .map_err(storage)?;
            if let Some(raw) = started {
                let record = decode_operation(raw)?;
                validate(&tx, &encode_id(record.current.operation.as_raw()))?;
                return Ok(StartTargetedDeletionOutcome::AlreadyCoveredBy(record.current));
            }
            let confirmed_at: Option<String> = tx
                .query_row(
                    "SELECT confirmed_at FROM deletion_confirmation WHERE request_id=?1",
                    [&id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            let staged: Option<RawRequest> = tx
                .query_row(
                    "SELECT request_id,purpose,exact_text,requested_at FROM deletion_request WHERE request_id=?1",
                    [&id],
                    raw_request,
                )
                .optional()
                .map_err(storage)?;
            let (Some(confirmed_at), Some(staged)) = (confirmed_at, staged) else {
                return Ok(StartTargetedDeletionOutcome::ConfirmationRequired);
            };
            let staged = decode_request(staged)?;
            // The confirmation row's timestamp is validated even though the
            // fact itself carries only the request identity.
            parse_time(&confirmed_at)?;
            let fact = OwnerConfirmationFact::from_durable(request);
            let Some(command) =
                staged.into_command(fact, WallClockWithTz::now(), required_participants)
            else {
                return Ok(StartTargetedDeletionOutcome::ConfirmationRequired);
            };
            let outcome = admit_deletion(&tx, &command, request)?;
            if matches!(outcome, StartTargetedDeletionOutcome::Started(_)) {
                tx.commit().map_err(storage)?;
            }
            Ok(outcome)
        })
        .await
    }

    async fn pending_targeted_deletions(
        &self,
        after: Option<DeletionRequestId>,
        limit: u32,
    ) -> Result<Vec<TargetedDeletionRequest>, PreservationTechnicalError> {
        check_page_limit(limit)?;
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let after = after.map(|id| encode_id(id.as_raw())).unwrap_or_default();
            let mut statement = tx
                .prepare(
                    "SELECT r.request_id,r.purpose,r.exact_text,r.requested_at
                     FROM deletion_request r
                     WHERE r.request_id>?1 AND NOT EXISTS
                         (SELECT 1 FROM deletion_confirmation c WHERE c.request_id=r.request_id)
                     ORDER BY r.request_id LIMIT ?2",
                )
                .map_err(storage)?;
            statement
                .query_map(params![after, limit], raw_request)
                .map_err(storage)?
                .map(|row| decode_request(row.map_err(storage)?))
                .collect()
        })
        .await
    }

    async fn deletion_status(
        &self,
        after: Option<DeletionOperationId>,
        limit: u32,
    ) -> Result<Vec<DeletionOperationRecord>, PreservationTechnicalError> {
        check_page_limit(limit)?;
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let after = after.map(|id| encode_id(id.as_raw())).unwrap_or_default();
            let ids = candidate_page(&tx, &after, limit, true)?;
            ids.into_iter()
                .map(|id| {
                    validate(&tx, &id)?;
                    decode_operation(load_operation(&tx, &id)?.ok_or_else(corrupt)?)
                })
                .collect()
        })
        .await
    }

    async fn deletion_surface_mark(
        &self,
    ) -> Result<DeletionSurfaceMark, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let torn: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM deletion_confirmation c
                         LEFT JOIN deletion_request r ON r.request_id=c.request_id
                         WHERE r.request_id IS NULL)",
                    (),
                    |row| row.get(0),
                )
                .map_err(storage)?;
            if torn {
                return Err(corrupt());
            }
            let (requests, newest_request): (i64, Option<String>) = tx
                .query_row(
                    "SELECT COUNT(*), (SELECT request_id FROM deletion_request ORDER BY rowid DESC LIMIT 1)
                     FROM deletion_request",
                    (),
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(storage)?;
            let (operations, newest_operation): (i64, Option<String>) = tx
                .query_row(
                    "SELECT COUNT(*), (SELECT operation_id||'/'||phase||'/'||sweep
                         FROM deletion_operation ORDER BY rowid DESC LIMIT 1)
                     FROM deletion_operation",
                    (),
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(storage)?;
            Ok(DeletionSurfaceMark::new(format!(
                "deletion-view/{requests}/{}/{operations}/{}",
                newest_request.as_deref().unwrap_or("-"),
                newest_operation.as_deref().unwrap_or("-")
            )))
        })
        .await
    }

    async fn change_deletion_lifecycle(
        &self,
        expected: DeletionOperationRef,
        change: DeletionLifecycleChange,
    ) -> Result<DeletionLifecycleOutcome, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(expected.operation.as_raw());
            validate(&tx, &id)?;
            let Some(raw) = load_operation(&tx, &id)? else {
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
                    // hold below — no writes, no generation advance — so this
                    // generic lifecycle call never moves a Held(Unavailable)
                    // operation back to Active; only an explicit Resume, or the
                    // §11 delayed-arrival sweep in
                    // note_host_transient_learning_arrival_sync, may.
                    if record.phase == DeletionOperationPhase::Held
                        && record.hold == Some(DeletionHoldReason::Unavailable)
                    {
                        return Ok(DeletionLifecycleOutcome::Held(
                            DeletionHoldReason::Unavailable,
                        ));
                    }
                    let sweep =
                        i64::try_from(expected.sweep.as_u64()).map_err(|_| corrupt())?;
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
                    let Some(next) = open_next_sweep(&tx, &id, sweep)? else {
                        tx.commit().map_err(storage)?;
                        return Ok(DeletionLifecycleOutcome::Held(
                            DeletionHoldReason::GenerationExhausted,
                        ));
                    };
                    current.sweep = next.sweep;
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
        check_page_limit(limit)?;
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let guard = lock_shared(&conn);
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let after = after.map(|id| encode_id(id.as_raw())).unwrap_or_default();
            let ids = candidate_page(&tx, &after, limit, false)?;
            ids.into_iter()
                .map(|id| {
                    validate(&tx, &id)?;
                    // An orphan condition candidate has no operation row:
                    // that is exactly the torn state `validate` refuses.
                    decode_operation(load_operation(&tx, &id)?.ok_or_else(corrupt)?)
                })
                .collect()
        })
        .await
    }

    async fn deletion_walk_cursor(
        &self,
        walk: DeletionWalk,
    ) -> Result<Option<DeletionOperationId>, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let guard = lock_shared(&conn);
            let after: Option<String> = guard
                .query_row(
                    "SELECT after_id FROM deletion_walk_cursor WHERE walk=?1",
                    [walk.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            match after {
                None => Ok(None),
                Some(text) => Ok(Some(DeletionOperationId::from_raw(
                    decode_id(&text).map_err(|_| corrupt())?,
                ))),
            }
        })
        .await
    }

    async fn set_deletion_walk_cursor(
        &self,
        walk: DeletionWalk,
        after: Option<DeletionOperationId>,
    ) -> Result<(), PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let mut guard = lock_shared(&conn);
            // The cursor is not completion state: it shares the single-writer
            // boundary so concurrent drivers cannot interleave a torn position,
            // and it never joins the operation/participant rows in a check.
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            match after {
                // A wrapped walk keeps no row: absence means "start at the
                // head", exactly like a fresh walk.
                None => {
                    tx.execute(
                        "DELETE FROM deletion_walk_cursor WHERE walk=?1",
                        [walk.as_str()],
                    )
                    .map_err(storage)?;
                }
                Some(operation) => {
                    tx.execute(
                        "INSERT INTO deletion_walk_cursor (walk,after_id) VALUES (?1,?2)
                         ON CONFLICT(walk) DO UPDATE SET after_id=excluded.after_id",
                        params![walk.as_str(), encode_id(operation.as_raw())],
                    )
                    .map_err(storage)?;
                }
            }
            tx.commit().map_err(storage)
        })
        .await
    }

    async fn current_erasure_conditions(
        &self,
        after: Option<DeletionOperationId>,
        limit: u32,
    ) -> Result<Vec<CurrentErasureCondition>, PreservationTechnicalError> {
        check_page_limit(limit)?;
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let after = after.map(|id| encode_id(id.as_raw())).unwrap_or_default();
            let ids = candidate_page(&tx, &after, limit, false)?;
            ids.iter()
                .map(|id| {
                    validate(&tx, id)?;
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
                        opened_at: parse_time(&opened)?,
                    })
                })
                .collect()
        })
        .await
    }

    async fn deletion_participants(
        &self,
        operation: DeletionOperationId,
        after: Option<ParticipantOwnerRef>,
        limit: u32,
    ) -> Result<Vec<DeletionParticipantRecord>, PreservationTechnicalError> {
        check_page_limit(limit)?;
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let guard = lock_shared(&conn);
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let id = encode_id(operation.as_raw());
            require_operation(&tx, &id)?;
            validate(&tx, &id)?;
            let after = after
                .map(ParticipantOwnerRef::storage_name)
                .unwrap_or_default();
            let mut statement = tx
                .prepare(
                    "SELECT participant_owner,state,sweep,hold_class,reported_at
                     FROM deletion_participant WHERE operation_id=?1 AND participant_owner>?2
                     ORDER BY participant_owner LIMIT ?3",
                )
                .map_err(storage)?;
            let rows: Vec<RawParticipant> = statement
                .query_map(params![id, after, limit], raw_participant)
                .map_err(storage)?
                .collect::<Result<_, _>>()
                .map_err(storage)?;
            rows.into_iter()
                .map(|raw| decode_participant(operation, raw))
                .collect()
        })
        .await
    }

    async fn deletion_operation_material(
        &self,
        operation: DeletionOperationId,
    ) -> Result<DeletionMaterialOutcome, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let guard = lock_shared(&conn);
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let id = encode_id(operation.as_raw());
            let Some((_sweep, phase)) = load_operation_state(&tx, &id)? else {
                return Ok(DeletionMaterialOutcome::Missing);
            };
            validate(&tx, &id)?;
            if phase == "completed" {
                return Ok(DeletionMaterialOutcome::Destroyed);
            }
            let Some(exact) = load_exact_target(&tx, &id)? else {
                // `validate` refuses an active or held operation without
                // material, so only a finalizing operation that already ran its
                // wipe reaches this branch (§12); a read must not resurrect it.
                return Ok(DeletionMaterialOutcome::Destroyed);
            };
            let mut statement = tx
                .prepare(
                    "SELECT material FROM deletion_semantic_hint WHERE operation_id=?1 ORDER BY ordinal",
                )
                .map_err(storage)?;
            let hints: Vec<DeletionSearchMaterial> = statement
                .query_map([&id], |r| r.get::<_, String>(0))
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?
                .into_iter()
                .map(DeletionSearchMaterial::new)
                .collect();
            Ok(DeletionMaterialOutcome::Material(
                DeletionOperationMaterial::new(TargetedDeletionTarget {
                    mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                        exact,
                    )),
                    semantic_hints: hints,
                }),
            ))
        })
        .await
    }

    async fn begin_participant_demand(
        &self,
        condition: ErasureConditionRef,
        participant: ParticipantOwnerRef,
    ) -> Result<ParticipantDemandOutcome, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(condition.operation.as_raw());
            validate(&tx, &id)?;
            let Some((sweep, phase)) = load_operation_state(&tx, &id)? else {
                return Ok(ParticipantDemandOutcome::Missing);
            };
            if phase == "completed" {
                return Ok(ParticipantDemandOutcome::Completed);
            }
            if sweep <= 0 || u64::try_from(sweep).ok() != Some(condition.sweep.as_u64()) {
                return Ok(ParticipantDemandOutcome::StaleSweep);
            }
            let stored: Option<String> = tx
                .query_row(
                    "SELECT state FROM deletion_participant WHERE operation_id=?1 AND participant_owner=?2",
                    params![id, participant.storage_name()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage)?;
            let Some(state) = stored else {
                return Ok(ParticipantDemandOutcome::NotRequired);
            };
            if state == "verified" {
                return Ok(ParticipantDemandOutcome::AlreadyVerified);
            }
            tx.execute(
                "UPDATE deletion_participant SET state='running',hold_class=NULL,reported_at=?3 WHERE operation_id=?1 AND participant_owner=?2",
                params![id, participant.storage_name(), WallClockWithTz::now().to_rfc3339()],
            )
            .map_err(storage)?;
            validate(&tx, &id)?;
            tx.commit().map_err(storage)?;
            Ok(ParticipantDemandOutcome::Marked(ParticipantProgress::Running {
                sweep: condition.sweep,
            }))
        })
        .await
    }

    async fn record_participant_completion(
        &self,
        fact: ParticipantCompletionFact,
    ) -> Result<ParticipantCompletionOutcome, PreservationTechnicalError> {
        let condition = fact.condition();
        let participant = fact.participant();
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(condition.operation.as_raw());
            validate(&tx, &id)?;
            let Some((sweep, phase)) = load_operation_state(&tx, &id)? else {
                return Ok(ParticipantCompletionOutcome::Missing);
            };
            if phase == "completed" {
                return Ok(ParticipantCompletionOutcome::Completed);
            }
            if sweep <= 0 || u64::try_from(sweep).ok() != Some(condition.sweep.as_u64()) {
                return Ok(ParticipantCompletionOutcome::StaleSweep);
            }
            let stored: Option<String> = tx
                .query_row(
                    "SELECT state FROM deletion_participant WHERE operation_id=?1 AND participant_owner=?2",
                    params![id, participant.storage_name()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage)?;
            let Some(state) = stored else {
                return Ok(ParticipantCompletionOutcome::NotRequired);
            };
            if state == "verified" {
                return Ok(
                    if fact.status() == ParticipantCompletionStatus::Verified {
                        ParticipantCompletionOutcome::Recorded(ParticipantProgress::Verified {
                            sweep: condition.sweep,
                        })
                    } else {
                        ParticipantCompletionOutcome::AlreadyVerified
                    },
                );
            }
            let (state_name, hold_name, progress) = match fact.status() {
                ParticipantCompletionStatus::MoreWork => (
                    "running",
                    None,
                    ParticipantProgress::Running {
                        sweep: condition.sweep,
                    },
                ),
                ParticipantCompletionStatus::LocalComplete => (
                    "local_complete",
                    None,
                    ParticipantProgress::LocalComplete {
                        sweep: condition.sweep,
                    },
                ),
                ParticipantCompletionStatus::Verified => (
                    "verified",
                    None,
                    ParticipantProgress::Verified {
                        sweep: condition.sweep,
                    },
                ),
                ParticipantCompletionStatus::Held(reason) => (
                    "held",
                    Some(reason.as_str()),
                    ParticipantProgress::Held {
                        sweep: condition.sweep,
                        reason,
                    },
                ),
            };
            if let ParticipantCompletionStatus::Held(_) = fact.status() {
                tx.execute(
                    "UPDATE deletion_participant SET state=?3,hold_class=?4,reported_at=?5 WHERE operation_id=?1 AND participant_owner=?2",
                    params![
                        id,
                        participant.storage_name(),
                        state_name,
                        hold_name,
                        fact.observed_at().to_rfc3339(),
                    ],
                )
                .map_err(storage)?;
            } else {
                tx.execute(
                    "UPDATE deletion_participant SET state=?3,sweep=?4,hold_class=?5,erased_count=?6,remainder_count=?7,reported_at=?8 WHERE operation_id=?1 AND participant_owner=?2",
                    params![
                        id,
                        participant.storage_name(),
                        state_name,
                        condition.sweep.as_u64() as i64,
                        hold_name,
                        i64::try_from(fact.erased_count()).map_err(|_| corrupt())?,
                        i64::try_from(fact.remainder_count()).map_err(|_| corrupt())?,
                        fact.observed_at().to_rfc3339(),
                    ],
                )
                .map_err(storage)?;
            }
            validate(&tx, &id)?;
            tx.commit().map_err(storage)?;
            Ok(ParticipantCompletionOutcome::Recorded(progress))
        })
        .await
    }

    async fn reconcile_deletion_sources(
        &self,
        expected: DeletionOperationRef,
        page_size: u32,
    ) -> Result<DeletionReconciliationOutcome, PreservationTechnicalError> {
        check_page_limit(page_size)?;
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(expected.operation.as_raw());
            validate(&tx, &id)?;
            let Some((sweep, phase)) = load_operation_state(&tx, &id)? else {
                return Ok(DeletionReconciliationOutcome::Missing);
            };
            let current = decode_ref(&id, sweep)?;
            if current != expected {
                return Ok(DeletionReconciliationOutcome::StaleSweep);
            }
            match phase.as_str() {
                "completed" => return Ok(DeletionReconciliationOutcome::Completed),
                "finalizing" => return Ok(DeletionReconciliationOutcome::Finalizing),
                "active" | "held" => {}
                _ => return Err(corrupt()),
            }
            if reconciliation_is_complete(&tx, &id, sweep)? {
                return Ok(DeletionReconciliationOutcome::Complete);
            }
            // `validate` refuses an active/held operation without protected
            // material, so the exact target is readable while the walk runs:
            // reconciliation happens before completion destroys it.
            let exact = load_exact_target(&tx, &id)?.ok_or_else(corrupt)?;
            let Some(identity) = next_incomplete_identity(&tx, &id, sweep)? else {
                return Err(corrupt());
            };
            let _table_complete =
                reconcile_table_page(&tx, &id, sweep, &exact, identity, page_size)?;
            validate(&tx, &id)?;
            tx.commit().map_err(storage)?;
            Ok(DeletionReconciliationOutcome::Advanced)
        })
        .await
    }

    async fn begin_deletion_finalizing(
        &self,
        expected: DeletionOperationRef,
    ) -> Result<DeletionFinalizationOutcome, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(expected.operation.as_raw());
            validate(&tx, &id)?;
            let row: Option<(i64, String, Option<String>)> = tx
                .query_row(
                    "SELECT sweep,phase,hold_reason FROM deletion_operation WHERE operation_id=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .map_err(storage)?;
            let Some((sweep, phase, hold)) = row else {
                return Ok(DeletionFinalizationOutcome::Missing);
            };
            let current = decode_ref(&id, sweep)?;
            if current != expected {
                return Ok(DeletionFinalizationOutcome::StaleSweep);
            }
            match phase.as_str() {
                "completed" => return Ok(DeletionFinalizationOutcome::CompletedAlready),
                "finalizing" => return Ok(DeletionFinalizationOutcome::Finalizing),
                "held" => {
                    let reason = hold
                        .as_deref()
                        .and_then(DeletionHoldReason::from_name)
                        .ok_or_else(corrupt)?;
                    return Ok(DeletionFinalizationOutcome::Held(reason));
                }
                "active" => {}
                _ => return Err(corrupt()),
            }
            let summary = completion_summary(&tx, &id, sweep)?;
            if !summary.all_verified() {
                return Ok(DeletionFinalizationOutcome::NotVerified(summary));
            }
            if !reconciliation_is_complete(&tx, &id, sweep)? {
                return Ok(DeletionFinalizationOutcome::ReconciliationIncomplete);
            }
            // `validate` refuses an active operation without protected
            // material, so the exact target is present while the sweep is
            // being verified.
            let exact = load_exact_target(&tx, &id)?.ok_or_else(corrupt)?;
            // §12 step 1 before entering Finalizing: a generation that still
            // has collected target data never finalizes. Returning to Active
            // happens before any material is destroyed, so the new sweep keeps
            // the material its participants need (§12).
            if system_remainder(&tx, &exact)? > 0 {
                let outcome = return_to_active_for_remainder(&tx, &id, sweep)?;
                tx.commit().map_err(storage)?;
                return Ok(outcome);
            }
            tx.execute(
                "UPDATE deletion_operation SET phase='finalizing',hold_reason=NULL WHERE operation_id=?1",
                [&id],
            )
            .map_err(storage)?;
            validate(&tx, &id)?;
            tx.commit().map_err(storage)?;
            Ok(DeletionFinalizationOutcome::Finalizing)
        })
        .await
    }

    async fn complete_deletion_finalizing(
        &self,
        expected: DeletionOperationRef,
    ) -> Result<DeletionFinalizationOutcome, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_deletion_blocking(self, move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(expected.operation.as_raw());
            validate(&tx, &id)?;
            let row: Option<(i64, String, String, String, Option<String>, i64)> = tx
                .query_row(
                    "SELECT sweep,phase,purpose,started_at,request_id,erased_total FROM deletion_operation WHERE operation_id=?1",
                    [&id],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage)?;
            let Some((sweep, phase, purpose, started_at, request_id, erased_total)) = row else {
                return Ok(DeletionFinalizationOutcome::Missing);
            };
            let current = decode_ref(&id, sweep)?;
            if current != expected {
                return Ok(DeletionFinalizationOutcome::StaleSweep);
            }
            match phase.as_str() {
                "completed" => return Ok(DeletionFinalizationOutcome::CompletedAlready),
                "finalizing" => {}
                "active" | "held" => return Ok(DeletionFinalizationOutcome::NotFinalizing),
                _ => return Err(corrupt()),
            }
            let purpose = decode_purpose(&purpose)?;
            let started_at = parse_time(&started_at)?;
            let erased_total = u64::try_from(erased_total).map_err(|_| corrupt())?;
            let Some(exact) = load_exact_target(&tx, &id)? else {
                // No readable target: the system-wide mechanical probe cannot
                // run, so completion fails closed instead of guessing the
                // target away (§12).
                return Ok(DeletionFinalizationOutcome::UnverifiableMaterial);
            };
            // §12 step 1, inside the commit transaction: the current
            // generation must not have grown a delayed arrival or remainder
            // after verification. A remainder returns the operation to
            // Active on a new sweep *without* destroying material, so a later
            // verification can never lack what it needs.
            if system_remainder(&tx, &exact)? > 0 {
                let outcome = return_to_active_for_remainder(&tx, &id, sweep)?;
                tx.commit().map_err(storage)?;
                return Ok(outcome);
            }
            tx.execute(
                "DELETE FROM deletion_search_material WHERE operation_id=?1",
                [&id],
            )
            .map_err(storage)?;
            tx.execute(
                "DELETE FROM deletion_semantic_hint WHERE operation_id=?1",
                [&id],
            )
            .map_err(storage)?;
            if let Some(request) = request_id.as_deref() {
                tx.execute(
                    "UPDATE deletion_request SET exact_text=NULL WHERE request_id=?1",
                    [request],
                )
                .map_err(storage)?;
            }
            tx.execute(
                "DELETE FROM erasure_condition_source WHERE operation_id=?1",
                [&id],
            )
            .map_err(storage)?;
            tx.execute(
                "DELETE FROM deletion_reconciliation WHERE operation_id=?1",
                [&id],
            )
            .map_err(storage)?;
            let recoverable: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM deletion_search_material WHERE operation_id=?1)
                         OR EXISTS(SELECT 1 FROM deletion_semantic_hint WHERE operation_id=?1)
                         OR EXISTS(SELECT 1 FROM erasure_condition_source WHERE operation_id=?1)
                         OR EXISTS(SELECT 1 FROM deletion_reconciliation WHERE operation_id=?1)
                         OR EXISTS(SELECT 1 FROM deletion_request r WHERE r.request_id=?2 AND r.exact_text IS NOT NULL)",
                    params![id, request_id],
                    |r| r.get(0),
                )
                .map_err(storage)?;
            if recoverable {
                return Err(corrupt());
            }
            let at = WallClockWithTz::now();
            let summary = completion_summary(&tx, &id, sweep)?;
            let final_erased: i64 = tx
                .query_row(
                    "SELECT COALESCE(SUM(erased_count),0) FROM deletion_participant WHERE operation_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .map_err(storage)?;
            let erased_count = erased_total.saturating_add(
                u64::try_from(final_erased).map_err(|_| corrupt())?,
            );
            tx.execute(
                "INSERT INTO deletion_completion_audit (operation_id,purpose,started_at,completed_at,sweep_count,participant_count,verified_count,erased_count) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    id,
                    purpose.as_str(),
                    started_at.to_rfc3339(),
                    at.to_rfc3339(),
                    sweep,
                    i64::try_from(summary.required).map_err(|_| corrupt())?,
                    i64::try_from(summary.verified).map_err(|_| corrupt())?,
                    i64::try_from(erased_count).map_err(|_| corrupt())?,
                ],
            )
            .map_err(storage)?;
            tx.execute(
                "INSERT INTO deletion_audit_participant (operation_id,participant_owner,final_state,erased_count)
                 SELECT operation_id,participant_owner,state,erased_count FROM deletion_participant WHERE operation_id=?1",
                [&id],
            )
            .map_err(storage)?;
            tx.execute(
                "UPDATE erasure_condition SET closed_at=?2 WHERE operation_id=?1 AND sweep=?3",
                params![id, at.to_rfc3339(), sweep],
            )
            .map_err(storage)?;
            tx.execute(
                "UPDATE deletion_operation SET phase='completed',hold_reason=NULL WHERE operation_id=?1",
                [&id],
            )
            .map_err(storage)?;
            validate(&tx, &id)?;
            tx.commit().map_err(storage)?;
            Ok(DeletionFinalizationOutcome::Completed)
        })
        .await
    }
}

#[cfg(feature = "test-support")]
impl Store {
    #[doc(hidden)]
    pub async fn wipe_protected_material_for_tests(
        &self,
        operation: DeletionOperationId,
    ) -> Result<(), PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(operation.as_raw());
            validate(&tx, &id)?;
            tx.execute(
                "UPDATE deletion_operation SET phase='finalizing' WHERE operation_id=?1",
                [&id],
            )
            .map_err(storage)?;
            tx.execute(
                "UPDATE deletion_participant SET state='verified',hold_class=NULL,remainder_count=0,reported_at=?2 WHERE operation_id=?1",
                params![id, WallClockWithTz::now().to_rfc3339()],
            )
            .map_err(storage)?;
            tx.execute(
                "DELETE FROM deletion_search_material WHERE operation_id=?1",
                [&id],
            )
            .map_err(storage)?;
            validate(&tx, &id)?;
            tx.commit().map_err(storage)?;
            Ok(())
        })
        .await
    }

    #[doc(hidden)]
    pub async fn hold_generation_exhausted_for_tests(
        &self,
        operation: DeletionOperationId,
    ) -> Result<(), PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(operation.as_raw());
            validate(&tx, &id)?;
            let sweep: i64 = tx
                .query_row(
                    "SELECT sweep FROM deletion_operation WHERE operation_id=?1",
                    [&id],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            tx.execute(
                "UPDATE erasure_condition SET sweep=?2 WHERE operation_id=?1 AND sweep=?3",
                params![id, i64::MAX, sweep],
            )
            .map_err(storage)?;
            tx.execute(
                "UPDATE erasure_condition_source SET sweep=?2 WHERE operation_id=?1 AND sweep=?3",
                params![id, i64::MAX, sweep],
            )
            .map_err(storage)?;
            tx.execute(
                "UPDATE deletion_participant SET sweep=?2 WHERE operation_id=?1 AND sweep=?3",
                params![id, i64::MAX, sweep],
            )
            .map_err(storage)?;
            tx.execute(
                "UPDATE deletion_reconciliation SET sweep=?2 WHERE operation_id=?1 AND sweep=?3",
                params![id, i64::MAX, sweep],
            )
            .map_err(storage)?;
            tx.execute(
                "UPDATE deletion_operation SET sweep=?2,phase='held',hold_reason='generation_exhausted' WHERE operation_id=?1",
                params![id, i64::MAX],
            )
            .map_err(storage)?;
            validate(&tx, &id)?;
            tx.commit().map_err(storage)?;
            Ok(())
        })
        .await
    }
}
