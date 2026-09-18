//! Canonical Group J admission and unfinished lifecycle. All mutations share
//! SQLite's Immediate writer boundary with AU14 and Task resume; no I/O or
//! participant erase runs under the transaction. Reads are SELECT-only and
//! bound their validation to the rows they actually return.

use std::sync::Arc;

use ene_preservation::*;
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, params_from_iter};

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

/// Indexed `(operation, sweep, source)` membership in the current sweep's
/// covered-source set.
///
/// Semantic owners match a candidate pin against this primary key instead of
/// receiving every covered identity in the demand command. One probe is one
/// indexed EXISTS; the caller never materializes the whole set.
pub(crate) fn source_is_covered(
    conn: &Connection,
    condition: ErasureConditionRef,
    source: RawId,
) -> rusqlite::Result<bool> {
    let Ok(sweep) = i64::try_from(condition.sweep.as_u64()) else {
        return Ok(false);
    };
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
}

/// One bounded, keyset-paged candidate page for both owner queries.
///
/// Candidates are unfinished operations plus torn orphan conditions with no
/// operation row at all: the union keeps the page bounded while an orphan
/// condition can never be omitted from a query result as a silent
/// authoritative empty set — it fails closed through `validate`.
///
/// Participant rows add no candidate class: they are read only for an
/// operation the caller already has (and their operation-side integrity is
/// enforced by `validate` on that operation), so an orphan participant row is
/// inert for the current-condition set rather than a silently missing
/// participant. `include_completed` widens the same union to terminal
/// operations for the status view; the orphan-condition half is unchanged, so
/// a torn condition still fails closed instead of dropping out of the page.
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
    let mut statement = conn
        .prepare(
            "SELECT identity_table,sweep,complete FROM deletion_reconciliation
             WHERE operation_id=?1",
        )
        .map_err(storage)?;
    let rows: Vec<(String, i64, i64)> = statement
        .query_map([operation], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    drop(statement);
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
    // The reconciliation cursor shape is part of the same structural
    // invariant: the SQL above keeps the operation/source/condition history
    // honest, and this keeps the exhaustive-walk premise from being torn.
    validate_reconciliation(conn, operation)
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
    // Fast skip: with no active/held operation mid-reconciliation, the
    // published correlation set is already the exhaustive one.
    let pending: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM deletion_reconciliation r
                 JOIN deletion_operation o ON o.operation_id=r.operation_id
                 WHERE r.complete=0 AND r.sweep=o.sweep AND o.phase IN ('active','held'))",
            (),
            |row| row.get(0),
        )
        .map_err(storage)?;
    if !pending {
        return Ok(None);
    }
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
    for (id, sweep) in candidates {
        validate(conn, &id)?;
        let target: Option<String> = conn
            .query_row(
                "SELECT exact_text FROM deletion_search_material WHERE operation_id=?1",
                [&id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        // Active/held operations always keep protected material (`validate`);
        // a missing row is torn state, never a "checked everything" default.
        let target = target.ok_or_else(corrupt)?;
        if source_identity_carries(conn, source, &target)? {
            return Ok(Some(decode_ref(&id, sweep)?.condition()));
        }
    }
    Ok(None)
}

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
///
/// The bounded publication is never the correctness set: while an operation's
/// current sweep is still being reconciled, an unpublished source identity is
/// checked directly ([`directly_covered_source`]), so a covered source cannot
/// escape enforcement just because it fell past a reconciliation page.
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

/// One mechanical text verdict against the canonical current conditions
/// (lifecycle §7/§11).
///
/// [`Self::Target`] carries the protected exact target that matched; it is
/// compared in place by the owning boundary and never rendered into a log, an
/// outcome, a completion fact, or an audit row. [`Self::Unreadable`] is a
/// current condition whose protected material was already wiped at finalizing:
/// the condition still covers, but no target remains to redact mechanically,
/// so callers fail closed (refuse; a collecting owner stores a body-free
/// marker) rather than treating an unreadable target as "not covering".
pub(crate) enum TextCoverage {
    Target(String),
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
/// — an exact substring match of a protected target. A current condition whose
/// protected material was already wiped (finalizing, §12 steps 2-3) leaves no
/// readable target at all, so the premise is unreadable and every body is
/// covered (fail closed) rather than served as uncovered.
pub(crate) struct TextCoveragePremise {
    /// `None` when a current condition's protected material is unreadable.
    targets: Option<Vec<String>>,
}

impl TextCoveragePremise {
    pub(crate) fn read(conn: &Connection) -> Result<Self, PreservationTechnicalError> {
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
                "SELECT operation_id FROM deletion_operation
                 WHERE phase!='completed' ORDER BY operation_id",
            )
            .map_err(storage)?;
        let ids: Vec<String> = statement
            .query_map((), |row| row.get(0))
            .map_err(storage)?
            .collect::<Result<_, _>>()
            .map_err(storage)?;
        drop(statement);
        let mut targets = Vec::with_capacity(ids.len());
        for id in ids {
            validate(conn, &id)?;
            let exact: Option<String> = conn
                .query_row(
                    "SELECT exact_text FROM deletion_search_material WHERE operation_id=?1",
                    [&id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            // The material row exists for every unfinished operation except a
            // finalizing one whose protected wipe already ran (§12 steps 2-3);
            // that operation's condition is still current, so no readable
            // comparison exists and the premise must cover every body.
            let Some(target) = exact else {
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

    /// Whether `text` carries a current condition's exact target. An
    /// unreadable premise covers every body.
    pub(crate) fn covers(&self, text: &str) -> bool {
        let Some(targets) = self.targets.as_ref() else {
            return true;
        };
        targets.iter().any(|target| text.contains(target.as_str()))
    }
}

/// Mechanical coverage of one incoming body by the canonical current erasure
/// conditions (lifecycle §7/§11).
///
/// This is the same predicate the A3 owner sweeps apply — an exact substring
/// match of each unfinished operation's protected mechanical target — read
/// from the canonical store inside the caller's transaction, so an empty
/// result across every current condition is the authoritative "not covered"
/// (no sentinel, no cached verdict).
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
        .cloned()
        .map(TextCoverage::Target))
}

/// The closure-aware mechanical text coverage with the covering condition
/// identity (lifecycle §7/§11).
///
/// Same predicate and bounds as [`covering_text`]; the identity is what the
/// observation receiving boundary needs to publish the covered occurrence as
/// a durable source correlation without copying the target.
pub(crate) fn covering_text_condition(
    conn: &Connection,
    text: &str,
) -> Result<Option<(ErasureConditionRef, TextCoverage)>, PreservationTechnicalError> {
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
    for (id, sweep) in ids {
        validate(conn, &id)?;
        let exact: Option<String> = conn
            .query_row(
                "SELECT exact_text FROM deletion_search_material WHERE operation_id=?1",
                [&id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let condition = decode_ref(&id, sweep)?.condition();
        // The material row exists for every unfinished operation except a
        // finalizing one whose protected wipe already ran (§12 steps 2-3);
        // that operation's condition is still current, so the body is
        // covered with no readable target.
        match exact {
            None => return Ok(Some((condition, TextCoverage::Unreadable))),
            Some(target) if !target.is_empty() && text.contains(&target) => {
                return Ok(Some((condition, TextCoverage::Target(target))));
            }
            Some(_) => {}
        }
    }
    Ok(None)
}

/// Source-correlation coverage of one logical input (lifecycle §7/§11): the
/// same closure-aware canonical read the inference claim and Task resume use,
/// applied across every source of one arrival.
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
/// protected target is no longer readable (a finalizing wipe) so no
/// mechanical comparison is possible at all.
///
/// This is the "erase collection" side of the A4 boundary contract for bodies
/// whose objective fact must survive (a Task result arrival seals its
/// delegation): the fact is committed, the body is not. The loop is bounded by
/// the number of current conditions — every pass removes at least one
/// condition's target, and a condition without material returns immediately.
pub(crate) fn redact_covered_text(
    conn: &Connection,
    text: &str,
) -> Result<String, PreservationTechnicalError> {
    let mut current = text.to_owned();
    loop {
        let Some(coverage) = covering_text(conn, &current)? else {
            return Ok(current);
        };
        let target = match coverage {
            TextCoverage::Target(target) => target,
            TextCoverage::Unreadable => {
                return Ok(String::from(crate::erasure::ERASED_MARKER));
            }
        };
        match crate::erasure::redact_exact(&current, &target) {
            Some((redacted, _)) => current = redacted,
            // `covering_text` matched the target, so a missing occurrence
            // would be an inconsistent mechanical predicate; fail closed
            // rather than storing a body that is still covered.
            None => return Err(corrupt()),
        }
    }
}

/// Whether one undelivered item's canonical source is under a current
/// erasure condition (lifecycle §7/§11), read inside the caller's
/// transaction.
///
/// The item's source identity is checked as a canonical source correlation,
/// and — when the source still carries a body — the body itself is compared
/// mechanically against every current condition's exact target. A source row
/// that no longer exists has no body left to re-materialize: it is not
/// covered here (the owner sweep removes the dangling reference). A malformed
/// stored source is unreadable canonical state and fails closed, never a
/// silent "not covered".
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
        // Body-free notification sources: there is no content to collect.
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
    let phase = match raw.2.as_str() {
        "active" => DeletionOperationPhase::Active,
        "held" => DeletionOperationPhase::Held,
        "finalizing" => DeletionOperationPhase::Finalizing,
        "completed" => DeletionOperationPhase::Completed,
        _ => return Err(corrupt()),
    };
    let purpose = decode_purpose(&raw.3)?;
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

type RawParticipant = (
    String,
    String,
    i64,
    Option<String>,
    i64,
    i64,
    Option<String>,
);

fn raw_participant(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawParticipant> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

/// One participant row is composed only when it is internally consistent: a
/// known owner and state name, a positive sweep, a hold class exactly when
/// held, non-negative counts, and a parseable report time. Anything else is
/// an unreadable row, never a guessed progress.
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
    let progress = match (raw.1.as_str(), hold) {
        ("pending", None) => ParticipantProgress::Pending,
        ("running", None) => ParticipantProgress::Running { sweep },
        ("local_complete", None) => ParticipantProgress::LocalComplete { sweep },
        ("verified", None) => ParticipantProgress::Verified { sweep },
        ("held", Some(reason)) => ParticipantProgress::Held { sweep, reason },
        _ => return Err(corrupt()),
    };
    let erased_count = u64::try_from(raw.4).map_err(|_| corrupt())?;
    let remainder_count = u64::try_from(raw.5).map_err(|_| corrupt())?;
    let reported_at = raw.6.as_deref().map(parse_time).transpose()?;
    Ok(DeletionParticipantRecord {
        participant: DeletionParticipantRef { operation, owner },
        progress,
        erased_count,
        remainder_count,
        reported_at,
    })
}

/// Storage vocabulary for one deletion purpose; unknown stored text fails
/// closed on decode.
fn decode_purpose(raw: &str) -> Result<DeletionPurpose, PreservationTechnicalError> {
    DeletionPurpose::from_name(raw).ok_or_else(corrupt)
}

type RawRequest = (String, String, Option<String>, String);

fn raw_request(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRequest> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

/// The staged journal holds the mechanical target only: semantic exploration
/// hints have no wire grammar in this slice, so a decoded request never
/// carries them and no Client can widen the confirmed search through staging.
///
/// A NULL `exact_text` is the A5 completion wipe: it is unreachable for a
/// request still awaiting a decision (only a completed operation's request is
/// wiped), so reading one as a pending request is unreadable stored state and
/// fails closed rather than fabricating an empty target.
fn decode_request(raw: RawRequest) -> Result<TargetedDeletionRequest, PreservationTechnicalError> {
    let Some(exact_text) = raw.2 else {
        return Err(corrupt());
    };
    if exact_text.is_empty() {
        return Err(corrupt());
    }
    Ok(TargetedDeletionRequest::from_durable(
        DeletionRequestId::from_raw(decode_id(&raw.0).map_err(|_| corrupt())?),
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                exact_text,
            )),
            semantic_hints: Vec::new(),
        },
        decode_purpose(&raw.1)?,
        parse_time(&raw.3)?,
    ))
}

fn decode_request_id(raw: &str) -> Result<DeletionRequestId, PreservationTechnicalError> {
    Ok(DeletionRequestId::from_raw(
        decode_id(raw).map_err(|_| corrupt())?,
    ))
}

/// One unfinished operation already holding the exact mechanical text, if
/// any. Completed operations never match: lifecycle §7 forbids treating a
/// finished deletion as a permanent keyword ban.
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
/// duplicate request: every known source correlation, every semantic hint, and
/// every required participant owner must already belong to it. Same mechanical
/// target is an idempotent request only if its scope is covered; a duplicate
/// never silently widens a confirmed operation. The participant snapshot is
/// part of the operation's scope, so a duplicate whose set needs an owner
/// outside the snapshot is a live-operation conflict, not a silent widening.
fn scope_covered(
    tx: &rusqlite::Transaction<'_>,
    current: DeletionOperationRef,
    sources: &[RawId],
    hints: &[DeletionSearchMaterial],
    participants: &[ParticipantOwnerRef],
) -> Result<bool, PreservationTechnicalError> {
    let mut covered = true;
    for source in sources {
        let found: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM erasure_condition_source WHERE operation_id=?1 AND sweep=?2 AND source=?3)",
                params![
                    encode_id(current.operation.as_raw()),
                    current.sweep.as_u64() as i64,
                    encode_id(*source)
                ],
                |r| r.get(0),
            )
            .map_err(storage)?;
        covered &= found;
    }
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

/// One durable identity table whose primary key can be named as a canonical
/// source correlation, with the column that can carry the exact target text.
#[derive(Debug, Clone, Copy)]
pub(crate) struct KnownSourceIdentity {
    pub(crate) table: &'static str,
    key: &'static str,
    body: &'static str,
}

/// The exact bounded page statement one reconciliation step runs for one
/// identity table.
///
/// Exposed so tests can `EXPLAIN QUERY PLAN` the production SQL. Table, key,
/// and body names are compile-time constants and never caller input; the
/// cursor, target text, and page size only ever travel as bound parameters.
///
/// The keyset predicate starts the scan at the durable cursor and `ORDER BY`
/// the primary key lets SQLite walk the key index (never a temp sort), so the
/// page reads at most `LIMIT` matching identities past the cursor; a page with
/// fewer matches than the limit means the ordered scan reached the table's
/// end. The substring predicate itself has no index, exactly like the A3
/// owner sweeps, so the walk is one table traversal in bounded pages — never
/// an unbounded single statement.
pub(crate) fn known_source_page_sql(identity: KnownSourceIdentity) -> String {
    format!(
        "SELECT {} FROM {} WHERE {} > ?1 AND instr({}, ?2) > 0 ORDER BY {} LIMIT ?3",
        identity.key, identity.table, identity.key, identity.body, identity.key
    )
}

/// The identity tables a first-party admission enumerates (lifecycle §4.1
/// point 4), each paired with the body column that can carry the exact target:
///
/// - `history_message.message_id`: the Owner-conversation origin of a
///   `task_context_entry` (a Task Agent `data_use` source), the accepted input
///   a dialogue reply derives from, the resume purpose source, an undelivered
///   source, and the History pin a `learning_summary` records.
/// - `activity_record.activity_id`: the Owner-management origin of a
///   `task_context_entry` and a resume instruction source.
/// - `learning_summary.summary_id` / `learning_memory.memory_id`: the derived
///   Learning identities (critical-areas §3.3). No inference producer claims
///   them in `data_use` yet (dialogue / learning sends carry no correlation in
///   this slice), but they are the canonical identities of derived rows whose
///   stored text carries the target, and the design keeps a known covered
///   source correlation durable from admission for the sends and writes that
///   derive from them.
/// - `action_attempt.attempt_id` / `task_result.result_id`: the Task Agent's
///   past-executed fact sources.
///
/// `task` / `task_revision` purpose bodies and `learning_memory_revision`
/// bodies are not identity tables here: no source correlation names them (a
/// Memory correlation names the Memory identity, not one revision), and their
/// redaction is the mechanical sweep's job.
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
/// exactly one row per known identity table.
///
/// The first-party path starts incomplete and publishes its first bounded
/// pages in the same admission transaction; the direct path names its whole
/// source scope itself, so its cursors commit already complete. A row set that
/// does not match the known table set is torn state and fails closed through
/// [`validate`].
fn insert_reconciliation_rows(
    tx: &rusqlite::Transaction<'_>,
    operation: &str,
    sweep: i64,
    complete: bool,
) -> Result<(), PreservationTechnicalError> {
    let complete = i64::from(complete);
    for identity in KNOWN_SOURCE_IDENTITIES {
        tx.execute(
            "INSERT INTO deletion_reconciliation (operation_id,sweep,identity_table,cursor,complete) VALUES (?1,?2,?3,'',?4)",
            params![operation, sweep, identity.table, complete],
        )
        .map_err(storage)?;
    }
    Ok(())
}

/// The next incomplete identity table for one operation + sweep, in the
/// canonical identity-table order, if any.
fn next_incomplete_identity(
    conn: &Connection,
    operation: &str,
    sweep: i64,
) -> Result<Option<KnownSourceIdentity>, PreservationTechnicalError> {
    let mut statement = conn
        .prepare(
            "SELECT identity_table FROM deletion_reconciliation
             WHERE operation_id=?1 AND sweep=?2 AND complete=0",
        )
        .map_err(storage)?;
    let incomplete: Vec<String> = statement
        .query_map(params![operation, sweep], |row| row.get(0))
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    Ok(KNOWN_SOURCE_IDENTITIES
        .iter()
        .copied()
        .find(|identity| incomplete.iter().any(|table| table == identity.table)))
}

/// Whether every known identity table has been walked to its end for one
/// operation's current sweep.
///
/// The answer is a read of the durable cursor rows, never a caller boolean.
/// A row set that names another table, another sweep, or a missing table is
/// not complete here and fails closed through [`validate`] on the operation.
fn reconciliation_is_complete(
    conn: &Connection,
    operation: &str,
    sweep: i64,
) -> Result<bool, PreservationTechnicalError> {
    let mut statement = conn
        .prepare(
            "SELECT identity_table,sweep,complete FROM deletion_reconciliation WHERE operation_id=?1",
        )
        .map_err(storage)?;
    let rows: Vec<(String, i64, i64)> = statement
        .query_map([operation], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    if rows.len() != KNOWN_SOURCE_IDENTITIES.len() {
        return Ok(false);
    }
    Ok(KNOWN_SOURCE_IDENTITIES.iter().all(|identity| {
        rows.iter().any(|(table, row_sweep, complete)| {
            table == identity.table && *row_sweep == sweep && *complete == 1
        })
    }))
}

/// One bounded reconciliation page for one identity table.
///
/// The page is read from the durable cursor, its covered identities are
/// published as current-sweep source correlations, the already-claimed uses
/// the page covers are associated, and the cursor (and, when the ordered scan
/// reached the table's end, the complete marker) advance — all in the caller's
/// transaction. Every write is a keyed `INSERT OR IGNORE` / idempotent
/// `UPDATE`, so re-running the same page after a rollback or on a retried
/// driver pass has no second semantic effect.
///
/// A page that found fewer identities than `page_size` proves the ordered scan
/// reached the table's end; a full page does not, so the next call reads the
/// next keyset range instead of assuming completion.
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

/// Publishes the first bounded page of every known identity table inside the
/// admission transaction (lifecycle §4.1 point 4).
///
/// This is the same bounded work shape as one owner sweep pass — one page per
/// table, in canonical order — not an exhaustive enumeration: the durable
/// cursors carry the continuation to
/// [`PreservationRepository::reconcile_deletion_sources`], and completion
/// refuses until they all report complete.
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

/// Associates the already-claimed uses one reconciliation page covers
/// (lifecycle §11 R2).
///
/// The page's new sources drive the join through the correlation index:
///
/// - an inference attempt whose ordered `data_use` names a page source;
/// - an unsealed task delegation under such an attempt;
/// - an unsealed task delegation whose business context source is a page
///   source (served by `idx_task_context_entry_origin_source`).
///
/// The target text is not needed here: the page has already decided which
/// identities carry it. Writes are `INSERT OR IGNORE` keyed by use identity,
/// so a retried page adds no second association.
fn associate_reconciled_page(
    tx: &rusqlite::Transaction<'_>,
    operation: &str,
    sources: &[String],
    held_at: &str,
) -> Result<(), PreservationTechnicalError> {
    if sources.is_empty() {
        return Ok(());
    }
    // Explicit positional numbering: `?1`/`?2` carry the operation and hold
    // time and the page sources take `?3..`; SQLite assigns a bare `?` the
    // next *unused* index after any explicit `?N`, so mixing forms would
    // silently shift the parameter count.
    let placeholders = (0..sources.len())
        .map(|index| format!("?{}", index + 3))
        .collect::<Vec<_>>()
        .join(",");
    // Operation and hold time are bound first, then the page sources.
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
    Ok(())
}

/// Closed work-kind vocabulary of [`erasure_use_hold`]: one claimed inference
/// attempt or one task delegation associated with an operation (an unsealed
/// execution whose delayed result must be collected, or a sealed execution
/// whose already-stored result body is observation-derived).
pub(crate) const USE_KIND_INFERENCE_ATTEMPT: &str = "inference_attempt";
pub(crate) const USE_KIND_TASK_DELEGATION: &str = "task_delegation";

/// Whether one source identity's stored body carries `target`.
///
/// This is the direct mechanical check the bounded publication must never
/// replace: the identity's primary key is an indexed lookup, and the body
/// comparison is the same exact-substring predicate the owner sweeps and the
/// acceptance boundaries use. It is how a source that has not reached a
/// reconciliation page yet is still recognized as covered.
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

/// Whether one inference attempt's ordered `data_use` names an identity whose
/// stored body carries `target`.
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

/// Whether one unsealed task delegation's durable premise carries `target`.
///
/// Mirrors the admission association mechanically, but without the published
/// source set: the relied revision / in-force purpose body, the delegated
/// workspace scope, the business context origin identities, and any attempt
/// already claimed under the delegation are each checked directly. A sealed
/// execution is excluded exactly like the admission association: its recorded
/// result cannot be produced again, and its stored body is the sweep's.
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

/// Direct mechanical correspondence of one already-claimed use with the
/// unfinished operations whose current-sweep reconciliation is still walking
/// (lifecycle §11 R2).
///
/// While a reconciliation sweep is incomplete, a covered source may not be
/// published yet, so the durable `erasure_use_hold` association may not exist
/// either. The claim's own premise is durable and indexed, so it is compared
/// directly against each unreconciled operation's protected target instead of
/// reading the bounded publication as the whole covered set. Finalizing
/// operations cannot be unreconciled (`validate` enforces the marker
/// invariant), so every candidate here has readable protected material.
fn directly_covered_use(
    conn: &Connection,
    use_kind: &str,
    use_id: RawId,
) -> Result<Option<String>, PreservationTechnicalError> {
    // Fast skip: with no active/held operation mid-reconciliation, every
    // covered use was already associated when its page committed.
    let pending: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM deletion_reconciliation r
                 JOIN deletion_operation o ON o.operation_id=r.operation_id
                 WHERE r.complete=0 AND r.sweep=o.sweep AND o.phase IN ('active','held'))",
            (),
            |row| row.get(0),
        )
        .map_err(storage)?;
    if !pending {
        return Ok(None);
    }
    let mut statement = conn
        .prepare(
            "SELECT o.operation_id FROM deletion_operation o
             WHERE o.phase IN ('active','held') AND EXISTS
                 (SELECT 1 FROM deletion_reconciliation r
                  WHERE r.operation_id=o.operation_id AND r.complete=0)
             ORDER BY o.operation_id",
        )
        .map_err(storage)?;
    let candidates: Vec<String> = statement
        .query_map((), |row| row.get(0))
        .map_err(storage)?
        .collect::<Result<_, _>>()
        .map_err(storage)?;
    drop(statement);
    for id in candidates {
        validate(conn, &id)?;
        let target: Option<String> = conn
            .query_row(
                "SELECT exact_text FROM deletion_search_material WHERE operation_id=?1",
                [&id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        // Active/held operations always keep protected material (`validate`);
        // a missing row is torn state, never a "checked everything" default.
        let target = target.ok_or_else(corrupt)?;
        let covered = match use_kind {
            USE_KIND_INFERENCE_ATTEMPT => {
                claimed_attempt_covers(conn, &encode_id(use_id), &target)?
            }
            USE_KIND_TASK_DELEGATION => {
                claimed_delegation_covers(conn, &encode_id(use_id), &target)?
            }
            _ => return Err(corrupt()),
        };
        if covered {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// Whether one already-claimed use was associated with a deletion operation
/// whose condition committed before the claim settles (lifecycle §11 R2).
///
/// The hold is the durable correspondence that a current-condition check
/// cannot provide after the operation completed: it names the claim, not the
/// target, so it refuses only that claim's delayed target-bearing body and
/// never becomes a keyword ban. Read inside the adopting boundary's own
/// transaction; a claim without a row is a genuine "not held" answer (no
/// sentinel, no cached verdict).
///
/// A missing row is not yet the answer while a sweep is still being
/// reconciled: the claim's own correlation is then checked directly against
/// the unreconciled operations' targets, and a match is written as the same
/// keyed `erasure_use_hold` row in the caller's transaction. That closes the
/// window between the condition commit and the end of the bounded
/// enumeration without ever consulting a page bound for correctness. The
/// direct check only considers unfinished operations, so a fresh origin after
/// completion is never held by a closed operation.
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
    if held {
        return Ok(true);
    }
    let Some(operation) = directly_covered_use(conn, use_kind, use_id)? else {
        return Ok(false);
    };
    conn.execute(
        "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at) VALUES (?1,?2,?3,?4)",
        params![
            use_kind,
            encode_id(use_id),
            operation,
            WallClockWithTz::now().to_rfc3339()
        ],
    )
    .map_err(storage)?;
    Ok(true)
}

impl Store {
    /// Whether one claimed inference attempt was associated with a deletion
    /// interval (lifecycle §11 R2).
    ///
    /// This is the durable-correspondence read the late-arrival boundaries use
    /// when no current condition can decide: the hold deliberately outlives
    /// completion, so a reply from a pre-deletion claim is still recognized as
    /// stale for erasure after `closed_at` is set. A read failure is a
    /// technical error; the caller refuses the delayed body (fail closed)
    /// rather than reading it as unheld.
    ///
    /// # Errors
    ///
    /// [`PreservationTechnicalError::StorageUnavailable`] when the hold table
    /// cannot be read.
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

    /// Synchronous [`Self::inference_claim_held`] for a publication predicate
    /// that must decide without awaiting (CCT §10.4). It reads the same
    /// statement; a read failure is a technical error the caller must fail
    /// closed on.
    ///
    /// # Errors
    ///
    /// [`PreservationTechnicalError::StorageUnavailable`] when the hold table
    /// cannot be read.
    pub fn inference_claim_held_sync(
        &self,
        claim: RawId,
    ) -> Result<bool, PreservationTechnicalError> {
        let guard = lock_shared(&self.conn);
        held_use(&guard, USE_KIND_INFERENCE_ATTEMPT, claim)
    }

    /// Whether one task delegation was associated with a deletion interval
    /// (lifecycle §11 R2).
    ///
    /// Same durable-correspondence read as [`Self::inference_claim_held`]: the
    /// hold outlives completion so a delayed observation body or result from a
    /// pre-deletion execution stays old-origin after `closed_at`. A read
    /// failure is a technical error the caller must fail closed on.
    ///
    /// # Errors
    ///
    /// [`PreservationTechnicalError::StorageUnavailable`] when the hold table
    /// cannot be read.
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

    /// Whether `condition` is the operation's current unfinished condition.
    ///
    /// Host-transient mutation is process memory, not this connection, so it
    /// cannot share the Immediate writer with the canonical row. This read is
    /// the same predicate durable participants re-check inside their erase
    /// transaction; a stale or unreadable answer must not mutate.
    ///
    /// # Errors
    ///
    /// [`PreservationTechnicalError::StorageUnavailable`] when the operation
    /// row cannot be read.
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

    /// Indexed membership of `candidates` in the current sweep's covered
    /// source set.
    ///
    /// Returns one flag per candidate, in candidate order. The statement is
    /// the `(operation, sweep, source)` primary key; this never loads the
    /// rest of the sweep. An unreadable condition is a technical error, not
    /// an empty set.
    pub async fn erasure_sources_covered(
        &self,
        condition: ErasureConditionRef,
        candidates: Vec<RawId>,
    ) -> Result<Vec<bool>, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let mut flags = Vec::with_capacity(candidates.len());
            for source in candidates {
                flags.push(source_is_covered(&guard, condition, source).map_err(storage)?);
            }
            Ok(flags)
        })
        .await
    }
}

/// The attempt-side hold enumeration of [`mark_inflight_uses`].
///
/// Exposed so tests can `EXPLAIN QUERY PLAN` the exact production statement:
/// the join must be driven from the operation's (bounded) covered sources
/// through `idx_inference_attempt_data_use_source`, never by scanning every
/// attempt's correlation rows.
pub(crate) const ASSOCIATE_ATTEMPTS_SQL: &str =
    "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
     SELECT ?3, a.ticket, ?1, ?4
     FROM erasure_condition_source s
     JOIN inference_attempt_data_use u ON u.source = s.source
     JOIN inference_attempt a ON a.ticket = u.ticket
     WHERE s.operation_id = ?1 AND s.sweep = ?2
     GROUP BY a.ticket";

/// Associates every already-claimed use whose durable provenance intersects
/// the operation's published source correlations with the operation
/// (lifecycle §11 R2).
///
/// Runs inside the admission transaction, after the operation's
/// `erasure_condition_source` rows exist. The same Immediate writer domain
/// serializes this against the claim transactions: a claim that committed
/// first is seen here and held; a claim that commits after sees the current
/// condition at its own gate instead (the `data_use` compare) and never
/// starts. The enumeration is mechanical:
///
/// - an inference attempt whose ordered `data_use` names a covered source
///   (driven from the bounded covered-source set through the correlation
///   index);
/// - an unsealed task delegation under an attempt held above, whose relied
///   revision or in-force purpose body still carries the target, whose
///   business context source is covered, or whose delegated workspace scope
///   carries the target. Purpose/context association still excludes sealed
///   executions: a recorded final result cannot be produced again;
/// - a task delegation that owns a covered observation occurrence, sealed or
///   not. The occurrence identity is the durable name of a discarded
///   observed body, so the already-stored result of a sealed execution is
///   still the sweep's derived personal data;
/// - an unsealed task delegation whose body-observing (`read`/`list`) Action
///   has started while the occurrence row is not yet durable. The in-memory
///   observation body is not a canonical source; associating the execution
///   here is what keeps that work "old origin" if Targeted Deletion runs
///   before the occurrence write.
///
/// The target text travels only as a bound parameter and is never copied into
/// a hold row.
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
    Ok(())
}

/// The observation-side delegation association of [`mark_inflight_uses`].
///
/// A body-observed occurrence whose source the admission survey found covered
/// was published into `erasure_condition_source` under its occurrence
/// identity; this statement turns that publication into the durable
/// `task_delegation` hold of the execution that replayed the occurrence.
/// Driven from the operation's bounded covered-source set through the
/// observation primary key, never by scanning the ledger. Sealed executions
/// are included: a recorded paraphrase of a discarded observation body is
/// still derived personal data, and the hold is the observation→delegation
/// correspondence the Task owner sweep uses to collect that already-stored
/// body without rewriting the execution seal.
pub(crate) const ASSOCIATE_OBSERVATION_DELEGATIONS_SQL: &str =
    "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
     SELECT ?3, o.delegation_id, ?1, ?4
     FROM erasure_condition_source s
     JOIN task_agent_observation o ON o.observation_id = s.source
     WHERE s.operation_id = ?1 AND s.sweep = ?2
     GROUP BY o.delegation_id";

/// Associates unsealed executions whose body-observing Action has started
/// but whose observation occurrence is not durable yet.
///
/// A `read`/`list` attempt is the producing work of a workspace-body
/// observation. Until the occurrence row exists, admission cannot survey that
/// discarded body, and a later write after the current condition closes would
/// otherwise look like a fresh origin. The hold names the execution, not the
/// text, and is the same correspondence the Task owner sweep and the
/// observation replay path already consult. Write confirmations are excluded:
/// they reproduce no workspace body.
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

/// Holds every delegation that owns at least one observation occurrence (the
/// fail-closed overflow fallback of [`survey_task_observation_sources`]).
///
/// When the bounded survey page cannot cover every occurrence, no occurrence
/// may be assumed clean: every execution that owns one is associated instead
/// of letting an unsurveyed delayed or already-sealed body through. The
/// statement scans the delegation table only on that overflow path; the
/// bounded path never runs it.
pub(crate) const HOLD_OBSERVING_DELEGATIONS_SQL: &str =
    "INSERT OR IGNORE INTO erasure_use_hold (use_kind,use_id,operation_id,held_at)
     SELECT ?3, d.delegation_id, ?1, ?4
     FROM delegation d
     WHERE EXISTS
         (SELECT 1 FROM task_agent_observation o WHERE o.delegation_id = d.delegation_id)";

/// The per-sweep bound on the observed occurrences one admission surveys.
///
/// The page bounds the SQL and the per-occurrence correlation checks one
/// admission performs. Overflow (more occurrences than the bound) fails
/// closed: every delegation that owns an occurrence is associated with the
/// operation instead of allowing an unsurveyed body through.
pub(crate) const TASK_OBSERVATION_SURVEY_LIMIT: u32 = 256;

/// One observation row read by the admission survey.
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

/// The SQL row columns of one surveyed observation, in select order, with the
/// joined delegation identity last so a torn ledger row is detectable.
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

/// The bounded survey page of one admission.
///
/// `rows` re-reads the operation's observation occurrences in canonical
/// identity order, including sealed executions: a recorded paraphrase of a
/// discarded observation body is still derived personal data. `overflow`
/// means more occurrences exist than [`TASK_OBSERVATION_SURVEY_LIMIT`] and
/// the caller must hold every delegation that owns one. An occurrence whose
/// delegation row is gone is torn ledger state and fails closed.
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

/// Whether one surveyed occurrence's source can carry the target.
///
/// The determination is mechanical and body-free on the ledger side:
///
/// - a refusal occurrence has no producing attempt and no source body
///   (the Action never started), so it carries nothing to survey;
/// - a producing attempt is resolved by identity from `action_attempt` and
///   its copied `(delegation, task, revision, workspace)` correlation must
///   agree with the occurrence row — a missing attempt or a disagreement is
///   unsurveyable and fails closed (covered);
/// - the attempt's resolved target is compared mechanically (the same
///   `instr`-style exact-text predicate the owner sweeps use);
/// - a body-observed occurrence reproduced workspace content at observation
///   time. The ledger stores no body and no content-version identity, and a
///   mutable workspace path is not that observed version: a later clean
///   read of the current path cannot prove the discarded body was unrelated
///   to the target, so the occurrence fails closed (covered).
///
/// A covered determination publishes the occurrence identity, never the
/// source body.
fn observation_source_covered(
    tx: &rusqlite::Transaction<'_>,
    observation: &SurveyedObservation,
    target: &str,
) -> Result<bool, PreservationTechnicalError> {
    let Some(attempt_text) = &observation.attempt else {
        // A refusal occurrence names no producing attempt: the Action never
        // started, so the occurrence's source is the fixed refusal class
        // itself and carries no workspace or internal body to survey. This is
        // the deliberately narrow edge of the fail-closed rule: only
        // occurrences whose source *can* carry a body (a producing attempt and
        // its workspace source) fail closed when they cannot be surveyed; a
        // fixed-class refusal has no such source. A body-observed occurrence
        // without a producing attempt is torn state and fails closed.
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
        // The occurrence names an attempt that is gone: the source cannot be
        // surveyed, so the occurrence is treated as covered (fail closed).
        return Ok(true);
    };
    if delegation != observation.delegation
        || task != observation.task
        || task_revision != observation.task_revision
        || Some(workspace) != observation.workspace
        || Some(real_target.clone()) != observation.path
    {
        // A correlation disagreement means the ledger row and the producing
        // attempt do not describe one source; fail closed rather than survey
        // the wrong source.
        return Ok(true);
    }
    if real_target.contains(target) {
        return Ok(true);
    }
    if !observation.body_observed {
        // A write confirmation or a refusal-class observation reproduces no
        // workspace content; the attempt target above is its only stored
        // source correlation.
        return Ok(false);
    }
    // A body-observed occurrence reproduced workspace content (`read` bytes
    // or a `list` listing). The current path contents are not a witness of
    // that observed version, and the ledger has no content-version identity
    // that could prove the discarded body was unrelated to the target. Fail
    // closed. A body-observed occurrence that names a write attempt is torn
    // ledger state and is covered by the same rule.
    Ok(true)
}

/// Publishes one covered occurrence identity as a durable source correlation
/// of the condition's current sweep.
///
/// The occurrence identity is not a body: it is the opaque name of the
/// observation occurrence, so a later claim that consumed it is held by the
/// existing AU14 source-coverage compare without any target text, hash, or
/// matcher being stored. The condition must still be current; a stale
/// condition is refused instead of being written.
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

/// Associates one delegation with the condition's operation durably
/// (`erasure_use_hold`, `task_delegation`).
///
/// The hold is objective metadata — the claim identity and the operation
/// identity only — and deliberately outlives completion so a delayed
/// target-bearing result from the execution is still collected after the
/// current condition closes. The condition must still be current.
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

/// Associates one body-observing execution with every unfinished operation.
///
/// AU5 has already inserted the `read`/`list` attempt: the Action started, so
/// refusing it would rewrite an objective start fact. While a current
/// condition exists the yet-unread (or already-read, not-yet-recorded) body
/// cannot be proven unrelated to the protected text without I/O this
/// transaction forbids, so the execution is held by identity. Lifecycle §11
/// collects delayed arrival onto the current operation for the whole
/// Active / Held / Finalizing interval: skipping `finalizing` would let a
/// read that started under a still-open condition look like a fresh origin
/// once the completion commit lands. This statement shares the Immediate
/// writer with the completion commit, so the two orders are exclusive: a
/// completion that commits first leaves no unfinished row and the start is
/// a post-closure origin; a start that commits first inserts the hold and
/// the delayed body stays old-origin. A store with no unfinished operation
/// is a no-op.
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

/// Every Client incarnation with durable uncleared body-delivery evidence,
/// read inside the admission transaction.
///
/// This is the authoritative required-incarnation read: the caller's snapshot
/// is a convenience, and the admission transaction must union the evidence it
/// can see itself so a delivery that committed between the caller's read and
/// this transaction is never omitted. A corrupt stored identity fails closed
/// instead of dropping an incarnation from the snapshot.
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

/// The canonical admission body: duplicate detection plus the
/// durable-before-enforce insert of operation, protected material, initial
/// condition, and source correlations (lifecycle §4.1).
///
/// `request` records Host-local request provenance. It is [`None`] for the
/// sealed-confirmation path and `Some` for a durably confirmed staged request;
/// `deletion_operation.request_id` is `UNIQUE`, so one request can never start
/// two operations. Both public admission entries share this body: there is one
/// set of insert, duplicate, and validation rules, never a second producer.
///
/// The first-party path (`request` is `Some`) additionally enumerates the
/// covered source correlations already durable in this store and publishes
/// them with the operation (§4.1 point 4). The sealed direct path keeps its
/// caller-provided `known_sources` unchanged, so a test seam that names its
/// sources explicitly is never widened behind its back.
fn admit_deletion(
    tx: &rusqlite::Transaction<'_>,
    command: &StartTargetedDeletionCommand,
    request: Option<DeletionRequestId>,
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
    // The caller's Client-incarnation list is a pre-transaction evidence read
    // and is therefore not authoritative: a body handed over between that read
    // and this transaction would be omitted from the durable snapshot, and its
    // local copy could later read as erased. The admission transaction reads
    // the same durable evidence itself and unions it here, so a delivery that
    // committed first is always snapshotted; one that commits after the
    // admission serialization sees the now-current condition at its own
    // boundary instead (lifecycle §8/§8.1).
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
            command.known_sources(),
            &command.target().semantic_hints,
            &participants,
        )?;
        return Ok(if covered {
            StartTargetedDeletionOutcome::AlreadyCoveredBy(record.current)
        } else {
            StartTargetedDeletionOutcome::HeldByOperation(record.current)
        });
    }
    // Stage 6 A4: every pre-condition observation occurrence is surveyed
    // mechanically against the exact target in this same transaction,
    // including sealed executions. A covered occurrence is published under
    // its own identity and associates its execution with the operation; a
    // body-observed mutable workspace source, a missing attempt, or a
    // bounded-overflow page fails closed by producing a hold instead of
    // allowing a delayed or already-sealed derived body through.
    let (covered_observations, observation_overflow) =
        survey_task_observation_sources(tx, material.expose_for_erasure())?;
    let current = DeletionOperationRef {
        operation: DeletionOperationId::from_raw(RawId::new()),
        sweep: DeletionSweepGeneration::from_u64(1),
    };
    let id = encode_id(current.operation.as_raw());
    let at = command.requested_at().to_rfc3339();
    let request_text = request.map(|request| encode_id(request.as_raw()));
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
    // §4.1 point 4: the first-party path initializes the durable reconciliation
    // cursors and publishes the first bounded page of every known identity
    // table in this same transaction as the operation, the protected material,
    // and the initial condition. The page is a work bound, not a correctness
    // bound: the durable cursors carry the continuation, and completion
    // refuses while any table is still incomplete. The direct path has no
    // enumeration to do — its caller-provided sources are its whole scope — so
    // its cursors commit already complete.
    let enumerates = request.is_some();
    insert_reconciliation_rows(tx, &id, 1, !enumerates)?;
    if enumerates {
        reconcile_admission_pages(
            tx,
            &id,
            1,
            material.expose_for_erasure(),
            DELETION_RECONCILIATION_PAGE_SIZE,
        )?;
    }
    for source in command.known_sources() {
        tx.execute(
            "INSERT OR IGNORE INTO erasure_condition_source (operation_id,sweep,source) VALUES (?1,1,?2)",
            params![id, encode_id(*source)],
        )
        .map_err(storage)?;
    }
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
    // Already-claimed uses whose provenance this operation covers are
    // associated with the operation in the same transaction that publishes
    // the condition (§4.1/§11 R2): the correspondence is durable before the
    // condition enforces, and it outlives completion so a delayed result can
    // still be recognized as stale for erasure.
    mark_inflight_uses(tx, &id, 1, material.expose_for_erasure(), &at)?;
    // The bounded survey page could not cover every occurrence: no
    // unsurveyed occurrence is assumed clean, and every delegation that owns
    // one is associated instead.
    if observation_overflow {
        tx.execute(
            HOLD_OBSERVING_DELEGATIONS_SQL,
            params![id, 1, USE_KIND_TASK_DELEGATION, at],
        )
        .map_err(storage)?;
    }
    // The required participant snapshot commits with the operation and the
    // condition: no participant effect can start before the operation that
    // needs it is durable (durable-before-enforce §4.1).
    for owner in &participants {
        tx.execute(
            "INSERT INTO deletion_participant (operation_id,participant_owner,state,sweep,erased_count,remainder_count) VALUES (?1,?2,'pending',1,0,0)",
            params![id, owner.storage_name()],
        )
        .map_err(storage)?;
    }
    // The admission commit may not publish a structurally impossible
    // operation; the post-insert check keeps the producer honest even against
    // future code that assembles rows differently.
    validate(tx, &id)?;
    Ok(StartTargetedDeletionOutcome::Started(current))
}

