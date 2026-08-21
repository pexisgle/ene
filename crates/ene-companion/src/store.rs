use crate::affect::{AffectBaseline, AffectState};
use crate::error::CompanionError;
use crate::ids::{CandidateId, MemoryId};
use crate::memory::{
    JournalAction, MemoryKind, MemoryRecord, MemoryScope, MemorySource, NewMemory, RecalledMemory,
};
use crate::soul::{NewSoul, Soul, parse_skill_refs};
use chrono::Utc;
use ene_session::{BodyId, SoulId};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::str::FromStr;

const SCHEMA: &str = "
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS souls (
  id TEXT PRIMARY KEY,
  character_ref TEXT NOT NULL,
  body_ref TEXT,
  voice_ref TEXT,
  skill_refs TEXT NOT NULL DEFAULT '[]',
  affect_baseline TEXT NOT NULL,
  valence REAL NOT NULL,
  arousal REAL NOT NULL,
  dominance REAL NOT NULL,
  trust REAL NOT NULL,
  affinity REAL NOT NULL,
  irritation REAL NOT NULL DEFAULT 0,
  curiosity REAL NOT NULL DEFAULT 0,
  fatigue REAL NOT NULL DEFAULT 0,
  mood_label TEXT NOT NULL,
  last_report_ts TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS memories (
  id TEXT PRIMARY KEY,
  soul_id TEXT NOT NULL,
  scope TEXT NOT NULL,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  content TEXT NOT NULL,
  embedding BLOB,
  confidence REAL NOT NULL DEFAULT 0.5,
  salience REAL NOT NULL DEFAULT 0.5,
  source TEXT NOT NULL,
  source_seq INTEGER,
  created_at TEXT NOT NULL,
  last_access TEXT NOT NULL,
  access_count INTEGER NOT NULL DEFAULT 0,
  superseded_by TEXT,
  expires_at TEXT,
  forgotten INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_mem_scope ON memories (scope, soul_id, kind);
CREATE INDEX IF NOT EXISTS idx_mem_title ON memories (soul_id, title);
CREATE VIRTUAL TABLE IF NOT EXISTS mem_fts USING fts5(id UNINDEXED, title, content);
CREATE TABLE IF NOT EXISTS memory_journal (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  ts TEXT NOT NULL,
  memory_id TEXT,
  soul_id TEXT NOT NULL,
  action TEXT NOT NULL,
  payload TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS memory_candidates (
  id TEXT PRIMARY KEY,
  soul_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  content TEXT NOT NULL,
  scope TEXT NOT NULL,
  confidence REAL NOT NULL,
  salience REAL NOT NULL,
  status TEXT NOT NULL,
  sensitive INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS packages (
  id TEXT NOT NULL,
  version TEXT NOT NULL,
  kind TEXT NOT NULL,
  path TEXT NOT NULL,
  digest TEXT,
  installed_at TEXT NOT NULL,
  PRIMARY KEY (id, version)
);
";

/// `companions.db`: souls, memories (not event-sourced), packages.
pub struct CompanionStore {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl CompanionStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CompanionError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
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
    pub fn reconnect(&self) -> Result<(), CompanionError> {
        let conn = Connection::open(&self.path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        *self.conn.lock() = conn;
        Ok(())
    }

    pub fn create_soul(&self, draft: &NewSoul) -> Result<Soul, CompanionError> {
        let id = SoulId::new();
        let now = Utc::now().to_rfc3339();
        let affect = AffectState::baseline(&draft.affect_baseline);
        let baseline = serde_json::to_string(&draft.affect_baseline)
            .map_err(|err| CompanionError::codec(err.to_string()))?;
        let skills = serde_json::to_string(&draft.skill_refs)
            .map_err(|err| CompanionError::codec(err.to_string()))?;
        let body = draft.body_ref.map(|id| id.to_string());
        self.conn.lock().execute(
            "INSERT INTO souls (
                id, character_ref, body_ref, voice_ref, skill_refs, affect_baseline,
                valence, arousal, dominance, trust, affinity, irritation, curiosity,
                fatigue, mood_label, last_report_ts, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                id.to_string(),
                draft.character_ref,
                body,
                draft.voice_ref,
                skills,
                baseline,
                affect.valence,
                affect.arousal,
                affect.dominance,
                affect.trust,
                affect.affinity,
                affect.irritation,
                affect.curiosity,
                affect.fatigue,
                affect.mood_label,
                affect.last_report_ts,
                now,
                now,
            ],
        )?;
        self.get_soul(id)?
            .ok_or_else(|| CompanionError::UnknownSoul(id.to_string()))
    }

    pub fn get_soul(&self, id: SoulId) -> Result<Option<Soul>, CompanionError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, character_ref, body_ref, voice_ref, skill_refs, affect_baseline,
                    valence, arousal, dominance, trust, affinity, irritation, curiosity,
                    fatigue, mood_label, last_report_ts, created_at, updated_at
             FROM souls WHERE id = ?1",
        )?;
        stmt.query_row(params![id.to_string()], row_to_soul)
            .optional()
            .map_err(CompanionError::from)
    }

    pub fn list_souls(&self) -> Result<Vec<Soul>, CompanionError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, character_ref, body_ref, voice_ref, skill_refs, affect_baseline,
                    valence, arousal, dominance, trust, affinity, irritation, curiosity,
                    fatigue, mood_label, last_report_ts, created_at, updated_at
             FROM souls ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], row_to_soul)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(CompanionError::from)
    }

    pub fn save_affect(&self, id: SoulId, affect: &AffectState) -> Result<(), CompanionError> {
        let now = Utc::now().to_rfc3339();
        self.conn.lock().execute(
            "UPDATE souls SET valence=?1, arousal=?2, dominance=?3, trust=?4, affinity=?5,
                    irritation=?6, curiosity=?7, fatigue=?8, mood_label=?9,
                    last_report_ts=?10, updated_at=?11 WHERE id=?12",
            params![
                affect.valence,
                affect.arousal,
                affect.dominance,
                affect.trust,
                affect.affinity,
                affect.irritation,
                affect.curiosity,
                affect.fatigue,
                affect.mood_label,
                affect.last_report_ts,
                now,
                id.to_string(),
            ],
        )?;
        Ok(())
    }

    pub fn set_body_ref(&self, id: SoulId, body: Option<BodyId>) -> Result<(), CompanionError> {
        let n = self.conn.lock().execute(
            "UPDATE souls SET body_ref = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                body.map(|id| id.to_string()),
                Utc::now().to_rfc3339(),
                id.to_string()
            ],
        )?;
        if n == 0 {
            return Err(CompanionError::UnknownSoul(id.to_string()));
        }
        Ok(())
    }

    pub fn insert_memory(&self, new: NewMemory) -> Result<MemoryRecord, CompanionError> {
        let id = MemoryId::new();
        let now = Utc::now().to_rfc3339();
        let record = MemoryRecord {
            id,
            soul_id: new.soul_id,
            scope: new.scope,
            kind: new.kind,
            title: new.title,
            content: new.content,
            confidence: new.confidence,
            salience: new.salience,
            source: new.source,
            source_seq: new.source_seq,
            created_at: now.clone(),
            last_access: now.clone(),
            access_count: 0,
            superseded_by: None,
            expires_at: new.expires_at,
            forgotten: false,
        };
        {
            let conn = self.conn.lock();
            conn.execute(
                "INSERT INTO memories (
                    id, soul_id, scope, kind, title, content, confidence, salience,
                    source, source_seq, created_at, last_access, access_count, expires_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, ?13)",
                params![
                    record.id.to_string(),
                    record.soul_id.to_string(),
                    record.scope.as_str(),
                    record.kind.as_str(),
                    record.title,
                    record.content,
                    record.confidence,
                    record.salience,
                    record.source.as_str(),
                    record.source_seq.map(|n| n as i64),
                    record.created_at,
                    record.last_access,
                    record.expires_at,
                ],
            )?;
            conn.execute(
                "INSERT INTO mem_fts (id, title, content) VALUES (?1, ?2, ?3)",
                params![record.id.to_string(), record.title, record.content],
            )?;
        }
        self.journal(
            Some(record.id),
            record.soul_id,
            JournalAction::Created,
            &serde_json::json!({ "title": record.title, "scope": record.scope.as_str() }),
        )?;
        Ok(record)
    }

    pub fn get_memory(&self, id: MemoryId) -> Result<Option<MemoryRecord>, CompanionError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, scope, kind, title, content, confidence, salience,
                    source, source_seq, created_at, last_access, access_count,
                    superseded_by, expires_at, forgotten
             FROM memories WHERE id = ?1",
        )?;
        stmt.query_row(params![id.to_string()], row_to_memory)
            .optional()
            .map_err(CompanionError::from)
    }

    pub fn list_memories(
        &self,
        soul_id: SoulId,
        scope: Option<MemoryScope>,
    ) -> Result<Vec<MemoryRecord>, CompanionError> {
        let conn = self.conn.lock();
        let sql = if scope.is_some() {
            "SELECT id, soul_id, scope, kind, title, content, confidence, salience,
                    source, source_seq, created_at, last_access, access_count,
                    superseded_by, expires_at, forgotten
             FROM memories WHERE soul_id = ?1 AND scope = ?2 AND forgotten = 0
             ORDER BY created_at DESC"
        } else {
            "SELECT id, soul_id, scope, kind, title, content, confidence, salience,
                    source, source_seq, created_at, last_access, access_count,
                    superseded_by, expires_at, forgotten
             FROM memories WHERE soul_id = ?1 AND forgotten = 0
             ORDER BY created_at DESC"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = if let Some(scope) = scope {
            stmt.query_map(params![soul_id.to_string(), scope.as_str()], row_to_memory)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![soul_id.to_string()], row_to_memory)?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    pub fn update_memory_content(
        &self,
        id: MemoryId,
        content: &str,
        soul_id: SoulId,
    ) -> Result<(), CompanionError> {
        let conn = self.conn.lock();
        let n = conn.execute(
            "UPDATE memories SET content = ?1 WHERE id = ?2",
            params![content, id.to_string()],
        )?;
        if n == 0 {
            return Err(CompanionError::UnknownMemory(id.to_string()));
        }
        conn.execute(
            "UPDATE mem_fts SET content = ?1 WHERE id = ?2",
            params![content, id.to_string()],
        )?;
        drop(conn);
        self.journal(
            Some(id),
            soul_id,
            JournalAction::Updated,
            &serde_json::json!({ "content": content }),
        )
    }

    pub fn set_scope(
        &self,
        id: MemoryId,
        scope: MemoryScope,
        soul_id: SoulId,
    ) -> Result<(), CompanionError> {
        let n = self.conn.lock().execute(
            "UPDATE memories SET scope = ?1 WHERE id = ?2",
            params![scope.as_str(), id.to_string()],
        )?;
        if n == 0 {
            return Err(CompanionError::UnknownMemory(id.to_string()));
        }
        self.journal(
            Some(id),
            soul_id,
            JournalAction::Updated,
            &serde_json::json!({ "scope": scope.as_str() }),
        )
    }

    pub fn supersede(
        &self,
        old: MemoryId,
        new: MemoryId,
        soul_id: SoulId,
    ) -> Result<(), CompanionError> {
        self.conn.lock().execute(
            "UPDATE memories SET superseded_by = ?1 WHERE id = ?2",
            params![new.to_string(), old.to_string()],
        )?;
        self.journal(
            Some(old),
            soul_id,
            JournalAction::Superseded,
            &serde_json::json!({ "by": new.to_string() }),
        )
    }

    pub fn forget(
        &self,
        id: MemoryId,
        soul_id: SoulId,
        action: JournalAction,
    ) -> Result<(), CompanionError> {
        self.conn.lock().execute(
            "UPDATE memories SET forgotten = 1, salience = 0 WHERE id = ?1",
            params![id.to_string()],
        )?;
        self.journal(Some(id), soul_id, action, &serde_json::json!({}))
    }

    pub fn find_by_title(
        &self,
        soul_id: SoulId,
        title: &str,
        kind: MemoryKind,
    ) -> Result<Option<MemoryRecord>, CompanionError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, scope, kind, title, content, confidence, salience,
                    source, source_seq, created_at, last_access, access_count,
                    superseded_by, expires_at, forgotten
             FROM memories
             WHERE soul_id = ?1 AND kind = ?2 AND lower(title) = lower(?3)
               AND superseded_by IS NULL AND forgotten = 0
             ORDER BY created_at DESC LIMIT 1",
        )?;
        stmt.query_row(
            params![soul_id.to_string(), kind.as_str(), title],
            row_to_memory,
        )
        .optional()
        .map_err(CompanionError::from)
    }

    pub fn find_shared_by_title(
        &self,
        title: &str,
        kind: MemoryKind,
    ) -> Result<Option<MemoryRecord>, CompanionError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, scope, kind, title, content, confidence, salience,
                    source, source_seq, created_at, last_access, access_count,
                    superseded_by, expires_at, forgotten
             FROM memories
             WHERE scope = 'shared' AND kind = ?1 AND lower(title) = lower(?2)
               AND superseded_by IS NULL AND forgotten = 0
             LIMIT 1",
        )?;
        stmt.query_row(params![kind.as_str(), title], row_to_memory)
            .optional()
            .map_err(CompanionError::from)
    }

    /// Recall private-of-this-soul plus all shared. Writer id is not returned
    /// in the user-facing text (D-7).
    pub fn recall(
        &self,
        soul_id: SoulId,
        query: &str,
        budget: usize,
        now: &str,
        weights: RecallWeights,
    ) -> Result<Vec<RecalledMemory>, CompanionError> {
        self.recall_ranked(soul_id, query, budget, now, weights, None)
    }

    /// Recall with an optional query embedding (cosine on `memories.embedding`).
    pub fn recall_ranked(
        &self,
        soul_id: SoulId,
        query: &str,
        budget: usize,
        now: &str,
        weights: RecallWeights,
        query_vec: Option<&[f32]>,
    ) -> Result<Vec<RecalledMemory>, CompanionError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, scope, kind, title, content, confidence, salience,
                    source, source_seq, created_at, last_access, access_count,
                    superseded_by, expires_at, forgotten, embedding
             FROM memories
             WHERE forgotten = 0 AND superseded_by IS NULL
               AND (scope = 'shared' OR soul_id = ?1)
               AND (expires_at IS NULL OR expires_at > ?2)",
        )?;
        let rows = stmt.query_map(params![soul_id.to_string(), now], |row| {
            let mem = row_to_memory(row)?;
            let blob: Option<Vec<u8>> = row.get(16)?;
            Ok((mem, blob))
        })?;
        let mut scored = Vec::new();
        let q = query.to_ascii_lowercase();
        for row in rows {
            let (mem, blob) = row?;
            let lex = lexical_score(&q, &mem.title, &mem.content);
            let embed = match (query_vec, blob.as_deref()) {
                (Some(query), Some(bytes)) => cosine(query, &decode_f32_slice(bytes)),
                _ => 0.0,
            };
            if !q.is_empty() && lex <= 0.0 && embed <= 0.0 {
                continue;
            }
            let recency = recency_score(&mem.last_access, now);
            let score = weights.lexical * lex
                + weights.recency * recency
                + weights.salience * mem.salience
                + weights.embedding * embed;
            scored.push((score, mem));
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut picked: Vec<RecalledMemory> = Vec::new();
        let mut touch_ids = Vec::new();
        for (score, mem) in scored {
            if picked.len() >= budget.max(1) {
                break;
            }
            if picked
                .iter()
                .any(|other: &RecalledMemory| titles_too_close(&other.title, &mem.title))
            {
                continue;
            }
            touch_ids.push(mem.id);
            picked.push(RecalledMemory {
                id: mem.id,
                kind: mem.kind,
                scope: mem.scope,
                title: mem.title,
                content: mem.content,
                score,
            });
        }
        drop(stmt);
        drop(conn);
        for id in touch_ids {
            self.touch(id)?;
        }
        Ok(picked)
    }

    pub fn set_embedding(&self, id: MemoryId, vector: &[f32]) -> Result<(), CompanionError> {
        self.conn.lock().execute(
            "UPDATE memories SET embedding = ?1 WHERE id = ?2",
            params![encode_f32_slice(vector), id.to_string()],
        )?;
        Ok(())
    }

    fn touch(&self, id: MemoryId) -> Result<(), CompanionError> {
        let now = Utc::now().to_rfc3339();
        self.conn.lock().execute(
            "UPDATE memories SET last_access = ?1, access_count = access_count + 1 WHERE id = ?2",
            params![now, id.to_string()],
        )?;
        Ok(())
    }

    pub fn standing_notes(
        &self,
        soul_id: SoulId,
        limit: usize,
    ) -> Result<Vec<String>, CompanionError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT title, content FROM memories
             WHERE forgotten = 0 AND superseded_by IS NULL
               AND kind IN ('preference', 'user_profile')
               AND (scope = 'shared' OR soul_id = ?1)
             ORDER BY last_access DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![soul_id.to_string(), limit as i64], |row| {
            let title: String = row.get(0)?;
            let content: String = row.get(1)?;
            Ok((title, content))
        })?;
        let mut notes = Vec::new();
        for row in rows {
            let (title, content) = row?;
            if title.trim().is_empty() && content.trim().is_empty() {
                continue;
            }
            notes.push(if content.trim().is_empty() {
                title
            } else {
                format!("{title}: {content}")
            });
        }
        Ok(notes)
    }

    pub fn decay_salience(&self, kind: MemoryKind, factor: f32) -> Result<(), CompanionError> {
        if matches!(kind, MemoryKind::Preference) {
            return Ok(());
        }
        self.conn.lock().execute(
            "UPDATE memories SET salience = salience * ?1
             WHERE kind = ?2 AND forgotten = 0 AND scope = 'private'",
            params![factor, kind.as_str()],
        )?;
        // shared: decay only when nobody is accessing — approximated by low access_count
        self.conn.lock().execute(
            "UPDATE memories SET salience = salience * ?1
             WHERE kind = ?2 AND forgotten = 0 AND scope = 'shared' AND access_count = 0",
            params![factor, kind.as_str()],
        )?;
        Ok(())
    }

    pub fn forgetting_candidates(
        &self,
        threshold: f32,
    ) -> Result<Vec<MemoryRecord>, CompanionError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, scope, kind, title, content, confidence, salience,
                    source, source_seq, created_at, last_access, access_count,
                    superseded_by, expires_at, forgotten
             FROM memories
             WHERE forgotten = 0 AND kind != 'commitment' AND salience < ?1
               AND superseded_by IS NULL",
        )?;
        let rows = stmt.query_map(params![threshold], row_to_memory)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(CompanionError::from)
    }

    pub fn insert_candidate(
        &self,
        cand: &crate::memory::MemoryCandidate,
    ) -> Result<(), CompanionError> {
        let now = Utc::now().to_rfc3339();
        self.conn.lock().execute(
            "INSERT INTO memory_candidates (
                id, soul_id, kind, title, content, scope, confidence, salience, status, sensitive, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10)",
            params![
                cand.id.to_string(),
                cand.soul_id.to_string(),
                cand.kind.as_str(),
                cand.title,
                cand.content,
                cand.scope.as_str(),
                cand.confidence,
                cand.salience,
                i32::from(cand.sensitive),
                now,
            ],
        )?;
        Ok(())
    }

    pub fn list_pending_candidates(
        &self,
        soul_id: SoulId,
    ) -> Result<Vec<crate::memory::MemoryCandidate>, CompanionError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, kind, title, content, scope, confidence, salience, sensitive
             FROM memory_candidates WHERE soul_id = ?1 AND status = 'pending'",
        )?;
        let rows = stmt.query_map(params![soul_id.to_string()], |row| {
            Ok(crate::memory::MemoryCandidate {
                id: CandidateId::from_str(&row.get::<_, String>(0)?)
                    .map_err(|err| sql_id(0, err))?,
                soul_id: SoulId::from_str(&row.get::<_, String>(1)?)
                    .map_err(|err| sql_id(1, err))?,
                kind: MemoryKind::parse(&row.get::<_, String>(2)?),
                title: row.get(3)?,
                content: row.get(4)?,
                scope: MemoryScope::parse(&row.get::<_, String>(5)?),
                confidence: row.get(6)?,
                salience: row.get(7)?,
                sensitive: row.get::<_, i32>(8)? != 0,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(CompanionError::from)
    }

    pub fn resolve_candidate(&self, id: CandidateId, status: &str) -> Result<(), CompanionError> {
        self.conn.lock().execute(
            "UPDATE memory_candidates SET status = ?1 WHERE id = ?2",
            params![status, id.to_string()],
        )?;
        Ok(())
    }

    pub fn journal(
        &self,
        memory_id: Option<MemoryId>,
        soul_id: SoulId,
        action: JournalAction,
        payload: &serde_json::Value,
    ) -> Result<(), CompanionError> {
        let blob =
            serde_json::to_string(payload).map_err(|err| CompanionError::codec(err.to_string()))?;
        self.conn.lock().execute(
            "INSERT INTO memory_journal (ts, memory_id, soul_id, action, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                Utc::now().to_rfc3339(),
                memory_id.map(|id| id.to_string()),
                soul_id.to_string(),
                action.as_str(),
                blob,
            ],
        )?;
        Ok(())
    }

    pub fn journal_len(&self) -> Result<u64, CompanionError> {
        let count: i64 =
            self.conn
                .lock()
                .query_row("SELECT COUNT(*) FROM memory_journal", [], |row| row.get(0))?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    pub fn record_package(
        &self,
        id: &str,
        version: &str,
        kind: &str,
        path: &str,
        digest: Option<&str>,
    ) -> Result<(), CompanionError> {
        self.conn.lock().execute(
            "INSERT OR REPLACE INTO packages (id, version, kind, path, digest, installed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, version, kind, path, digest, Utc::now().to_rfc3339(),],
        )?;
        Ok(())
    }

    pub fn package_path(&self, id: &str, version: &str) -> Result<Option<String>, CompanionError> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT path FROM packages WHERE id = ?1 AND version = ?2",
            params![id, version],
            |row| row.get(0),
        )
        .optional()
        .map_err(CompanionError::from)
    }

    pub fn list_packages(&self) -> Result<Vec<(String, String, String, String)>, CompanionError> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT id, version, kind, path FROM packages ORDER BY installed_at")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(CompanionError::from)
    }
}

