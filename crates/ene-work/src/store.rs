use crate::error::WorkError;
use crate::events::{DelegationEvent, DelegationEventPayload};
use crate::types::{
    Artifact, ArtifactKind, DelegationMode, Job, JobStatus, NewJob, NewSchedule, NewToolExecution,
    OpenQuestion, Schedule, ScheduleAction, ToolExecStatus, ToolExecution,
};
use chrono::{DateTime, Utc};
use cron::Schedule as Cron;
use ene_session::{DelegationId, QuestionId, SoulId};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use uuid::Uuid;

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
        let mut conn = Connection::open(&path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        crate::migrations::apply_migrations(&mut conn)?;
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
        let mut conn = Connection::open(&self.path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        crate::migrations::apply_migrations(&mut conn)?;
        *self.conn.lock() = conn;
        Ok(())
    }

    pub fn insert_job(&self, new: &NewJob) -> Result<Job, WorkError> {
        let id = new.id.unwrap_or_default();
        let now = Utc::now().to_rfc3339();
        let success_criteria =
            serde_json::to_string(&new.success_criteria).unwrap_or_else(|_| "[]".to_owned());
        let allowed_tools =
            serde_json::to_string(&new.allowed_tools).unwrap_or_else(|_| "[]".to_owned());
        self.conn.lock().execute(
            "INSERT INTO jobs (
                id, soul_id, title, goal, mode, status, progress_fraction, progress_note,
                workspace_dir, error_class, created_from_turn, plan, brief, plan_approved,
                success_criteria, allowed_tools, pending_allowed_tools, created_at, ended_at
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
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
                &success_criteria,
                &allowed_tools,
                None::<String>,
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
                    success_criteria, allowed_tools, pending_allowed_tools, created_at, ended_at
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
        let mut stmt = conn.prepare(
            "SELECT id FROM jobs WHERE status IN
            ('running', 'verifying', 'queued', 'created')",
        )?;
        let ids: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE jobs SET status = 'interrupted', ended_at = ?1
             WHERE status IN ('running', 'verifying')",
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

    pub fn pending_allowed_tools(
        &self,
        id: DelegationId,
    ) -> Result<Option<Vec<String>>, WorkError> {
        let raw: Option<String> = self
            .conn
            .lock()
            .query_row(
                "SELECT pending_allowed_tools FROM jobs WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|encoded| serde_json::from_str(&encoded).ok()))
    }

    pub fn set_pending_allowed_tools(
        &self,
        id: DelegationId,
        tools: Option<&[String]>,
    ) -> Result<(), WorkError> {
        let encoded = tools.map(|list| serde_json::to_string(list).unwrap_or_else(|_| "[]".into()));
        let n = self.conn.lock().execute(
            "UPDATE jobs SET pending_allowed_tools = ?1 WHERE id = ?2",
            params![encoded, id.to_string()],
        )?;
        if n == 0 {
            return Err(WorkError::UnknownJob(id.to_string()));
        }
        Ok(())
    }

    pub fn set_allowed_tools(&self, id: DelegationId, tools: &[String]) -> Result<(), WorkError> {
        let encoded = serde_json::to_string(tools).unwrap_or_else(|_| "[]".to_owned());
        let n = self.conn.lock().execute(
            "UPDATE jobs SET allowed_tools = ?1 WHERE id = ?2",
            params![encoded, id.to_string()],
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
                    success_criteria, allowed_tools, pending_allowed_tools, created_at, ended_at
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
                    success_criteria, allowed_tools, pending_allowed_tools, created_at, ended_at
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
             WHERE soul_id = ?1 AND status IN ('created', 'queued', 'running', 'verifying')",
            params![soul.to_string()],
            |row| row.get(0),
        )?;
        u32::try_from(n).map_err(|err| WorkError::Codec(err.to_string()))
    }

    pub fn has_active_jobs(&self) -> Result<bool, WorkError> {
        let n: i64 = self.conn.lock().query_row(
            "SELECT COUNT(*) FROM jobs
             WHERE status IN ('created', 'queued', 'running', 'verifying')",
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

    pub fn append_delegation_event(
        &self,
        id: DelegationId,
        payload: &DelegationEventPayload,
    ) -> Result<(), WorkError> {
        self.append_delegation_event_at(id, payload, &Utc::now().to_rfc3339())
    }

    pub fn append_delegation_event_at(
        &self,
        id: DelegationId,
        payload: &DelegationEventPayload,
        created_at: &str,
    ) -> Result<(), WorkError> {
        let payload_json =
            serde_json::to_string(&payload).map_err(|err| WorkError::Codec(err.to_string()))?;
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO delegation_events (delegation_id, created_at, payload)
             VALUES (?1,?2,?3)",
            params![id.to_string(), created_at, payload_json],
        )?;
        Ok(())
    }

    pub fn append_question(&self, id: DelegationId, prompt: &str) -> Result<QuestionId, WorkError> {
        self.append_question_at(id, prompt, &Utc::now().to_rfc3339())
    }

    pub fn append_question_at(
        &self,
        id: DelegationId,
        prompt: &str,
        created_at: &str,
    ) -> Result<QuestionId, WorkError> {
        let question_id = QuestionId::new();
        self.append_delegation_event_at(
            id,
            &DelegationEventPayload::Question {
                question_id,
                prompt: prompt.to_owned(),
            },
            created_at,
        )?;
        Ok(question_id)
    }

    pub fn delegation_events(&self, id: DelegationId) -> Result<Vec<DelegationEvent>, WorkError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT event_seq, created_at, payload
             FROM delegation_events WHERE delegation_id = ?1 ORDER BY event_seq",
        )?;
        let rows = stmt.query_map(params![id.to_string()], |row| {
            let event_seq: i64 = row.get(0)?;
            let created_at: String = row.get(1)?;
            let payload_json: String = row.get(2)?;
            let payload: DelegationEventPayload =
                serde_json::from_str(&payload_json).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?;
            Ok(DelegationEvent {
                event_seq,
                delegation_id: id,
                created_at,
                payload,
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
        self.append_delegation_event(id, &DelegationEventPayload::ModeSet { mode })?;
        self.append_delegation_event(id, &DelegationEventPayload::DepthSet { depth })?;
        Ok(())
    }

    pub fn delegation_mode(&self, id: DelegationId) -> Result<Option<DelegationMode>, WorkError> {
        if let Some(job) = self.get_job(id)? {
            return Ok(Some(job.mode));
        }
        let mut mode = None;
        for event in self.delegation_events(id)? {
            if let DelegationEventPayload::ModeSet { mode: next } = event.payload {
                mode = Some(next);
            }
        }
        Ok(mode)
    }

    pub fn delegation_depth(&self, id: DelegationId) -> Result<Option<u32>, WorkError> {
        let mut depth = None;
        for event in self.delegation_events(id)? {
            if let DelegationEventPayload::DepthSet { depth: next } = event.payload {
                depth = Some(next);
            }
        }
        Ok(depth)
    }

    pub fn open_questions(&self, id: DelegationId) -> Result<Vec<OpenQuestion>, WorkError> {
        let events = self.delegation_events(id)?;
        let mut pending: Vec<OpenQuestion> = Vec::new();
        for event in events {
            match event.payload {
                DelegationEventPayload::Question {
                    question_id,
                    prompt,
                } => {
                    pending.push(OpenQuestion {
                        delegation_id: id,
                        question_id,
                        prompt,
                        asked_at: event.created_at,
                    });
                }
                DelegationEventPayload::Answer { question_id, .. } => {
                    pending.retain(|question| question.question_id != question_id);
                }
                DelegationEventPayload::Assumption { .. } => {
                    pending.clear();
                }
                _ => {}
            }
        }
        Ok(pending)
    }

    pub fn has_delegation_events(&self, id: DelegationId) -> Result<bool, WorkError> {
        let conn = self.conn.lock();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM delegation_events WHERE delegation_id = ?1",
            params![id.to_string()],
            |row| row.get(0),
        )?;
        Ok(count > 0)
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

pub fn next_fire(spec: &str, tz_name: &str, from: DateTime<Utc>) -> Result<String, WorkError> {
    if let Some(duration) = parse_interval(spec) {
        let next = from.with_timezone(&Utc) + duration;
        return Ok(next.to_rfc3339());
    }
    // The cron crate numbers Sunday as 1; user-facing cron uses Sunday as 0 or 7.
    let cron_spec = normalize_cron_spec(spec);
    let schedule = Cron::from_str(&cron_spec).map_err(|_| schedule_spec_error(spec))?;
    let tz: chrono_tz::Tz = tz_name.parse().unwrap_or(chrono_tz::UTC);
    let local = from.with_timezone(&tz);
    let next = schedule
        .after(&local)
        .next()
        .ok_or_else(|| WorkError::Schedule("no next fire".to_owned()))?;
    Ok(next.with_timezone(&Utc).to_rfc3339())
}

fn normalize_cron_spec(spec: &str) -> String {
    let mut fields: Vec<String> = spec.split_whitespace().map(ToOwned::to_owned).collect();
    if fields.len() == 5 {
        fields.insert(0, "0".to_owned());
    }
    if matches!(fields.len(), 6 | 7) {
        fields[5] = normalize_cron_day_of_week(&fields[5]);
    }
    fields.join(" ")
}

fn normalize_cron_day_of_week(field: &str) -> String {
    field
        .split(',')
        .map(|item| {
            normalize_numeric_day_of_week_item(item).map_or_else(
                || item.to_owned(),
                |values| {
                    values
                        .into_iter()
                        .map(|value| value.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                },
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn normalize_numeric_day_of_week_item(item: &str) -> Option<Vec<u8>> {
    let (base, step) = match item.split_once('/') {
        Some((base, raw_step)) if !base.is_empty() && !raw_step.is_empty() => {
            if raw_step.contains('/') {
                return None;
            }
            let step = raw_step.parse::<u8>().ok()?;
            if !(1..=7).contains(&step) {
                return None;
            }
            (base, usize::from(step))
        }
        Some(_) => return None,
        None => (item, 1),
    };
    if base == "?" || (base == "*" && !item.contains('/')) {
        return None;
    }

    let (start, end) = if base == "*" {
        (0, 7)
    } else if let Some((raw_start, raw_end)) = base.split_once('-') {
        (raw_start.parse::<u8>().ok()?, raw_end.parse::<u8>().ok()?)
    } else {
        let start = base.parse::<u8>().ok()?;
        (start, if item.contains('/') { 7 } else { start })
    };
    if start > 7 || end > 7 || start > end {
        return None;
    }

    let mut weekdays = [false; 7];
    for value in (start..=end).step_by(step) {
        weekdays[usize::from(value % 7)] = true;
    }
    Some(
        (0_u8..=6)
            .filter_map(|day| weekdays[usize::from(day)].then_some(day + 1))
            .collect(),
    )
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
        success_criteria: serde_json::from_str(&row.get::<_, String>(14)?).unwrap_or_default(),
        allowed_tools: serde_json::from_str(&row.get::<_, String>(15)?).unwrap_or_default(),
        pending_allowed_tools: row
            .get::<_, Option<String>>(16)?
            .and_then(|raw| serde_json::from_str(&raw).ok()),
        created_at: row.get(17)?,
        ended_at: row.get(18)?,
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
