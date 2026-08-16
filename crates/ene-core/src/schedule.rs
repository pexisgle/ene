//! Persistent scheduler domain model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Schedule validation / computation errors.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    /// The schedule name is empty or whitespace-only.
    #[error("schedule name must not be empty")]
    EmptyName,
    /// A required field is missing for the schedule kind.
    #[error("{field} is required for {kind} schedules")]
    MissingField {
        field: &'static str,
        kind: &'static str,
    },
    /// The timezone name is not a valid IANA zone.
    #[error("invalid timezone `{value}`: {detail}")]
    InvalidTimezone { value: String, detail: String },
    /// The cron expression is not valid.
    #[error("invalid cron expression `{value}`: {detail}")]
    InvalidCron { value: String, detail: String },
    /// The interval must be positive.
    #[error("interval must be at least 1 second (got {value})")]
    InvalidInterval { value: i64 },
    /// A one-shot `start_at` must lie in the future.
    #[error("one-shot start_at must be in the future (got {value})")]
    InvalidStartAt { value: DateTime<Utc> },
    /// No occurrence exists (e.g. a cron expression that never fires).
    #[error("schedule has no upcoming occurrence")]
    NoNextOccurrence,
}

/// How often a schedule fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScheduleKind {
    /// Fires once at `start_at`, then completes.
    OneShot,
    /// Fires at a fixed rate anchored on `start_at` (`start_at + k * interval`).
    Interval,
    /// Fires per a cron expression evaluated in the schedule's timezone.
    Cron,
    /// Fires once per process start (its semantic; no cross-start dedupe).
    Startup,
}

impl ScheduleKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OneShot => "one_shot",
            Self::Interval => "interval",
            Self::Cron => "cron",
            Self::Startup => "startup",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "one_shot" => Self::OneShot,
            "interval" => Self::Interval,
            "cron" => Self::Cron,
            "startup" => Self::Startup,
            other => {
                tracing::error!(
                    other,
                    "unrecognized ScheduleKind in DB, falling back to OneShot"
                );
                Self::OneShot
            }
        }
    }
}

/// Whether a scheduled action requires user confirmation before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScheduleConfirmation {
    /// Execute as soon as the actor is idle.
    None,
    /// Ask the user (via `PermissionRequired`) before starting the action.
    Confirm,
}

impl ScheduleConfirmation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Confirm => "confirm",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "none" => Self::None,
            "confirm" => Self::Confirm,
            other => {
                tracing::error!(
                    other,
                    "unrecognized ScheduleConfirmation in DB, falling back to None"
                );
                Self::None
            }
        }
    }
}

/// What a scheduled run executes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleAction {
    /// Direct tool invocation with JSON arguments.
    Tool {
        /// Fully-qualified tool name (e.g. `fs.write`).
        name: String,
        /// JSON-encoded tool arguments.
        arguments: serde_json::Value,
    },
    /// Companion speech / processing prompt run as a turn.
    Prompt {
        /// The prompt text composed into the turn.
        text: String,
        allow_tools: bool,
    },
}

/// A persistent schedule row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: i64,
    /// Unique display/reference name.
    pub name: String,
    pub kind: ScheduleKind,
    pub enabled: bool,
    /// IANA timezone name (e.g. `Asia/Tokyo`); used for cron evaluation.
    pub timezone: String,
    /// Cron expression (5 or 6 fields); `Some` only for `Cron`.
    pub cron_expr: Option<String>,
    /// Interval in seconds; `Some` only for `Interval`.
    pub interval_secs: Option<i64>,
    /// Anchor instant: fire time for `OneShot`, rate anchor for `Interval`.
    pub start_at: Option<DateTime<Utc>>,
    pub action: ScheduleAction,
    pub confirmation: ScheduleConfirmation,
    /// Extra attempts after a failed run (0 = no retries).
    pub max_retries: i64,
    /// Delay before a retry attempt, in seconds.
    pub retry_delay_secs: i64,
    /// Next due instant; `None` when nothing is pending (completed one-shot).
    pub next_run_at: Option<DateTime<Utc>>,
    /// The failed run a pending retry belongs to; cleared when the retry fires.
    pub pending_retry_of_run_id: Option<i64>,
    /// When the last run attempt was claimed.
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_status: Option<ScheduleRunStatus>,
    /// Total claimed run attempts (including skips).
    pub run_count: i64,
    pub fail_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Payload for creating a schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSchedule {
    /// Unique display/reference name.
    pub name: String,
    pub kind: ScheduleKind,
    /// IANA timezone name.
    pub timezone: String,
    /// Cron expression for `Cron` schedules.
    pub cron_expr: Option<String>,
    /// Interval in seconds for `Interval` schedules.
    pub interval_secs: Option<i64>,
    /// Anchor instant for `OneShot` / `Interval` schedules.
    pub start_at: Option<DateTime<Utc>>,
    pub action: ScheduleAction,
    pub confirmation: ScheduleConfirmation,
    /// Extra attempts after a failed run.
    pub max_retries: i64,
    /// Delay before a retry attempt, in seconds.
    pub retry_delay_secs: i64,
}

/// Outcome of a schedule run attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ScheduleRunStatus {
    /// Claimed and executing (or waiting for confirmation).
    Running,
    /// Finished successfully.
    Success,
    /// Finished with an error; eligible for retry.
    Failed,
    /// The actor was busy with a conversation at fire time.
    SkippedBusy,
    /// The fire arrived beyond the late-execution grace window.
    SkippedLate,
    /// The user denied the confirmation prompt.
    Denied,
    /// The confirmation prompt was never answered before the timeout.
    TimedOut,
    /// The process restarted while the run was in flight.
    Interrupted,
    /// Waiting for a confirmation decision.
    AwaitingApproval,
}

impl ScheduleRunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::SkippedBusy => "skipped_busy",
            Self::SkippedLate => "skipped_late",
            Self::Denied => "denied",
            Self::TimedOut => "timed_out",
            Self::Interrupted => "interrupted",
            Self::AwaitingApproval => "awaiting_approval",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "success" => Self::Success,
            "failed" => Self::Failed,
            "skipped_busy" => Self::SkippedBusy,
            "skipped_late" => Self::SkippedLate,
            "denied" => Self::Denied,
            "timed_out" => Self::TimedOut,
            "interrupted" => Self::Interrupted,
            "awaiting_approval" => Self::AwaitingApproval,
            other => {
                tracing::error!(
                    other,
                    "unrecognized ScheduleRunStatus in DB, falling back to Interrupted"
                );
                Self::Interrupted
            }
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Success
                | Self::Failed
                | Self::SkippedBusy
                | Self::SkippedLate
                | Self::Denied
                | Self::TimedOut
                | Self::Interrupted
        )
    }
}

/// One attempt row in the schedule run history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleRun {
    pub id: i64,
    pub schedule_id: i64,
    /// The occurrence instant this attempt was claimed for.
    pub scheduled_at: DateTime<Utc>,
    /// When the attempt was claimed.
    pub started_at: Option<DateTime<Utc>>,
    /// When the attempt reached a terminal status.
    pub finished_at: Option<DateTime<Utc>>,
    pub status: ScheduleRunStatus,
    /// The failed attempt this attempt retries (chain head for the first attempt).
    pub retry_of_run_id: Option<i64>,
    /// Number of retries already performed for this logical fire (0 = first attempt).
    pub retries: i64,
    /// Failure detail for `Failed` attempts.
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}
