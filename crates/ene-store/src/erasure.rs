//! Local-erasure participants for the Companion and Learning owners
//! (Targeted Deletion lifecycle §9–§10).
//!
//! Each implementation owns exactly its own durable rows and never updates
//! another domain's tables:
//!
//! - [`CompanionErasureParticipant`] sweeps the content columns of
//!   `history_message` and `activity_record` with the operation's exact
//!   mechanical target, and removes `undelivered` reporting references whose
//!   canonical source is gone (the reference carries no body; the source-side
//!   erasure governs the referenced content).
//! - [`LearningErasureParticipant`] sweeps `learning_summary`,
//!   `learning_memory`, and `learning_memory_revision` content, the derived
//!   `learning_memory_term` token index, and follows the recorded
//!   correspondence: a revision whose evidence Summary no longer exists is
//!   erased, and a Memory whose current revision is erased goes with its
//!   history and tokens, so derived text can never remain as the only copy of
//!   the erased evidence.
//!
//! The mechanical layer is mandatory and LLM-independent: matching is exact
//! substring / token equality, never a model judgment. Work is bounded per
//! demand ([`ERASURE_SCAN_ROWS`] scanned rows), the continuation cursor is
//! owned by the participant, and `(operation, sweep, participant)` is
//! idempotent: re-scanning the same range after a crash deletes nothing new
//! because the target rows are already gone. A demand for a different
//! condition starts a fresh sweep, so a completion is never mixed across
//! generations, and a lost cursor (restart) simply restarts the sweep from its
//! head — never from a remembered position that could skip rows.
//!
//! The History→Summary correlation is the recorded correspondence only: the
//! Summary's own source pins. A pin is correlated when the operation's
//! covered sources name it (the durable link the Host publishes with the
//! sweep) or when the pinned message still carries the target. Reading the
//! History row for that check is a read-only cross-owner lookup; the Learning
//! participant never writes another owner's rows. A Summary that paraphrases
//! the target without an exact occurrence and without a recorded covered
//! source is not mechanically reachable in this slice — no LLM and no
//! guessed correlation is used to reach it.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef,
};
use ene_primitive::{RawId, WallClockWithTz};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use crate::codec::{
    SOURCE_KIND_ACTIVITY_RECORD, SOURCE_KIND_HISTORY_MESSAGE, encode_id, lock_shared,
};
use crate::{Store, run_blocking};

/// Rows one bounded demand scans per owner before reporting more work.
///
/// The bound is on scanned rows, not matched rows: a table without a match is
/// still walked one page at a time, so no demand performs an unbounded scan
/// and the remainder check is a bounded traversal rather than one query that
/// reads the whole table.
pub const ERASURE_SCAN_ROWS: u32 = 64;

/// Content columns of the Companion owner. Shared with the test-support
/// remainder probe so a test never checks a different column set than the
/// sweep covers.
pub(crate) const COMPANION_CONTENT: &[(&str, &str)] =
    &[("history_message", "body"), ("activity_record", "body")];

/// Content columns of the Learning owner, plus the derived token index whose
/// membership is matched by token equality (not substring).
pub(crate) const LEARNING_CONTENT: &[(&str, &str)] = &[
    ("learning_summary", "content"),
    ("learning_memory", "content"),
    ("learning_memory_revision", "content"),
];

const HISTORY_MESSAGE_KEY: &str = "message_id";
const ACTIVITY_RECORD_KEY: &str = "activity_id";
const UNDELIVERED_KEY: &str = "undelivered_id";
const SUMMARY_KEY: &str = "summary_id";
const MEMORY_KEY: &str = "memory_id";
const REVISION_KEY: &str = "revision";
pub(crate) const TERM_TABLE: &str = "learning_memory_term";

/// Which stage of one sweep the participant is in. `Verify` re-walks exactly
/// the same rows as `Erase`; only a completed `Verify` walk with zero matches
/// produces a verified fact (§10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SweepPhase {
    Erase,
    Verify,
}

