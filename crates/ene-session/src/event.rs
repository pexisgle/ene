use crate::block::{Block, InnerAspect};
use crate::error::SessionError;
use crate::ids::{
    BodyId, CallId, ClientId, DelegationId, EpochId, QuestionId, SessionId, SoulId, TurnId,
};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

/// Payload schema version written by this binary.
pub const PAYLOAD_VERSION: u32 = 1;

/// Event kind strings as stored in `session_events.kind`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    #[serde(rename = "session/start")]
    SessionStart,
    #[serde(rename = "session/title")]
    SessionTitle,
    #[serde(rename = "session/summary")]
    SessionSummary,
    #[serde(rename = "session/end")]
    SessionEnd,
    #[serde(rename = "session/reopen")]
    SessionReopen,
    #[serde(rename = "session/archived")]
    SessionArchived,
    #[serde(rename = "fork/point")]
    ForkPoint,
    #[serde(rename = "turn/start")]
    TurnStart,
    #[serde(rename = "turn/end")]
    TurnEnd,
    #[serde(rename = "step/start")]
    StepStart,
    #[serde(rename = "step/end")]
    StepEnd,
    #[serde(rename = "user/message")]
    UserMessage,
    #[serde(rename = "assistant/message")]
    AssistantMessage,
    #[serde(rename = "assistant/thinking")]
    AssistantThinking,
    #[serde(rename = "inner/message")]
    InnerMessage,
    #[serde(rename = "context/system_message")]
    ContextSystemMessage,
    #[serde(rename = "context/epoch")]
    ContextEpoch,
    #[serde(rename = "compaction/applied")]
    CompactionApplied,
    #[serde(rename = "tool/call")]
    ToolCall,
    #[serde(rename = "tool/result")]
    ToolResult,
    #[serde(rename = "tool/spill")]
    ToolSpill,
    #[serde(rename = "tool/pruned")]
    ToolPruned,
    #[serde(rename = "question/asked")]
    QuestionAsked,
    #[serde(rename = "approval/decision")]
    ApprovalDecision,
    #[serde(rename = "redaction")]
    Redaction,
    #[serde(rename = "delegation/start")]
    DelegationStart,
    #[serde(rename = "delegation/progress")]
    DelegationProgress,
    #[serde(rename = "delegation/question")]
    DelegationQuestion,
    #[serde(rename = "delegation/answer")]
    DelegationAnswer,
    #[serde(rename = "delegation/end")]
    DelegationEnd,
    #[serde(rename = "inbox/enqueued")]
    InboxEnqueued,
    #[serde(rename = "inbox/claimed")]
    InboxClaimed,
    #[serde(rename = "inbox/cancelled")]
    InboxCancelled,
    /// Forward-compatible unknown kind; stored and skipped by projection.
    Unknown(String),
}

