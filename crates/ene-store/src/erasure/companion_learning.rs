use std::collections::HashMap;

use ene_preservation::{ErasureConditionRef, ParticipantOwnerRef};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::Store;
use crate::codec::{SOURCE_KIND_ACTIVITY_RECORD, SOURCE_KIND_HISTORY_MESSAGE};

use super::{
    ErasurePageError, LocalErasureParticipant, PageOutcome, PageRequest, SweepCursor, bounded_step,
};

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
const TERM_TABLE: &str = "learning_memory_term";

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

fn exact_page_with(
    tx: &Transaction<'_>,
    table: &str,
    key_column: &str,
    content_column: &str,
    request: &PageRequest<'_>,
    delete: impl FnOnce(&[String]) -> Result<u64, rusqlite::Error>,
) -> Result<PageOutcome, ErasurePageError> {
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
    let deleted = if request.delete { delete(&keys)? } else { 0 };
    Ok(PageOutcome {
        scanned,
        matched: u64::try_from(keys.len()).unwrap_or(u64::MAX),
        deleted,
        last,
    })
}

fn exact_page(
    tx: &Transaction<'_>,
    table: &str,
    key_column: &str,
    content_column: &str,
    request: &PageRequest<'_>,
) -> Result<PageOutcome, ErasurePageError> {
    exact_page_with(tx, table, key_column, content_column, request, |keys| {
        delete_keys(tx, table, key_column, keys)
    })
}

pub(crate) const SQL_DANGLING_UNDELIVERED: &str = "(u.source_kind = ?1 AND NOT EXISTS (SELECT 1 FROM history_message h WHERE h.message_id = u.source_id)) OR (u.source_kind = ?2 AND NOT EXISTS (SELECT 1 FROM activity_record a WHERE a.activity_id = u.source_id))";

fn undelivered_page(
    tx: &Transaction<'_>,
    request: &PageRequest<'_>,
) -> Result<PageOutcome, ErasurePageError> {
    let sql = format!(
        "SELECT u.{UNDELIVERED_KEY}, ({SQL_DANGLING_UNDELIVERED}) \
         FROM undelivered u WHERE u.{UNDELIVERED_KEY} > ?3 ORDER BY u.{UNDELIVERED_KEY} LIMIT ?4"
    );
    let (keys, scanned, last) = {
        let mut statement = tx.prepare(&sql)?;
        let mut rows = statement.query(params![
            SOURCE_KIND_HISTORY_MESSAGE,
            SOURCE_KIND_ACTIVITY_RECORD,
            request.after,
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
) -> Result<(), ErasurePageError> {
    bounded_step(
        tx,
        cursor,
        target,
        COMPANION_TABLES,
        |tx, table, request| match table {
            0 => exact_page(
                tx,
                COMPANION_CONTENT[0].0,
                HISTORY_MESSAGE_KEY,
                COMPANION_CONTENT[0].1,
                request,
            ),
            1 => exact_page(
                tx,
                COMPANION_CONTENT[1].0,
                ACTIVITY_RECORD_KEY,
                COMPANION_CONTENT[1].1,
                request,
            ),
            _ => undelivered_page(tx, request),
        },
    )
}

const LEARNING_TABLES: usize = 4;

fn delete_memories(tx: &Transaction<'_>, memories: &[String]) -> Result<u64, rusqlite::Error> {
    if memories.is_empty() {
        return Ok(0);
    }
    let current = delete_keys(tx, "learning_memory", MEMORY_KEY, memories)?;
    let revisions = delete_keys(tx, "learning_memory_revision", MEMORY_KEY, memories)?;
    let terms = delete_keys(tx, TERM_TABLE, MEMORY_KEY, memories)?;
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
) -> Result<PageOutcome, ErasurePageError> {
    let (keys, scanned, last) = {
        let mut statement = tx.prepare(&format!(
            "SELECT {SUMMARY_KEY}, source_start, source_end, instr({}, ?2) > 0 \
             FROM learning_summary WHERE {SUMMARY_KEY} > ?1 ORDER BY {SUMMARY_KEY} LIMIT ?3",
            LEARNING_CONTENT[0].1
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
) -> Result<PageOutcome, ErasurePageError> {
    let (rows, scanned, last) = {
        let mut statement = tx.prepare(&format!(
            "SELECT {MEMORY_KEY}, {REVISION_KEY}, summary_id, instr({}, ?3) > 0 \
             FROM learning_memory_revision \
             WHERE ({MEMORY_KEY}, {REVISION_KEY}) > (?1, ?2) \
             ORDER BY {MEMORY_KEY}, {REVISION_KEY} LIMIT ?4",
            LEARNING_CONTENT[2].1
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
) -> Result<PageOutcome, ErasurePageError> {
    exact_page_with(
        tx,
        LEARNING_CONTENT[1].0,
        MEMORY_KEY,
        LEARNING_CONTENT[1].1,
        request,
        |keys| delete_memories(tx, keys),
    )
}

fn term_page(
    tx: &Transaction<'_>,
    request: &PageRequest<'_>,
) -> Result<PageOutcome, ErasurePageError> {
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
) -> Result<PageOutcome, ErasurePageError> {
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
) -> Result<(), ErasurePageError> {
    let mut covered_memo: HashMap<String, bool> = HashMap::new();
    let mut memo: HashMap<String, bool> = HashMap::new();
    let condition = cursor.condition;
    bounded_step(tx, cursor, target, LEARNING_TABLES, |tx, table, request| {
        learning_page(tx, table, condition, request, &mut covered_memo, &mut memo)
    })
}

#[must_use]
pub fn companion_erasure_participant(store: Store) -> LocalErasureParticipant {
    LocalErasureParticipant::new(ParticipantOwnerRef::Companion, companion_step, store)
}

#[must_use]
pub fn learning_erasure_participant(store: Store) -> LocalErasureParticipant {
    LocalErasureParticipant::new(ParticipantOwnerRef::Learning, learning_step, store)
}