/// Participant-owned continuation state for one sweep (§9).
///
/// Keyed by the condition it belongs to. A demand for another
/// `(operation, sweep)` starts a fresh cursor, so an older generation's
/// position never advances a newer sweep and a completion is never mixed
/// across generations. The cursor is deliberately not durable: a restart
/// restarts the sweep from its head, which is safe because every erase action
/// is idempotent and the durable rows carry no cursor state to corrupt.
#[derive(Debug, Clone)]
struct SweepCursor {
    condition: ErasureConditionRef,
    phase: SweepPhase,
    /// Index into the owner's table order; one past the end means the walk
    /// finished its current phase.
    table: usize,
    /// Last scanned primary-key component in the current table. The empty
    /// string is the smallest stored identity text, so it is the head.
    after: Option<String>,
    /// Second primary-key component for the composite revision key (the
    /// revision number), and the rowid keyset for the derived token table.
    after_ordinal: i64,
    /// Rows erased so far in this sweep, including cascaded rows.
    erased: u64,
    /// Rows a verification page found before returning to erasing.
    remainder: u64,
    /// The verify walk completed with zero matches for this sweep.
    verified: bool,
}

impl SweepCursor {
    fn fresh(condition: ErasureConditionRef) -> Self {
        Self {
            condition,
            phase: SweepPhase::Erase,
            table: 0,
            after: None,
            after_ordinal: 0,
            erased: 0,
            remainder: 0,
            verified: false,
        }
    }

    fn reset_position(&mut self) {
        self.after = None;
        self.after_ordinal = 0;
    }

    fn begin_verify(&mut self) {
        self.phase = SweepPhase::Verify;
        self.table = 0;
        self.remainder = 0;
        self.reset_position();
    }

    fn fact(&self, owner: ParticipantOwnerRef, at: WallClockWithTz) -> ParticipantCompletionFact {
        if self.verified {
            ParticipantCompletionFact::verified(self.condition, owner, self.erased, at)
        } else {
            ParticipantCompletionFact::more_work(
                self.condition,
                owner,
                self.erased,
                self.remainder,
                at,
            )
        }
    }
}

/// One scanned page of one owner table.
#[derive(Debug, Default)]
struct PageOutcome {
    /// Rows the page walked.
    scanned: u32,
    /// Rows the mechanical predicate matched.
    matched: u64,
    /// Rows actually deleted (zero on a verification page).
    deleted: u64,
    /// Last scanned key and its ordinal component, when the page was not
    /// empty.
    last: Option<(String, i64)>,
}

/// One bounded page request within a sweep.
struct PageRequest<'a> {
    /// Exact mechanical target.
    target: &'a str,
    /// Last scanned key of the table; the empty string is the head.
    after: &'a str,
    /// Second primary-key component (revision number or rowid).
    after_ordinal: i64,
    /// Rows the page may scan.
    limit: i64,
    /// `true` erases the page's matches; `false` is the verification walk.
    delete: bool,
}

/// The step function one owner applies to a bounded budget of scanned rows.
type LocalStep = fn(
    tx: &Transaction<'_>,
    cursor: &mut SweepCursor,
    target: &str,
    covered: &[RawId],
) -> Result<(), rusqlite::Error>;

fn lock_cursor(slot: &Mutex<Option<SweepCursor>>) -> MutexGuard<'_, Option<SweepCursor>> {
    match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Protected exact-text material of one demand, or [`None`] when the demand
/// carries no usable mechanical target (a correlation-only scope or an empty
/// text). A local owner can never erase mechanically without it.
fn exact_text(command: &DemandLocalErasureCommand) -> Option<String> {
    let target = command.scope().target()?;
    let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
    let text = material.expose_for_erasure();
    if text.is_empty() {
        return None;
    }
    Some(text.to_owned())
}

