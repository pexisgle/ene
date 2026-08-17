use ene_session::{DelegationId, SoulId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationMode {
    Public,
    Internal,
}

impl DelegationMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        if raw == "internal" {
            Self::Internal
        } else {
            Self::Public
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Created,
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl JobStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            _ => Self::Created,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: DelegationId,
    pub soul_id: SoulId,
    pub title: String,
    pub goal: String,
    pub mode: DelegationMode,
    pub status: JobStatus,
    pub progress_fraction: Option<f32>,
    pub progress_note: Option<String>,
    pub workspace_dir: String,
    pub error_class: Option<String>,
    pub created_from_turn: Option<String>,
    pub plan: Option<String>,
    pub brief: Option<String>,
    pub plan_approved: bool,
    pub created_at: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewJob {
    pub id: Option<DelegationId>,
    pub soul_id: SoulId,
    pub title: String,
    pub goal: String,
    pub mode: DelegationMode,
    pub workspace_dir: String,
    pub created_from_turn: Option<String>,
    pub plan: Option<String>,
    pub brief: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleAction {
    Remind,
    Job,
    Turn,
}

impl ScheduleAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Remind => "remind",
            Self::Job => "job",
            Self::Turn => "turn",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "job" => Self::Job,
            "turn" => Self::Turn,
            _ => Self::Remind,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewSchedule {
    pub soul_id: SoulId,
    pub name: String,
    pub spec: String,
    pub timezone: String,
    pub action: ScheduleAction,
    pub action_ref: Option<String>,
    pub important: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    pub id: String,
    pub soul_id: SoulId,
    pub name: String,
    pub spec: String,
    pub timezone: String,
    pub action: ScheduleAction,
    pub action_ref: Option<String>,
    pub enabled: bool,
    pub important: bool,
    pub last_fired: Option<String>,
    pub next_fire: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    Text,
    Markdown,
    Csv,
}

impl ArtifactKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Markdown => "markdown",
            Self::Csv => "csv",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        Self::try_parse(raw).unwrap_or(Self::Text)
    }

    pub fn try_parse(raw: &str) -> Result<Self, crate::error::WorkError> {
        match raw {
            "text" => Ok(Self::Text),
            "markdown" => Ok(Self::Markdown),
            "csv" => Ok(Self::Csv),
            other => Err(crate::error::WorkError::UnsupportedArtifact(
                other.to_owned(),
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Artifact {
    pub id: String,
    pub soul_id: SoulId,
    pub job_id: Option<DelegationId>,
    pub kind: ArtifactKind,
    pub title: String,
    pub path: String,
    pub mime: Option<String>,
    pub size_bytes: Option<i64>,
    pub created_at: String,
    pub delivered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeReason {
    SideEffectTool,
    StepBudget,
}

impl UpgradeReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SideEffectTool => "side_effect_tool",
            Self::StepBudget => "step_budget",
        }
    }
}

/// Report the companion should speak (D-13). Never a status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionReport {
    pub speech: String,
    pub inner_intent: Option<String>,
    /// Progress companion speech does not open a surface conversation turn.
    pub starts_conversation: bool,
}

/// Child question still waiting for a parent answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenQuestion {
    pub delegation_id: ene_session::DelegationId,
    pub mailbox_seq: i64,
    pub prompt: String,
    pub asked_at: String,
}

/// Parent-facing combined ask-user turn for one or more child questions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombinedQuestionTurn {
    pub speech: String,
    pub questions: Vec<OpenQuestion>,
}

/// Delegation resource guards mirrored from kernel `DelegationSettings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkDelegationSettings {
    pub max_active: u32,
    pub max_depth: u32,
    pub question_timeout_hours: u32,
}

impl Default for WorkDelegationSettings {
    fn default() -> Self {
        Self {
            max_active: 8,
            max_depth: 3,
            question_timeout_hours: 24,
        }
    }
}
