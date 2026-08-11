//! Persistent scheduler queries.
//!
//! The claim/finish pair is transactional: claiming a fire atomically
//! re-validates the schedule, advances `next_run_at`, and inserts the run
//! history row, so a restart or a duplicate dispatch can never execute the
//! same occurrence twice. Retries are represented by a pointer on the
//! schedule row (`pending_retry_of_run_id`) plus one history row per attempt.

use super::{EneMemoryError, MemoryStore};
use crate::entities;
use chrono::{DateTime, Duration, Utc};
use ene_core::{
    NewSchedule, Schedule, ScheduleAction, ScheduleKind, ScheduleRun, ScheduleRunStatus,
};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, TransactionTrait, TryIntoModel,
};

/// Detects `SQLite` busy/locked errors.
///
/// `busy_timeout` does not cover `SQLITE_BUSY_SNAPSHOT` (extended code 517):
/// in WAL mode a writer that started its transaction from a stale snapshot
/// fails immediately even though another connection's commit just finished.
/// The scheduler's claim/finish writes race each other (and the timer's
/// reads) across pool connections, so a single lost write would otherwise
/// leave a run row `Running` forever.
fn is_busy_error(err: &EneMemoryError) -> bool {
    let EneMemoryError::MemoryStoreError(
        sea_orm::DbErr::Query(sea_orm::RuntimeErr::SqlxError(err))
        | sea_orm::DbErr::Exec(sea_orm::RuntimeErr::SqlxError(err)),
    ) = err
    else {
        return false;
    };
    matches!(
        err.as_ref(),
        sea_orm::sqlx::Error::Database(db_err)
            if matches!(
                db_err.code().as_deref(),
                Some("5" | "6" | "517" | "261" | "262")
            )
    )
}

/// How the actor wants a claimed fire handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FireClaimMode {
    /// Execute the action now (the actor holds the single-flight gate).
    Execute,
    /// The actor was busy with a conversation; record and move on.
    SkipBusy,
    /// The fire arrived beyond the late-execution grace; record and move on.
    SkipLate,
    /// Record the run and wait for a user confirmation decision.
    AwaitConfirmation,
}

/// Result of a successful fire claim.
#[derive(Debug, Clone)]
pub struct ClaimedFire {
    /// The schedule snapshot read inside the claim transaction.
    pub schedule: Schedule,
    /// The run history row created for this fire.
    pub run_id: i64,
    /// Whether this fire is a retry of a previous failed run.
    pub is_retry: bool,
}

fn model_to_schedule(m: entities::schedules::Model) -> Result<Schedule, EneMemoryError> {
    Ok(Schedule {
        id: m.id,
        name: m.name,
        kind: ScheduleKind::from_db_str(&m.kind),
        enabled: m.enabled,
        timezone: m.timezone,
        cron_expr: m.cron_expr,
        interval_secs: m.interval_secs,
        start_at: m.start_at,
        action: serde_json::from_str::<ScheduleAction>(&m.action)?,
        confirmation: ene_core::ScheduleConfirmation::from_db_str(&m.confirmation),
        max_retries: m.max_retries,
        retry_delay_secs: m.retry_delay_secs,
        next_run_at: m.next_run_at,
        pending_retry_of_run_id: m.pending_retry_of_run_id,
        last_run_at: m.last_run_at,
        last_status: m.last_status.as_deref().map(ScheduleRunStatus::from_db_str),
        run_count: m.run_count,
        fail_count: m.fail_count,
        created_at: m.created_at,
        updated_at: m.updated_at,
    })
}

fn model_to_run(m: entities::schedule_runs::Model) -> ScheduleRun {
    ScheduleRun {
        id: m.id,
        schedule_id: m.schedule_id,
        scheduled_at: m.scheduled_at,
        started_at: m.started_at,
        finished_at: m.finished_at,
        status: ScheduleRunStatus::from_db_str(&m.status),
        retry_of_run_id: m.retry_of_run_id,
        retries: m.retries,
        error: m.error,
        created_at: m.created_at,
    }
}