/// Runs one bounded demand inside a single `Immediate` transaction: either
/// the whole page of erase actions commits, or none of them does, so a
/// re-driven demand repeats only idempotent work. The in-memory cursor is
/// updated only after the commit: a rollback leaves the continuation where it
/// was, and a crash that loses it restarts the sweep from the head.
fn run_local_demand(
    store: &Store,
    sweep: &Mutex<Option<SweepCursor>>,
    condition: ErasureConditionRef,
    owner: ParticipantOwnerRef,
    target: &str,
    covered: &[RawId],
    step: LocalStep,
) -> Result<ParticipantCompletionFact, rusqlite::Error> {
    let mut guard = lock_shared(&store.conn);
    let tx = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Deleted cells are zeroed instead of being left recoverable in freed
    // pages; the flag is connection-scoped and only ever raised on this path.
    tx.execute_batch("PRAGMA secure_delete = ON")?;
    let mut cursor = {
        let slot = lock_cursor(sweep);
        match slot.as_ref() {
            Some(existing) if existing.condition == condition => existing.clone(),
            _ => SweepCursor::fresh(condition),
        }
    };
    let at = WallClockWithTz::now();
    step(&tx, &mut cursor, target, covered)?;
    tx.commit()?;
    let fact = cursor.fact(owner, at);
    *lock_cursor(sweep) = Some(cursor);
    Ok(fact)
}

