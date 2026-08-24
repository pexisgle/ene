use crate::error::WorkError;
use crate::types::{
    Artifact, ArtifactKind, DelegationMode, Job, JobStatus, NewJob, NewSchedule, NewToolExecution,
    OpenQuestion, Schedule, ScheduleAction, ToolExecStatus, ToolExecution,
};
use chrono::{DateTime, Utc};
use cron::Schedule as Cron;
use ene_session::{DelegationId, SoulId};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use uuid::Uuid;

const SCHEMA: &str = "
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
CREATE TABLE IF NOT EXISTS mcp_servers (
  id TEXT PRIMARY KEY,
  transport TEXT NOT NULL,
  command TEXT,
  url TEXT,
  enabled INTEGER NOT NULL DEFAULT 1,
  args TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS mailbox (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  delegation_id TEXT NOT NULL,
  direction TEXT NOT NULL,
  kind TEXT NOT NULL,
  body TEXT NOT NULL,
  ts TEXT NOT NULL,
  question_seq INTEGER
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

/// Jobs / schedules / artifacts. Opens the same file as `companions.db`.
pub struct WorkStore {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl WorkStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WorkError> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        let plan_approved_exists = conn
            .prepare("SELECT plan_approved FROM jobs LIMIT 0")
            .is_ok();
        if !plan_approved_exists {
            conn.execute(
                "ALTER TABLE jobs ADD COLUMN plan_approved INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        let mcp_args_exists = conn.prepare("SELECT args FROM mcp_servers LIMIT 0").is_ok();
        if !mcp_args_exists {
            conn.execute(
                "ALTER TABLE mcp_servers ADD COLUMN args TEXT NOT NULL DEFAULT '[]'",
                [],
            )?;
        }
        let mailbox_question_seq_exists = conn
            .prepare("SELECT question_seq FROM mailbox LIMIT 0")
            .is_ok();
        if !mailbox_question_seq_exists {
            conn.execute("ALTER TABLE mailbox ADD COLUMN question_seq INTEGER", [])?;
        }
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
    pub fn reconnect(&self) -> Result<(), WorkError> {
        let conn = Connection::open(&self.path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        *self.conn.lock() = conn;
        Ok(())
    }

    pub fn insert_job(&self, new: &NewJob) -> Result<Job, WorkError> {
        let id = new.id.unwrap_or_default();
        let now = Utc::now().to_rfc3339();
        self.conn.lock().execute(
            "INSERT INTO jobs (
                id, soul_id, title, goal, mode, status, progress_fraction, progress_note,
                workspace_dir, error_class, created_from_turn, plan, brief, plan_approved,
                created_at, ended_at
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![
                id.to_string(),
                new.soul_id.to_string(),
                &new.title,
                &new.goal,
                new.mode.as_str(),
                JobStatus::Created.as_str(),
                None::<f32>,
                None::<String>,
                &new.workspace_dir,
                None::<String>,
                &new.created_from_turn,
                &new.plan,
                &new.brief,
                0_i32,
                now,
                None::<String>,
            ],
        )?;
        self.get_job(id)?
            .ok_or_else(|| WorkError::UnknownJob(id.to_string()))
    }

    pub fn get_job(&self, id: DelegationId) -> Result<Option<Job>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, title, goal, mode, status, progress_fraction, progress_note,
                    workspace_dir, error_class, created_from_turn, plan, brief, plan_approved,
                    created_at, ended_at
             FROM jobs WHERE id = ?1",
        )?;
        stmt.query_row(params![id.to_string()], row_job)
            .optional()
            .map_err(WorkError::from)
    }

    pub fn set_status(
        &self,
        id: DelegationId,
        status: JobStatus,
        error_class: Option<&str>,
    ) -> Result<(), WorkError> {
        let ended = matches!(
            status,
            JobStatus::Completed
                | JobStatus::Failed
                | JobStatus::Cancelled
                | JobStatus::Interrupted
        )
        .then(|| Utc::now().to_rfc3339());
        let n = self.conn.lock().execute(
            "UPDATE jobs SET status = ?1, error_class = ?2, ended_at = COALESCE(?3, ended_at) WHERE id = ?4",
            params![status.as_str(), error_class, ended, id.to_string()],
        )?;
        if n == 0 {
            return Err(WorkError::UnknownJob(id.to_string()));
        }
        Ok(())
    }

    pub fn set_progress(
        &self,
        id: DelegationId,
        fraction: Option<f32>,
        note: Option<&str>,
    ) -> Result<(), WorkError> {
        let n = self.conn.lock().execute(
            "UPDATE jobs SET status = 'running', progress_fraction = ?1, progress_note = ?2 WHERE id = ?3",
            params![fraction, note, id.to_string()],
        )?;
        if n == 0 {
            return Err(WorkError::UnknownJob(id.to_string()));
        }
        Ok(())
    }

    pub fn interrupt_running(&self) -> Result<Vec<Job>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT id FROM jobs WHERE status IN ('running', 'queued', 'created')")?;
        let ids: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE jobs SET status = 'interrupted', ended_at = ?1 WHERE status = 'running'",
            params![now],
        )?;
        drop(conn);
        let mut out = Vec::new();
        for id in ids {
            let id = DelegationId::from_str(&id).map_err(|_| WorkError::UnknownJob(id))?;
            if let Some(job) = self.get_job(id)? {
                out.push(job);
            }
        }
        Ok(out)
    }

    pub fn insert_schedule(&self, new: &NewSchedule) -> Result<Schedule, WorkError> {
        self.insert_schedule_at(new, Utc::now())
    }

    pub fn insert_schedule_at(
        &self,
        new: &NewSchedule,
        now: DateTime<Utc>,
    ) -> Result<Schedule, WorkError> {
        let id = Uuid::now_v7().to_string();
        let next = next_fire(&new.spec, &new.timezone, now)?;
        self.conn.lock().execute(
            "INSERT INTO schedules (
                id, soul_id, name, spec, timezone, action_kind, action_ref,
                enabled, important, last_fired, next_fire
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,1,?8,NULL,?9)",
            params![
                id,
                new.soul_id.to_string(),
                &new.name,
                &new.spec,
                &new.timezone,
                new.action.as_str(),
                &new.action_ref,
                i32::from(new.important),
                next,
            ],
        )?;
        self.get_schedule(&id)?
            .ok_or_else(|| WorkError::UnknownSchedule(id))
    }

    pub fn get_schedule(&self, id: &str) -> Result<Option<Schedule>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, name, spec, timezone, action_kind, action_ref,
                    enabled, important, last_fired, next_fire
             FROM schedules WHERE id = ?1",
        )?;
        stmt.query_row(params![id], row_sched)
            .optional()
            .map_err(WorkError::from)
    }

    pub fn due_schedules(&self, now: DateTime<Utc>) -> Result<Vec<Schedule>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, name, spec, timezone, action_kind, action_ref,
                    enabled, important, last_fired, next_fire
             FROM schedules WHERE enabled = 1 AND next_fire IS NOT NULL AND next_fire <= ?1",
        )?;
        let rows = stmt.query_map(params![now.to_rfc3339()], row_sched)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(WorkError::from)
    }

    pub fn mark_fired(&self, id: &str, now: DateTime<Utc>) -> Result<(), WorkError> {
        let sched = self
            .get_schedule(id)?
            .ok_or_else(|| WorkError::UnknownSchedule(id.to_owned()))?;
        let next = next_fire(&sched.spec, &sched.timezone, now)?;
        self.conn.lock().execute(
            "UPDATE schedules SET last_fired = ?1, next_fire = ?2 WHERE id = ?3",
            params![now.to_rfc3339(), next, id],
        )?;
        Ok(())
    }

    pub fn defer_next_fire(&self, id: &str, when: DateTime<Utc>) -> Result<(), WorkError> {
        self.conn.lock().execute(
            "UPDATE schedules SET next_fire = ?1 WHERE id = ?2",
            params![when.to_rfc3339(), id],
        )?;
        Ok(())
    }

    pub fn set_plan_approved(&self, id: DelegationId) -> Result<(), WorkError> {
        let n = self.conn.lock().execute(
            "UPDATE jobs SET plan_approved = 1 WHERE id = ?1",
            params![id.to_string()],
        )?;
        if n == 0 {
            return Err(WorkError::UnknownJob(id.to_string()));
        }
        Ok(())
    }

    pub fn list_jobs(&self, soul: SoulId) -> Result<Vec<Job>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, title, goal, mode, status, progress_fraction, progress_note,
                    workspace_dir, error_class, created_from_turn, plan, brief, plan_approved,
                    created_at, ended_at
             FROM jobs WHERE soul_id = ?1 AND mode = 'public' ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![soul.to_string()], row_job)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(WorkError::from)
    }

    pub fn list_jobs_all(&self) -> Result<Vec<Job>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, title, goal, mode, status, progress_fraction, progress_note,
                    workspace_dir, error_class, created_from_turn, plan, brief, plan_approved,
                    created_at, ended_at
             FROM jobs WHERE mode = 'public' ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], row_job)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(WorkError::from)
    }

    pub fn list_schedules(&self, soul: Option<SoulId>) -> Result<Vec<Schedule>, WorkError> {
        let conn = self.conn.lock();
        if let Some(soul) = soul {
            let mut stmt = conn.prepare(
                "SELECT id, soul_id, name, spec, timezone, action_kind, action_ref,
                        enabled, important, last_fired, next_fire
                 FROM schedules WHERE soul_id = ?1 ORDER BY name",
            )?;
            let rows = stmt.query_map(params![soul.to_string()], row_sched)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(WorkError::from)
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, soul_id, name, spec, timezone, action_kind, action_ref,
                        enabled, important, last_fired, next_fire
                 FROM schedules ORDER BY name",
            )?;
            let rows = stmt.query_map([], row_sched)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(WorkError::from)
        }
    }

    pub fn set_schedule_enabled(&self, id: &str, enabled: bool) -> Result<(), WorkError> {
        let n = self.conn.lock().execute(
            "UPDATE schedules SET enabled = ?1 WHERE id = ?2",
            params![i32::from(enabled), id],
        )?;
        if n == 0 {
            return Err(WorkError::UnknownSchedule(id.to_owned()));
        }
        Ok(())
    }

    pub fn delete_schedule(&self, id: &str) -> Result<(), WorkError> {
        let n = self
            .conn
            .lock()
            .execute("DELETE FROM schedules WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(WorkError::UnknownSchedule(id.to_owned()));
        }
        Ok(())
    }

    pub fn get_artifact(&self, id: &str) -> Result<Option<Artifact>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, job_id, kind, title, path, mime, size_bytes, created_at, delivered
             FROM artifacts WHERE id = ?1",
        )?;
        stmt.query_row(params![id], row_art)
            .optional()
            .map_err(WorkError::from)
    }

    pub fn count_active(&self, soul: SoulId) -> Result<u32, WorkError> {
        let n: i64 = self.conn.lock().query_row(
            "SELECT COUNT(*) FROM jobs
             WHERE soul_id = ?1 AND status IN ('created', 'queued', 'running')",
            params![soul.to_string()],
            |row| row.get(0),
        )?;
        u32::try_from(n).map_err(|err| WorkError::Codec(err.to_string()))
    }

    pub fn has_active_jobs(&self) -> Result<bool, WorkError> {
        let n: i64 = self.conn.lock().query_row(
            "SELECT COUNT(*) FROM jobs
             WHERE status IN ('created', 'queued', 'running')",
            [],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn set_plan(&self, id: DelegationId, plan: &str) -> Result<(), WorkError> {
        let n = self.conn.lock().execute(
            "UPDATE jobs SET plan = ?1 WHERE id = ?2",
            params![plan, id.to_string()],
        )?;
        if n == 0 {
            return Err(WorkError::UnknownJob(id.to_string()));
        }
        Ok(())
    }

    pub fn list_artifacts(&self, soul: SoulId) -> Result<Vec<Artifact>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, job_id, kind, title, path, mime, size_bytes, created_at, delivered
             FROM artifacts WHERE soul_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![soul.to_string()], row_art)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(WorkError::from)
    }

    pub fn register_artifact(&self, art: Artifact) -> Result<Artifact, WorkError> {
        self.conn.lock().execute(
            "INSERT INTO artifacts (
                id, soul_id, job_id, kind, title, path, mime, size_bytes, created_at, delivered
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                art.id,
                art.soul_id.to_string(),
                art.job_id.map(|id| id.to_string()),
                art.kind.as_str(),
                art.title,
                art.path,
                art.mime,
                art.size_bytes,
                art.created_at,
                i32::from(art.delivered),
            ],
        )?;
        Ok(art)
    }

    pub fn deliver(&self, id: &str) -> Result<(), WorkError> {
        self.conn.lock().execute(
            "UPDATE artifacts SET delivered = 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn mark_delivered(&self, id: &str, path: &str) -> Result<(), WorkError> {
        self.conn.lock().execute(
            "UPDATE artifacts SET delivered = 1, path = ?2 WHERE id = ?1",
            params![id, path],
        )?;
        Ok(())
    }

    pub fn artifacts_for(&self, job: DelegationId) -> Result<Vec<Artifact>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, soul_id, job_id, kind, title, path, mime, size_bytes, created_at, delivered
             FROM artifacts WHERE job_id = ?1",
        )?;
        let rows = stmt.query_map(params![job.to_string()], row_art)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(WorkError::from)
    }

    pub fn mailbox_push(
        &self,
        id: DelegationId,
        direction: &str,
        kind: &str,
        body: &str,
    ) -> Result<(), WorkError> {
        self.mailbox_push_at(id, direction, kind, body, &Utc::now().to_rfc3339())
    }

    pub fn mailbox_push_at(
        &self,
        id: DelegationId,
        direction: &str,
        kind: &str,
        body: &str,
        ts: &str,
    ) -> Result<(), WorkError> {
        self.mailbox_push_at_for_question(id, None, direction, kind, body, ts)
    }

    pub fn mailbox_push_for_question(
        &self,
        id: DelegationId,
        question_seq: i64,
        kind: &str,
        body: &str,
    ) -> Result<(), WorkError> {
        self.mailbox_push_at_for_question(
            id,
            Some(question_seq),
            "parent_to_child",
            kind,
            body,
            &Utc::now().to_rfc3339(),
        )
    }

    fn mailbox_push_at_for_question(
        &self,
        id: DelegationId,
        question_seq: Option<i64>,
        direction: &str,
        kind: &str,
        body: &str,
        ts: &str,
    ) -> Result<(), WorkError> {
        self.conn.lock().execute(
            "INSERT INTO mailbox (delegation_id, direction, kind, body, ts, question_seq)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![id.to_string(), direction, kind, body, ts, question_seq],
        )?;
        Ok(())
    }

    pub fn mailbox(&self, id: DelegationId) -> Result<Vec<(String, String, String)>, WorkError> {
        Ok(self
            .mailbox_entries(id)?
            .into_iter()
            .map(|entry| (entry.direction, entry.kind, entry.body))
            .collect())
    }

    pub fn mailbox_entries(&self, id: DelegationId) -> Result<Vec<MailboxEntry>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT seq, direction, kind, body, ts, question_seq
             FROM mailbox WHERE delegation_id = ?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![id.to_string()], |row| {
            Ok(MailboxEntry {
                seq: row.get(0)?,
                direction: row.get(1)?,
                kind: row.get(2)?,
                body: row.get(3)?,
                ts: row.get(4)?,
                question_seq: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(WorkError::from)
    }

    pub fn record_meta(
        &self,
        id: DelegationId,
        mode: DelegationMode,
        depth: u32,
    ) -> Result<(), WorkError> {
        self.mailbox_push(id, "meta", "mode", mode.as_str())?;
        self.mailbox_push(id, "meta", "depth", &depth.to_string())?;
        Ok(())
    }

    pub fn delegation_mode(&self, id: DelegationId) -> Result<Option<DelegationMode>, WorkError> {
        if let Some(job) = self.get_job(id)? {
            return Ok(Some(job.mode));
        }
        for entry in self.mailbox_entries(id)? {
            if entry.direction == "meta" && entry.kind == "mode" {
                return Ok(Some(DelegationMode::parse(&entry.body)));
            }
        }
        Ok(None)
    }

    pub fn delegation_depth(&self, id: DelegationId) -> Result<Option<u32>, WorkError> {
        for entry in self.mailbox_entries(id)? {
            if entry.direction == "meta"
                && entry.kind == "depth"
                && let Ok(depth) = entry.body.parse::<u32>()
            {
                return Ok(Some(depth));
            }
        }
        Ok(None)
    }

    pub fn open_questions(&self, id: DelegationId) -> Result<Vec<OpenQuestion>, WorkError> {
        let entries = self.mailbox_entries(id)?;
        let mut pending: Vec<MailboxEntry> = Vec::new();
        for entry in entries {
            if entry.direction == "child_to_parent" && entry.kind == "question" {
                pending.push(entry);
            } else if entry.direction == "parent_to_child" && entry.kind == "answer" {
                if let Some(question_seq) = entry.question_seq {
                    pending.retain(|question| question.seq != question_seq);
                } else if !pending.is_empty() {
                    pending.remove(0);
                }
            } else if entry.direction == "parent_to_child" && entry.kind == "assumption" {
                pending.clear();
            }
        }
        Ok(pending
            .into_iter()
            .map(|entry| OpenQuestion {
                delegation_id: id,
                mailbox_seq: entry.seq,
                prompt: entry.body,
                asked_at: entry.ts,
            })
            .collect())
    }

    pub fn upsert_mcp(
        &self,
        id: &str,
        transport: &str,
        command: Option<&str>,
        url: Option<&str>,
    ) -> Result<(), WorkError> {
        self.conn.lock().execute(
            "INSERT INTO mcp_servers (id, transport, command, url, enabled, args)
             VALUES (?1,?2,?3,?4,1,'[]')
             ON CONFLICT(id) DO UPDATE SET transport=?2, command=?3, url=?4",
            params![id, transport, command, url],
        )?;
        Ok(())
    }

    pub fn list_mcp(&self) -> Result<Vec<crate::McpServer>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, transport, command, url, enabled, args FROM mcp_servers ORDER BY id",
        )?;
        let rows = stmt.query_map([], row_mcp)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn replace_mcp(&self, servers: &[crate::McpServer]) -> Result<(), WorkError> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM mcp_servers", [])?;
        for server in servers {
            let args = serde_json::to_string(&server.args)
                .map_err(|err| WorkError::Codec(err.to_string()))?;
            conn.execute(
                "INSERT INTO mcp_servers (id, transport, command, url, enabled, args)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    server.id,
                    server.transport,
                    server.command,
                    server.url,
                    i32::from(server.enabled),
                    args
                ],
            )?;
        }
        Ok(())
    }

    pub fn insert_tool_execution(
        &self,
        new: &NewToolExecution,
    ) -> Result<ToolExecution, WorkError> {
        let now = Utc::now().to_rfc3339();
        self.conn.lock().execute(
            "INSERT INTO tool_executions (
                execution_id, job_id, soul_id, tool_name, plugin_id, call_id,
                status, error_class, result_json, started_at, ended_at, completion_delivered
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                new.execution_id,
                new.job_id.map(|id| id.to_string()),
                new.soul_id.to_string(),
                new.tool_name,
                new.plugin_id,
                new.call_id,
                ToolExecStatus::Running.as_str(),
                None::<String>,
                None::<String>,
                now,
                None::<String>,
                0_i32,
            ],
        )?;
        self.get_tool_execution(&new.execution_id)?
            .ok_or_else(|| WorkError::UnknownExecution(new.execution_id.clone()))
    }

    pub fn get_tool_execution(
        &self,
        execution_id: &str,
    ) -> Result<Option<ToolExecution>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT execution_id, job_id, soul_id, tool_name, plugin_id, call_id,
                    status, error_class, result_json, started_at, ended_at, completion_delivered
             FROM tool_executions WHERE execution_id = ?1",
        )?;
        stmt.query_row(params![execution_id], row_tool_exec)
            .optional()
            .map_err(WorkError::from)
    }

    pub fn complete_tool_execution_once(
        &self,
        execution_id: &str,
        status: ToolExecStatus,
        error_class: Option<&str>,
        result_json: Option<&str>,
    ) -> Result<bool, WorkError> {
        let now = Utc::now().to_rfc3339();
        let n = self.conn.lock().execute(
            "UPDATE tool_executions
             SET status = ?1, error_class = ?2, result_json = ?3, ended_at = ?4, completion_delivered = 1
             WHERE execution_id = ?5 AND completion_delivered = 0",
            params![
                status.as_str(),
                error_class,
                result_json,
                now,
                execution_id
            ],
        )?;
        Ok(n > 0)
    }

    pub fn list_running_tool_executions(&self) -> Result<Vec<ToolExecution>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT execution_id, job_id, soul_id, tool_name, plugin_id, call_id,
                    status, error_class, result_json, started_at, ended_at, completion_delivered
             FROM tool_executions WHERE status IN ('pending','running')",
        )?;
        let rows = stmt.query_map([], row_tool_exec)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_tool_executions_for_job(
        &self,
        job_id: DelegationId,
    ) -> Result<Vec<ToolExecution>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT execution_id, job_id, soul_id, tool_name, plugin_id, call_id,
                    status, error_class, result_json, started_at, ended_at, completion_delivered
             FROM tool_executions WHERE job_id = ?1",
        )?;
        let rows = stmt.query_map(params![job_id.to_string()], row_tool_exec)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxEntry {
    pub seq: i64,
    pub direction: String,
    pub kind: String,
    pub body: String,
    pub ts: String,
    pub question_seq: Option<i64>,
}