/// Reads the durable participant aggregate of one operation (§10).
///
/// Bounded by the operation's own snapshot rows; the operation's current
/// sweep joins the aggregate so a summary can never be returned for a
/// generation the operation has already left.
fn completion_summary(
    conn: &Connection,
    operation: &str,
    sweep: i64,
) -> Result<DeletionCompletionSummary, PreservationTechnicalError> {
    let sweep = u64::try_from(sweep).map_err(|_| corrupt())?;
    if sweep == 0 {
        return Err(corrupt());
    }
    let (required, verified, local_complete, in_progress, held): (i64, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(state='verified'),0),
                    COALESCE(SUM(state='local_complete'),0),
                    COALESCE(SUM(state IN ('pending','running')),0),
                    COALESCE(SUM(state='held'),0)
             FROM deletion_participant WHERE operation_id=?1 AND sweep=?2",
            params![operation, i64::try_from(sweep).map_err(|_| corrupt())?],
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
        .map_err(storage)?;
    let count = |value: i64| u64::try_from(value).map_err(|_| corrupt());
    Ok(DeletionCompletionSummary {
        operation: DeletionOperationId::from_raw(decode_id(operation).map_err(|_| corrupt())?),
        sweep: DeletionSweepGeneration::from_u64(sweep),
        required: count(required)?,
        verified: count(verified)?,
        local_complete: count(local_complete)?,
        in_progress: count(in_progress)?,
        held: count(held)?,
    })
}