/// Deletes the rows named by `keys` from one table inside the caller's
/// transaction. `table` / `column` are compile-time constants; the identities
/// travel only as bound parameters.
fn delete_keys(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    keys: &[String],
) -> Result<u64, rusqlite::Error> {
    if keys.is_empty() {
        return Ok(0);
    }
    let placeholders = std::iter::repeat_n("?", keys.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("DELETE FROM {table} WHERE {column} IN ({placeholders})");
    let mut statement = tx.prepare(&sql)?;
    let deleted = statement.execute(rusqlite::params_from_iter(keys.iter()))?;
    Ok(u64::try_from(deleted).unwrap_or(u64::MAX))
}

fn count_keys(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    keys: &[String],
) -> Result<u64, rusqlite::Error> {
    if keys.is_empty() {
        return Ok(0);
    }
    let placeholders = std::iter::repeat_n("?", keys.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} IN ({placeholders})");
    let counted: i64 = {
        let mut statement = tx.prepare(&sql)?;
        statement.query_row(rusqlite::params_from_iter(keys.iter()), |row| row.get(0))?
    };
    Ok(u64::try_from(counted).unwrap_or(u64::MAX))
}

/// One exact-content page over a single-key table: walks `key > after` in key
/// order, matches `instr(content, target) > 0`, and deletes the matches.
fn exact_page(
    tx: &Transaction<'_>,
    table: &str,
    key_column: &str,
    content_column: &str,
    request: &PageRequest<'_>,
) -> Result<PageOutcome, rusqlite::Error> {
    let sql = format!(
        "SELECT {key_column}, instr({content_column}, ?2) > 0 FROM {table} \
         WHERE {key_column} > ?1 ORDER BY {key_column} LIMIT ?3"
    );
    let (keys, scanned, last) = {
        let mut statement = tx.prepare(&sql)?;
        let mut rows = statement.query(params![request.after, request.target, request.limit])?;
        let mut keys: Vec<String> = Vec::new();
        let mut scanned = 0u32;
        let mut last: Option<(String, i64)> = None;
        while let Some(row) = rows.next()? {
            scanned += 1;
            let key: String = row.get(0)?;
            let hit: bool = row.get(1)?;
            if hit {
                keys.push(key.clone());
            }
            last = Some((key, 0));
        }
        (keys, scanned, last)
    };
    let deleted = if request.delete {
        delete_keys(tx, table, key_column, &keys)?
    } else {
        0
    };
    Ok(PageOutcome {
        scanned,
        matched: u64::try_from(keys.len()).unwrap_or(u64::MAX),
        deleted,
        last,
    })
}

/// One page of the `undelivered` reference table: a row matches when its
/// canonical History or activity source no longer exists. The row itself
/// carries no body, so the source side's erasure governs the content and only
/// the dangling reference is removed here.
fn undelivered_page(
    tx: &Transaction<'_>,
    request: &PageRequest<'_>,
) -> Result<PageOutcome, rusqlite::Error> {
    let sql = format!(
        "SELECT u.{UNDELIVERED_KEY}, \
         ((u.source_kind = ?2 AND NOT EXISTS \
             (SELECT 1 FROM history_message h WHERE h.{HISTORY_MESSAGE_KEY} = u.source_id)) \
          OR (u.source_kind = ?3 AND NOT EXISTS \
             (SELECT 1 FROM activity_record a WHERE a.{ACTIVITY_RECORD_KEY} = u.source_id))) \
         FROM undelivered u WHERE u.{UNDELIVERED_KEY} > ?1 ORDER BY u.{UNDELIVERED_KEY} LIMIT ?4"
    );
    let (keys, scanned, last) = {
        let mut statement = tx.prepare(&sql)?;
        let mut rows = statement.query(params![
            request.after,
            SOURCE_KIND_HISTORY_MESSAGE,
            SOURCE_KIND_ACTIVITY_RECORD,
            request.limit
        ])?;
        let mut keys: Vec<String> = Vec::new();
        let mut scanned = 0u32;
        let mut last: Option<(String, i64)> = None;
        while let Some(row) = rows.next()? {
            scanned += 1;
            let key: String = row.get(0)?;
            let hit: bool = row.get(1)?;
            if hit {
                keys.push(key.clone());
            }
            last = Some((key, 0));
        }
        (keys, scanned, last)
    };
    let deleted = if request.delete {
        delete_keys(tx, "undelivered", UNDELIVERED_KEY, &keys)?
    } else {
        0
    };
    Ok(PageOutcome {
        scanned,
        matched: u64::try_from(keys.len()).unwrap_or(u64::MAX),
        deleted,
        last,
    })
}

/// Whether one History pin names a message whose body carries the exact
/// target. A pin that no longer resolves is not proof of a target (normal
/// retention and this sweep both remove rows); the exact-content match stays
/// the authoritative remainder condition.
fn pin_target_bearing(
    tx: &Transaction<'_>,
    pin: &str,
    target: &str,
    memo: &mut HashMap<String, bool>,
) -> Result<bool, rusqlite::Error> {
    if let Some(hit) = memo.get(pin) {
        return Ok(*hit);
    }
    let hit: Option<bool> = tx
        .query_row(
            "SELECT instr(body, ?2) > 0 FROM history_message WHERE message_id = ?1",
            params![pin, target],
            |row| row.get(0),
        )
        .optional()?;
    let hit = hit.unwrap_or(false);
    memo.insert(pin.to_owned(), hit);
    Ok(hit)
}

/// Counts the mechanical exact-text remainder over every durable content
/// column the Companion and Learning sweeps cover, the derived token index,
/// and the undelivered references whose canonical source is gone.
///
/// The column list is the participants' shared definition (see
/// [`COMPANION_CONTENT`] / [`LEARNING_CONTENT`]), so a test probe can never
/// check a different column set than the sweep covers. The exact text travels
/// only as a bound parameter.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn exact_remainder_probe(
    conn: &rusqlite::Connection,
    text: &str,
) -> Result<u64, rusqlite::Error> {
    let mut sql = String::from("SELECT 0");
    for (table, column) in COMPANION_CONTENT.iter().chain(LEARNING_CONTENT) {
        sql.push_str(&format!(
            " + (SELECT COUNT(*) FROM {table} WHERE instr({column}, ?1) > 0)"
        ));
    }
    sql.push_str(&format!(
        " + (SELECT COUNT(*) FROM {TERM_TABLE} WHERE term = ?1)"
    ));
    sql.push_str(&format!(
        " + (SELECT COUNT(*) FROM undelivered u WHERE \
           (u.source_kind = '{SOURCE_KIND_HISTORY_MESSAGE}' AND NOT EXISTS \
               (SELECT 1 FROM history_message h WHERE h.message_id = u.source_id)) \
           OR (u.source_kind = '{SOURCE_KIND_ACTIVITY_RECORD}' AND NOT EXISTS \
               (SELECT 1 FROM activity_record a WHERE a.activity_id = u.source_id)))"
    ));
    let counted: i64 = conn.query_row(&sql, params![text], |row| row.get(0))?;
    Ok(u64::try_from(counted).unwrap_or(u64::MAX))
}

// --- Companion owner -------------------------------------------------------

/// Tables of the Companion sweep, in order: History bodies, activity-record
/// bodies, then the undelivered references that name them.
const COMPANION_TABLES: usize = 3;