pub fn next_fire(spec: &str, tz_name: &str, from: DateTime<Utc>) -> Result<String, WorkError> {
    if let Some(duration) = parse_interval(spec) {
        let next = from.with_timezone(&Utc) + duration;
        return Ok(next.to_rfc3339());
    }
    let cron_spec = if spec.split_whitespace().count() == 5 {
        format!("0 {spec}")
    } else {
        spec.to_owned()
    };
    let schedule = Cron::from_str(&cron_spec).map_err(|_| schedule_spec_error(spec))?;
    let tz: chrono_tz::Tz = tz_name.parse().unwrap_or(chrono_tz::UTC);
    let local = from.with_timezone(&tz);
    let next = schedule
        .after(&local)
        .next()
        .ok_or_else(|| WorkError::Schedule("no next fire".to_owned()))?;
    Ok(next.with_timezone(&Utc).to_rfc3339())
}

fn parse_interval(spec: &str) -> Option<chrono::Duration> {
    let trimmed = spec.trim();
    if !trimmed.to_ascii_lowercase().starts_with("every ") {
        return None;
    }
    let value = trimmed[6..].trim();
    if value.len() < 2 {
        return None;
    }
    let (digits, unit) = value.split_at(value.len() - 1);
    let n: u32 = digits.parse().ok()?;
    if n == 0 {
        return None;
    }
    match unit.to_ascii_lowercase().as_str() {
        "s" => Some(chrono::Duration::seconds(i64::from(n))),
        "m" => Some(chrono::Duration::minutes(i64::from(n))),
        "h" => Some(chrono::Duration::hours(i64::from(n))),
        "d" => Some(chrono::Duration::days(i64::from(n))),
        _ => None,
    }
}

