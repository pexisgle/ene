//! System-wide mechanical remainder verification for the completion boundary
//! (lifecycle §12/§18).
//!
//! Each owner's bounded sweep verifies its own surface. This probe is the
//! independent system-wide cross-check the completion boundary runs before it
//! destroys protected material: over a **closed** list of canonical durable
//! content columns — the union of every owner's mechanical predicate — it
//! proves that no stored value carries the exact deletion target.
//!
//! Properties:
//!
//! * mechanical and LLM-independent: SQLite `instr` substring matching (token
//!   equality for the derived recall index), never a model judgment;
//! * bounded per statement: each surface is walked with a keyset page of
//!   [`PROBE_PAGE_ROWS`] rows, so no single statement performs an unbounded
//!   scan and a match short-circuits its surface;
//! * complete: a zero result means every page of every surface was walked and
//!   found nothing; the caller (`PreservationRepository`) fails closed
//!   (abandons completion and opens a new sweep) on a non-zero result;
//! * no caller input: table and column names are compile-time constants; the
//!   target text only ever travels as a bound parameter;
//! * no target-derived state: the probe is a read. It never writes, never
//!   caches a verdict, and never copies the target into a row.
//!
//! The Companion, Learning, Task, and Action surfaces are derived from those
//! owners' own sweep declarations, so a swept column is probed with the same
//! predicate; the remaining surfaces mirror predicates owned outside this
//! crate. The probe is not a second owner contract: a participant's
//! `Verified` fact still covers the derived correlation shapes only that
//! owner can judge (pinned evidence, orphaned revisions, dangling reporting
//! references), and the probe re-checks the mechanical exact-text condition
//! system-wide.

use rusqlite::{Connection, params};

use crate::codec::{SOURCE_KIND_ACTIVITY_RECORD, SOURCE_KIND_HISTORY_MESSAGE};

use super::companion_learning::{COMPANION_CONTENT, LEARNING_CONTENT, SQL_DANGLING_UNDELIVERED};
use super::task_action_inference::{action_content_surface, task_content_surface};

/// Rows one probe page reads. The bound keeps a single statement from scanning
/// an unbounded row set; the surface walk continues from the last rowid.
const PROBE_PAGE_ROWS: u32 = 64;

/// The closed canonical content surface whose mechanical predicate lives
/// outside this crate, `(table, content column)`.
///
/// * Permission: the intent journal's subject and quoted rationale, and the
///   consent route fields.
/// * Credential: ref metadata, pending registration metadata, and pairing
///   descriptors/tokens (the local metadata surface; never the external
///   credential value, which lives outside the store).
/// * Presence: attribution and relocation identities.
///
/// The Companion, Learning, Task, and Action surfaces are not repeated here:
/// [`system_remainder`] derives them from those owners' own sweep
/// declarations, so the probe and the sweeps cannot drift apart.
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