impl MemoryStore {
    /// Validates a new schedule, computes its first fire time, and inserts it.
    pub async fn insert_schedule(
        &self,
        new: &NewSchedule,
        now: DateTime<Utc>,
    ) -> Result<Schedule, EneMemoryError> {
        let next_run_at = ene_core::first_run_at(new, now)?;
        let active = entities::schedules::ActiveModel {
            name: Set(new.name.clone()),
            kind: Set(new.kind.as_str().to_string()),
            enabled: Set(true),
            timezone: Set(new.timezone.clone()),
            cron_expr: Set(new.cron_expr.clone()),
            interval_secs: Set(new.interval_secs),
            start_at: Set(new.start_at),
            action: Set(serde_json::to_string(&new.action)?),
            confirmation: Set(new.confirmation.as_str().to_string()),
            max_retries: Set(new.max_retries),
            retry_delay_secs: Set(new.retry_delay_secs),
            next_run_at: Set(Some(next_run_at)),
            pending_retry_of_run_id: Set(None),
            last_run_at: Set(None),
            last_status: Set(None),
            run_count: Set(0),
            fail_count: Set(0),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        };
        let inserted = active.insert(&self.db).await?;
        let model: entities::schedules::Model = inserted.try_into_model()?;
        model_to_schedule(model)
    }

    /// Fetch a schedule by id.
    pub async fn get_schedule(&self, id: i64) -> Result<Option<Schedule>, EneMemoryError> {
        let maybe = entities::schedules::Entity::find_by_id(id)
            .one(&self.db)
            .await?;
        maybe.map(model_to_schedule).transpose()
    }

    /// Replace the editable fields of an existing schedule.
    ///
    /// Runs the same [`ene_core::first_run_at`] validation as
    /// [`Self::insert_schedule`] (name, kind fields, timezone, cron,
    /// interval, future one-shot start), recomputes `next_run_at`, and keeps
    /// the id, enabled flag, and run counters untouched.
    pub async fn update_schedule(
        &self,
        id: i64,
        new: &NewSchedule,
        now: DateTime<Utc>,
    ) -> Result<Schedule, EneMemoryError> {
        let next_run_at = ene_core::first_run_at(new, now)?;
        let active = entities::schedules::ActiveModel {
            id: Set(id),
            name: Set(new.name.clone()),
            kind: Set(new.kind.as_str().to_string()),
            timezone: Set(new.timezone.clone()),
            cron_expr: Set(new.cron_expr.clone()),
            interval_secs: Set(new.interval_secs),
            start_at: Set(new.start_at),
            action: Set(serde_json::to_string(&new.action)?),
            confirmation: Set(new.confirmation.as_str().to_string()),
            max_retries: Set(new.max_retries),
            retry_delay_secs: Set(new.retry_delay_secs),
            next_run_at: Set(Some(next_run_at)),
            pending_retry_of_run_id: Set(None),
            updated_at: Set(now),
            ..Default::default()
        };
        let updated = active.update(&self.db).await?;
        let model: entities::schedules::Model = updated.try_into_model()?;
        model_to_schedule(model)
    }

    /// Fetch a schedule by its unique name.
    pub async fn get_schedule_by_name(
        &self,
        name: &str,
    ) -> Result<Option<Schedule>, EneMemoryError> {
        let maybe = entities::schedules::Entity::find()
            .filter(entities::schedules::Column::Name.eq(name))
            .one(&self.db)
            .await?;
        maybe.map(model_to_schedule).transpose()
    }

    /// List all schedules ordered by name.
    pub async fn list_schedules(&self) -> Result<Vec<Schedule>, EneMemoryError> {
        let rows = entities::schedules::Entity::find()
            .order_by_asc(entities::schedules::Column::Name)
            .all(&self.db)
            .await?;
        rows.into_iter().map(model_to_schedule).collect()
    }

