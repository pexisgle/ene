use blake3::Hash;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS audit (
    seq INTEGER PRIMARY KEY,
    ts_ms INTEGER NOT NULL,
    kind TEXT NOT NULL,
    payload BLOB NOT NULL,
    prev_hash BLOB NOT NULL,
    hash BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// Audit log failures. A failed write refuses the triggering operation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AuditError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("hash chain broken at seq {0}")]
    BrokenChain(i64),
    #[error("codec: {0}")]
    Codec(String),
}

/// One verified audit row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub seq: i64,
    pub ts_ms: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub hash_hex: String,
}

/// Append-only `audit.db` with a blake3 hash chain (P-908).
pub struct AuditLog {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl AuditLog {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AuditError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reopen the on-disk database after restore (same path).
    pub fn reconnect(&self) -> Result<(), AuditError> {
        let conn = Connection::open(&self.path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        *self.conn.lock() = conn;
        Ok(())
    }

    pub fn append(
        &self,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<AuditRecord, AuditError> {
        let blob = serde_json::to_vec(payload).map_err(|err| AuditError::Codec(err.to_string()))?;
        let ts_ms = chrono::Utc::now().timestamp_millis();
        let conn = self.conn.lock();
        let prev: Vec<u8> = conn
            .query_row(
                "SELECT hash FROM audit ORDER BY seq DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or_else(|| vec![0_u8; 32]);
        let hash = chain_hash(&prev, kind, &blob);
        conn.execute(
            "INSERT INTO audit (ts_ms, kind, payload, prev_hash, hash) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts_ms, kind, blob, prev, hash.as_bytes().as_slice()],
        )?;
        let seq = conn.last_insert_rowid();
        Ok(AuditRecord {
            seq,
            ts_ms,
            kind: kind.to_owned(),
            payload: payload.clone(),
            hash_hex: hash.to_hex().to_string(),
        })
    }

    pub fn verify_chain(&self) -> Result<(), AuditError> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT seq, kind, payload, prev_hash, hash FROM audit ORDER BY seq")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })?;
        let mut expected_prev = vec![0_u8; 32];
        for row in rows {
            let (seq, kind, payload, prev, hash) = row?;
            if prev != expected_prev {
                return Err(AuditError::BrokenChain(seq));
            }
            let computed = chain_hash(&prev, &kind, &payload);
            if computed.as_bytes().as_slice() != hash.as_slice() {
                return Err(AuditError::BrokenChain(seq));
            }
            expected_prev = hash;
        }
        Ok(())
    }

    pub fn records(&self) -> Result<Vec<AuditRecord>, AuditError> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT seq, ts_ms, kind, payload, hash FROM audit ORDER BY seq")?;
        let rows = stmt.query_map([], |row| {
            let payload: Vec<u8> = row.get(3)?;
            let hash: Vec<u8> = row.get(4)?;
            Ok(AuditRecord {
                seq: row.get(0)?,
                ts_ms: row.get(1)?,
                kind: row.get(2)?,
                payload: serde_json::from_slice(&payload)
                    .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?,
                hash_hex: hex::encode(&hash),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(AuditError::from)
    }
}

fn chain_hash(prev: &[u8], kind: &str, payload: &[u8]) -> Hash {
    let mut hasher = blake3::Hasher::new();
    hasher.update(prev);
    hasher.update(kind.as_bytes());
    hasher.update(payload);
    hasher.finalize()
}
