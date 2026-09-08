use crate::error::WorkError;
use rusqlite::{Connection, OptionalExtension};

/// Schema version for work tables in `companions.db`.
///
/// Stored as `meta.work_storage_version` rather than `PRAGMA user_version`
/// because this file is also opened by [`ene_companion::CompanionStore`].
pub const WORK_STORAGE_VERSION: u32 = 2;

const META_KEY: &str = "work_storage_version";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS jobs (
  id TEXT PRIMARY KEY,
  soul_id TEXT NOT NULL,
  title TEXT NOT NULL,
  goal TEXT NOT NULL,
  mode TEXT NOT NULL,
  status TEXT NOT NULL,
  progress_fraction REAL,
  progress_note TEXT,
  workspace_dir TEXT NOT NULL,
  error_class TEXT,
  created_from_turn TEXT,
  plan TEXT,
  brief TEXT,
  plan_approved INTEGER NOT NULL DEFAULT 0,
  success_criteria TEXT NOT NULL DEFAULT '[]',
  allowed_tools TEXT NOT NULL DEFAULT '[]',
  pending_allowed_tools TEXT,
  created_at TEXT NOT NULL,
  ended_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_jobs_soul ON jobs (soul_id, status, created_at DESC);
CREATE TABLE IF NOT EXISTS artifacts (
  id TEXT PRIMARY KEY,
  soul_id TEXT NOT NULL,
  job_id TEXT,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  path TEXT NOT NULL,
  mime TEXT,
  size_bytes INTEGER,
  created_at TEXT NOT NULL,
  delivered INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS schedules (
  id TEXT PRIMARY KEY,
  soul_id TEXT NOT NULL,
  name TEXT NOT NULL,
  spec TEXT NOT NULL,
  timezone TEXT NOT NULL,
  action_kind TEXT NOT NULL,
  action_ref TEXT,
  enabled INTEGER NOT NULL DEFAULT 1,
  important INTEGER NOT NULL DEFAULT 0,
  last_fired TEXT,
  next_fire TEXT
);
CREATE INDEX IF NOT EXISTS idx_sched_next ON schedules (enabled, next_fire);
CREATE TABLE IF NOT EXISTS delegation_events (
  event_seq INTEGER PRIMARY KEY AUTOINCREMENT,
  delegation_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_delegation_events_delegation
  ON delegation_events (delegation_id, event_seq);
CREATE TABLE IF NOT EXISTS mcp_servers (
  id TEXT PRIMARY KEY,
  transport TEXT NOT NULL,
  command TEXT,
  url TEXT,
  enabled INTEGER NOT NULL DEFAULT 1,
  args TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS tool_executions (
  execution_id TEXT PRIMARY KEY,
  job_id TEXT,
  soul_id TEXT NOT NULL,
  tool_name TEXT NOT NULL,
  plugin_id TEXT,
  call_id TEXT NOT NULL,
  status TEXT NOT NULL,
  error_class TEXT,
  result_json TEXT,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  completion_delivered INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_tool_exec_job ON tool_executions (job_id, status);
";

/// Apply work-table migrations in a single transaction per open.
///
/// Pre-versioned databases (no `work_storage_version`) take the v1 path, which
/// creates current tables and adds columns that older `CREATE TABLE` shapes
/// omitted. Duplicate-column errors are ignored so databases that already have
/// those columns (created by the previous probe/`ALTER` opener) stamp v1
/// without failing.
pub(crate) fn apply_migrations(conn: &mut Connection) -> Result<(), WorkError> {
    let current = read_version(conn)?;
    if current > WORK_STORAGE_VERSION {
        return Err(WorkError::StorageTooNew {
            found: current,
            supported: WORK_STORAGE_VERSION,
        });
    }
    if current == WORK_STORAGE_VERSION {
        return Ok(());
    }
    let tx = conn.transaction()?;
    let mut version = current;
    while version < WORK_STORAGE_VERSION {
        let next = version + 1;
        migrate(next, &tx)?;
        stamp_version(&tx, next)?;
        version = next;
    }
    tx.commit()?;
    Ok(())
}

fn migrate(version: u32, conn: &Connection) -> Result<(), WorkError> {
    match version {
        1 => migrate_v1(conn),
        2 => migrate_v2(conn),
        _ => Err(WorkError::MissingMigration(version)),
    }
}

fn migrate_v1(conn: &Connection) -> Result<(), WorkError> {
    conn.execute_batch(SCHEMA)?;
    add_column(
        conn,
        "ALTER TABLE jobs ADD COLUMN plan_approved INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column(
        conn,
        "ALTER TABLE jobs ADD COLUMN success_criteria TEXT NOT NULL DEFAULT '[]'",
    )?;
    add_column(
        conn,
        "ALTER TABLE jobs ADD COLUMN allowed_tools TEXT NOT NULL DEFAULT '[]'",
    )?;
    add_column(
        conn,
        "ALTER TABLE jobs ADD COLUMN pending_allowed_tools TEXT",
    )?;
    add_column(
        conn,
        "ALTER TABLE mcp_servers ADD COLUMN args TEXT NOT NULL DEFAULT '[]'",
    )?;
    Ok(())
}

fn migrate_v2(conn: &Connection) -> Result<(), WorkError> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS mailbox;
         CREATE TABLE IF NOT EXISTS delegation_events (
           event_seq INTEGER PRIMARY KEY AUTOINCREMENT,
           delegation_id TEXT NOT NULL,
           created_at TEXT NOT NULL,
           payload TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_delegation_events_delegation
           ON delegation_events (delegation_id, event_seq);",
    )?;
    Ok(())
}

fn add_column(conn: &Connection, sql: &str) -> Result<(), WorkError> {
    match conn.execute_batch(sql) {
        Ok(()) => Ok(()),
        Err(err) if is_duplicate_column(&err) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn is_duplicate_column(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(_, Some(message)) => {
            message.contains("duplicate column name")
        }
        _ => err.to_string().contains("duplicate column name"),
    }
}

fn read_version(conn: &Connection) -> Result<u32, WorkError> {
    let has_meta: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !has_meta {
        return Ok(0);
    }
    let value: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = ?1", [META_KEY], |row| {
            row.get(0)
        })
        .optional()?;
    match value {
        None => Ok(0),
        Some(raw) => raw
            .parse()
            .map_err(|_| WorkError::InvalidStorageVersion(raw)),
    }
}

fn stamp_version(conn: &Connection, version: u32) -> Result<(), WorkError> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![META_KEY, version.to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{WORK_STORAGE_VERSION, add_column, apply_migrations, read_version, stamp_version};
    use crate::error::WorkError;
    use crate::store::WorkStore;
    use crate::types::{DelegationMode, NewJob};
    use ene_companion::{CompanionStore, NewSoul};
    use ene_session::{DelegationId, SoulId};
    use rusqlite::{Connection, OptionalExtension};
    use tempfile::TempDir;

    const LEGACY_V0: &str = "
CREATE TABLE jobs (
  id TEXT PRIMARY KEY,
  soul_id TEXT NOT NULL,
  title TEXT NOT NULL,
  goal TEXT NOT NULL,
  mode TEXT NOT NULL,
  status TEXT NOT NULL,
  progress_fraction REAL,
  progress_note TEXT,
  workspace_dir TEXT NOT NULL,
  error_class TEXT,
  created_from_turn TEXT,
  plan TEXT,
  brief TEXT,
  created_at TEXT NOT NULL,
  ended_at TEXT
);
CREATE TABLE mcp_servers (
  id TEXT PRIMARY KEY,
  transport TEXT NOT NULL,
  command TEXT,
  url TEXT,
  enabled INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE mailbox (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  delegation_id TEXT NOT NULL,
  direction TEXT NOT NULL,
  kind TEXT NOT NULL,
  body TEXT NOT NULL,
  ts TEXT NOT NULL
);
";

    fn pragma_wal_and_fk(conn: &Connection) {
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal.to_ascii_lowercase(), "wal");
        let foreign_keys: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);
        let user_version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, 0);
    }

    fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
        conn.prepare(&format!("SELECT {column} FROM {table} LIMIT 0"))
            .is_ok()
    }

    fn table_exists(conn: &Connection, table: &str) -> bool {
        conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(true),
        )
        .optional()
        .is_ok_and(|row| row.is_some())
    }

    #[test]
    fn fresh_database_initializes_latest_schema() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("companions.db");
        let store = WorkStore::open(&path).unwrap();
        drop(store);
        let conn = Connection::open(&path).unwrap();
        pragma_wal_and_fk(&conn);
        assert_eq!(read_version(&conn).unwrap(), WORK_STORAGE_VERSION);
        assert!(column_exists(&conn, "jobs", "plan_approved"));
        assert!(column_exists(&conn, "jobs", "success_criteria"));
        assert!(column_exists(&conn, "jobs", "allowed_tools"));
        assert!(column_exists(&conn, "jobs", "pending_allowed_tools"));
        assert!(column_exists(&conn, "mcp_servers", "args"));
        assert!(table_exists(&conn, "delegation_events"));
        assert!(!table_exists(&conn, "mailbox"));
    }

    #[test]
    fn legacy_v0_database_gains_columns_and_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("companions.db");
        let job_id = DelegationId::new();
        let soul_id = SoulId::new();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(LEGACY_V0).unwrap();
            conn.execute(
                "INSERT INTO jobs (
                    id, soul_id, title, goal, mode, status, workspace_dir, created_at
                 ) VALUES (?1, ?2, 't', 'g', 'public', 'created', '/tmp', '2020-01-01T00:00:00Z')",
                rusqlite::params![job_id.to_string(), soul_id.to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO mcp_servers (id, transport, enabled) VALUES ('git', 'stdio', 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO mailbox (delegation_id, direction, kind, body, ts)
                 VALUES (?1, 'parent_to_child', 'note', 'hi', '2020-01-01T00:00:00Z')",
                rusqlite::params![job_id.to_string()],
            )
            .unwrap();
            assert!(!column_exists(&conn, "jobs", "plan_approved"));
            assert!(!column_exists(&conn, "mailbox", "question_seq"));
        }

        let store = WorkStore::open(&path).unwrap();
        let job = store.get_job(job_id).unwrap().unwrap();
        assert!(!job.plan_approved);
        assert!(job.success_criteria.is_empty());
        assert!(job.allowed_tools.is_empty());
        assert!(job.pending_allowed_tools.is_none());

        let mcp = store.list_mcp().unwrap();
        assert_eq!(mcp.len(), 1);
        assert!(mcp[0].args.is_empty());

        let events = store.delegation_events(job_id).unwrap();
        assert!(events.is_empty());

        let conn = Connection::open(&path).unwrap();
        pragma_wal_and_fk(&conn);
        assert_eq!(read_version(&conn).unwrap(), WORK_STORAGE_VERSION);
        assert!(!table_exists(&conn, "mailbox"));
    }

    #[test]
    fn partial_schema_adds_only_missing_columns() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("companions.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(LEGACY_V0).unwrap();
            conn.execute_batch(
                "ALTER TABLE jobs ADD COLUMN plan_approved INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE jobs ADD COLUMN success_criteria TEXT NOT NULL DEFAULT '[]';",
            )
            .unwrap();
        }
        WorkStore::open(&path).unwrap();
        let conn = Connection::open(&path).unwrap();
        assert!(column_exists(&conn, "jobs", "plan_approved"));
        assert!(column_exists(&conn, "jobs", "allowed_tools"));
        assert!(column_exists(&conn, "jobs", "pending_allowed_tools"));
        assert!(column_exists(&conn, "mcp_servers", "args"));
        assert!(table_exists(&conn, "delegation_events"));
        assert!(!table_exists(&conn, "mailbox"));
    }

    #[test]
    fn already_current_schema_without_version_stamps_without_failing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("companions.db");
        let store = WorkStore::open(&path).unwrap();
        drop(store);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute("DELETE FROM meta WHERE key = 'work_storage_version'", [])
                .unwrap();
            assert_eq!(read_version(&conn).unwrap(), 0);
        }
        WorkStore::open(&path).unwrap();
        let conn = Connection::open(&path).unwrap();
        assert_eq!(read_version(&conn).unwrap(), WORK_STORAGE_VERSION);
    }

    #[test]
    fn storage_too_new_is_rejected() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("companions.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
                .unwrap();
            stamp_version(&conn, WORK_STORAGE_VERSION + 1).unwrap();
        }
        let Err(err) = WorkStore::open(&path) else {
            panic!("expected StorageTooNew");
        };
        assert!(
            matches!(
                err,
                WorkError::StorageTooNew {
                    found,
                    supported: WORK_STORAGE_VERSION
                } if found == WORK_STORAGE_VERSION + 1
            ),
            "{err}"
        );
    }

    #[test]
    fn failed_migration_does_not_keep_partial_schema() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("companions.db");
        let mut conn = Connection::open(&path).unwrap();
        conn.execute_batch(LEGACY_V0).unwrap();
        {
            let tx = conn.transaction().unwrap();
            add_column(
                &tx,
                "ALTER TABLE jobs ADD COLUMN plan_approved INTEGER NOT NULL DEFAULT 0",
            )
            .unwrap();
            assert!(tx.execute_batch("ALTER TABLE jobs ADD COLUMN").is_err());
        }
        assert!(!column_exists(&conn, "jobs", "plan_approved"));
        assert_eq!(read_version(&conn).unwrap(), 0);
    }

    #[test]
    fn companion_and_work_share_file_without_clobbering_meta() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("companions.db");
        let companions = CompanionStore::open(&path).unwrap();
        let soul = companions.create_soul(&NewSoul::text_only("char")).unwrap();
        let store = WorkStore::open(&path).unwrap();
        store
            .insert_job(&NewJob {
                id: None,
                soul_id: soul.id,
                title: "t".into(),
                goal: "g".into(),
                mode: DelegationMode::Public,
                workspace_dir: "/tmp".into(),
                created_from_turn: None,
                plan: None,
                brief: None,
                success_criteria: Vec::new(),
                allowed_tools: Vec::new(),
            })
            .unwrap();
        assert_eq!(companions.get_soul(soul.id).unwrap().unwrap().id, soul.id);
        assert_eq!(store.list_jobs(soul.id).unwrap().len(), 1);

        let conn = Connection::open(&path).unwrap();
        pragma_wal_and_fk(&conn);
        assert_eq!(read_version(&conn).unwrap(), WORK_STORAGE_VERSION);

        let work_first = dir.path().join("other.db");
        WorkStore::open(&work_first).unwrap();
        let companions = CompanionStore::open(&work_first).unwrap();
        companions.create_soul(&NewSoul::text_only("char")).unwrap();
        let conn = Connection::open(&work_first).unwrap();
        assert_eq!(read_version(&conn).unwrap(), WORK_STORAGE_VERSION);
    }

    #[test]
    fn apply_migrations_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("db.sqlite");
        let mut conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .unwrap();
        apply_migrations(&mut conn).unwrap();
        apply_migrations(&mut conn).unwrap();
        assert_eq!(read_version(&conn).unwrap(), WORK_STORAGE_VERSION);
    }
}
