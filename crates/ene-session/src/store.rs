use crate::error::SessionError;
use crate::event::{
    EventKind, EventPayload, LoggedEvent, NewEvent, SessionCreatedBy, SessionEndReason,
    TurnOutcome, v1,
};
use crate::ids::{BodyId, DelegationId, SessionId, SoulId, UsageId};
use crate::inbox::{InboxItem, OpenTurn, open_turns, unclaimed_inbox};
use crate::usage::{NewUsage, UsageRow, UsageTotals};
use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::oneshot;

/// On-disk schema version. Version greater than this refuses to open.
pub const STORAGE_VERSION: u32 = 1;

const BUSY_TIMEOUT_MS: u64 = 5000;

/// Conversation vs diagnostic child session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Conversation,
    Delegation,
}

impl SessionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Delegation => "delegation",
        }
    }

    fn parse(raw: &str) -> Self {
        if raw == "delegation" {
            Self::Delegation
        } else {
            Self::Conversation
        }
    }
}

/// Arguments for opening a new session row + `session/start`.
#[derive(Debug, Clone, Copy)]
pub struct NewSession {
    pub soul_id: SoulId,
    pub body_id: Option<BodyId>,
    pub kind: SessionKind,
    pub delegation_id: Option<DelegationId>,
    pub created_by: SessionCreatedBy,
}

/// All-or-nothing write (entries + usage). No registers in v1.0.
#[derive(Debug, Clone, Default)]
pub struct Transaction {
    pub entries: Vec<NewEvent>,
    pub usage: Vec<NewUsage>,
}

/// Result of a committed transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitResult {
    pub first_seq: Option<u64>,
    pub seqs: Vec<u64>,
    pub ts: String,
}

/// Content-addressed spill or image blob beside `sessions.db`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpillObject {
    pub sha256: String,
    pub mime: Option<String>,
    pub bytes: Vec<u8>,
}

/// Listing row (projection; log remains authoritative).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    pub id: SessionId,
    pub soul_id: SoulId,
    pub kind: SessionKind,
    pub delegation_id: Option<DelegationId>,
    pub title: Option<String>,
    pub created_at: String,
    pub ended_at: Option<String>,
    pub end_reason: Option<String>,
    pub archived: bool,
    pub parent_session_id: Option<SessionId>,
    pub fork_seq: Option<u64>,
    pub next_seq: u64,
}

/// D-5 recovery summary for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    pub session_id: SessionId,
    pub interrupted_turns: Vec<OpenTurn>,
    pub abandoned_inbox: Vec<InboxItem>,
}

