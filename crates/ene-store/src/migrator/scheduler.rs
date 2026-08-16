//! Persistent scheduler tables.
//!
//! `schedules` holds the schedule definitions plus the scheduler's own
//! bookkeeping (`next_run_at`, retry pointer, counters). `schedule_runs`
//! holds one row per claimed fire attempt so history survives restarts.
//! Both tables are created `IF NOT EXISTS` so the migration is additive and
//! idempotent, matching the other post-initial migrations.

use sea_orm_migration::prelude::*;

pub struct SchedulerMigration;

impl MigrationName for SchedulerMigration {
    fn name(&self) -> &'static str {
        "m20260804_000007_scheduler"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for SchedulerMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Schedules::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Schedules::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Schedules::Name)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(Schedules::Kind).string().not_null())
                    .col(
                        ColumnDef::new(Schedules::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(ColumnDef::new(Schedules::Timezone).string().not_null())
                    .col(ColumnDef::new(Schedules::CronExpr).string())
                    .col(ColumnDef::new(Schedules::IntervalSecs).big_integer())
                    .col(ColumnDef::new(Schedules::StartAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(Schedules::Action).string().not_null())
                    .col(
                        ColumnDef::new(Schedules::Confirmation)
                            .string()
                            .not_null()
                            .default("none"),
                    )
                    .col(
                        ColumnDef::new(Schedules::MaxRetries)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Schedules::RetryDelaySecs)
                            .big_integer()
                            .not_null()
                            .default(60),
                    )
                    .col(ColumnDef::new(Schedules::NextRunAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(Schedules::PendingRetryOfRunId).big_integer())
                    .col(ColumnDef::new(Schedules::LastRunAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(Schedules::LastStatus).string())
                    .col(
                        ColumnDef::new(Schedules::RunCount)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Schedules::FailCount)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Schedules::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Schedules::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(ScheduleRuns::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ScheduleRuns::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ScheduleRuns::ScheduleId)
                            .big_integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ScheduleRuns::ScheduledAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ScheduleRuns::StartedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(ScheduleRuns::FinishedAt).timestamp_with_time_zone())
                    .col(ColumnDef::new(ScheduleRuns::Status).string().not_null())
                    .col(ColumnDef::new(ScheduleRuns::RetryOfRunId).big_integer())
                    .col(
                        ColumnDef::new(ScheduleRuns::Retries)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(ScheduleRuns::Error).string())
                    .col(
                        ColumnDef::new(ScheduleRuns::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_schedule_runs_schedule_id")
                            .from(ScheduleRuns::Table, ScheduleRuns::ScheduleId)
                            .to(Schedules::Table, Schedules::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_schedule_runs_schedule_id")
                    .table(ScheduleRuns::Table)
                    .col(ScheduleRuns::ScheduleId)
                    .col(ScheduleRuns::ScheduledAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_schedules_next_run_at")
                    .table(Schedules::Table)
                    .col(Schedules::Enabled)
                    .col(Schedules::NextRunAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(Iden)]
enum Schedules {
    Table,
    Id,
    Name,
    Kind,
    Enabled,
    Timezone,
    CronExpr,
    IntervalSecs,
    StartAt,
    Action,
    Confirmation,
    MaxRetries,
    RetryDelaySecs,
    NextRunAt,
    PendingRetryOfRunId,
    LastRunAt,
    LastStatus,
    RunCount,
    FailCount,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum ScheduleRuns {
    Table,
    Id,
    ScheduleId,
    ScheduledAt,
    StartedAt,
    FinishedAt,
    Status,
    RetryOfRunId,
    Retries,
    Error,
    CreatedAt,
}