fn companion_step(
    tx: &Transaction<'_>,
    cursor: &mut SweepCursor,
    target: &str,
    _covered: &[RawId],
) -> Result<(), rusqlite::Error> {
    let mut budget = ERASURE_SCAN_ROWS;
    while budget > 0 {
        if cursor.verified {
            break;
        }
        if cursor.table >= COMPANION_TABLES {
            if cursor.phase == SweepPhase::Erase {
                cursor.begin_verify();
                continue;
            }
            cursor.verified = true;
            break;
        }
        let limit = i64::from(budget);
        let after = cursor.after.clone().unwrap_or_default();
        let erasing = cursor.phase == SweepPhase::Erase;
        let request = PageRequest {
            target,
            after: &after,
            after_ordinal: 0,
            limit,
            delete: erasing,
        };
        let outcome = match cursor.table {
            0 => exact_page(
                tx,
                COMPANION_CONTENT[0].0,
                HISTORY_MESSAGE_KEY,
                COMPANION_CONTENT[0].1,
                &request,
            )?,
            1 => exact_page(
                tx,
                COMPANION_CONTENT[1].0,
                ACTIVITY_RECORD_KEY,
                COMPANION_CONTENT[1].1,
                &request,
            )?,
            _ => undelivered_page(tx, &request)?,
        };
        budget = budget.saturating_sub(outcome.scanned);
        if outcome.matched > 0 && !erasing {
            // A verification page found target-bearing rows: the sweep has
            // more erasing to do and must re-walk from the head of the table
            // that still holds them.
            cursor.remainder = outcome.matched;
            cursor.phase = SweepPhase::Erase;
            cursor.reset_position();
            continue;
        }
        if erasing {
            cursor.erased += outcome.deleted;
        }
        if outcome.scanned < u32::try_from(limit).unwrap_or(u32::MAX) {
            cursor.table += 1;
            cursor.reset_position();
        } else if let Some((last, ordinal)) = outcome.last {
            cursor.after = Some(last);
            cursor.after_ordinal = ordinal;
        }
    }
    Ok(())
}

/// Companion-owned local erasure (SO §4.3/4.4): History bodies, activity
/// records, and the undelivered references that name an erased source.
pub struct CompanionErasureParticipant {
    store: Store,
    sweep: Arc<Mutex<Option<SweepCursor>>>,
}

impl CompanionErasureParticipant {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            store,
            sweep: Arc::new(Mutex::new(None)),
        }
    }
}

impl ErasureParticipant for CompanionErasureParticipant {
    fn owner(&self) -> ParticipantOwnerRef {
        ParticipantOwnerRef::Companion
    }

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>> {
        let store = self.store.clone();
        let sweep = Arc::clone(&self.sweep);
        Box::pin(async move {
            let condition = command.condition();
            let owner = command.participant();
            let held = |reason| {
                ParticipantCompletionFact::held(condition, owner, reason, WallClockWithTz::now())
            };
            let Some(target) = exact_text(&command) else {
                // A local owner must receive the protected material. Without
                // it no mechanical sweep exists, so this is an explicit
                // retryable hold, never a fabricated completion.
                return held(ParticipantHoldClass::Failed);
            };
            let covered = command.scope().sources().to_vec();
            match run_blocking(move || {
                run_local_demand(
                    &store,
                    &sweep,
                    condition,
                    owner,
                    &target,
                    &covered,
                    companion_step,
                )
            })
            .await
            {
                Ok(fact) => fact,
                Err(_) => held(ParticipantHoldClass::Failed),
            }
        })
    }
}

// --- Learning owner --------------------------------------------------------

/// Tables of the Learning sweep, in order: evidence Summaries, Memory
/// revisions, current Memories, then the derived recall token index.
const LEARNING_TABLES: usize = 4;