enum WriteOp {
    Create {
        spec: NewSession,
        reply: oneshot::Sender<Result<SessionId, SessionError>>,
    },
    Commit {
        tx: Transaction,
        reply: oneshot::Sender<Result<CommitResult, SessionError>>,
    },
    Fork {
        source: SessionId,
        boundary: u64,
        reply: oneshot::Sender<Result<SessionId, SessionError>>,
    },
    Recover {
        reply: oneshot::Sender<Result<Vec<RecoveryReport>, SessionError>>,
    },
    CloseWriter {
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    ReopenWriter {
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    RecordSpill {
        sha256: String,
        size_bytes: u64,
        mime: Option<String>,
        reply: oneshot::Sender<Result<(), SessionError>>,
    },
    Shutdown,
}

/// Append-only session store with a single writer thread.
pub struct SessionStore {
    tx: mpsc::Sender<WriteOp>,
    join: Option<JoinHandle<()>>,
    reader: parking_lot::Mutex<Connection>,
    path: PathBuf,
}

impl SessionStore {
    /// Open (or create) `sessions.db` at `path`.
    pub async fn open(path: impl AsRef<Path>, synchronous: &str) -> Result<Self, SessionError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let sync = synchronous.to_owned();
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        let path_for_thread = path.clone();
        let join = std::thread::Builder::new()
            .name("ene-session-writer".to_owned())
            .spawn(move || writer_loop(path_for_thread, sync, rx, ready_tx))
            .map_err(SessionError::from)?;
        ready_rx.await.map_err(|_| SessionError::WriterClosed)??;
        let reader = open_connection(&path, synchronous)?;
        Ok(Self {
            tx,
            join: Some(join),
            reader: parking_lot::Mutex::new(reader),
            path,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Directory for content-addressed spill / image blobs (`<data>/spill`).
    #[must_use]
    pub fn spill_dir(&self) -> PathBuf {
        spill_dir_for(&self.path)
    }

    /// Store bytes under their SHA-256 hex and record them in `spill_objects`.
    pub async fn put_spill(
        &self,
        bytes: &[u8],
        mime: Option<&str>,
    ) -> Result<String, SessionError> {
        let sha256 = sha256_hex(bytes);
        let dir = spill_dir_for(&self.path);
        std::fs::create_dir_all(&dir)?;
        let dest = dir.join(&sha256);
        if !dest.exists() {
            let tmp = dir.join(format!(".{sha256}.tmp"));
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, &dest)?;
        }
        let size_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let mime = mime.map(ToOwned::to_owned);
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(WriteOp::RecordSpill {
                sha256: sha256.clone(),
                size_bytes,
                mime,
                reply,
            })
            .map_err(|_| SessionError::WriterClosed)?;
        rx.await.map_err(|_| SessionError::WriterClosed)??;
        Ok(sha256)
    }

    /// Load a spill blob by SHA-256 hex id. Missing files are `Ok(None)`.
    pub fn get_spill(&self, id: &str) -> Result<Option<SpillObject>, SessionError> {
        let sha256 = parse_spill_id(id)?;
        let dest = spill_dir_for(&self.path).join(sha256);
        if !dest.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&dest)?;
        let mime = {
            let reader = self.reader.lock();
            reader
                .query_row(
                    "SELECT mime FROM spill_objects WHERE sha256 = ?1",
                    [sha256],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten()
        };
        Ok(Some(SpillObject {
            sha256: sha256.to_owned(),
            mime,
            bytes,
        }))
    }

    pub async fn create_session(&self, spec: NewSession) -> Result<SessionId, SessionError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(WriteOp::Create { spec, reply })
            .map_err(|_| SessionError::WriterClosed)?;
        rx.await.map_err(|_| SessionError::WriterClosed)?
    }

    pub async fn commit(&self, tx: Transaction) -> Result<CommitResult, SessionError> {
        if let Some(first) = tx.entries.first()
            && tx
                .entries
                .iter()
                .any(|event| event.session_id != first.session_id)
        {
            return Err(SessionError::MixedSessionTransaction);
        }
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(WriteOp::Commit { tx, reply })
            .map_err(|_| SessionError::WriterClosed)?;
        rx.await.map_err(|_| SessionError::WriterClosed)?
    }

    pub async fn fork(&self, source: SessionId, boundary: u64) -> Result<SessionId, SessionError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(WriteOp::Fork {
                source,
                boundary,
                reply,
            })
            .map_err(|_| SessionError::WriterClosed)?;
        rx.await.map_err(|_| SessionError::WriterClosed)?
    }

    /// Close open turns and abandon unclaimed inbox (D-5). Does not resume work.
    pub async fn recover_interrupted(&self) -> Result<Vec<RecoveryReport>, SessionError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(WriteOp::Recover { reply })
            .map_err(|_| SessionError::WriterClosed)?;
        rx.await.map_err(|_| SessionError::WriterClosed)?
    }

