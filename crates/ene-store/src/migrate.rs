use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

/// Schema version applied by [`run`](run).
///
/// Version 2 adds the nullable `history_message.local_id` column with its
/// device-pairing tables (the companion-local unique index it briefly
/// carried is dropped again by version 3). Version 3 adds the nullable
/// `history_message.command_id` column with the durable
/// `(companion_id, command_id)` unique replay index; `local_id` stays as
/// correspondence metadata only. Version 4 adds the `credential_pending`
/// table for registration approvals; the usable marker stays
/// `credential_ref`, so no approved table is created. Version 8 adds the
/// nullable `history_message.round_intent` / `round_intent_ref` columns
/// that persist the client round intent of the request fingerprint.
const CURRENT_VERSION: u64 = 8;

/// Forward-only schema: tables first, then the supporting indexes.
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

/// Version 2 upgrade, applied once when the stored version is below 2.
///
/// Forward-only: existing history rows keep `local_id` NULL, and the new
/// tables use `IF NOT EXISTS` so a fresh database and an upgraded one
/// converge on the same shape. `NULL` local ids never collide under the
/// unique index because SQLite treats each `NULL` as distinct. Version 3
/// drops this index again when it promotes `command_id` to the durable
/// replay key; the column itself stays as correspondence metadata.
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

/// Version 3 upgrade, applied once when the stored version is below 3.
///
/// Forward-only: existing history rows keep `command_id` NULL, and `NULL`
/// command ids never collide under the new unique index because SQLite
/// treats each `NULL` as distinct. The retired
/// `(companion_id, local_id)` unique index is dropped; the `local_id`
/// column stays as stored correspondence metadata, never a key.
const MIGRATION_V3: &str = "
ALTER TABLE history_message ADD COLUMN command_id TEXT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_history_message_companion_command ON history_message (companion_id, command_id);
DROP INDEX IF EXISTS idx_history_message_companion_local;
";

/// Version 4 upgrade, applied once when the stored version is below 4.
///
/// Forward-only: introduces `credential_pending` for registration
/// approvals. Fresh and upgraded databases converge via `IF NOT EXISTS`;
/// no pre-existing rows can reference the new table, so nothing is
/// backfilled. The usable marker stays `credential_ref` (the register
/// flow creates the ref only at approval time), so no approved table is
/// created here.
const MIGRATION_V4: &str = "
CREATE TABLE IF NOT EXISTS credential_pending (
provider TEXT NOT NULL,
label TEXT NOT NULL,
requested_at TEXT NOT NULL,
PRIMARY KEY (provider, label)
);
";

/// Version 5 upgrade, applied once when the stored version is below 5.
///
/// Forward-only: adds the opaque wire projections and the replay
/// fingerprint. `history_message.round_wire` carries the only round
/// string that ever crosses the wire (minted fresh per round,
/// unrelated to the domain bytes); `client_counter`/`client_random`
/// carry the sending incarnation for fingerprint comparison; all three
/// stay `NULL` on pre-existing rows, which read back as absent. Same
/// for `paired_device.wire` (opaque device projection with a uniqueness
/// guard for new rows), except pre-existing paired rows are backfilled
/// to the legacy continuity projection (their device identity rendering)
/// so already-provisioned clients keep resolving after migration; new
/// approvals mint a fresh opaque projection instead. Fresh and upgraded
/// databases converge via `IF NOT EXISTS`; nothing else is backfilled.
const MIGRATION_V5: &str = "
ALTER TABLE history_message ADD COLUMN round_wire TEXT NULL;
ALTER TABLE history_message ADD COLUMN client_counter INTEGER NULL;
ALTER TABLE history_message ADD COLUMN client_random INTEGER NULL;
ALTER TABLE paired_device ADD COLUMN wire TEXT NULL;
UPDATE paired_device SET wire = device_id WHERE wire IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_paired_device_wire ON paired_device (wire);
";

/// Version 6 upgrade, applied once when the stored version is below 6.
///
/// Forward-only: introduces `management_intent` for durable management
/// intent replay (design §18.2 idempotency keys). Each row binds one
/// intent key to the fingerprint it decided on plus its terminal
/// outcome snapshot, so an exact retry replays without re-executing
/// while a reused id with new content conflicts instead of rebinding.
/// Fresh and upgraded databases converge via `IF NOT EXISTS`; no
/// pre-existing rows can reference the new table, so nothing is
/// backfilled. Intents decided before this version simply have no
/// replay row: their retries take the normal premise-checked path.
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

/// Version 7 upgrade, applied once when the stored version is below 7.
///
/// Forward-only: introduces `inference_attempt` for the provider-I/O
/// linearization point. Each row claims one ticket's attempt under one
/// consent premise in the same short transaction that verifies it, so a
/// consent mutation either precedes the claim (the claim fails stale
/// before any byte leaves) or follows it (adoption decides separately).
/// Fresh and upgraded databases converge via `IF NOT EXISTS`; ticket
/// ids are single-use, so nothing is backfilled and re-claiming one
/// ticket answers stale instead of sending twice.
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

/// Version 8 upgrade, applied once when the stored version is below 8.
///
/// Forward-only: adds the persisted client round intent of the history
/// request fingerprint (`round_intent` kind, `round_intent_ref`
/// payload). The intent is request semantics — Auto, force-new, or a
/// join of the round reference the Client sent — so a restart can still
/// decide replay from durable state. Pre-existing rows keep both
/// columns `NULL`: their intent was never stored, so their sameness
/// cannot be proven and every replay attempt against them fails closed
/// instead of being guessed.
const MIGRATION_V8: &str = "
ALTER TABLE history_message ADD COLUMN round_intent TEXT NULL;
ALTER TABLE history_message ADD COLUMN round_intent_ref TEXT NULL;
";
/// Creates or upgrades the schema on an open connection.
///
/// Atomic: pending migrations and the version bump commit together in
/// one transaction, so a crash mid-migration rolls back to the
/// pre-migration state and the next open retries from scratch. The
/// commit is the sole version-advancement boundary. Rejects a database
/// newer than this binary understands instead of guessing.
pub(super) fn run(conn: &mut Connection) -> Result<(), String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS _schema_version (version INTEGER NOT NULL)")
        .map_err(|error| error.to_string())?;
    let stored: Option<i64> = tx
        .query_row("SELECT version FROM _schema_version LIMIT 1", (), |row| {
            row.get(0)
        })
        .optional()
        .map_err(|error| error.to_string())?;
    let stored_version = match stored {
        Some(raw) => u64::try_from(raw).map_err(|_| String::from("schema version out of range"))?,
        None => 0,
    };
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
    let current =
        i64::try_from(CURRENT_VERSION).map_err(|_| String::from("schema version out of range"))?;
    if stored.is_none() {
        tx.execute(
            "INSERT INTO _schema_version (version) VALUES (?1)",
            rusqlite::params![current],
        )
        .map_err(|error| error.to_string())?;
    } else if stored_version < CURRENT_VERSION {
        tx.execute(
            "UPDATE _schema_version SET version = ?1",
            rusqlite::params![current],
        )
        .map_err(|error| error.to_string())?;
    }
    tx.commit().map_err(|error| error.to_string())?;
    Ok(())
}