/// Deletes whole Memories: the current row, every revision, and the derived
/// recall tokens. A Memory whose current recognition is erased must not leave
/// an older revision or a recall index entry behind.
fn delete_memories(tx: &Transaction<'_>, memories: &[String]) -> Result<u64, rusqlite::Error> {
    if memories.is_empty() {
        return Ok(0);
    }
    let revisions = count_keys(tx, "learning_memory_revision", MEMORY_KEY, memories)?;
    let terms = count_keys(tx, TERM_TABLE, MEMORY_KEY, memories)?;
    let current = delete_keys(tx, "learning_memory", MEMORY_KEY, memories)?;
    delete_keys(tx, "learning_memory_revision", MEMORY_KEY, memories)?;
    delete_keys(tx, TERM_TABLE, MEMORY_KEY, memories)?;
    Ok(current + revisions + terms)
}

/// One page of `learning_summary`: a row matches when its content carries the
/// exact target or one of its recorded History source pins names a
/// target-bearing message. The recorded pins are the only mechanical
/// History→Summary correspondence the schema keeps.
fn summary_page(
    tx: &Transaction<'_>,
    request: &PageRequest<'_>,
    covered: &HashSet<String>,
    memo: &mut HashMap<String, bool>,
) -> Result<PageOutcome, rusqlite::Error> {
    let (keys, scanned, last) = {
        let mut statement = tx.prepare(&format!(
            "SELECT {SUMMARY_KEY}, source_start, source_end, instr(content, ?2) > 0 \
             FROM learning_summary WHERE {SUMMARY_KEY} > ?1 ORDER BY {SUMMARY_KEY} LIMIT ?3"
        ))?;
        let mut rows = statement.query(params![request.after, request.target, request.limit])?;
        let mut keys: Vec<String> = Vec::new();
        let mut scanned = 0u32;
        let mut last: Option<(String, i64)> = None;
        let mut pages: Vec<(String, String, String, bool)> = Vec::new();
        while let Some(row) = rows.next()? {
            scanned += 1;
            let key: String = row.get(0)?;
            let start: String = row.get(1)?;
            let end: String = row.get(2)?;
            let content_hit: bool = row.get(3)?;
            pages.push((key.clone(), start, end, content_hit));
            last = Some((key, 0));
        }
        drop(rows);
        for (key, start, end, content_hit) in pages {
            let correlated = content_hit
                || covered.contains(&start)
                || covered.contains(&end)
                || pin_target_bearing(tx, &start, request.target, memo)?
                || pin_target_bearing(tx, &end, request.target, memo)?;
            if correlated {
                keys.push(key);
            }
        }
        (keys, scanned, last)
    };
    let deleted = if request.delete {
        delete_keys(tx, "learning_summary", SUMMARY_KEY, &keys)?
    } else {
        0
    };
    Ok(PageOutcome {
        scanned,
        matched: u64::try_from(keys.len()).unwrap_or(u64::MAX),
        deleted,
        last,
    })
}

