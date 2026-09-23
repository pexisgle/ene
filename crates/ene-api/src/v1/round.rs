use serde::{Deserialize, Serialize};

use super::refs::RevalidationReasonWire;
use super::refs::{ClientLocalId, CompanionWireRef, RoundWireId, StreamWireId, TextLangWire};

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubmitTextInput {
    pub companion: CompanionWireRef,
    pub round: Option<RoundWireId>,
    #[serde(default)]
    pub fresh: bool,
    pub local_id: ClientLocalId,
    pub body: TextBodyWire,
}

impl core::fmt::Debug for SubmitTextInput {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SubmitTextInput")
            .field("companion", &self.companion)
            .field("round", &self.round)
            .field("fresh", &self.fresh)
            .field("local_id", &self.local_id)
            .field("body", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextBodyWire {
    pub text: String,
    pub lang: TextLangWire,
}

impl core::fmt::Debug for TextBodyWire {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TextBodyWire")
            .field("text", &"[redacted]")
            .field("lang", &self.lang)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RoundIntakeOutcomeWire {
    AcceptedForRound {
        round: RoundWireId,
    },
    StaleRound {
        current_round: Option<RoundWireId>,
        current_generation: u64,
    },
    HeldForTransition,
    NeedsRevalidation {
        reason: RevalidationReasonWire,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextStreamOpen {
    pub stream: StreamWireId,
    pub round: RoundWireId,
    pub generation: u64,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextStreamFrameWire {
    pub stream: StreamWireId,
    pub seq: u64,
    pub delta: String,
    pub is_final: bool,
}

impl core::fmt::Debug for TextStreamFrameWire {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TextStreamFrameWire")
            .field("stream", &self.stream)
            .field("seq", &self.seq)
            .field("delta", &"[redacted]")
            .field("is_final", &self.is_final)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StreamClose {
    Completed,
    Interrupted,
    Cancelled,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextStreamClose {
    pub stream: StreamWireId,
    pub status: StreamClose,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConfirmPresentationWire {
    pub round: RoundWireId,
    pub stream: Option<StreamWireId>,
    pub status: PresentationStatus,
    pub detail: Option<String>,
}

impl core::fmt::Debug for ConfirmPresentationWire {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ConfirmPresentationWire")
            .field("round", &self.round)
            .field("stream", &self.stream)
            .field("status", &self.status)
            .field("detail", &self.detail.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PresentationStatus {
    Presented,
    Unknown,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HistoryRole {
    Owner,
    Companion,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HistoryItem {
    pub round: RoundWireId,
    pub role: HistoryRole,
    pub text: String,
    pub at: String,
}

impl core::fmt::Debug for HistoryItem {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("HistoryItem")
            .field("round", &self.round)
            .field("role", &self.role)
            .field("text", &"[redacted]")
            .field("at", &self.at)
            .finish()
    }
}

pub const HISTORY_LIMIT_MAX: u64 = 200;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HistoryRequest {
    pub companion: CompanionWireRef,
    pub since: Option<String>,
    pub limit: u64,
    pub round: Option<RoundWireId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryResponse {
    Items(Vec<HistoryItem>),
    InvalidRequest,
    Unavailable,
    StaleCompanion,
}

#[cfg(test)]
mod tests {
    use super::super::refs::{ClientLocalId, CompanionWireRef, RoundWireId, TextLangWire};
    use super::{ConfirmPresentationWire, HistoryItem, HistoryRole, PresentationStatus};
    use super::{SubmitTextInput, TextBodyWire, TextStreamFrameWire};
    use uuid::Uuid;

    fn input() -> SubmitTextInput {
        SubmitTextInput {
            companion: CompanionWireRef(String::from("companion-1")),
            round: Some(RoundWireId(String::from("round-1"))),
            fresh: false,
            local_id: ClientLocalId(String::from("local-1")),
            body: TextBodyWire {
                text: String::from("hello companion"),
                lang: TextLangWire(String::from("en")),
            },
        }
    }

    #[test]
    fn input_debug_keeps_refs_and_redacts_body() {
        let rendered = format!("{:?}", input());
        assert!(rendered.contains("companion-1"));
        assert!(rendered.contains("round-1"));
        assert!(rendered.contains("local-1"));
        assert!(!rendered.contains("hello companion"));
    }

    #[test]
    fn frame_debug_redacts_delta() {
        let frame = TextStreamFrameWire {
            stream: super::super::refs::StreamWireId(Uuid::new_v4()),
            seq: 3,
            delta: String::from("partial words"),
            is_final: false,
        };
        let rendered = format!("{frame:?}");
        assert!(!rendered.contains("partial words"));
        assert!(rendered.contains("seq"));
    }

    #[test]
    fn history_item_debug_redacts_text() {
        let item = HistoryItem {
            round: RoundWireId(String::from("round-9")),
            role: HistoryRole::Owner,
            text: String::from("private words"),
            at: String::from("2026-09-08T12:00:00+09:00"),
        };
        let rendered = format!("{item:?}");
        assert!(!rendered.contains("private words"));
        assert!(rendered.contains("round-9"));
    }

    #[test]
    fn presentation_detail_debug_redacts_value() {
        let confirm = ConfirmPresentationWire {
            round: RoundWireId(String::from("round-2")),
            stream: None,
            status: PresentationStatus::Failed,
            detail: Some(String::from("downstream said why")),
        };
        let rendered = format!("{confirm:?}");
        assert!(!rendered.contains("downstream said why"));
    }
}