/// Opens the next sweep generation for one unfinished operation in place (§6).
///
/// The closing sweep's erased counts are accumulated into the operation's
/// lifetime total first, the current condition's cumulative source
/// correlations are copied forward and the old sweep's source rows deleted in
/// the same transaction (a crash cannot publish a current sweep that silently
/// drops coverage), and every required participant resets to `Pending` for the
/// new generation — erasure or verification done for the old sweep never
/// counts for the new one.
///
/// The reconciliation cursors reset with the generation: the delayed arrivals
/// that opened this sweep can carry identities the previous walk had already
/// passed, so the exhaustive walk starts over for the current sweep. Deleting
/// the old rows and inserting the fresh incomplete ones shares this
/// transaction, so a crash can never leave a completeness marker attached to
/// an unwalked generation.
///
/// Returns [`None`] when the generation space is exhausted, after marking the
/// operation `Held(GenerationExhausted)`: an old generation is never reused.
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
    insert_reconciliation_rows(tx, id, next, false)?;
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

/// The system-wide mechanical remainder verification (§12/§18).
///
/// Wraps the closed canonical content-surface probe: `0` means a complete walk
/// found no stored value carrying the target; any other value is collected
/// target data the completion boundary must not destroy material for.
fn system_remainder(
    tx: &rusqlite::Transaction<'_>,
    target: &str,
) -> Result<u64, PreservationTechnicalError> {
    crate::erasure::system_remainder(tx, target).map_err(storage)
}