/// One page of `learning_memory_revision`: a row matches when its content
/// carries the exact target or its recorded evidence Summary no longer
/// exists. Erasing a revision that is the target's *current* revision erases
/// the whole Memory (current row, all revisions, tokens), because the current
/// recognition is then grounded only in erased evidence; an older revision is
/// removed on its own so unrelated newer recognition survives (§6.4).
fn revision_page(
    tx: &Transaction<'_>,
    request: &PageRequest<'_>,
) -> Result<PageOutcome, rusqlite::Error> {
    let (rows, scanned, last) = {
        let mut statement = tx.prepare(&format!(
            "SELECT {MEMORY_KEY}, {REVISION_KEY}, summary_id, instr(content, ?3) > 0 \
             FROM learning_memory_revision \
             WHERE ({MEMORY_KEY}, {REVISION_KEY}) > (?1, ?2) \
             ORDER BY {MEMORY_KEY}, {REVISION_KEY} LIMIT ?4"
        ))?;
        let mut rows = statement.query(params![
            request.after,
            request.after_ordinal,
            request.target,
            request.limit
        ])?;
        let mut rows_out: Vec<(String, i64, Option<String>, bool)> = Vec::new();
        let mut scanned = 0u32;
        let mut last: Option<(String, i64)> = None;
        while let Some(row) = rows.next()? {
            scanned += 1;
            let memory: String = row.get(0)?;
            let revision: i64 = row.get(1)?;
            let summary: Option<String> = row.get(2)?;
            let content_hit: bool = row.get(3)?;
            last = Some((memory.clone(), revision));
            rows_out.push((memory, revision, summary, content_hit));
        }
        (rows_out, scanned, last)
    };
    let mut matched = 0u64;
    let mut deleted = 0u64;
    let mut whole_memories: Vec<String> = Vec::new();
    for (memory, revision, summary, content_hit) in rows {
        let orphaned = match summary.as_deref() {
            None => false,
            Some(summary) => {
                let exists: bool = tx.query_row(
                    &format!(
                        "SELECT EXISTS(SELECT 1 FROM learning_summary WHERE {SUMMARY_KEY} = ?1)"
                    ),
                    params![summary],
                    |row| row.get(0),
                )?;
                !exists
            }
        };
        if !(content_hit || orphaned) {
            continue;
        }
        matched += 1;
        if !request.delete {
            continue;
        }
        let current: bool = tx.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM learning_memory \
                 WHERE {MEMORY_KEY} = ?1 AND {REVISION_KEY} = ?2)"
            ),
            params![memory, revision],
            |row| row.get(0),
        )?;
        if current {
            whole_memories.push(memory);
        } else {
            let removed = tx.execute(
                &format!(
                    "DELETE FROM learning_memory_revision \
                     WHERE {MEMORY_KEY} = ?1 AND {REVISION_KEY} = ?2"
                ),
                params![memory, revision],
            )?;
            deleted += u64::try_from(removed).unwrap_or(u64::MAX);
        }
    }
    deleted += delete_memories(tx, &whole_memories)?;
    Ok(PageOutcome {
        scanned,
        matched,
        deleted,
        last,
    })
}

/// One page of current Memories. A matching recognition is erased whole (its
/// current row, every revision, and the derived tokens): the change history of
/// an erased recognition is not separable from it, so no revision may remain
/// as the only copy of the erased body.
fn memory_page(
    tx: &Transaction<'_>,
    request: &PageRequest<'_>,
) -> Result<PageOutcome, rusqlite::Error> {
    let (table, content_column) = LEARNING_CONTENT[1];
    let (keys, scanned, last) = {
        let mut statement = tx.prepare(&format!(
            "SELECT {MEMORY_KEY}, instr({content_column}, ?2) > 0 FROM {table} \
             WHERE {MEMORY_KEY} > ?1 ORDER BY {MEMORY_KEY} LIMIT ?3"
        ))?;
        let mut rows = statement.query(params![request.after, request.target, request.limit])?;
        let mut keys: Vec<String> = Vec::new();
        let mut scanned = 0u32;
        let mut last: Option<(String, i64)> = None;
        while let Some(row) = rows.next()? {
            scanned += 1;
            let key: String = row.get(0)?;
            let hit: bool = row.get(1)?;
            if hit {
                keys.push(key.clone());
            }
            last = Some((key, 0));
        }
        (keys, scanned, last)
    };
    let deleted = if request.delete {
        delete_memories(tx, &keys)?
    } else {
        0
    };
    Ok(PageOutcome {
        scanned,
        matched: u64::try_from(keys.len()).unwrap_or(u64::MAX),
        deleted,
        last,
    })
}

/// One page of the derived recall token index. A token that equals the exact
/// target is removed on its own; tokens of erased Memories are removed by the
/// Memory cascade. Matching the whole token (never a substring) keeps
/// unrelated Memories indexed (§6.4).
fn term_page(
    tx: &Transaction<'_>,
    request: &PageRequest<'_>,
) -> Result<PageOutcome, rusqlite::Error> {
    let (keys, scanned, last) = {
        let mut statement = tx.prepare(&format!(
            "SELECT rowid, term FROM {TERM_TABLE} WHERE rowid > ?1 ORDER BY rowid LIMIT ?2"
        ))?;
        let mut rows = statement.query(params![request.after_ordinal, request.limit])?;
        let mut keys: Vec<String> = Vec::new();
        let mut scanned = 0u32;
        let mut last: Option<(String, i64)> = None;
        while let Some(row) = rows.next()? {
            scanned += 1;
            let rowid: i64 = row.get(0)?;
            let term: String = row.get(1)?;
            if term == request.target {
                keys.push(rowid.to_string());
            }
            last = Some((String::new(), rowid));
        }
        (keys, scanned, last)
    };
    let deleted = if request.delete {
        delete_keys(tx, TERM_TABLE, "rowid", &keys)?
    } else {
        0
    };
    Ok(PageOutcome {
        scanned,
        matched: u64::try_from(keys.len()).unwrap_or(u64::MAX),
        deleted,
        last,
    })
}

