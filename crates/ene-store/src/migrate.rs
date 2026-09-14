use rusqlite::{Connection, TransactionBehavior};

const CURRENT_VERSION: i64 = 24;

const SCHEMA: &str = "
CREATE TABLE action_attempt (
attempt_id TEXT PRIMARY KEY,
task_id TEXT NOT NULL,
task_revision INTEGER NOT NULL,
delegation_id TEXT NOT NULL,
workspace_assoc_id TEXT NOT NULL,
real_target TEXT NOT NULL,
operation TEXT NOT NULL,
relied_evaluation TEXT NOT NULL UNIQUE,
certainty TEXT NOT NULL,
grounds TEXT NULL,
started_at TEXT NOT NULL
);
CREATE TABLE companion (
companion_id TEXT PRIMARY KEY,
lifecycle TEXT NOT NULL,
created_at TEXT NOT NULL
);
CREATE TABLE consent_record (
capability TEXT PRIMARY KEY,
id TEXT NOT NULL,
rev INTEGER NOT NULL,
provider TEXT NOT NULL,
model TEXT NOT NULL,
credential_id TEXT NOT NULL
);
CREATE TABLE credential_pending (
provider TEXT NOT NULL,
label TEXT NOT NULL,
requested_at TEXT NOT NULL,
PRIMARY KEY (provider, label)
);
CREATE TABLE credential_ref (
id TEXT PRIMARY KEY,
provider TEXT NOT NULL,
label TEXT NOT NULL,
UNIQUE (provider, label)
);
CREATE TABLE credential_set (
id INTEGER PRIMARY KEY CHECK (id = 1),
rev INTEGER NOT NULL
);
CREATE TABLE delegation (
delegation_id TEXT PRIMARY KEY,
task_id TEXT NOT NULL,
task_revision INTEGER NOT NULL,
delegator TEXT NOT NULL,
agent TEXT NOT NULL,
scope_assoc TEXT NULL,
scope_folder TEXT NULL,
scope_save_target TEXT NULL
);
CREATE TABLE erasure_condition (
operation_id TEXT NOT NULL,
sweep INTEGER NOT NULL,
PRIMARY KEY (operation_id, sweep)
);
CREATE TABLE erasure_condition_source (
operation_id TEXT NOT NULL,
sweep INTEGER NOT NULL,
source TEXT NOT NULL,
PRIMARY KEY (operation_id, sweep, source)
);
CREATE TABLE history_message (
message_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
round_id TEXT NOT NULL,
role TEXT NOT NULL,
body TEXT NOT NULL,
lang TEXT NOT NULL,
at TEXT NOT NULL,
presence_generation INTEGER NOT NULL,
local_id TEXT NULL,
command_id TEXT NULL,
round_wire TEXT NULL,
client_counter INTEGER NULL,
client_random INTEGER NULL,
round_intent TEXT NULL,
round_intent_ref TEXT NULL,
at_utc TEXT NULL
);
CREATE TABLE inference_attempt (
ticket TEXT PRIMARY KEY,
consent_id TEXT NOT NULL,
consent_rev INTEGER NOT NULL,
provider TEXT NOT NULL,
model TEXT NOT NULL,
started_at TEXT NOT NULL,
capability TEXT NOT NULL DEFAULT 'dialogue',
consumer TEXT NULL,
purpose TEXT NULL,
credential_set_rev INTEGER NULL,
delegation_id TEXT NULL,
task_id TEXT NULL,
task_revision INTEGER NULL,
data_use_count INTEGER NULL
);
CREATE TABLE inference_attempt_data_use (
ticket TEXT NOT NULL,
ordinal INTEGER NOT NULL,
source TEXT NOT NULL,
PRIMARY KEY (ticket, ordinal)
);
CREATE TABLE learning_memory (
memory_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
revision INTEGER NOT NULL,
content TEXT NOT NULL,
importance INTEGER NOT NULL,
temporal TEXT NOT NULL,
recall_suppressed INTEGER NOT NULL,
updated_at TEXT NOT NULL
);
CREATE TABLE learning_memory_revision (
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
CREATE TABLE learning_memory_term (
term TEXT NOT NULL,
memory_id TEXT NOT NULL,
companion_id TEXT NOT NULL,
PRIMARY KEY (companion_id, term, memory_id)
);
CREATE TABLE learning_summary (
summary_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
content TEXT NOT NULL,
source_kind TEXT NOT NULL,
source_start TEXT NOT NULL,
source_end TEXT NOT NULL,
formed_at TEXT NOT NULL
);
CREATE TABLE management_intent (
intent_id TEXT PRIMARY KEY,
kind TEXT NOT NULL,
target TEXT NOT NULL,
base TEXT NOT NULL,
rationale_origin TEXT NOT NULL,
rationale_quote TEXT NULL,
outcome TEXT NOT NULL,
mark TEXT NULL
);
CREATE TABLE paired_device (
device_id TEXT PRIMARY KEY,
descriptor TEXT NOT NULL,
paired_at TEXT NOT NULL,
wire TEXT NULL
);
CREATE TABLE pairing_pending (
descriptor TEXT PRIMARY KEY,
requested_at TEXT NOT NULL
);
CREATE TABLE presence_attribution (
companion_id TEXT PRIMARY KEY,
state TEXT NOT NULL,
active_client TEXT,
generation INTEGER NOT NULL
);
CREATE TABLE presence_transition_log (
transition_seq INTEGER PRIMARY KEY AUTOINCREMENT,
companion_id TEXT NOT NULL,
old_state TEXT NOT NULL,
new_state TEXT NOT NULL,
old_gen INTEGER NOT NULL,
new_gen INTEGER NOT NULL,
reason TEXT NOT NULL,
at TEXT NOT NULL
);
CREATE TABLE task (
task_id TEXT PRIMARY KEY,
revision INTEGER NOT NULL,
purpose_adopted_revision INTEGER NOT NULL,
purpose_text TEXT NOT NULL,
assignee TEXT NOT NULL,
progress TEXT NULL
);
CREATE TABLE task_context_entry (
entry_id TEXT PRIMARY KEY,
task_id TEXT NOT NULL,
revision INTEGER NOT NULL,
item_kind TEXT NOT NULL,
purpose_adopted_revision INTEGER NULL,
origin_kind TEXT NOT NULL,
origin_source TEXT NOT NULL,
acquired_at TEXT NOT NULL
);
CREATE TABLE task_result (
result_id TEXT PRIMARY KEY,
task_id TEXT NOT NULL,
task_revision INTEGER NOT NULL,
delegation_id TEXT NOT NULL UNIQUE,
body TEXT NOT NULL,
adopted_revision INTEGER NULL,
recorded_at TEXT NOT NULL
);
CREATE TABLE task_result_attempt (
result_id TEXT NOT NULL,
attempt_id TEXT NOT NULL,
PRIMARY KEY (result_id, attempt_id)
);
CREATE TABLE task_revision (
task_id TEXT NOT NULL,
revision INTEGER NOT NULL,
purpose_adopted_revision INTEGER NOT NULL,
purpose_text TEXT NOT NULL,
assignee TEXT NOT NULL,
PRIMARY KEY (task_id, revision)
);
CREATE TABLE undelivered (
undelivered_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
source_message TEXT NOT NULL,
status TEXT NOT NULL,
round_id TEXT NOT NULL,
presence_generation INTEGER NOT NULL,
created_at TEXT NOT NULL
);
CREATE TABLE usage_fact (
ticket TEXT PRIMARY KEY,
provider TEXT NOT NULL,
model TEXT NOT NULL,
input_tokens INTEGER,
output_tokens INTEGER,
source TEXT NOT NULL
);
CREATE TABLE workspace_assoc (
assoc_id TEXT PRIMARY KEY,
task_id TEXT NOT NULL,
folder TEXT NOT NULL,
save_target TEXT
);
CREATE INDEX idx_action_attempt_delegation ON action_attempt (delegation_id);
CREATE INDEX idx_action_attempt_task ON action_attempt (task_id);
CREATE INDEX idx_erasure_condition_source_source ON erasure_condition_source (source);
CREATE INDEX idx_history_message_companion ON history_message (companion_id);
CREATE INDEX idx_history_message_companion_at ON history_message (companion_id, at_utc);
CREATE UNIQUE INDEX idx_history_message_companion_command ON history_message (companion_id, command_id);
CREATE INDEX idx_history_message_companion_role ON history_message (companion_id, role);
CREATE INDEX idx_history_message_round ON history_message (round_id);
CREATE INDEX idx_inference_attempt_delegation ON inference_attempt (delegation_id);
CREATE INDEX idx_learning_memory_companion ON learning_memory (companion_id);
CREATE INDEX idx_learning_memory_recall_importance ON learning_memory (companion_id, importance DESC) WHERE recall_suppressed = 0;
CREATE INDEX idx_learning_memory_recall_newest ON learning_memory (companion_id) WHERE recall_suppressed = 0;
CREATE INDEX idx_learning_memory_term_memory ON learning_memory_term (memory_id);
CREATE INDEX idx_paired_device_descriptor ON paired_device (descriptor);
CREATE UNIQUE INDEX idx_paired_device_wire ON paired_device (wire);
CREATE INDEX idx_task_context_entry_task ON task_context_entry (task_id, revision);
CREATE INDEX idx_task_result_unadopted ON task_result (recorded_at, result_id) WHERE adopted_revision IS NULL;
CREATE INDEX idx_undelivered_companion_status ON undelivered (companion_id, status);
CREATE INDEX idx_workspace_assoc_task ON workspace_assoc (task_id);
INSERT INTO credential_set (id, rev) VALUES (1, 0);
";

/// Initializes only an empty database. Existing databases must have the exact
/// current version; opening them never repairs or rewrites durable state.
/// The immediate transaction serializes initializers and publishes the schema,
/// seed, and version atomically.
pub(super) fn run(conn: &mut Connection) -> Result<(), String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let stored: i64 = tx
        .query_row("PRAGMA user_version", (), |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if stored == CURRENT_VERSION {
        return Ok(());
    }
    if stored != 0 {
        return Err(String::from("unsupported schema version"));
    }
    let populated: bool = tx
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%')",
            (),
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if populated {
        return Err(String::from("unversioned database is not empty"));
    }
    tx.execute_batch(SCHEMA)
        .map_err(|error| error.to_string())?;
    tx.pragma_update(None, "user_version", CURRENT_VERSION)
        .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_once_and_reject_unsupported_schemas_without_changes() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();
        conn.execute("UPDATE credential_set SET rev = 7", ())
            .unwrap();
        let changes = conn.total_changes();
        run(&mut conn).unwrap();
        assert_eq!(conn.total_changes(), changes);
        assert_eq!(
            conn.query_row("SELECT rev FROM credential_set", (), |r| r.get::<_, i64>(0))
                .unwrap(),
            7
        );
        for version in [-1, 0, 1, 23, 25] {
            conn.pragma_update(None, "user_version", version).unwrap();
            assert!(run(&mut conn).is_err());
            assert_eq!(
                conn.query_row("PRAGMA user_version", (), |r| r.get::<_, i64>(0))
                    .unwrap(),
                version
            );
            assert_eq!(conn.total_changes(), changes);
        }
    }

    #[test]
    fn failed_initialization_rolls_back_and_can_be_retried() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("atomic.db");
        let reader = Connection::open(&path).unwrap();
        reader
            .execute_batch("BEGIN; SELECT * FROM sqlite_schema;")
            .unwrap();
        let mut writer = Connection::open(&path).unwrap();
        writer.busy_timeout(std::time::Duration::ZERO).unwrap();
        // The reader allows DDL under the reserved lock but prevents commit.
        assert!(run(&mut writer).is_err());
        assert_eq!(
            writer
                .query_row("SELECT COUNT(*) FROM sqlite_schema", (), |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            writer
                .query_row("PRAGMA user_version", (), |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        reader.execute_batch("ROLLBACK").unwrap();
        run(&mut writer).unwrap();
    }

    #[test]
    fn concurrent_initializers_publish_one_schema_and_seed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("concurrent.db");
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    let mut conn = Connection::open(&path).unwrap();
                    conn.busy_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    barrier.wait();
                    run(&mut conn).unwrap();
                    assert_eq!(
                        conn.query_row("SELECT COUNT(*) FROM credential_set", (), |r| r
                            .get::<_, i64>(0))
                            .unwrap(),
                        1
                    );
                });
            }
        });
    }
}