    pub async fn close_writer(&self) -> Result<(), SessionError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(WriteOp::CloseWriter { reply })
            .map_err(|_| SessionError::WriterClosed)?;
        rx.await.map_err(|_| SessionError::WriterClosed)?
    }

    pub async fn reopen_writer(&self) -> Result<(), SessionError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(WriteOp::ReopenWriter { reply })
            .map_err(|_| SessionError::WriterClosed)?;
        rx.await.map_err(|_| SessionError::WriterClosed)?
    }

    pub fn reload_reader(&self, synchronous: &str) -> Result<(), SessionError> {
        let conn = open_connection(&self.path, synchronous)?;
        *self.reader.lock() = conn;
        Ok(())
    }

    pub fn load_events(
        &self,
        session_id: SessionId,
        since_seq: u64,
    ) -> Result<Vec<LoggedEvent>, SessionError> {
        let conn = self.reader.lock();
        load_events_conn(&conn, session_id, since_seq)
    }

    pub fn get_session(&self, session_id: SessionId) -> Result<SessionMeta, SessionError> {
        let conn = self.reader.lock();
        get_session_conn(&conn, session_id)?
            .ok_or_else(|| SessionError::SessionNotFound(session_id.to_string()))
    }

    pub fn list_sessions(&self, soul_id: Option<SoulId>) -> Result<Vec<SessionMeta>, SessionError> {
        let conn = self.reader.lock();
        list_sessions_conn(&conn, soul_id)
    }

    pub fn last_event_ts(&self, session_id: SessionId) -> Result<Option<String>, SessionError> {
        let conn = self.reader.lock();
        let mut stmt = conn.prepare(
            "SELECT ts FROM session_events WHERE session_id = ?1 ORDER BY seq DESC LIMIT 1",
        )?;
        stmt.query_row([session_id.to_string()], |row| row.get(0))
            .optional()
            .map_err(SessionError::from)
    }

    pub fn usage_totals(&self, session_id: SessionId) -> Result<UsageTotals, SessionError> {
        let conn = self.reader.lock();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
                    COUNT(*)
             FROM session_usage WHERE session_id = ?1",
        )?;
        stmt.query_row([session_id.to_string()], |row| {
            Ok(UsageTotals {
                input_tokens: row.get::<_, i64>(0)? as u64,
                output_tokens: row.get::<_, i64>(1)? as u64,
                cache_read_tokens: row.get::<_, i64>(2)? as u64,
                cache_write_tokens: row.get::<_, i64>(3)? as u64,
                rows: row.get::<_, i64>(4)? as u64,
            })
        })
        .map_err(SessionError::from)
    }

    pub fn list_usage(&self, session_id: SessionId) -> Result<Vec<UsageRow>, SessionError> {
        let conn = self.reader.lock();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, seq, soul_id, lane, task, provider, model, entry_seq,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    cost_micro_usd, adjustment, created_at
             FROM session_usage WHERE session_id = ?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map([session_id.to_string()], |row| {
            Ok(UsageRow {
                id: UsageId::from_uuid(parse_uuid(&row.get::<_, String>(0)?)?),
                session_id: SessionId::from_uuid(parse_uuid(&row.get::<_, String>(1)?)?),
                seq: row.get::<_, i64>(2)? as u64,
                soul_id: crate::ids::SoulId::from_uuid(parse_uuid(&row.get::<_, String>(3)?)?),
                lane: row.get(4)?,
                task: row.get(5)?,
                provider: row.get(6)?,
                model: row.get(7)?,
                entry_seq: row.get::<_, Option<i64>>(8)?.map(|value| value as u64),
                input_tokens: row.get::<_, i64>(9)? as u32,
                output_tokens: row.get::<_, i64>(10)? as u32,
                cache_read_tokens: row.get::<_, i64>(11)? as u32,
                cache_write_tokens: row.get::<_, i64>(12)? as u32,
                cost_micro_usd: row.get(13)?,
                adjustment: row.get::<_, i64>(14)? != 0,
                created_at: row.get(15)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(SessionError::from)
    }
}

impl Drop for SessionStore {
    fn drop(&mut self) {
        drop(self.tx.send(WriteOp::Shutdown));
        if let Some(join) = self.join.take() {
            drop(join.join());
        }
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "writer thread must own the db path, pragma, and command channel"
)]
fn writer_loop(
    path: PathBuf,
    synchronous: String,
    rx: mpsc::Receiver<WriteOp>,
    ready: oneshot::Sender<Result<(), SessionError>>,
) {
    let opened = open_connection(&path, &synchronous).and_then(|conn| {
        init_schema(&conn)?;
        Ok(conn)
    });
    let mut conn = match opened {
        Ok(conn) => {
            drop(ready.send(Ok(())));
            Some(conn)
        }
        Err(err) => {
            drop(ready.send(Err(err)));
            return;
        }
    };
    while let Ok(op) = rx.recv() {
        match op {
            WriteOp::Shutdown => break,
            WriteOp::CloseWriter { reply } => {
                if let Some(open) = conn.take() {
                    drop(open.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);"));
                }
                drop(reply.send(Ok(())));
            }
            WriteOp::ReopenWriter { reply } => {
                let result = if conn.is_some() {
                    Ok(())
                } else {
                    open_connection(&path, &synchronous).map(|opened| {
                        conn = Some(opened);
                    })
                };
                drop(reply.send(result));
            }
            WriteOp::Create { spec, reply } => {
                let result = conn
                    .as_mut()
                    .ok_or(SessionError::WriterClosed)
                    .and_then(|open| create_session_conn(open, spec));
                drop(reply.send(result));
            }
            WriteOp::Commit { tx, reply } => {
                let result = conn
                    .as_mut()
                    .ok_or(SessionError::WriterClosed)
                    .and_then(|open| commit_conn(open, &tx));
                drop(reply.send(result));
            }
            WriteOp::Fork {
                source,
                boundary,
                reply,
            } => {
                let result = conn
                    .as_mut()
                    .ok_or(SessionError::WriterClosed)
                    .and_then(|open| fork_conn(open, source, boundary));
                drop(reply.send(result));
            }
            WriteOp::Recover { reply } => {
                let result = conn
                    .as_mut()
                    .ok_or(SessionError::WriterClosed)
                    .and_then(recover_conn);
                drop(reply.send(result));
            }
            WriteOp::RecordSpill {
                sha256,
                size_bytes,
                mime,
                reply,
            } => {
                let result = conn
                    .as_mut()
                    .ok_or(SessionError::WriterClosed)
                    .and_then(|open| record_spill_conn(open, &sha256, size_bytes, mime.as_deref()));
                drop(reply.send(result));
            }
        }
    }
}

fn open_connection(path: &Path, synchronous: &str) -> Result<Connection, SessionError> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    let sync = if synchronous.eq_ignore_ascii_case("FULL") {
        "FULL"
    } else {
        "NORMAL"
    };
    conn.pragma_update(None, "synchronous", sync)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> Result<(), SessionError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS meta (
          key   TEXT PRIMARY KEY,
          value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS sessions (
          id                TEXT PRIMARY KEY,
          soul_id           TEXT NOT NULL,
          kind              TEXT NOT NULL DEFAULT 'conversation',
          delegation_id     TEXT,
          title             TEXT,
          created_at        TEXT NOT NULL,
          ended_at          TEXT,
          end_reason        TEXT,
          archived          INTEGER NOT NULL DEFAULT 0,
          parent_session_id TEXT,
          fork_seq          INTEGER,
          next_seq          INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS session_events (
          session_id TEXT    NOT NULL REFERENCES sessions(id),
          seq        INTEGER NOT NULL,
          ts         TEXT    NOT NULL,
          kind       TEXT    NOT NULL,
          payload    BLOB    NOT NULL,
          PRIMARY KEY (session_id, seq)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS idx_sessions_soul ON sessions (soul_id, kind, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_events_kind ON session_events (session_id, kind);
        CREATE TABLE IF NOT EXISTS spill_objects (
          sha256      TEXT PRIMARY KEY,
          size_bytes  INTEGER NOT NULL,
          mime        TEXT,
          created_at  TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS session_usage (
          id                  TEXT PRIMARY KEY,
          session_id          TEXT    NOT NULL,
          seq                 INTEGER NOT NULL,
          soul_id             TEXT    NOT NULL,
          lane                TEXT    NOT NULL,
          task                TEXT    NOT NULL,
          provider            TEXT    NOT NULL,
          model               TEXT    NOT NULL,
          entry_seq           INTEGER,
          input_tokens        INTEGER NOT NULL DEFAULT 0,
          output_tokens       INTEGER NOT NULL DEFAULT 0,
          cache_read_tokens   INTEGER NOT NULL DEFAULT 0,
          cache_write_tokens  INTEGER NOT NULL DEFAULT 0,
          cost_micro_usd      INTEGER,
          adjustment          INTEGER NOT NULL DEFAULT 0,
          details             BLOB,
          created_at          TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_usage_session ON session_usage (session_id, seq);
        CREATE INDEX IF NOT EXISTS idx_usage_soul_task ON session_usage (soul_id, task, created_at);
        ",
    )?;
    let existing: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'storage_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match existing {
        None => {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('storage_version', ?1)",
                [STORAGE_VERSION.to_string()],
            )?;
        }
        Some(value) => {
            let found: u32 = value.parse().map_err(|_| {
                SessionError::IntegrityCheckFailed(format!("invalid storage_version {value}"))
            })?;
            if found > STORAGE_VERSION {
                return Err(SessionError::StorageTooNew {
                    found,
                    supported: STORAGE_VERSION,
                });
            }
            if found < STORAGE_VERSION {
                apply_migrations(conn, found)?;
            }
        }
    }
    let check: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if check != "ok" {
        return Err(SessionError::IntegrityCheckFailed(check));
    }
    scan_seq_gaps(conn)?;
    Ok(())
}

fn apply_migrations(conn: &Connection, from: u32) -> Result<(), SessionError> {
    let mut version = from;
    while version < STORAGE_VERSION {
        let next = version + 1;
        match next {
            1 => migrate_to_v1(conn)?,
            _ => {
                return Err(SessionError::IntegrityCheckFailed(format!(
                    "missing migration to storage_version {next}"
                )));
            }
        }
        conn.execute(
            "INSERT INTO meta (key, value) VALUES ('storage_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [next.to_string()],
        )?;
        version = next;
    }
    Ok(())
}

fn migrate_to_v1(conn: &Connection) -> Result<(), SessionError> {
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(())
}

fn scan_seq_gaps(conn: &Connection) -> Result<(), SessionError> {
    let mut sessions = conn.prepare("SELECT id, next_seq FROM sessions")?;
    let rows = sessions.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
    })?;
    for row in rows {
        let (id, next_seq) = row?;
        if next_seq == 0 {
            continue;
        }
        let mut stmt =
            conn.prepare("SELECT seq FROM session_events WHERE session_id = ?1 ORDER BY seq")?;
        let seqs = stmt
            .query_map([&id], |r| r.get::<_, i64>(0).map(|v| v as u64))?
            .collect::<Result<Vec<_>, _>>()?;
        let Some(&first) = seqs.first() else {
            return Err(SessionError::SeqGap {
                session_id: id,
                expected: 1,
                found: 0,
            });
        };
        let mut expected = first;
        for seq in seqs {
            if seq != expected {
                return Err(SessionError::SeqGap {
                    session_id: id,
                    expected,
                    found: seq,
                });
            }
            expected = expected
                .checked_add(1)
                .ok_or_else(|| SessionError::SeqOverflow(id.clone()))?;
        }
        if expected.saturating_sub(1) != next_seq {
            return Err(SessionError::SeqGap {
                session_id: id,
                expected: next_seq,
                found: expected.saturating_sub(1),
            });
        }
    }
    Ok(())
}

fn now_ts() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn create_session_conn(conn: &mut Connection, spec: NewSession) -> Result<SessionId, SessionError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let id = SessionId::new();
    let ts = now_ts();
    tx.execute(
        "INSERT INTO sessions (id, soul_id, kind, delegation_id, created_at, next_seq)
         VALUES (?1, ?2, ?3, ?4, ?5, 0)",
        rusqlite::params![
            id.to_string(),
            spec.soul_id.to_string(),
            spec.kind.as_str(),
            spec.delegation_id.map(|d| d.to_string()),
            ts
        ],
    )?;
    insert_event(
        &tx,
        &NewEvent::new(
            id,
            EventKind::SessionStart,
            EventPayload::SessionStart {
                v: v1(),
                soul_id: spec.soul_id,
                body_id: spec.body_id,
                created_by: spec.created_by,
            },
        ),
        &ts,
    )?;
    tx.commit()?;
    Ok(id)
}

fn commit_conn(conn: &mut Connection, batch: &Transaction) -> Result<CommitResult, SessionError> {
    let ts = now_ts();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut seqs = Vec::new();
    for event in &batch.entries {
        let seq = insert_event(&tx, event, &ts)?;
        apply_session_projection(&tx, event, &ts)?;
        seqs.push(seq);
    }
    for usage in &batch.usage {
        insert_usage(&tx, usage, &ts)?;
    }
    tx.commit()?;
    Ok(CommitResult {
        first_seq: seqs.first().copied(),
        seqs,
        ts,
    })
}

fn insert_event(
    tx: &rusqlite::Transaction<'_>,
    event: &NewEvent,
    ts: &str,
) -> Result<u64, SessionError> {
    let exists: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM sessions WHERE id = ?1",
            [event.session_id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Err(SessionError::MissingParent(event.session_id.to_string()));
    }
    tx.execute(
        "UPDATE sessions SET next_seq = next_seq + 1 WHERE id = ?1",
        [event.session_id.to_string()],
    )?;
    let next: i64 = tx.query_row(
        "SELECT next_seq FROM sessions WHERE id = ?1",
        [event.session_id.to_string()],
        |row| row.get(0),
    )?;
    let seq =
        u64::try_from(next).map_err(|_| SessionError::SeqOverflow(event.session_id.to_string()))?;
    let payload = event.payload.encode()?;
    tx.execute(
        "INSERT INTO session_events (session_id, seq, ts, kind, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            event.session_id.to_string(),
            seq as i64,
            ts,
            event.kind.as_str(),
            payload
        ],
    )?;
    Ok(seq)
}

fn apply_session_projection(
    tx: &rusqlite::Transaction<'_>,
    event: &NewEvent,
    ts: &str,
) -> Result<(), SessionError> {
    match &event.payload {
        EventPayload::SessionTitle { title, .. } => {
            tx.execute(
                "UPDATE sessions SET title = ?1 WHERE id = ?2",
                rusqlite::params![title, event.session_id.to_string()],
            )?;
        }
        EventPayload::SessionEnd { reason, .. } => {
            tx.execute(
                "UPDATE sessions SET ended_at = ?1, end_reason = ?2 WHERE id = ?3",
                rusqlite::params![
                    ts,
                    match reason {
                        SessionEndReason::Explicit => "explicit",
                        SessionEndReason::IdleTimeout => "idle_timeout",
                    },
                    event.session_id.to_string()
                ],
            )?;
        }
        EventPayload::SessionReopen { .. } => {
            tx.execute(
                "UPDATE sessions SET ended_at = NULL, end_reason = NULL WHERE id = ?1",
                [event.session_id.to_string()],
            )?;
        }
        EventPayload::SessionArchived { archived, .. } => {
            tx.execute(
                "UPDATE sessions SET archived = ?1 WHERE id = ?2",
                rusqlite::params![i64::from(*archived), event.session_id.to_string()],
            )?;
        }
        EventPayload::ForkPoint {
            source_session_id,
            boundary_seq,
            ..
        } => {
            tx.execute(
                "UPDATE sessions SET parent_session_id = ?1, fork_seq = ?2 WHERE id = ?3",
                rusqlite::params![
                    source_session_id.to_string(),
                    *boundary_seq as i64,
                    event.session_id.to_string()
                ],
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn insert_usage(
    tx: &rusqlite::Transaction<'_>,
    usage: &NewUsage,
    ts: &str,
) -> Result<(), SessionError> {
    let seq: i64 = tx.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM session_usage WHERE session_id = ?1",
        [usage.session_id.to_string()],
        |row| row.get(0),
    )?;
    let id = UsageId::new();
    tx.execute(
        "INSERT INTO session_usage (
            id, session_id, seq, soul_id, lane, task, provider, model, entry_seq,
            input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
            cost_micro_usd, adjustment, created_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        rusqlite::params![
            id.to_string(),
            usage.session_id.to_string(),
            seq,
            usage.soul_id.to_string(),
            usage.lane,
            usage.task,
            usage.provider,
            usage.model,
            usage.entry_seq.map(|s| s as i64),
            i64::from(usage.input_tokens),
            i64::from(usage.output_tokens),
            i64::from(usage.cache_read_tokens),
            i64::from(usage.cache_write_tokens),
            usage.cost_micro_usd,
            i64::from(usage.adjustment),
            ts
        ],
    )?;
    Ok(())
}

fn fork_conn(
    conn: &mut Connection,
    source: SessionId,
    boundary: u64,
) -> Result<SessionId, SessionError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let meta = get_session_conn(&tx, source)?
        .ok_or_else(|| SessionError::SessionNotFound(source.to_string()))?;
    if boundary > meta.next_seq {
        return Err(SessionError::ForkBoundary {
            boundary,
            next_seq: meta.next_seq,
        });
    }
    let events = load_events_conn(&tx, source, 0)?;
    let prefix: Vec<LoggedEvent> = events
        .into_iter()
        .filter(|event| event.seq <= boundary)
        .collect();
    let new_id = SessionId::new();
    let ts = now_ts();
    tx.execute(
        "INSERT INTO sessions (id, soul_id, kind, delegation_id, title, created_at, archived,
            parent_session_id, fork_seq, next_seq)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, 0)",
        rusqlite::params![
            new_id.to_string(),
            meta.soul_id.to_string(),
            meta.kind.as_str(),
            meta.delegation_id.map(|d| d.to_string()),
            meta.title,
            ts,
            source.to_string(),
            boundary as i64
        ],
    )?;
    insert_event(
        &tx,
        &NewEvent::new(
            new_id,
            EventKind::ForkPoint,
            EventPayload::ForkPoint {
                v: v1(),
                source_session_id: source,
                boundary_seq: boundary,
            },
        ),
        &ts,
    )?;
    for event in prefix {
        let payload = event.payload.encode()?;
        tx.execute(
            "UPDATE sessions SET next_seq = next_seq + 1 WHERE id = ?1",
            [new_id.to_string()],
        )?;
        let seq: i64 = tx.query_row(
            "SELECT next_seq FROM sessions WHERE id = ?1",
            [new_id.to_string()],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO session_events (session_id, seq, ts, kind, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                new_id.to_string(),
                seq,
                event.ts,
                event.kind.as_str(),
                payload
            ],
        )?;
    }
    tx.commit()?;
    Ok(new_id)
}

fn recover_conn(conn: &mut Connection) -> Result<Vec<RecoveryReport>, SessionError> {
    let sessions = list_all_sessions_conn(conn)?;
    let mut reports = Vec::new();
    let ts = now_ts();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for meta in sessions {
        let events = load_events_conn(&tx, meta.id, 0)?;
        let turns = open_turns(&events);
        let inbox = unclaimed_inbox(&events);
        if turns.is_empty() && inbox.is_empty() {
            continue;
        }
        for turn in &turns {
            insert_event(
                &tx,
                &NewEvent::new(
                    meta.id,
                    EventKind::TurnEnd,
                    EventPayload::TurnEnd {
                        v: v1(),
                        turn_id: turn.turn_id,
                        outcome: TurnOutcome::Interrupted,
                        error_class: None,
                        error_detail: None,
                    },
                ),
                &ts,
            )?;
        }
        for item in &inbox {
            insert_event(
                &tx,
                &NewEvent::new(
                    meta.id,
                    EventKind::InboxCancelled,
                    EventPayload::InboxCancelled {
                        v: v1(),
                        entry_seq: item.seq,
                        reason: crate::event::InboxCancelReason::AbandonedInterrupt,
                    },
                ),
                &ts,
            )?;
        }
        reports.push(RecoveryReport {
            session_id: meta.id,
            interrupted_turns: turns,
            abandoned_inbox: inbox,
        });
    }
    tx.commit()?;
    Ok(reports)
}

fn load_events_conn(
    conn: &Connection,
    session_id: SessionId,
    since_seq: u64,
) -> Result<Vec<LoggedEvent>, SessionError> {
    let mut stmt = conn.prepare(
        "SELECT seq, ts, kind, payload FROM session_events
         WHERE session_id = ?1 AND seq >= ?2 ORDER BY seq",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![session_id.to_string(), since_seq as i64],
        |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        },
    )?;
    let mut events = Vec::new();
    for row in rows {
        let (seq, ts, kind_str, blob) = row?;
        let kind = EventKind::parse(&kind_str);
        let payload = EventPayload::decode(&kind, &blob)?;
        events.push(LoggedEvent {
            session_id,
            seq,
            ts,
            kind,
            payload,
        });
    }
    Ok(events)
}