fn learning_page(
    tx: &Transaction<'_>,
    table: usize,
    request: &PageRequest<'_>,
    covered: &HashSet<String>,
    memo: &mut HashMap<String, bool>,
) -> Result<PageOutcome, rusqlite::Error> {
    match table {
        0 => summary_page(tx, request, covered, memo),
        1 => revision_page(tx, request),
        2 => memory_page(tx, request),
        _ => term_page(tx, request),
    }
}

fn learning_step(
    tx: &Transaction<'_>,
    cursor: &mut SweepCursor,
    target: &str,
    covered: &[RawId],
) -> Result<(), rusqlite::Error> {
    let covered: HashSet<String> = covered.iter().map(|raw| encode_id(*raw)).collect();
    let mut memo: HashMap<String, bool> = HashMap::new();
    let mut budget = ERASURE_SCAN_ROWS;
    while budget > 0 {
        if cursor.verified {
            break;
        }
        if cursor.table >= LEARNING_TABLES {
            if cursor.phase == SweepPhase::Erase {
                cursor.begin_verify();
                continue;
            }
            cursor.verified = true;
            break;
        }
        let limit = i64::from(budget);
        let after = cursor.after.clone().unwrap_or_default();
        let erasing = cursor.phase == SweepPhase::Erase;
        let request = PageRequest {
            target,
            after: &after,
            after_ordinal: cursor.after_ordinal,
            limit,
            delete: erasing,
        };
        let outcome = learning_page(tx, cursor.table, &request, &covered, &mut memo)?;
        budget = budget.saturating_sub(outcome.scanned);
        if outcome.matched > 0 && !erasing {
            // Verification found rows the erase walk must remove: re-walk
            // from the head of the table that still holds them.
            cursor.remainder = outcome.matched;
            cursor.phase = SweepPhase::Erase;
            cursor.reset_position();
            continue;
        }
        if erasing {
            cursor.erased += outcome.deleted;
        }
        if outcome.scanned < u32::try_from(limit).unwrap_or(u32::MAX) {
            cursor.table += 1;
            cursor.reset_position();
        } else if let Some((last, ordinal)) = outcome.last {
            cursor.after = Some(last);
            cursor.after_ordinal = ordinal;
        }
    }
    Ok(())
}

/// Learning-owned local erasure (SO §4.5-4.9): Experience Summary evidence,
/// current and historical Memory content, and the derived recall index.
pub struct LearningErasureParticipant {
    store: Store,
    sweep: Arc<Mutex<Option<SweepCursor>>>,
}

impl LearningErasureParticipant {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            store,
            sweep: Arc::new(Mutex::new(None)),
        }
    }
}

impl ErasureParticipant for LearningErasureParticipant {
    fn owner(&self) -> ParticipantOwnerRef {
        ParticipantOwnerRef::Learning
    }

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>> {
        let store = self.store.clone();
        let sweep = Arc::clone(&self.sweep);
        Box::pin(async move {
            let condition = command.condition();
            let owner = command.participant();
            let held = |reason| {
                ParticipantCompletionFact::held(condition, owner, reason, WallClockWithTz::now())
            };
            let Some(target) = exact_text(&command) else {
                return held(ParticipantHoldClass::Failed);
            };
            let covered = command.scope().sources().to_vec();
            match run_blocking(move || {
                run_local_demand(
                    &store,
                    &sweep,
                    condition,
                    owner,
                    &target,
                    &covered,
                    learning_step,
                )
            })
            .await
            {
                Ok(fact) => fact,
                Err(_) => held(ParticipantHoldClass::Failed),
            }
        })
    }
}
