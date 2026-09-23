use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef,
};
use ene_primitive::WallClockWithTz;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use crate::codec::{SOURCE_KIND_ACTIVITY_RECORD, SOURCE_KIND_HISTORY_MESSAGE, lock_shared};
use crate::{Store, run_blocking};

pub const ERASURE_SCAN_ROWS: u32 = 64;

pub(crate) const COMPANION_CONTENT: &[(&str, &str)] =
    &[("history_message", "body"), ("activity_record", "body")];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SweepPhase {
    Erase,
    Verify,
}

#[derive(Debug, Clone)]
struct SweepCursor {
    condition: ErasureConditionRef,
    phase: SweepPhase,
    table: usize,
    after: Option<String>,
    after_ordinal: i64,
    erased: u64,
    remainder: u64,
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

#[derive(Debug, Default)]
struct PageOutcome {
    scanned: u32,
    matched: u64,
    deleted: u64,
    last: Option<(String, i64)>,
}

struct PageRequest<'a> {
    target: &'a str,
    after: &'a str,
    after_ordinal: i64,
    limit: i64,
    delete: bool,
}

type LocalStep =
    fn(tx: &Transaction<'_>, cursor: &mut SweepCursor, target: &str) -> Result<(), rusqlite::Error>;

fn lock_cursor(slot: &Mutex<Option<SweepCursor>>) -> MutexGuard<'_, Option<SweepCursor>> {
    match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn exact_text(command: &DemandLocalErasureCommand) -> Option<String> {
    let target = command.scope().target()?;
    let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
    let text = material.expose_for_erasure();
    if text.is_empty() {
        return None;
    }
    Some(text.to_owned())
}

fn run_local_demand(
    store: &Store,
    sweep: &Mutex<Option<SweepCursor>>,
    condition: ErasureConditionRef,
    owner: ParticipantOwnerRef,
    target: &str,
    step: LocalStep,
) -> Result<ParticipantCompletionFact, rusqlite::Error> {
    let mut guard = lock_shared(&store.conn);
    let tx = guard.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let at = WallClockWithTz::now();
    if !crate::preservation::condition_is_current(&tx, condition)? {
        return Ok(ParticipantCompletionFact::local_complete(
            condition, owner, 0, 0, at,
        ));
    }
    let mut cursor = {
        let slot = lock_cursor(sweep);
        match slot.as_ref() {
            Some(existing) if existing.condition == condition => existing.clone(),
            _ => SweepCursor::fresh(condition),
        }
    };
    step(&tx, &mut cursor, target)?;
    tx.commit()?;
    let fact = cursor.fact(owner, at);
    *lock_cursor(sweep) = Some(cursor);
    Ok(fact)
}

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

const COMPANION_TABLES: usize = 3;

fn companion_step(
    tx: &Transaction<'_>,
    cursor: &mut SweepCursor,
    target: &str,
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
            #[cfg(any(test, feature = "test-support"))]
            store.test_parks.erasure_mutation.pause_if_armed().await;
            let condition = command.condition();
            let owner = command.participant();
            let held = |reason| {
                ParticipantCompletionFact::held(condition, owner, reason, WallClockWithTz::now())
            };
            let Some(target) = exact_text(&command) else {
                return held(ParticipantHoldClass::Failed);
            };
            match run_blocking(move || {
                run_local_demand(&store, &sweep, condition, owner, &target, companion_step)
            })
            .await
            {
                Ok(fact) => fact,
                Err(_) => held(ParticipantHoldClass::Failed),
            }
        })
    }
}

const LEARNING_TABLES: usize = 4;

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

fn pin_source_covered(
    tx: &Transaction<'_>,
    condition: ErasureConditionRef,
    pin: &str,
    memo: &mut HashMap<String, bool>,
) -> Result<bool, rusqlite::Error> {
    if let Some(hit) = memo.get(pin) {
        return Ok(*hit);
    }
    let Ok(sweep) = i64::try_from(condition.sweep.as_u64()) else {
        memo.insert(pin.to_owned(), false);
        return Ok(false);
    };
    let hit: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM erasure_condition_source \
         WHERE operation_id=?1 AND sweep=?2 AND source=?3)",
        params![
            crate::codec::encode_id(condition.operation.as_raw()),
            sweep,
            pin
        ],
        |row| row.get(0),
    )?;
    memo.insert(pin.to_owned(), hit);
    Ok(hit)
}

fn summary_page(
    tx: &Transaction<'_>,
    condition: ErasureConditionRef,
    request: &PageRequest<'_>,
    covered_memo: &mut HashMap<String, bool>,
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
                || pin_source_covered(tx, condition, &start, covered_memo)?
                || pin_source_covered(tx, condition, &end, covered_memo)?
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
    condition: ErasureConditionRef,
    request: &PageRequest<'_>,
    covered_memo: &mut HashMap<String, bool>,
    memo: &mut HashMap<String, bool>,
) -> Result<PageOutcome, rusqlite::Error> {
    match table {
        0 => summary_page(tx, condition, request, covered_memo, memo),
        1 => revision_page(tx, request),
        2 => memory_page(tx, request),
        _ => term_page(tx, request),
    }
}

fn learning_step(
    tx: &Transaction<'_>,
    cursor: &mut SweepCursor,
    target: &str,
) -> Result<(), rusqlite::Error> {
    let mut covered_memo: HashMap<String, bool> = HashMap::new();
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
        let outcome = learning_page(
            tx,
            cursor.table,
            cursor.condition,
            &request,
            &mut covered_memo,
            &mut memo,
        )?;
        budget = budget.saturating_sub(outcome.scanned);
        if outcome.matched > 0 && !erasing {
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
            #[cfg(any(test, feature = "test-support"))]
            store.test_parks.erasure_mutation.pause_if_armed().await;
            let condition = command.condition();
            let owner = command.participant();
            let held = |reason| {
                ParticipantCompletionFact::held(condition, owner, reason, WallClockWithTz::now())
            };
            let Some(target) = exact_text(&command) else {
                return held(ParticipantHoldClass::Failed);
            };
            match run_blocking(move || {
                run_local_demand(&store, &sweep, condition, owner, &target, learning_step)
            })
            .await
            {
                Ok(fact) => fact,
                Err(_) => held(ParticipantHoldClass::Failed),
            }
        })
    }
}