/// The §12 step-1 look: a `Finalizing` operation whose current generation
/// collected new target data must return to `Active` on a new sweep *before*
/// any protected material is destroyed, so a later verification always has the
/// material it needs.
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
            let outcome = admit_deletion(&tx, &command, None)?;
            if matches!(outcome, StartTargetedDeletionOutcome::Started(_)) {
                tx.commit().map_err(storage)?;
            }
            Ok(outcome)
        })
        .await
    }

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
                let covered = scope_covered(
                    &tx,
                    record.current,
                    &[],
                    &command.target().semantic_hints,
                    &[],
                )?;
                return Ok(if covered {
                    StageTargetedDeletionRequestOutcome::AlreadyCoveredBy(record.current)
                } else {
                    StageTargetedDeletionRequestOutcome::HeldByOperation(record.current)
                });
            }
            // The same scope (mechanical text plus declared purpose) reuses its
            // staged row: a duplicate intent never mints a second request, and
            // a confirmed one reports that the canonical admission is owed.
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
        // One durable determination: the confirmation row is written (or
        // observed) under the single-writer boundary before any admission runs.
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
            // The confirmation row was just written (or observed) for this
            // request, so its absence in the admission transaction is canonical
            // corruption, never a retryable state.
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
            // Single use: a request that already produced an operation never
            // produces a second one, whatever the operation's phase.
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
            // Durable premises, re-read in the admission transaction: the
            // Owner confirmation row and the staged scope row. Neither is a
            // caller input, so no wire payload can start an operation.
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
            let fact =
                OwnerConfirmationFact::from_durable(request, parse_time(&confirmed_at)?);
            let Some(command) =
                staged.into_command(fact, WallClockWithTz::now(), required_participants)
            else {
                return Ok(StartTargetedDeletionOutcome::ConfirmationRequired);
            };
            let outcome = admit_deletion(&tx, &command, Some(request))?;
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
        if !(1..=100).contains(&limit) {
            return Err(PreservationTechnicalError::InvalidLimit);
        }
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
        if !(1..=100).contains(&limit) {
            return Err(PreservationTechnicalError::InvalidLimit);
        }
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let after = after.map(|id| encode_id(id.as_raw())).unwrap_or_default();
            let ids = candidate_page(&tx, &after, limit, true)?;
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
                        .ok_or_else(corrupt)?;
                    decode_operation(raw)
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
            // A confirmation row without its request is torn canonical state:
            // fail closed instead of publishing a surface mark over it.
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
            // Durable row ids use random UUIDs, so "newest" is insertion order
            // (`rowid`), never a lexical maximum. The newest operation carries
            // its phase and sweep so a lifecycle advance moves the mark.
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
                    // Cumulative source coverage is inherited by copying the
                    // current sweep forward; the old sweep's source rows are
                    // then deleted in the same transaction so an unfinished
                    // operation keeps source correlations only in its current
                    // sweep. Historical erasure_condition rows remain as
                    // lifecycle/history. The copy-then-delete order with a
                    // single commit keeps a crash from publishing a current
                    // sweep that silently drops coverage. A new generation
                    // re-opens every participant: erasure or verification done
                    // for the old sweep never counts for the new one (§6). The
                    // owner set is unchanged — the snapshot is fixed at
                    // admission — and only progress resets.
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
        if !(1..=100).contains(&limit) {
            return Err(PreservationTechnicalError::InvalidLimit);
        }
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            // A read transaction gives validation and the bounded page the same snapshot.
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let after = after.map(|id| encode_id(id.as_raw())).unwrap_or_default();
            let ids = candidate_page(&tx, &after, limit, false)?;
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
            let ids = candidate_page(&tx, &after, limit, false)?;
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

    async fn deletion_participants(
        &self,
        operation: DeletionOperationId,
        after: Option<ParticipantOwnerRef>,
        limit: u32,
    ) -> Result<Vec<DeletionParticipantRecord>, PreservationTechnicalError> {
        if !(1..=100).contains(&limit) {
            return Err(PreservationTechnicalError::InvalidLimit);
        }
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            // A read transaction gives validation and the bounded page the same snapshot.
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let id = encode_id(operation.as_raw());
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM deletion_operation WHERE operation_id=?1)",
                    [&id],
                    |r| r.get(0),
                )
                .map_err(storage)?;
            if !exists {
                return Err(PreservationTechnicalError::UnknownOperation);
            }
            validate(&tx, &id)?;
            let after = after.map(ParticipantOwnerRef::storage_name).unwrap_or_default();
            let mut statement = tx
                .prepare(
                    "SELECT participant_owner,state,sweep,hold_class,erased_count,remainder_count,reported_at
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
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let id = encode_id(operation.as_raw());
            let row: Option<(i64, String)> = tx
                .query_row(
                    "SELECT sweep,phase FROM deletion_operation WHERE operation_id=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            let Some((_sweep, phase)) = row else {
                return Ok(DeletionMaterialOutcome::Missing);
            };
            validate(&tx, &id)?;
            if phase == "completed" {
                // Completion wipes the material before the completed commit
                // (§3.1); a protected read cannot resurrect it.
                return Ok(DeletionMaterialOutcome::Destroyed);
            }
            let material_exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM deletion_search_material WHERE operation_id=?1)",
                    [&id],
                    |r| r.get(0),
                )
                .map_err(storage)?;
            if !material_exists {
                // `validate` refuses an active or held operation without
                // material, so only a finalizing operation that already ran its
                // wipe reaches this branch (§12); a read must not resurrect it.
                return Ok(DeletionMaterialOutcome::Destroyed);
            }
            let exact: Option<String> = tx
                .query_row(
                    "SELECT exact_text FROM deletion_search_material WHERE operation_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage)?;
            let exact = exact.ok_or_else(corrupt)?;
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
            // Covered-source identities stay in the canonical
            // `(operation, sweep, source)` primary key. Materializing them
            // here would allocate in proportion to the whole sweep before
            // any participant ran, which is the work the page-sized
            // reconciliation walk already bounded (§9). Semantic owners
            // probe membership per candidate instead.
            Ok(DeletionMaterialOutcome::Material(
                DeletionOperationMaterial::new(
                    TargetedDeletionTarget {
                        mechanical: MechanicalDeletionTarget::ExactText(
                            DeletionSearchMaterial::new(exact),
                        ),
                        semantic_hints: hints,
                    },
                    Vec::new(),
                ),
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
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(condition.operation.as_raw());
            validate(&tx, &id)?;
            let row: Option<(i64, String)> = tx
                .query_row(
                    "SELECT sweep,phase FROM deletion_operation WHERE operation_id=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            let Some((sweep, phase)) = row else {
                return Ok(ParticipantDemandOutcome::Missing);
            };
            if phase == "completed" {
                return Ok(ParticipantDemandOutcome::Completed);
            }
            // Only the current generation accepts a demand. An older command
            // must not reopen a participant the new sweep already reset.
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
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(condition.operation.as_raw());
            validate(&tx, &id)?;
            let row: Option<(i64, String)> = tx
                .query_row(
                    "SELECT sweep,phase FROM deletion_operation WHERE operation_id=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            let Some((sweep, phase)) = row else {
                return Ok(ParticipantCompletionOutcome::Missing);
            };
            if phase == "completed" {
                return Ok(ParticipantCompletionOutcome::Completed);
            }
            // Stale generations never update current state (§6/§9.1), and the
            // fact's own condition is the generation it was minted against.
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
                // Verification is terminal for the sweep: an idempotent repeat
                // is recorded, a later downgrading report cannot reopen it.
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
                // A held report carries no usable counts, so the last reported
                // counts stay durable instead of being erased to zero.
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

    async fn deletion_completion_summary(
        &self,
        operation: DeletionOperationId,
    ) -> Result<DeletionCompletionSummary, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let id = encode_id(operation.as_raw());
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM deletion_operation WHERE operation_id=?1)",
                    [&id],
                    |r| r.get(0),
                )
                .map_err(storage)?;
            if !exists {
                return Err(PreservationTechnicalError::UnknownOperation);
            }
            validate(&tx, &id)?;
            let sweep: i64 = tx
                .query_row(
                    "SELECT sweep FROM deletion_operation WHERE operation_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .map_err(storage)?;
            completion_summary(&tx, &id, sweep)
        })
        .await
    }

    async fn reconcile_deletion_sources(
        &self,
        expected: DeletionOperationRef,
        page_size: u32,
    ) -> Result<DeletionReconciliationOutcome, PreservationTechnicalError> {
        if !(1..=100).contains(&page_size) {
            return Err(PreservationTechnicalError::InvalidLimit);
        }
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let mut guard = lock_shared(&conn);
            let tx = guard
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let id = encode_id(expected.operation.as_raw());
            validate(&tx, &id)?;
            let row: Option<(i64, String)> = tx
                .query_row(
                    "SELECT sweep,phase FROM deletion_operation WHERE operation_id=?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            let Some((sweep, phase)) = row else {
                return Ok(DeletionReconciliationOutcome::Missing);
            };
            let current = decode_ref(&id, sweep)?;
            if current != expected {
                return Ok(DeletionReconciliationOutcome::StaleSweep);
            }
            match phase.as_str() {
                "completed" => return Ok(DeletionReconciliationOutcome::Completed),
                // The finalizing marker implies a complete walk by invariant;
                // a step on it has nothing to publish.
                "finalizing" => return Ok(DeletionReconciliationOutcome::Finalizing),
                "active" | "held" => {}
                _ => return Err(corrupt()),
            }
            if reconciliation_is_complete(&tx, &id, sweep)? {
                return Ok(DeletionReconciliationOutcome::Complete);
            }
            let exact: Option<String> = tx
                .query_row(
                    "SELECT exact_text FROM deletion_search_material WHERE operation_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage)?;
            // `validate` refuses an active/held operation without protected
            // material, so the exact target is readable while the walk runs:
            // reconciliation happens before completion destroys it.
            let exact = exact.ok_or_else(corrupt)?;
            let Some(identity) = next_incomplete_identity(&tx, &id, sweep)? else {
                return Err(corrupt());
            };
            // A page was published: report the work, not a completion inferred
            // from the page shape. The next call observes `Complete` (or the
            // next page) from the durable cursors, so a caller never reads
            // "one full page" as "the walk is over".
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
        run_blocking(move || {
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
                // The durable finalizing marker: the completion commit is
                // owed, and repeating the begin step changes nothing.
                "finalizing" => return Ok(DeletionFinalizationOutcome::Finalizing),
                // A hold is a retryable-incomplete decision that is never
                // turned into a completion: the explicit resume decides.
                "held" => {
                    let reason = match hold.as_deref() {
                        Some("unavailable") => DeletionHoldReason::Unavailable,
                        Some("generation_exhausted") => DeletionHoldReason::GenerationExhausted,
                        _ => return Err(corrupt()),
                    };
                    return Ok(DeletionFinalizationOutcome::Held(reason));
                }
                "active" => {}
                _ => return Err(corrupt()),
            }
            // The completion premise is the durable aggregate of the required
            // snapshot for *this* sweep. A caller cannot substitute one local
            // completion, one successful transaction, a Client ACK, or an LLM
            // self-report for it (§10).
            let summary = completion_summary(&tx, &id, sweep)?;
            if !summary.all_verified() {
                return Ok(DeletionFinalizationOutcome::NotVerified(summary));
            }
            // The second completion premise is the current sweep's exhaustive
            // covered-source reconciliation: while any known identity table is
            // still being walked, the already-claimed in-flight uses are not
            // all durably associated, so no completion candidate may be
            // entered (§4.1 point 4, §18). Fail closed with the explicit
            // outcome; a page bound never decides completion.
            if !reconciliation_is_complete(&tx, &id, sweep)? {
                return Ok(DeletionFinalizationOutcome::ReconciliationIncomplete);
            }
            // `validate` refuses an active operation without protected
            // material, so the exact target is present while the sweep is
            // being verified.
            let exact: String = tx
                .query_row(
                    "SELECT exact_text FROM deletion_search_material WHERE operation_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage)?
                .ok_or_else(corrupt)?;
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
        run_blocking(move || {
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
                // Only the durable Finalizing marker allows the completion
                // commit; an active or held operation has unfinished
                // participant work or an explicit recovery decision owed.
                "active" | "held" => return Ok(DeletionFinalizationOutcome::NotFinalizing),
                _ => return Err(corrupt()),
            }
            let purpose = decode_purpose(&purpose)?;
            let started_at = parse_time(&started_at)?;
            let erased_total = u64::try_from(erased_total).map_err(|_| corrupt())?;
            let exact_row: Option<String> = tx
                .query_row(
                    "SELECT exact_text FROM deletion_search_material WHERE operation_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage)?;
            let Some(exact) = exact_row else {
                // No readable target: the system-wide mechanical probe cannot
                // run, so completion fails closed instead of guessing the
                // target away (§12).
                return Ok(DeletionFinalizationOutcome::UnverifiableMaterial);
            };
            // The completion commit re-reads the exhaustive-walk premise even
            // though the finalizing marker implies it: the completion premise
            // is never taken from the marker alone.
            if !reconciliation_is_complete(&tx, &id, sweep)? {
                return Ok(DeletionFinalizationOutcome::ReconciliationIncomplete);
            }
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
            // §12 step 2: destroy the operation-lifetime target/search
            // material, its semantic hints, its staged request's exact text,
            // and every source correlation. Every store connection raises
            // `secure_delete` at open, so deleted cells are zeroed instead of
            // being left recoverable in freed pages.
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
            // The reconciliation cursors are operation-lifetime correlation
            // state and share the completion wipe; the completed-operation
            // invariant keeps zero rows.
            tx.execute(
                "DELETE FROM deletion_reconciliation WHERE operation_id=?1",
                [&id],
            )
            .map_err(storage)?;
            // §12 step 3: the canonical rows must no longer be able to
            // reconstruct the target.
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
            // §12 step 4: the body-free audit. `validate` already refused a
            // finalizing operation with a non-verified participant, so the
            // snapshot copied here is verified-only by invariant.
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
            // §12 step 5: close the current condition. Steps 2-6 share this
            // one commit, so a crash can never observe a closed condition with
            // an unfinished operation.
            tx.execute(
                "UPDATE erasure_condition SET closed_at=?2 WHERE operation_id=?1 AND sweep=?3",
                params![id, at.to_rfc3339(), sweep],
            )
            .map_err(storage)?;
            // §12 step 6: the terminal commit.
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

    async fn deletion_completion_audit(
        &self,
        operation: DeletionOperationId,
    ) -> Result<Option<DeletionCompletionAudit>, PreservationTechnicalError> {
        let conn = Arc::clone(&self.conn);
        run_blocking(move || {
            let guard = lock_shared(&conn);
            let tx = guard.unchecked_transaction().map_err(storage)?;
            let id = encode_id(operation.as_raw());
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM deletion_operation WHERE operation_id=?1)",
                    [&id],
                    |r| r.get(0),
                )
                .map_err(storage)?;
            if !exists {
                return Err(PreservationTechnicalError::UnknownOperation);
            }
            validate(&tx, &id)?;
            let row: Option<(String, String, String, i64, i64)> = tx
                .query_row(
                    "SELECT purpose,started_at,completed_at,sweep_count,erased_count FROM deletion_completion_audit WHERE operation_id=?1",
                    [&id],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage)?;
            let Some((purpose, started_at, completed_at, sweep_count, erased_count)) = row else {
                return Ok(None);
            };
            let mut statement = tx
                .prepare(
                    "SELECT participant_owner,final_state,erased_count FROM deletion_audit_participant WHERE operation_id=?1 ORDER BY participant_owner",
                )
                .map_err(storage)?;
            let rows: Vec<(String, String, i64)> = statement
                .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(storage)?
                .collect::<Result<_, _>>()
                .map_err(storage)?;
            let participants = rows
                .into_iter()
                .map(|(owner, state, erased)| {
                    if state != "verified" {
                        return Err(corrupt());
                    }
                    Ok(DeletionAuditParticipant {
                        owner: ParticipantOwnerRef::from_storage_name(&owner)
                            .ok_or_else(corrupt)?,
                        status: DeletionAuditStatus::Verified,
                        erased_count: u64::try_from(erased).map_err(|_| corrupt())?,
                    })
                })
                .collect::<Result<Vec<_>, PreservationTechnicalError>>()?;
            Ok(Some(DeletionCompletionAudit {
                operation,
                purpose: decode_purpose(&purpose)?,
                started_at: parse_time(&started_at)?,
                completed_at: parse_time(&completed_at)?,
                sweep_count: u64::try_from(sweep_count).map_err(|_| corrupt())?,
                erased_count: u64::try_from(erased_count).map_err(|_| corrupt())?,
                participants,
            }))
        })
        .await
    }
}

#[cfg(feature = "test-support")]
impl Store {
    /// Test-support only: advances one operation to a `finalizing` state with
    /// every participant verified and its protected material wiped, so a test
    /// can exercise the unreadable-target fail-closed path of the acceptance
    /// boundaries and of the presentation coverage premise.
    ///
    /// Production never reaches this shape: the completion commit wipes
    /// material, writes the audit, closes the condition, and commits
    /// `completed` in one transaction, so a readable condition always has its
    /// material while it is unfinished. A5 completion on this seam fails
    /// closed with `UnverifiableMaterial` instead of guessing.
    ///
    /// # Errors
    ///
    /// [`PreservationTechnicalError`] when the operation is unknown or torn.
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
            // The post-write check keeps the seam from publishing a shape the
            // production validation would refuse.
            validate(&tx, &id)?;
            tx.commit().map_err(storage)?;
            Ok(())
        })
        .await
    }

    /// Test-support only: forces one unfinished operation into the durable
    /// `Held(GenerationExhausted)` shape.
    ///
    /// Production reaches this shape exactly when the sweep counter cannot
    /// advance (`sweep.checked_add(1)` overflows), so the fixture moves the
    /// operation, its current condition, its source correlations, its
    /// reconciliation cursors, and its participant rows to the maximum sweep
    /// together and records the hold class. The canonical store then refuses
    /// every lifecycle change for it (`Resume` included), which is what
    /// fail-closed recovery must observe.
    ///
    /// # Errors
    ///
    /// [`PreservationTechnicalError`] when the operation is unknown or torn.
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