fn schedule_spec_error(spec: &str) -> WorkError {
    WorkError::Schedule(format!(
        "invalid schedule spec '{spec}'. use cron (e.g. '0 9 * * *') or interval (e.g. 'every 1h')"
    ))
}

fn row_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: DelegationId::from_str(&row.get::<_, String>(0)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })?,
        soul_id: SoulId::from_str(&row.get::<_, String>(1)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
        })?,
        title: row.get(2)?,
        goal: row.get(3)?,
        mode: DelegationMode::parse(&row.get::<_, String>(4)?),
        status: JobStatus::parse(&row.get::<_, String>(5)?),
        progress_fraction: row.get(6)?,
        progress_note: row.get(7)?,
        workspace_dir: row.get(8)?,
        error_class: row.get(9)?,
        created_from_turn: row.get(10)?,
        plan: row.get(11)?,
        brief: row.get(12)?,
        plan_approved: row.get::<_, i32>(13)? != 0,
        created_at: row.get(14)?,
        ended_at: row.get(15)?,
    })
}

fn row_mcp(row: &rusqlite::Row<'_>) -> rusqlite::Result<crate::McpServer> {
    let args_raw: String = row.get(5)?;
    let args = serde_json::from_str(&args_raw).unwrap_or_default();
    Ok(crate::McpServer {
        id: row.get(0)?,
        transport: row.get(1)?,
        command: row.get(2)?,
        url: row.get(3)?,
        enabled: row.get::<_, i32>(4)? != 0,
        args,
    })
}

