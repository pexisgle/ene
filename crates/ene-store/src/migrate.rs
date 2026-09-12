use rusqlite::{Connection, TransactionBehavior};

use ene_primitive::WallClockWithTz;

use crate::codec::decode_id;

const CURRENT_VERSION: u64 = 15;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS companion (
companion_id TEXT PRIMARY KEY,
lifecycle TEXT NOT NULL,
created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS history_message (
message_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
round_id TEXT NOT NULL,
role TEXT NOT NULL,
body TEXT NOT NULL,
lang TEXT NOT NULL,
at TEXT NOT NULL,
presence_generation INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS undelivered (
undelivered_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
source_message TEXT NOT NULL,
status TEXT NOT NULL,
round_id TEXT NOT NULL,
presence_generation INTEGER NOT NULL,
created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS presence_attribution (
companion_id TEXT PRIMARY KEY,
state TEXT NOT NULL,
active_client TEXT,
generation INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS presence_transition_log (
transition_seq INTEGER PRIMARY KEY AUTOINCREMENT,
companion_id TEXT NOT NULL,
old_state TEXT NOT NULL,
new_state TEXT NOT NULL,
old_gen INTEGER NOT NULL,
new_gen INTEGER NOT NULL,
reason TEXT NOT NULL,
at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS consent_record (
id TEXT PRIMARY KEY,
rev INTEGER NOT NULL,
provider TEXT NOT NULL,
model TEXT NOT NULL,
credential_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS credential_ref (
id TEXT PRIMARY KEY,
provider TEXT NOT NULL,
label TEXT NOT NULL,
UNIQUE (provider, label)
);
CREATE TABLE IF NOT EXISTS usage_fact (
ticket TEXT PRIMARY KEY,
provider TEXT NOT NULL,
model TEXT NOT NULL,
input_tokens INTEGER,
output_tokens INTEGER,
source TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_history_message_companion ON history_message (companion_id);
CREATE INDEX IF NOT EXISTS idx_history_message_round ON history_message (round_id);
CREATE INDEX IF NOT EXISTS idx_undelivered_companion_status ON undelivered (companion_id, status);
";

/// Existing history rows keep `local_id` NULL; fresh and upgraded databases
/// converge via `IF NOT EXISTS`.
const MIGRATION_V2: &str = "
ALTER TABLE history_message ADD COLUMN local_id TEXT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_history_message_companion_local ON history_message (companion_id, local_id);
CREATE TABLE IF NOT EXISTS pairing_pending (
descriptor TEXT PRIMARY KEY,
requested_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS paired_device (
device_id TEXT PRIMARY KEY,
descriptor TEXT NOT NULL,
paired_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_paired_device_descriptor ON paired_device (descriptor);
";

/// Existing history rows keep `command_id` NULL, and `NULL` command ids never
/// collide under the new unique index because SQLite treats each `NULL` as
/// distinct. The retired `(companion_id, local_id)` unique index is dropped;
/// the `local_id` column stays as stored correspondence metadata, never a key.
const MIGRATION_V3: &str = "
ALTER TABLE history_message ADD COLUMN command_id TEXT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_history_message_companion_command ON history_message (companion_id, command_id);
DROP INDEX IF EXISTS idx_history_message_companion_local;
";

/// Introduces `credential_pending` for registration approvals; pre-existing
/// rows cannot reference the new table, so nothing is backfilled. The usable
/// marker stays `credential_ref` (the register flow creates the ref only at
/// approval time), so no approved table exists.
const MIGRATION_V4: &str = "
CREATE TABLE IF NOT EXISTS credential_pending (
provider TEXT NOT NULL,
label TEXT NOT NULL,
requested_at TEXT NOT NULL,
PRIMARY KEY (provider, label)
);
";

/// Adds the opaque wire projections and the replay fingerprint. `round_wire`
/// is the only round string that ever crosses the wire (minted fresh per
/// round, unrelated to the domain bytes); `client_counter`/`client_random`
/// carry the sending incarnation for fingerprint comparison. Pre-opaque rows
/// keep all three `NULL` and read back as absent. Pre-existing paired devices
/// are backfilled to their identity rendering so already-provisioned clients
/// keep resolving; new approvals mint a fresh opaque projection instead.
const MIGRATION_V5: &str = "
ALTER TABLE history_message ADD COLUMN round_wire TEXT NULL;
ALTER TABLE history_message ADD COLUMN client_counter INTEGER NULL;
ALTER TABLE history_message ADD COLUMN client_random INTEGER NULL;
ALTER TABLE paired_device ADD COLUMN wire TEXT NULL;
UPDATE paired_device SET wire = device_id WHERE wire IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_paired_device_wire ON paired_device (wire);
";

/// Introduces `management_intent` for durable management intent replay
/// (design §18.2 idempotency keys): each row binds one intent key to the
/// fingerprint it decided on plus its terminal outcome snapshot, so an exact
/// retry replays without re-executing while a reused id with new content
/// conflicts instead of rebinding. Intents decided before this version have
/// no replay row; their retries take the normal premise-checked path.
const MIGRATION_V6: &str = "
CREATE TABLE IF NOT EXISTS management_intent (
intent_id TEXT PRIMARY KEY,
kind TEXT NOT NULL,
target TEXT NOT NULL,
base TEXT NOT NULL,
rationale_origin TEXT NOT NULL,
rationale_quote TEXT NULL,
outcome TEXT NOT NULL,
mark TEXT NULL
);
";

/// Introduces `inference_attempt` for the provider-I/O linearization point:
/// one ticket's attempt is claimed under one consent premise in the same
/// short transaction that verifies it, so a consent mutation either precedes
/// the claim (the claim fails stale before any byte leaves) or follows it
/// (adoption decides separately). Ticket ids are single-use, so re-claiming
/// one answers stale instead of sending twice.
const MIGRATION_V7: &str = "
CREATE TABLE IF NOT EXISTS inference_attempt (
ticket TEXT PRIMARY KEY,
consent_id TEXT NOT NULL,
consent_rev INTEGER NOT NULL,
provider TEXT NOT NULL,
model TEXT NOT NULL,
started_at TEXT NOT NULL
);
";

/// Persists the client round intent of the history request fingerprint
/// (`round_intent` kind, `round_intent_ref` payload). The intent is request
/// semantics — Auto, force-new, or a join of the Client-supplied round
/// reference — so a restart can still decide replay from durable state.
/// Pre-existing rows keep both columns `NULL`: their intent was never stored,
/// so their sameness cannot be proven and replay attempts against them fail
/// closed instead of being guessed.
const MIGRATION_V8: &str = "
ALTER TABLE history_message ADD COLUMN round_intent TEXT NULL;
ALTER TABLE history_message ADD COLUMN round_intent_ref TEXT NULL;
";

/// Introduces the Learning group: Summary evidence, Memory current rows, and
/// the append-only revision chain that keeps past recognition and grounds.
/// Current rows and revisions are separate on purpose: the current row is the
/// recognition in use, while revisions are the change history that corrections
/// and normal forgetting never delete. `summary_id` on a revision is the
/// grounds relation for that revision; the Summary table is shared by every
/// change one formation produced.
const MIGRATION_V9: &str = "
CREATE TABLE IF NOT EXISTS learning_summary (
summary_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
content TEXT NOT NULL,
source_kind TEXT NOT NULL,
source_start TEXT NOT NULL,
source_end TEXT NOT NULL,
formed_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS learning_memory (
memory_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
revision INTEGER NOT NULL,
content TEXT NOT NULL,
importance INTEGER NOT NULL,
temporal TEXT NOT NULL,
recall_suppressed INTEGER NOT NULL,
updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_learning_memory_companion ON learning_memory (companion_id);
CREATE TABLE IF NOT EXISTS learning_memory_revision (
memory_id TEXT NOT NULL,
revision INTEGER NOT NULL,
companion_id TEXT NOT NULL,
content TEXT NOT NULL,
importance INTEGER NOT NULL,
temporal TEXT NOT NULL,
recall_suppressed INTEGER NOT NULL,
change_kind TEXT NOT NULL,
summary_id TEXT,
at TEXT NOT NULL,
PRIMARY KEY (memory_id, revision)
);
";
/// Capability-scopes the consent assignment and the inference attempt.
///
/// Stage 2 kept one logical consent row; every capability now has its own
/// row, and the duplicate question is capability-scoped too. The rebuild
/// moves the existing Stage 2 row to the dialogue capability, so an old
/// environment's dialogue assignment keeps working while learning stays
/// unassigned until the Owner assigns it. `inference_attempt.capability`
/// lets the claim read the row for the capability it was admitted under.
const MIGRATION_V10: &str = "
CREATE TABLE consent_record_capability (
capability TEXT PRIMARY KEY,
id TEXT NOT NULL,
rev INTEGER NOT NULL,
provider TEXT NOT NULL,
model TEXT NOT NULL,
credential_id TEXT NOT NULL
);
INSERT INTO consent_record_capability (capability, id, rev, provider, model, credential_id)
SELECT 'dialogue', id, rev, provider, model, credential_id FROM consent_record;
DROP TABLE consent_record;
ALTER TABLE consent_record_capability RENAME TO consent_record;
ALTER TABLE inference_attempt ADD COLUMN capability TEXT NOT NULL DEFAULT 'dialogue';
";

/// Introduces the durable credential-set revision for the secret-scrub
/// currentness premise. A scrub records the revision; writers compare it
/// inside their transaction so content prepared before a value change cannot
/// land afterwards. The revision advances atomically with a usable ref
/// becoming registered, with a successful approval/re-approval, and with the
/// Host startup sweep. Seeded at zero: an environment with no usable
/// credential yet.
const MIGRATION_V11: &str = "
CREATE TABLE IF NOT EXISTS credential_set (
id INTEGER PRIMARY KEY CHECK (id = 1),
rev INTEGER NOT NULL
);
INSERT OR IGNORE INTO credential_set (id, rev) VALUES (1, 0);
";

/// Backs the reply-adoption supersession probe: the newest-Owner check for
/// one companion seeks this covering index and stops at the first hit, so
/// a reply append never scans History to prove recency.
const MIGRATION_V12: &str = "
CREATE INDEX IF NOT EXISTS idx_history_message_companion_role ON history_message (companion_id, role);
";
/// Adds the canonical UTC timestamp projection that orders and range-filters
/// history inside SQL: fixed-width `Z` renderings compare lexically exactly
/// like the instants they represent, regardless of the creation offsets kept
/// in `at`. The stored `at` stays the display/provenance rendering (offset
/// included); `at_utc` is query material only. Existing rows are backfilled
/// in Rust because SQLite's date functions would lose nanosecond precision.
const MIGRATION_V13: &str = "
ALTER TABLE history_message ADD COLUMN at_utc TEXT NULL;
CREATE INDEX IF NOT EXISTS idx_history_message_companion_at ON history_message (companion_id, at_utc);
";

/// Backfills [`MIGRATION_V13`]'s `at_utc` for rows written before it.
///
/// Runs inside the migration transaction: a stored timestamp that cannot be
/// parsed fails the migration instead of leaving a row invisible to `since`
/// filters. The table is read once into memory; history is already bounded
/// by retention in this stage.
fn backfill_history_at_utc(tx: &rusqlite::Transaction<'_>) -> Result<(), String> {
    let rows = {
        let mut statement = tx
            .prepare("SELECT message_id, at FROM history_message WHERE at_utc IS NULL")
            .map_err(|error| error.to_string())?;
        let mapped = statement
            .query_map((), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?;
        let mut rows = Vec::new();
        for row in mapped {
            rows.push(row.map_err(|error| error.to_string())?);
        }
        rows
    };
    for (message_id, at) in rows {
        let canonical = WallClockWithTz::parse_rfc3339(&at)
            .map_err(|_| String::from("history timestamp cannot be normalized"))?
            .to_rfc3339_utc();
        tx.execute(
            "UPDATE history_message SET at_utc = ?2 WHERE message_id = ?1",
            rusqlite::params![message_id, canonical],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Introduces the derived recall token index and the partial B-tree indexes
/// behind `recall_candidates`.
///
/// `learning_memory_term` is rebuildable derived state (persistence-recovery
/// §5 R): every row is recomputed from `learning_memory.content` by the same
/// tokenizer the queries use, so the table is never the Memory's canonical
/// record and a future Memory deletion must delete its token rows with it.
/// The primary key serves the lexical companion-plus-term lookup; the
/// `memory_id` index serves the per-Memory refresh that rewrites one
/// Memory's rows on every revision. The two partial indexes cover only
/// non-suppressed rows, so each candidate arm walks at most `limit` live
/// entries instead of stepping over the whole suppressed prefix. Token rows
/// of suppressed memories stay in place; suppression is a recall-time
/// filter, and clearing it needs no reindexing.
const MIGRATION_V14: &str = "
CREATE TABLE IF NOT EXISTS learning_memory_term (
term TEXT NOT NULL,
memory_id TEXT NOT NULL,
companion_id TEXT NOT NULL,
PRIMARY KEY (companion_id, term, memory_id)
);
CREATE INDEX IF NOT EXISTS idx_learning_memory_term_memory ON learning_memory_term (memory_id);
CREATE INDEX IF NOT EXISTS idx_learning_memory_recall_newest ON learning_memory (companion_id) WHERE recall_suppressed = 0;
CREATE INDEX IF NOT EXISTS idx_learning_memory_recall_importance ON learning_memory (companion_id, importance DESC) WHERE recall_suppressed = 0;
";

/// Introduces the Task group. `task` is the D1 current row, `task_revision`
/// the D2 history, `task_context_entry` the adopted context (AU2/AU4: the
/// adopted purpose entry), and `workspace_assoc` holds a row only when a workspace
/// association was confirmed at creation. Creation writes all present rows in
/// one transaction, so a crash leaves no partial AU2 unit.
const MIGRATION_V15: &str = "
CREATE TABLE IF NOT EXISTS task (
task_id TEXT PRIMARY KEY,
revision INTEGER NOT NULL,
purpose_adopted_revision INTEGER NOT NULL,
purpose_text TEXT NOT NULL,
assignee TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS task_revision (
task_id TEXT NOT NULL,
revision INTEGER NOT NULL,
purpose_adopted_revision INTEGER NOT NULL,
purpose_text TEXT NOT NULL,
assignee TEXT NOT NULL,
PRIMARY KEY (task_id, revision)
);
CREATE TABLE IF NOT EXISTS task_context_entry (
entry_id TEXT PRIMARY KEY,
task_id TEXT NOT NULL,
revision INTEGER NOT NULL,
purpose_adopted_revision INTEGER NOT NULL,
origin_kind TEXT NOT NULL,
origin_source TEXT NOT NULL,
acquired_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_task_context_entry_task ON task_context_entry (task_id, revision);
CREATE TABLE IF NOT EXISTS workspace_assoc (
assoc_id TEXT PRIMARY KEY,
task_id TEXT NOT NULL,
folder TEXT NOT NULL,
save_target TEXT
);
CREATE INDEX IF NOT EXISTS idx_workspace_assoc_task ON workspace_assoc (task_id);
";

/// Derives the recall token rows for pre-index memories inside the
/// migration transaction, so an upgraded database answers lexical recall
/// from the index immediately. Fresh databases backfill zero rows.
fn backfill_recall_tokens(tx: &rusqlite::Transaction<'_>) -> Result<(), String> {
    let mut select = tx
        .prepare("SELECT memory_id, companion_id, content FROM learning_memory")
        .map_err(|error| error.to_string())?;
    let rows: Vec<(String, String, String)> = select
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    drop(select);
    let mut insert = tx
        .prepare(
            "INSERT OR IGNORE INTO learning_memory_term (term, memory_id, companion_id) VALUES (?1, ?2, ?3)",
        )
        .map_err(|error| error.to_string())?;
    for (memory, companion, content) in &rows {
        // Stored ids were encoded by this same store; an undecodable one is
        // a corrupt primary the migration must not silently reindex.
        decode_id(memory).map_err(|error| error.to_string())?;
        decode_id(companion).map_err(|error| error.to_string())?;
        for term in ene_learning::recall_index_terms(content) {
            insert
                .execute(rusqlite::params![term, memory, companion])
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Atomic: pending migrations and the version bump commit together in one
/// transaction, so a crash mid-migration rolls back to the pre-migration
/// state and the next open retries from scratch. The commit is the sole
/// version-advancement boundary. Forward-only: a database newer than this
/// binary understands is rejected instead of guessing.
pub(super) fn run(conn: &mut Connection) -> Result<(), String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let stored: i64 = tx
        .query_row("PRAGMA user_version", (), |row| row.get(0))
        .map_err(|error| error.to_string())?;
    let stored_version =
        u64::try_from(stored).map_err(|_| String::from("schema version out of range"))?;
    if stored_version > CURRENT_VERSION {
        return Err(String::from("schema version newer than supported"));
    }
    tx.execute_batch(SCHEMA)
        .map_err(|error| error.to_string())?;
    if stored_version < 2 {
        tx.execute_batch(MIGRATION_V2)
            .map_err(|error| error.to_string())?;
    }
    if stored_version < 3 {
        tx.execute_batch(MIGRATION_V3)
            .map_err(|error| error.to_string())?;
    }
    if stored_version < 4 {
        tx.execute_batch(MIGRATION_V4)
            .map_err(|error| error.to_string())?;
    }
    if stored_version < 5 {
        tx.execute_batch(MIGRATION_V5)
            .map_err(|error| error.to_string())?;
    }
    if stored_version < 6 {
        tx.execute_batch(MIGRATION_V6)
            .map_err(|error| error.to_string())?;
    }
    if stored_version < 7 {
        tx.execute_batch(MIGRATION_V7)
            .map_err(|error| error.to_string())?;
    }
    if stored_version < 8 {
        tx.execute_batch(MIGRATION_V8)
            .map_err(|error| error.to_string())?;
    }
    if stored_version < 9 {
        tx.execute_batch(MIGRATION_V9)
            .map_err(|error| error.to_string())?;
    }
    if stored_version < 10 {
        tx.execute_batch(MIGRATION_V10)
            .map_err(|error| error.to_string())?;
    }
    if stored_version < 11 {
        tx.execute_batch(MIGRATION_V11)
            .map_err(|error| error.to_string())?;
    }
    if stored_version < 12 {
        tx.execute_batch(MIGRATION_V12)
            .map_err(|error| error.to_string())?;
    }
    if stored_version < 13 {
        tx.execute_batch(MIGRATION_V13)
            .map_err(|error| error.to_string())?;
        backfill_history_at_utc(&tx)?;
    }
    if stored_version < 14 {
        tx.execute_batch(MIGRATION_V14)
            .map_err(|error| error.to_string())?;
        backfill_recall_tokens(&tx)?;
    }
    if stored_version < 15 {
        tx.execute_batch(MIGRATION_V15)
            .map_err(|error| error.to_string())?;
    }
    let current =
        i64::try_from(CURRENT_VERSION).map_err(|_| String::from("schema version out of range"))?;
    tx.pragma_update(None, "user_version", current)
        .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    Ok(())
}