fn get_session_conn(
    conn: &Connection,
    session_id: SessionId,
) -> Result<Option<SessionMeta>, SessionError> {
    let mut stmt = conn.prepare(
        "SELECT id, soul_id, kind, delegation_id, title, created_at, ended_at, end_reason,
                archived, parent_session_id, fork_seq, next_seq
         FROM sessions WHERE id = ?1",
    )?;
    stmt.query_row([session_id.to_string()], row_to_meta)
        .optional()
        .map_err(SessionError::from)
}

fn list_all_sessions_conn(conn: &Connection) -> Result<Vec<SessionMeta>, SessionError> {
    let mut stmt = conn.prepare(
        "SELECT id, soul_id, kind, delegation_id, title, created_at, ended_at, end_reason,
                archived, parent_session_id, fork_seq, next_seq
         FROM sessions ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([], row_to_meta)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(SessionError::from)
}

fn list_sessions_conn(
    conn: &Connection,
    soul_id: Option<SoulId>,
) -> Result<Vec<SessionMeta>, SessionError> {
    if let Some(soul) = soul_id {
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, kind, delegation_id, title, created_at, ended_at, end_reason,
                    archived, parent_session_id, fork_seq, next_seq
             FROM sessions WHERE soul_id = ?1 AND kind = 'conversation'
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([soul.to_string()], row_to_meta)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(SessionError::from)
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, kind, delegation_id, title, created_at, ended_at, end_reason,
                    archived, parent_session_id, fork_seq, next_seq
             FROM sessions WHERE kind = 'conversation'
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_meta)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(SessionError::from)
    }
}

