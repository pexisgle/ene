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
//! The list deliberately mirrors the owner predicates; it is not a second
//! owner contract. A participant's `Verified` fact still covers the derived
//! correlation shapes only that owner can judge (pinned evidence, orphaned
//! revisions, dangling reporting references), and the probe re-checks the
//! mechanical exact-text condition system-wide.

use rusqlite::{Connection, params};

use crate::codec::{SOURCE_KIND_ACTIVITY_RECORD, SOURCE_KIND_HISTORY_MESSAGE};

/// Rows one probe page reads. The bound keeps a single statement from scanning
/// an unbounded row set; the surface walk continues from the last rowid.
const PROBE_PAGE_ROWS: u32 = 64;

/// The closed canonical content surface, `(table, content column)`.
///
/// Every entry mirrors one semantic owner's mechanical sweep predicate:
///
/// * Companion: History bodies, activity-record bodies.
/// * Learning: Summary content, Memory content, revision content.
/// * Task: the in-force purpose, every revision purpose, result bodies, the
///   observation occurrence path correlation, and the internal workspace /
///   delegation path copies.
/// * Action: the attempt's resolved target path.
/// * Permission: the intent journal's subject and quoted rationale, and the
///   consent route fields.
/// * Credential: ref metadata, pending registration metadata, and pairing
///   descriptors/tokens (the local metadata surface; never the external
///   credential value, which lives outside the store).
/// * Presence: attribution and relocation identities.
const SYSTEM_CONTENT: &[(&str, &str)] = &[
    ("history_message", "body"),
    ("activity_record", "body"),
    ("learning_summary", "content"),
    ("learning_memory", "content"),
    ("learning_memory_revision", "content"),
    ("task", "purpose_text"),
    ("task_revision", "purpose_text"),
    ("task_result", "body"),
    ("task_agent_observation", "path"),
    ("workspace_assoc", "folder"),
    ("workspace_assoc", "save_target"),
    ("delegation", "scope_folder"),
    ("delegation", "scope_save_target"),
    ("action_attempt", "real_target"),
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

/// Counts stored values carrying `target` across one `(table, column)`.
///
/// The walk is a keyset page over the table's implicit `rowid`: every
/// statement reads at most [`PROBE_PAGE_ROWS`] rows, a page with a match ends
/// the surface walk (a non-zero result is all the caller needs), and an empty
/// or short page proves the surface exhausted. Nullable columns are compared
/// against `''`, so a NULL value is never a match and never a type error.
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

/// Counts the system-wide mechanical remainder of `target` over the canonical
/// durable content surface.
///
/// `0` is the completion premise: a complete walk of every surface found no
/// stored value carrying the exact target. A positive result is a collected
/// delayed-arrival / remainder fact; the caller must not destroy protected
/// material while it stands and must open a new sweep instead (§12).
pub(crate) fn system_remainder(conn: &Connection, target: &str) -> Result<u64, rusqlite::Error> {
    let mut found = 0u64;
    for (table, column) in SYSTEM_CONTENT {
        found = found.saturating_add(surface_remainder(conn, table, column, target)?);
    }
    // The derived recall index is matched by whole-token equality, exactly as
    // the Learning sweep matches it; a token containing the target as a
    // substring is not this token and is never re-indexed into a match.
    let term: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM learning_memory_term WHERE term = ?1)",
        [target],
        |row| row.get(0),
    )?;
    if term {
        found = found.saturating_add(1);
    }
    // Reporting references whose canonical source is gone are the derived
    // cleanup the Companion sweep performs; a new dangling reference is
    // remaining collection work for that owner, never a completion.
    let dangling: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM undelivered u WHERE
             (u.source_kind = ?1 AND NOT EXISTS
                 (SELECT 1 FROM history_message h WHERE h.message_id = u.source_id))
          OR (u.source_kind = ?2 AND NOT EXISTS
                 (SELECT 1 FROM activity_record a WHERE a.activity_id = u.source_id)))",
        params![SOURCE_KIND_HISTORY_MESSAGE, SOURCE_KIND_ACTIVITY_RECORD],
        |row| row.get(0),
    )?;
    if dangling {
        found = found.saturating_add(1);
    }
    Ok(found)
}