impl EventKind {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::SessionStart => "session/start",
            Self::SessionTitle => "session/title",
            Self::SessionSummary => "session/summary",
            Self::SessionEnd => "session/end",
            Self::SessionReopen => "session/reopen",
            Self::SessionArchived => "session/archived",
            Self::ForkPoint => "fork/point",
            Self::TurnStart => "turn/start",
            Self::TurnEnd => "turn/end",
            Self::StepStart => "step/start",
            Self::StepEnd => "step/end",
            Self::UserMessage => "user/message",
            Self::AssistantMessage => "assistant/message",
            Self::AssistantThinking => "assistant/thinking",
            Self::InnerMessage => "inner/message",
            Self::ContextSystemMessage => "context/system_message",
            Self::ContextEpoch => "context/epoch",
            Self::CompactionApplied => "compaction/applied",
            Self::ToolCall => "tool/call",
            Self::ToolResult => "tool/result",
            Self::ToolSpill => "tool/spill",
            Self::ToolPruned => "tool/pruned",
            Self::QuestionAsked => "question/asked",
            Self::ApprovalDecision => "approval/decision",
            Self::Redaction => "redaction",
            Self::DelegationStart => "delegation/start",
            Self::DelegationProgress => "delegation/progress",
            Self::DelegationQuestion => "delegation/question",
            Self::DelegationAnswer => "delegation/answer",
            Self::DelegationEnd => "delegation/end",
            Self::InboxEnqueued => "inbox/enqueued",
            Self::InboxClaimed => "inbox/claimed",
            Self::InboxCancelled => "inbox/cancelled",
            Self::Unknown(kind) => kind.as_str(),
        }
    }

    #[must_use]
    pub fn parse(kind: &str) -> Self {
        match kind {
            "session/start" => Self::SessionStart,
            "session/title" => Self::SessionTitle,
            "session/summary" => Self::SessionSummary,
            "session/end" => Self::SessionEnd,
            "session/reopen" => Self::SessionReopen,
            "session/archived" => Self::SessionArchived,
            "fork/point" => Self::ForkPoint,
            "turn/start" => Self::TurnStart,
            "turn/end" => Self::TurnEnd,
            "step/start" => Self::StepStart,
            "step/end" => Self::StepEnd,
            "user/message" => Self::UserMessage,
            "assistant/message" => Self::AssistantMessage,
            "assistant/thinking" => Self::AssistantThinking,
            "inner/message" => Self::InnerMessage,
            "context/system_message" => Self::ContextSystemMessage,
            "context/epoch" => Self::ContextEpoch,
            "compaction/applied" => Self::CompactionApplied,
            "tool/call" => Self::ToolCall,
            "tool/result" => Self::ToolResult,
            "tool/spill" => Self::ToolSpill,
            "tool/pruned" => Self::ToolPruned,
            "question/asked" => Self::QuestionAsked,
            "approval/decision" => Self::ApprovalDecision,
            "redaction" => Self::Redaction,
            "delegation/start" => Self::DelegationStart,
            "delegation/progress" => Self::DelegationProgress,
            "delegation/question" => Self::DelegationQuestion,
            "delegation/answer" => Self::DelegationAnswer,
            "delegation/end" => Self::DelegationEnd,
            "inbox/enqueued" => Self::InboxEnqueued,
            "inbox/claimed" => Self::InboxClaimed,
            "inbox/cancelled" => Self::InboxCancelled,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

impl Display for EventKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lane recorded on `turn/start`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LaneId {
    Dialogue,
    Delegation(DelegationId),
}

impl LaneId {
    #[must_use]
    pub fn as_str(&self) -> String {
        match self {
            Self::Dialogue => "dialogue".to_owned(),
            Self::Delegation(id) => format!("delegation:{id}"),
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        raw.strip_prefix("delegation:")
            .and_then(|id| id.parse().ok())
            .map_or(Self::Dialogue, Self::Delegation)
    }
}

/// Who started a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOrigin {
    User,
    Proactive,
    Scheduled,
    Delegation,
    Subagent,
}

/// How the user (or system) triggered a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnTrigger {
    Text,
    Voice,
    Timer,
    System,
}

/// Terminal turn outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Interrupted,
    Cancelled,
    Failed,
}

/// Step finish classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepOutcome {
    Next,
    Stop,
    Error,
}

/// Inbox class (wake / inject / interrupt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxClass {
    Wake,
    Inject,
    Interrupt,
}

/// Who enqueued an inbox item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxSource {
    User,
    Delegation,
    AskUser,
    Approval,
    Steer,
}

/// Why an inbox item was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxCancelReason {
    User,
    AbandonedInterrupt,
}

/// Session creator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionCreatedBy {
    Client,
    Schedule,
    Import,
}

/// Why a conversation session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEndReason {
    Explicit,
    IdleTimeout,
}

/// Tool-result status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Ok,
    Error,
    Cancelled,
    Denied,
}

