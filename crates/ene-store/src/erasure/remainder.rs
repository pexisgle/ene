use rusqlite::{Connection, params};

use crate::codec::{SOURCE_KIND_ACTIVITY_RECORD, SOURCE_KIND_HISTORY_MESSAGE};

use super::companion_learning::{COMPANION_CONTENT, LEARNING_CONTENT, SQL_DANGLING_UNDELIVERED};
use super::task_action_inference::{action_content_surface, task_content_surface};

const PROBE_PAGE_ROWS: u32 = 64;

const SYSTEM_CONTENT: &[(&str, &str)] = &[
    ("management_intent", "target"),
    ("management_intent", "rationale_quote"),
    ("consent_record", "id"),
    ("consent_record", "provider"),
    ("consent_record", "model"),
    ("consent_record", "credential_id"),
    ("credential_ref", "id"),
    ("credential_ref", "provider"),
    ("credential_ref", "label"),
    ("credential_pending", "provider"),
    ("credential_pending", "label"),
    ("paired_device", "device_id"),
    ("paired_device", "descriptor"),
    ("paired_device", "wire"),
    ("pairing_pending", "pending_id"),
    ("pairing_pending", "descriptor"),
    ("pairing_pending", "origin_connection"),
    ("presence_attribution", "companion_id"),
    ("presence_attribution", "active_client"),
    ("relocation_hint", "companion_id"),
    ("relocation_hint", "last_client"),
    ("relocation_hint", "recovery_destination"),
    ("presence_transition_log", "companion_id"),
];

fn surface_remainder(
    conn: &Connection,
    table: &str,
    column: &str,
    target: &str,
) -> Result<u64, rusqlite::Error> {
    let sql = format!(
        "SELECT rowid, instr(COALESCE({column}, ''), ?2) > 0 FROM {table} \
         WHERE rowid > ?1 ORDER BY rowid LIMIT ?3"
    );
    let mut statement = conn.prepare(&sql)?;
    let mut after: i64 = 0;
    let mut found = 0u64;
    loop {
        let mut scanned = 0u32;
        let mut page_hits = 0u64;
        {
            let mut rows = statement.query(params![after, target, PROBE_PAGE_ROWS])?;
            while let Some(row) = rows.next()? {
                scanned += 1;
                after = row.get(0)?;
                if row.get::<_, bool>(1)? {
                    page_hits += 1;
                }
            }
        }
        found = found.saturating_add(page_hits);
        if page_hits > 0 || scanned < PROBE_PAGE_ROWS {
            return Ok(found);
        }
    }
}

pub(crate) fn system_remainder(conn: &Connection, target: &str) -> Result<u64, rusqlite::Error> {
    let mut found = 0u64;
    let surfaces = COMPANION_CONTENT
        .iter()
        .copied()
        .chain(LEARNING_CONTENT.iter().copied())
        .chain(task_content_surface())
        .chain(action_content_surface())
        .chain(SYSTEM_CONTENT.iter().copied());
    for (table, column) in surfaces {
        found = found.saturating_add(surface_remainder(conn, table, column, target)?);
    }
    let term: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM learning_memory_term WHERE term = ?1)",
        [target],
        |row| row.get(0),
    )?;
    if term {
        found = found.saturating_add(1);
    }
    let dangling: bool = conn.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM undelivered u WHERE {SQL_DANGLING_UNDELIVERED})"),
        params![SOURCE_KIND_HISTORY_MESSAGE, SOURCE_KIND_ACTIVITY_RECORD],
        |row| row.get(0),
    )?;
    if dangling {
        found = found.saturating_add(1);
    }
    Ok(found)
}