fn row_to_meta(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionMeta> {
    Ok(SessionMeta {
        id: SessionId::from_uuid(parse_uuid(&row.get::<_, String>(0)?)?),
        soul_id: SoulId::from_uuid(parse_uuid(&row.get::<_, String>(1)?)?),
        kind: SessionKind::parse(&row.get::<_, String>(2)?),
        delegation_id: match row.get::<_, Option<String>>(3)? {
            Some(value) => Some(DelegationId::from_uuid(parse_uuid(&value)?)),
            None => None,
        },
        title: row.get(4)?,
        created_at: row.get(5)?,
        ended_at: row.get(6)?,
        end_reason: row.get(7)?,
        archived: row.get::<_, i64>(8)? != 0,
        parent_session_id: match row.get::<_, Option<String>>(9)? {
            Some(value) => Some(SessionId::from_uuid(parse_uuid(&value)?)),
            None => None,
        },
        fork_seq: row.get::<_, Option<i64>>(10)?.map(|v| v as u64),
        next_seq: row.get::<_, i64>(11)? as u64,
    })
}

fn parse_uuid(raw: &str) -> rusqlite::Result<uuid::Uuid> {
    uuid::Uuid::parse_str(raw).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn spill_dir_for(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("spill")
}

fn parse_spill_id(id: &str) -> Result<&str, SessionError> {
    if id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(id)
    } else {
        Err(SessionError::InvalidId(format!("spill ref {id}")))
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for &byte in digest.as_slice() {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

fn record_spill_conn(
    conn: &Connection,
    sha256: &str,
    size_bytes: u64,
    mime: Option<&str>,
) -> Result<(), SessionError> {
    let size = i64::try_from(size_bytes).unwrap_or(i64::MAX);
    conn.execute(
        "INSERT OR IGNORE INTO spill_objects (sha256, size_bytes, mime, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![sha256, size, mime, now_ts()],
    )?;
    Ok(())
}