/// Typed payload stored as `MessagePack`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventPayload {
    SessionStart {
        v: u32,
        soul_id: SoulId,
        body_id: Option<BodyId>,
        created_by: SessionCreatedBy,
    },
    SessionTitle {
        v: u32,
        title: String,
    },
    SessionSummary {
        v: u32,
        scope: String,
        summary: String,
    },
    SessionEnd {
        v: u32,
        reason: SessionEndReason,
        summary_ref: Option<u64>,
    },
    SessionReopen {
        v: u32,
        previous_end_seq: u64,
    },
    SessionArchived {
        v: u32,
        archived: bool,
    },
    ForkPoint {
        v: u32,
        source_session_id: SessionId,
        boundary_seq: u64,
    },
    TurnStart {
        v: u32,
        turn_id: TurnId,
        lane: String,
        origin: TurnOrigin,
        delegation_id: Option<DelegationId>,
        trigger: TurnTrigger,
    },
    TurnEnd {
        v: u32,
        turn_id: TurnId,
        outcome: TurnOutcome,
        error_class: Option<String>,
    },
    StepStart {
        v: u32,
        turn_id: TurnId,
        step_index: u32,
    },
    StepEnd {
        v: u32,
        turn_id: TurnId,
        step_index: u32,
        outcome: StepOutcome,
        finish_reason: Option<String>,
    },
    UserMessage {
        v: u32,
        turn_id: Option<TurnId>,
        blocks: Vec<Block>,
        input_modality: String,
        client_id: ClientId,
    },
    AssistantMessage {
        v: u32,
        turn_id: TurnId,
        step_index: u32,
        blocks: Vec<Block>,
        finish_reason: String,
        token_count: Option<u32>,
    },
    AssistantThinking {
        v: u32,
        turn_id: TurnId,
        step_index: u32,
        blocks: Vec<Block>,
        model_id: String,
    },
    InnerMessage {
        v: u32,
        turn_id: Option<TurnId>,
        step_index: Option<u32>,
        aspects: Vec<InnerAspect>,
        blocks: Vec<Block>,
        model_visible: bool,
    },
    ContextSystemMessage {
        v: u32,
        blocks: Vec<Block>,
        source_key: String,
    },
    ContextEpoch {
        v: u32,
        epoch_id: EpochId,
        reason: String,
    },
    CompactionApplied {
        v: u32,
        from_seq: u64,
        to_seq: u64,
        summary_event_seq: u64,
    },
    ToolCall {
        v: u32,
        turn_id: TurnId,
        step_index: u32,
        call_id: CallId,
        tool_name: String,
        source: String,
        args: serde_json::Value,
    },
    ToolResult {
        v: u32,
        call_id: CallId,
        status: ToolStatus,
        blocks: Vec<Block>,
        spill_ref: Option<String>,
        error_class: Option<String>,
        duration_ms: u64,
    },
    ToolSpill {
        v: u32,
        call_id: CallId,
        spill_ref: String,
        size_bytes: u64,
        summary_blocks: Vec<Block>,
    },
    ToolPruned {
        v: u32,
        call_id: CallId,
        from_seq: u64,
        original_size: u64,
        kept_chars: u64,
    },
    QuestionAsked {
        v: u32,
        turn_id: TurnId,
        call_id: Option<CallId>,
        question_id: QuestionId,
        blocks: Vec<Block>,
        channel: String,
    },
    ApprovalDecision {
        v: u32,
        call_id: CallId,
        decision: String,
        mode: String,
        policy_ref: Option<String>,
        reason: Option<String>,
    },
    Redaction {
        v: u32,
        target_seq: u64,
        reason: String,
    },
    DelegationStart {
        v: u32,
        delegation_id: DelegationId,
        mode: String,
        goal_excerpt: String,
        budget: serde_json::Value,
    },
    DelegationProgress {
        v: u32,
        delegation_id: DelegationId,
        note: String,
        fraction: Option<f32>,
    },
    DelegationQuestion {
        v: u32,
        delegation_id: DelegationId,
        question_id: QuestionId,
        question: String,
    },
    DelegationAnswer {
        v: u32,
        delegation_id: DelegationId,
        question_id: QuestionId,
    },
    DelegationEnd {
        v: u32,
        delegation_id: DelegationId,
        outcome: String,
        error_class: Option<String>,
        artifact_ids: Vec<String>,
        summary: String,
    },
    InboxEnqueued {
        v: u32,
        lane: String,
        class: InboxClass,
        source: InboxSource,
        ref_seq: Option<u64>,
    },
    InboxClaimed {
        v: u32,
        entry_seq: u64,
        turn_id: TurnId,
    },
    InboxCancelled {
        v: u32,
        entry_seq: u64,
        reason: InboxCancelReason,
    },
    /// Unknown kind or newer `v` — keep bytes, skip in projection.
    Skipped {
        original_kind: String,
        v: u32,
        raw: Vec<u8>,
    },
}

