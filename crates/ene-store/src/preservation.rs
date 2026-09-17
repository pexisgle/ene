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
/// completed operation has every row `verified` for that sweep. A participant
/// row for an unknown owner, a foreign sweep, or a missing snapshot is torn
/// canonical state and fails closed, never a silently incomplete set.
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
         OR (o.phase='completed' AND EXISTS
             (SELECT 1 FROM deletion_participant p WHERE p.operation_id=o.operation_id AND p.state != 'verified'))))
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

type RawRequest = (String, String, String, String);

fn raw_request(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRequest> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

/// The staged journal holds the mechanical target only: semantic exploration
/// hints have no wire grammar in this slice, so a decoded request never
/// carries them and no Client can widen the confirmed search through staging.
fn decode_request(raw: RawRequest) -> Result<TargetedDeletionRequest, PreservationTechnicalError> {
    if raw.2.is_empty() {
        return Err(corrupt());
    }
    Ok(TargetedDeletionRequest::from_durable(
        DeletionRequestId::from_raw(decode_id(&raw.0).map_err(|_| corrupt())?),
        TargetedDeletionTarget {
            mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(raw.2)),
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

/// The canonical admission body: duplicate detection plus the
/// durable-before-enforce insert of operation, protected material, initial
/// condition, and source correlations (lifecycle §4.1).
///
/// `request` records Host-local request provenance. It is [`None`] for the
/// sealed-confirmation path and `Some` for a durably confirmed staged request;
/// `deletion_operation.request_id` is `UNIQUE`, so one request can never start
/// two operations. Both public admission entries share this body: there is one
/// set of insert, duplicate, and validation rules, never a second producer.
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
    let MechanicalDeletionTarget::ExactText(material) = &command.target().mechanical;
    if let Some(record) = unfinished_by_exact_text(tx, material.expose_for_erasure())? {
        let covered = scope_covered(
            tx,
            record.current,
            command.known_sources(),
            &command.target().semantic_hints,
            command.required_participants(),
        )?;
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
    for source in command.known_sources() {
        tx.execute(
            "INSERT OR IGNORE INTO erasure_condition_source (operation_id,sweep,source) VALUES (?1,1,?2)",
            params![id, encode_id(*source)],
        )
        .map_err(storage)?;
    }
    // The required participant snapshot commits with the operation and the
    // condition: no participant effect can start before the operation that
    // needs it is durable (durable-before-enforce §4.1).
    for owner in command.required_participants() {
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
                    // A new generation re-opens every participant: erasure or
                    // verification done for the old sweep never counts for the
                    // new one (§6). The owner set is unchanged — the snapshot
                    // is fixed at admission — and only progress resets.
                    tx.execute(
                        "UPDATE deletion_participant SET state='pending',sweep=?2,hold_class=NULL,erased_count=0,remainder_count=0,reported_at=NULL WHERE operation_id=?1",
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
            let Some((sweep, phase)) = row else {
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
            let mut statement = tx
                .prepare(
                    "SELECT source FROM erasure_condition_source WHERE operation_id=?1 AND sweep=?2 ORDER BY source",
                )
                .map_err(storage)?;
            let sources: Vec<RawId> = statement
                .query_map(params![id, sweep], |r| r.get::<_, String>(0))
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?
                .into_iter()
                .map(|text| decode_id(&text).map_err(|_| corrupt()))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(DeletionMaterialOutcome::Material(
                DeletionOperationMaterial::new(
                    TargetedDeletionTarget {
                        mechanical: MechanicalDeletionTarget::ExactText(
                            DeletionSearchMaterial::new(exact),
                        ),
                        semantic_hints: hints,
                    },
                    sources,
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
}