fn row_sched(row: &rusqlite::Row<'_>) -> rusqlite::Result<Schedule> {
    Ok(Schedule {
        id: row.get(0)?,
        soul_id: SoulId::from_str(&row.get::<_, String>(1)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
        })?,
        name: row.get(2)?,
        spec: row.get(3)?,
        timezone: row.get(4)?,
        action: ScheduleAction::parse(&row.get::<_, String>(5)?),
        action_ref: row.get(6)?,
        enabled: row.get::<_, i32>(7)? != 0,
        important: row.get::<_, i32>(8)? != 0,
        last_fired: row.get(9)?,
        next_fire: row.get(10)?,
    })
}

fn row_art(row: &rusqlite::Row<'_>) -> rusqlite::Result<Artifact> {
    Ok(Artifact {
        id: row.get(0)?,
        soul_id: SoulId::from_str(&row.get::<_, String>(1)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
        })?,
        job_id: row
            .get::<_, Option<String>>(2)?
            .map(|raw| {
                DelegationId::from_str(&raw).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })
            })
            .transpose()?,
        kind: ArtifactKind::parse(&row.get::<_, String>(3)?),
        title: row.get(4)?,
        path: row.get(5)?,
        mime: row.get(6)?,
        size_bytes: row.get(7)?,
        created_at: row.get(8)?,
        delivered: row.get::<_, i32>(9)? != 0,
    })
}

fn row_tool_exec(row: &rusqlite::Row<'_>) -> rusqlite::Result<ToolExecution> {
    Ok(ToolExecution {
        execution_id: row.get(0)?,
        job_id: row
            .get::<_, Option<String>>(1)?
            .map(|raw| {
                DelegationId::from_str(&raw).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })
            })
            .transpose()?,
        soul_id: SoulId::from_str(&row.get::<_, String>(2)?).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
        })?,
        tool_name: row.get(3)?,
        plugin_id: row.get(4)?,
        call_id: row.get(5)?,
        status: ToolExecStatus::parse(&row.get::<_, String>(6)?),
        error_class: row.get(7)?,
        result_json: row.get(8)?,
        started_at: row.get(9)?,
        ended_at: row.get(10)?,
        completion_delivered: row.get::<_, i32>(11)? != 0,
    })
}