impl EventPayload {
    #[must_use]
    pub fn version(&self) -> u32 {
        match self {
            Self::SessionStart { v, .. }
            | Self::SessionTitle { v, .. }
            | Self::SessionSummary { v, .. }
            | Self::SessionEnd { v, .. }
            | Self::SessionReopen { v, .. }
            | Self::SessionArchived { v, .. }
            | Self::ForkPoint { v, .. }
            | Self::TurnStart { v, .. }
            | Self::TurnEnd { v, .. }
            | Self::StepStart { v, .. }
            | Self::StepEnd { v, .. }
            | Self::UserMessage { v, .. }
            | Self::AssistantMessage { v, .. }
            | Self::AssistantThinking { v, .. }
            | Self::InnerMessage { v, .. }
            | Self::ContextSystemMessage { v, .. }
            | Self::ContextEpoch { v, .. }
            | Self::CompactionApplied { v, .. }
            | Self::ToolCall { v, .. }
            | Self::ToolResult { v, .. }
            | Self::ToolSpill { v, .. }
            | Self::ToolPruned { v, .. }
            | Self::QuestionAsked { v, .. }
            | Self::ApprovalDecision { v, .. }
            | Self::Redaction { v, .. }
            | Self::DelegationStart { v, .. }
            | Self::DelegationProgress { v, .. }
            | Self::DelegationQuestion { v, .. }
            | Self::DelegationAnswer { v, .. }
            | Self::DelegationEnd { v, .. }
            | Self::InboxEnqueued { v, .. }
            | Self::InboxClaimed { v, .. }
            | Self::InboxCancelled { v, .. }
            | Self::Skipped { v, .. } => *v,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, SessionError> {
        rmp_serde::to_vec_named(self).map_err(SessionError::codec)
    }

    pub fn decode(kind: &EventKind, bytes: &[u8]) -> Result<Self, SessionError> {
        if matches!(kind, EventKind::Unknown(_)) {
            return Ok(Self::Skipped {
                original_kind: kind.as_str().to_owned(),
                v: 0,
                raw: bytes.to_vec(),
            });
        }
        match rmp_serde::from_slice::<Self>(bytes) {
            Ok(payload) if payload.version() > PAYLOAD_VERSION => Ok(Self::Skipped {
                original_kind: kind.as_str().to_owned(),
                v: payload.version(),
                raw: bytes.to_vec(),
            }),
            Ok(payload) => Ok(payload),
            Err(err) => {
                tracing::warn!(kind = %kind, error = %err, "skipping unreadable session payload");
                Ok(Self::Skipped {
                    original_kind: kind.as_str().to_owned(),
                    v: 0,
                    raw: bytes.to_vec(),
                })
            }
        }
    }

    #[must_use]
    pub fn blocks(&self) -> &[Block] {
        match self {
            Self::UserMessage { blocks, .. }
            | Self::AssistantMessage { blocks, .. }
            | Self::AssistantThinking { blocks, .. }
            | Self::InnerMessage { blocks, .. }
            | Self::ContextSystemMessage { blocks, .. }
            | Self::ToolResult { blocks, .. }
            | Self::QuestionAsked { blocks, .. }
            | Self::ToolSpill {
                summary_blocks: blocks,
                ..
            } => blocks.as_slice(),
            _ => &[],
        }
    }

    #[must_use]
    pub fn surface_search_text(&self) -> String {
        match self {
            Self::SessionTitle { title, .. } => title.clone(),
            Self::UserMessage { blocks, .. } | Self::AssistantMessage { blocks, .. } => blocks
                .iter()
                .filter_map(Block::as_text)
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        }
    }

    #[must_use]
    pub fn turn_id(&self) -> Option<TurnId> {
        match *self {
            Self::TurnStart { turn_id, .. }
            | Self::TurnEnd { turn_id, .. }
            | Self::StepStart { turn_id, .. }
            | Self::StepEnd { turn_id, .. }
            | Self::AssistantMessage { turn_id, .. }
            | Self::AssistantThinking { turn_id, .. }
            | Self::ToolCall { turn_id, .. }
            | Self::QuestionAsked { turn_id, .. }
            | Self::InboxClaimed { turn_id, .. } => Some(turn_id),
            Self::UserMessage { turn_id, .. } | Self::InnerMessage { turn_id, .. } => turn_id,
            _ => None,
        }
    }
}

/// Persisted event after seq/ts assignment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoggedEvent {
    pub session_id: SessionId,
    pub seq: u64,
    pub ts: String,
    pub kind: EventKind,
    pub payload: EventPayload,
}

/// Event waiting for the writer to assign `seq` / `ts`.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    pub session_id: SessionId,
    pub kind: EventKind,
    pub payload: EventPayload,
}

impl NewEvent {
    #[must_use]
    pub fn new(session_id: SessionId, kind: EventKind, payload: EventPayload) -> Self {
        Self {
            session_id,
            kind,
            payload,
        }
    }
}

pub fn v1() -> u32 {
    PAYLOAD_VERSION
}