    /// Enable or disable a schedule; returns whether a row was updated.
    pub async fn set_schedule_enabled(
        &self,
        id: i64,
        enabled: bool,
        now: DateTime<Utc>,
    ) -> Result<bool, EneMemoryError> {
        let updated = entities::schedules::Entity::update_many()
            .col_expr(entities::schedules::Column::Enabled, Expr::value(enabled))
            .col_expr(entities::schedules::Column::UpdatedAt, Expr::value(now))
            .filter(entities::schedules::Column::Id.eq(id))
            .exec(&self.db)
            .await?;
        Ok(updated.rows_affected > 0)
    }

    /// Delete a schedule and its run history (cascade); returns whether a row
    /// was removed.
    pub async fn delete_schedule(&self, id: i64) -> Result<bool, EneMemoryError> {
        let deleted = entities::schedules::Entity::delete_by_id(id)
            .exec(&self.db)
            .await?;
        Ok(deleted.rows_affected > 0)
    }

    /// Schedules whose `next_run_at` is due at or before `now`.
    pub async fn list_due_schedules(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<Schedule>, EneMemoryError> {
        let rows = entities::schedules::Entity::find()
            .filter(entities::schedules::Column::Enabled.eq(true))
            .filter(entities::schedules::Column::NextRunAt.is_not_null())
            .filter(entities::schedules::Column::NextRunAt.lte(now))
            .order_by_asc(entities::schedules::Column::NextRunAt)
            .all(&self.db)
            .await?;
        rows.into_iter().map(model_to_schedule).collect()
    }

    /// The earliest `next_run_at` among enabled schedules, excluding ids the
    /// timer already dispatched (so it never busy-loops on a queued fire).
    pub async fn next_due_time_excluding(
        &self,
        exclude: &[i64],
    ) -> Result<Option<DateTime<Utc>>, EneMemoryError> {
        let mut query = entities::schedules::Entity::find()
            .filter(entities::schedules::Column::Enabled.eq(true))
            .filter(entities::schedules::Column::NextRunAt.is_not_null());
        if !exclude.is_empty() {
            query =
                query.filter(entities::schedules::Column::Id.is_not_in(exclude.iter().copied()));
        }
        let maybe = query
            .order_by_asc(entities::schedules::Column::NextRunAt)
            .one(&self.db)
            .await?;
        Ok(maybe.and_then(|m| m.next_run_at))
    }

    /// Atomically claim a fire for `schedule_id`, or return `None` when the
    /// fire is stale (schedule missing, disabled, or `next_run_at` no longer
    /// equals `scheduled_at`).
    ///
    /// The claim always inserts a run history row and advances
    /// `next_run_at` before returning, so a crash mid-run or a duplicate
    /// dispatch cannot re-execute the same occurrence.
    pub async fn claim_fire(
        &self,
        schedule_id: i64,
        scheduled_at: DateTime<Utc>,
        now: DateTime<Utc>,
        mode: FireClaimMode,
    ) -> Result<Option<ClaimedFire>, EneMemoryError> {
        let mut attempts = 5;
        loop {
            match self
                .claim_fire_once(schedule_id, scheduled_at, now, mode)
                .await
            {
                Err(e) if is_busy_error(&e) && attempts > 1 => {
                    attempts -= 1;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                other => return other,
            }
        }
    }

    async fn claim_fire_once(
        &self,
        schedule_id: i64,
        scheduled_at: DateTime<Utc>,
        now: DateTime<Utc>,
        mode: FireClaimMode,
    ) -> Result<Option<ClaimedFire>, EneMemoryError> {
        let txn = self.db.begin().await?;
        let Some(model) = entities::schedules::Entity::find_by_id(schedule_id)
            .one(&txn)
            .await?
        else {
            txn.rollback().await?;
            return Ok(None);
        };
        if !model.enabled || model.next_run_at.as_ref() != Some(&scheduled_at) {
            txn.rollback().await?;
            return Ok(None);
        }
        let schedule = model_to_schedule(model.clone())?;
        let is_retry = model.pending_retry_of_run_id.is_some();

        let (status, next_run_at, retry_of_run_id, retries) = match mode {
            FireClaimMode::Execute | FireClaimMode::AwaitConfirmation => {
                let status = if mode == FireClaimMode::AwaitConfirmation {
                    ScheduleRunStatus::AwaitingApproval
                } else {
                    ScheduleRunStatus::Running
                };
                if let Some(retry_pointer) = model.pending_retry_of_run_id {
                    let previous = entities::schedule_runs::Entity::find_by_id(retry_pointer)
                        .one(&txn)
                        .await?;
                    let retries = previous.map_or(0, |p| p.retries + 1);
                    (
                        status,
                        ene_core::next_occurrence_after(&schedule, scheduled_at),
                        Some(retry_pointer),
                        retries,
                    )
                } else {
                    (
                        status,
                        ene_core::next_occurrence_after(&schedule, scheduled_at),
                        None,
                        0,
                    )
                }
            }
            FireClaimMode::SkipBusy | FireClaimMode::SkipLate => {
                let status = if mode == FireClaimMode::SkipLate {
                    ScheduleRunStatus::SkippedLate
                } else {
                    ScheduleRunStatus::SkippedBusy
                };
                if is_retry {
                    // A skipped retry is re-armed, not consumed.
                    (
                        status,
                        Some(now + Duration::seconds(model.retry_delay_secs.max(1))),
                        model.pending_retry_of_run_id,
                        0,
                    )
                } else {
                    let after = if mode == FireClaimMode::SkipLate {
                        now
                    } else {
                        scheduled_at
                    };
                    (
                        status,
                        ene_core::next_occurrence_after(&schedule, after),
                        None,
                        0,
                    )
                }
            }
        };

        let terminal_now = if status.is_terminal() {
            Some(now)
        } else {
            None
        };
        let run = entities::schedule_runs::ActiveModel {
            schedule_id: Set(model.id),
            scheduled_at: Set(scheduled_at),
            started_at: Set(Some(now)),
            finished_at: Set(terminal_now),
            status: Set(status.as_str().to_string()),
            retry_of_run_id: Set(retry_of_run_id),
            retries: Set(retries),
            error: Set(None),
            created_at: Set(now),
            ..Default::default()
        }
        .insert(&txn)
        .await?;

        let mut updated: entities::schedules::ActiveModel = model.clone().into();
        updated.next_run_at = Set(next_run_at);
        if matches!(
            mode,
            FireClaimMode::Execute | FireClaimMode::AwaitConfirmation
        ) {
            updated.pending_retry_of_run_id = Set(None);
        }
        updated.last_run_at = Set(Some(now));
        updated.last_status = Set(Some(status.as_str().to_string()));
        updated.run_count = Set(model.run_count + 1);
        updated.updated_at = Set(now);
        updated.update(&txn).await?;
        txn.commit().await?;

        Ok(Some(ClaimedFire {
            schedule,
            run_id: run.id,
            is_retry,
        }))
    }

    /// Record a terminal status for a run and update its schedule counters.
    ///
    /// Failed runs arm a retry (setting `next_run_at` and the retry pointer)
    /// when `retries < max_retries`. Idempotent: runs already terminal are
    /// left untouched, so a retry-fire outcome can never overwrite a newer
    /// attempt's history.
    pub async fn finish_run(
        &self,
        schedule_id: i64,
        run_id: i64,
        status: ScheduleRunStatus,
        error: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<(), EneMemoryError> {
        let mut attempts = 5;
        loop {
            match self
                .finish_run_once(schedule_id, run_id, status, error.clone(), now)
                .await
            {
                Err(e) if is_busy_error(&e) && attempts > 1 => {
                    attempts -= 1;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                other => return other,
            }
        }
    }

    async fn finish_run_once(
        &self,
        schedule_id: i64,
        run_id: i64,
        status: ScheduleRunStatus,
        error: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<(), EneMemoryError> {
        let txn = self.db.begin().await?;
        let Some(run) = entities::schedule_runs::Entity::find_by_id(run_id)
            .one(&txn)
            .await?
        else {
            txn.rollback().await?;
            return Ok(());
        };
        let run_status = ScheduleRunStatus::from_db_str(&run.status);
        // Status-transition guard: a terminal row is immutable, and the only
        // legal transitions are `awaiting_approval -> denied | timed_out |
        // skipped_busy | running` and `running -> success | failed`. This
        // keeps a stale confirmation timeout from clobbering an approved run
        // that already started executing (`running`).
        let transition_allowed = match run_status {
            ScheduleRunStatus::AwaitingApproval => matches!(
                status,
                ScheduleRunStatus::Denied
                    | ScheduleRunStatus::TimedOut
                    | ScheduleRunStatus::SkippedBusy
            ),
            ScheduleRunStatus::Running => matches!(
                status,
                ScheduleRunStatus::Success | ScheduleRunStatus::Failed
            ),
            _ => false,
        };
        if run.schedule_id != schedule_id || run_status.is_terminal() || !transition_allowed {
            txn.rollback().await?;
            return Ok(());
        }
        let mut run_update: entities::schedule_runs::ActiveModel = run.clone().into();
        run_update.status = Set(status.as_str().to_string());
        run_update.finished_at = Set(Some(now));
        run_update.error = Set(error);
        run_update.update(&txn).await?;

        if let Some(schedule) = entities::schedules::Entity::find_by_id(schedule_id)
            .one(&txn)
            .await?
        {
            let mut schedule_update: entities::schedules::ActiveModel = schedule.clone().into();
            schedule_update.last_run_at = Set(Some(now));
            schedule_update.last_status = Set(Some(status.as_str().to_string()));
            schedule_update.updated_at = Set(now);
            if status == ScheduleRunStatus::Failed {
                schedule_update.fail_count = Set(schedule.fail_count + 1);
                if run.retries < schedule.max_retries && schedule.pending_retry_of_run_id.is_none()
                {
                    schedule_update.next_run_at = Set(Some(
                        now + Duration::seconds(schedule.retry_delay_secs.max(1)),
                    ));
                    schedule_update.pending_retry_of_run_id = Set(Some(run_id));
                }
            }
            schedule_update.update(&txn).await?;
        }
        txn.commit().await?;
        Ok(())
    }

    /// Transition an approved run from `awaiting_approval` to `running` when
    /// execution actually starts.
    ///
    /// Without this, a stale confirmation timeout could still mark the run
    /// `timed_out` mid-execution, and a crash mid-execution would be
    /// reconciled as `timed_out` instead of `interrupted`.
    pub async fn mark_run_running(
        &self,
        schedule_id: i64,
        run_id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), EneMemoryError> {
        let txn = self.db.begin().await?;
        let Some(run) = entities::schedule_runs::Entity::find_by_id(run_id)
            .one(&txn)
            .await?
        else {
            txn.rollback().await?;
            return Ok(());
        };
        if run.schedule_id != schedule_id
            || ScheduleRunStatus::from_db_str(&run.status) != ScheduleRunStatus::AwaitingApproval
        {
            txn.rollback().await?;
            return Ok(());
        }
        let mut run_update: entities::schedule_runs::ActiveModel = run.into();
        run_update.status = Set(ScheduleRunStatus::Running.as_str().to_string());
        run_update.update(&txn).await?;
        if let Some(schedule) = entities::schedules::Entity::find_by_id(schedule_id)
            .one(&txn)
            .await?
        {
            let mut schedule_update: entities::schedules::ActiveModel = schedule.into();
            schedule_update.last_status =
                Set(Some(ScheduleRunStatus::Running.as_str().to_string()));
            schedule_update.updated_at = Set(now);
            schedule_update.update(&txn).await?;
        }
        txn.commit().await?;
        Ok(())
    }

    /// Mark runs left in flight by a crash (`running` → `interrupted`,
    /// `awaiting_approval` → `timed_out`) and sync the owning schedules'
    /// `last_status`.
    pub async fn reconcile_startup(&self, now: DateTime<Utc>) -> Result<(), EneMemoryError> {
        let txn = self.db.begin().await?;
        let running_ids: Vec<i64> = entities::schedule_runs::Entity::find()
            .filter(entities::schedule_runs::Column::Status.eq(ScheduleRunStatus::Running.as_str()))
            .all(&txn)
            .await?
            .into_iter()
            .map(|r| r.schedule_id)
            .collect();
        let awaiting_ids: Vec<i64> = entities::schedule_runs::Entity::find()
            .filter(
                entities::schedule_runs::Column::Status
                    .eq(ScheduleRunStatus::AwaitingApproval.as_str()),
            )
            .all(&txn)
            .await?
            .into_iter()
            .map(|r| r.schedule_id)
            .collect();

        entities::schedule_runs::Entity::update_many()
            .col_expr(
                entities::schedule_runs::Column::Status,
                Expr::value(ScheduleRunStatus::Interrupted.as_str()),
            )
            .col_expr(
                entities::schedule_runs::Column::FinishedAt,
                Expr::value(now),
            )
            .filter(entities::schedule_runs::Column::Status.eq(ScheduleRunStatus::Running.as_str()))
            .exec(&txn)
            .await?;
        entities::schedule_runs::Entity::update_many()
            .col_expr(
                entities::schedule_runs::Column::Status,
                Expr::value(ScheduleRunStatus::TimedOut.as_str()),
            )
            .col_expr(
                entities::schedule_runs::Column::FinishedAt,
                Expr::value(now),
            )
            .filter(
                entities::schedule_runs::Column::Status
                    .eq(ScheduleRunStatus::AwaitingApproval.as_str()),
            )
            .exec(&txn)
            .await?;

        if !awaiting_ids.is_empty() {
            entities::schedules::Entity::update_many()
                .col_expr(
                    entities::schedules::Column::LastStatus,
                    Expr::value(ScheduleRunStatus::TimedOut.as_str()),
                )
                .col_expr(entities::schedules::Column::UpdatedAt, Expr::value(now))
                .filter(entities::schedules::Column::Id.is_in(awaiting_ids.iter().copied()))
                .exec(&txn)
                .await?;
        }
        // A schedule whose only in-flight run was `awaiting_approval` must
        // keep `timed_out`, so the `interrupted` sync applies only to
        // schedules that actually had a `running` run.
        if !running_ids.is_empty() {
            entities::schedules::Entity::update_many()
                .col_expr(
                    entities::schedules::Column::LastStatus,
                    Expr::value(ScheduleRunStatus::Interrupted.as_str()),
                )
                .col_expr(entities::schedules::Column::UpdatedAt, Expr::value(now))
                .filter(entities::schedules::Column::Id.is_in(running_ids.iter().copied()))
                .exec(&txn)
                .await?;
        }
        txn.commit().await?;
        Ok(())
    }

    /// Arm completed startup schedules for this process start: their
    /// `next_run_at` becomes `now`, so the timer fires each of them exactly
    /// once per app start.
    pub async fn arm_startup_schedules(&self, now: DateTime<Utc>) -> Result<u64, EneMemoryError> {
        let updated = entities::schedules::Entity::update_many()
            .col_expr(entities::schedules::Column::NextRunAt, Expr::value(now))
            .col_expr(entities::schedules::Column::UpdatedAt, Expr::value(now))
            .filter(entities::schedules::Column::Enabled.eq(true))
            .filter(entities::schedules::Column::Kind.eq(ScheduleKind::Startup.as_str()))
            .filter(entities::schedules::Column::NextRunAt.is_null())
            .exec(&self.db)
            .await?;
        Ok(updated.rows_affected)
    }

    /// Recent run history for a schedule, newest first.
    pub async fn list_runs(
        &self,
        schedule_id: i64,
        limit: u64,
    ) -> Result<Vec<ScheduleRun>, EneMemoryError> {
        let rows = entities::schedule_runs::Entity::find()
            .filter(entities::schedule_runs::Column::ScheduleId.eq(schedule_id))
            .order_by_desc(entities::schedule_runs::Column::ScheduledAt)
            .order_by_desc(entities::schedule_runs::Column::Id)
            .limit(limit)
            .all(&self.db)
            .await?;
        Ok(rows.into_iter().map(model_to_run).collect())
    }
}
