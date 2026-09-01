use crate::types::DelegationMode;
use ene_session::{DelegationId, QuestionId};
use serde::{Deserialize, Serialize};

/// One append-only delegation event row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationEvent {
    pub event_seq: i64,
    pub delegation_id: DelegationId,
    pub created_at: String,
    pub payload: DelegationEventPayload,
}

/// Typed payload for append-only delegation communication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DelegationEventPayload {
    ModeSet {
        mode: DelegationMode,
    },
    DepthSet {
        depth: u32,
    },
    Task {
        goal: String,
    },
    Message {
        body: String,
    },
    Cancel,
    Assumption {
        note: String,
    },
    Answer {
        question_id: QuestionId,
        body: String,
    },
    Question {
        question_id: QuestionId,
        prompt: String,
    },
    Progress {
        note: String,
    },
    Complete {
        summary: String,
    },
    Failed {
        summary: String,
    },
    ToolComplete {
        summary: String,
    },
    ChildReport {
        kind: String,
        body: String,
    },
}