/// Recall hybrid weights (vector term redistributed onto lexical when unused).
#[derive(Debug, Clone, Copy)]
pub struct RecallWeights {
    pub lexical: f32,
    pub recency: f32,
    pub salience: f32,
    pub embedding: f32,
    pub mmr_lambda: f32,
}

impl Default for RecallWeights {
    fn default() -> Self {
        Self {
            lexical: 0.5,
            recency: 0.25,
            salience: 0.25,
            embedding: 0.35,
            mmr_lambda: 0.7,
        }
    }
}

fn row_to_soul(row: &rusqlite::Row<'_>) -> rusqlite::Result<Soul> {
    let id = SoulId::from_str(&row.get::<_, String>(0)?).map_err(|err| sql_id(0, err))?;
    let body = row
        .get::<_, Option<String>>(2)?
        .map(|raw| BodyId::from_str(&raw).map_err(|err| sql_id(2, err)))
        .transpose()?;
    let baseline: AffectBaseline =
        serde_json::from_str(&row.get::<_, String>(5)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(err))
        })?;
    let skills = parse_skill_refs(&row.get::<_, String>(4)?).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::other(err.to_string())),
        )
    })?;
    Ok(Soul {
        id,
        character_ref: row.get(1)?,
        body_ref: body,
        voice_ref: row.get(3)?,
        skill_refs: skills,
        affect_baseline: baseline,
        affect: AffectState {
            valence: row.get(6)?,
            arousal: row.get(7)?,
            dominance: row.get(8)?,
            trust: row.get(9)?,
            affinity: row.get(10)?,
            irritation: row.get(11)?,
            curiosity: row.get(12)?,
            fatigue: row.get(13)?,
            mood_label: row.get(14)?,
            last_report_ts: row.get(15)?,
        },
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

fn row_to_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRecord> {
    Ok(MemoryRecord {
        id: MemoryId::from_str(&row.get::<_, String>(0)?).map_err(|err| sql_id(0, err))?,
        soul_id: SoulId::from_str(&row.get::<_, String>(1)?).map_err(|err| sql_id(1, err))?,
        scope: MemoryScope::parse(&row.get::<_, String>(2)?),
        kind: MemoryKind::parse(&row.get::<_, String>(3)?),
        title: row.get(4)?,
        content: row.get(5)?,
        confidence: row.get(6)?,
        salience: row.get(7)?,
        source: MemorySource::parse(&row.get::<_, String>(8)?),
        source_seq: row.get::<_, Option<i64>>(9)?.map(|n| n as u64),
        created_at: row.get(10)?,
        last_access: row.get(11)?,
        access_count: row.get::<_, i64>(12)? as u32,
        superseded_by: row
            .get::<_, Option<String>>(13)?
            .map(|raw| MemoryId::from_str(&raw).map_err(|err| sql_id(13, err)))
            .transpose()?,
        expires_at: row.get(14)?,
        forgotten: row.get::<_, i32>(15)? != 0,
    })
}

fn sql_id(idx: usize, err: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        idx,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::other(err.to_string())),
    )
}

fn lexical_score(query: &str, title: &str, content: &str) -> f32 {
    if query.trim().is_empty() {
        return 0.0;
    }
    let hay = format!("{title} {content}").to_ascii_lowercase();
    let mut hits = 0u32;
    let mut parts = 0u32;
    for token in query.split_whitespace() {
        parts += 1;
        if hay.contains(token) {
            hits += 1;
        }
    }
    if parts == 0 {
        0.0
    } else {
        f32::from(u16::try_from(hits).unwrap_or(u16::MAX))
            / f32::from(u16::try_from(parts).unwrap_or(u16::MAX))
    }
}

fn recency_score(last_access: &str, now: &str) -> f32 {
    let Ok(then) = chrono::DateTime::parse_from_rfc3339(last_access) else {
        return 0.5;
    };
    let Ok(now) = chrono::DateTime::parse_from_rfc3339(now) else {
        return 0.5;
    };
    let days = (now - then).num_minutes() as f32 / (60.0 * 24.0);
    (-days / 14.0).exp().clamp(0.0, 1.0)
}

fn encode_f32_slice(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len().saturating_mul(4));
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn decode_f32_slice(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.is_empty() || left.len() != right.len() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut left_norm = 0.0_f32;
    let mut right_norm = 0.0_f32;
    for (a, b) in left.iter().zip(right) {
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    let denom = left_norm.sqrt() * right_norm.sqrt();
    if denom <= f32::EPSILON {
        0.0
    } else {
        (dot / denom).clamp(0.0, 1.0)
    }
}

fn titles_too_close(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

#[cfg(test)]
mod ranking_tests {
    use super::cosine;

    #[test]
    fn cosine_is_one_for_parallel_and_zero_for_orthogonal() {
        assert!((cosine(&[1.0, 0.0], &[2.0, 0.0]) - 1.0).abs() < 1e-5);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-5);
        assert!(cosine(&[1.0], &[1.0, 0.0]).abs() < 1e-5);
    }
}
