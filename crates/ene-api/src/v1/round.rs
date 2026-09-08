//! One-to-one text round trip: input candidate, stream, ack (IPC §13.1).
//!
//! Inputs are proposals, never acceptances: the Host issues rounds, keeps
//! streams ordered per stream, and reports presentation back through
//! outcomes. Stale, held, and needs-revalidation are Ok-side domain
//! outcomes, never errors and never retried automatically.

use serde::{Deserialize, Serialize};

use super::refs::RevalidationReasonWire;
use super::refs::{ClientLocalId, CompanionWireRef, RoundWireId, StreamWireId, TextLangWire};

/// Owner text input candidate. Acceptance, round issuance, and attribution
/// checks happen Host-side (IB X-B).
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubmitTextInput {
    /// Opaque Companion reference. Echoed, never interpreted.
    pub companion: CompanionWireRef,
    /// Target round, or [`None`] to request a new round. Old rounds are
    /// never rebound from this field.
    pub round: Option<RoundWireId>,
    /// Client-local correspondence ID for matching acks to sends.
    pub local_id: ClientLocalId,
    /// Message body. Redacted from [`core::fmt::Debug`].
    pub body: TextBodyWire,
}

impl core::fmt::Debug for SubmitTextInput {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SubmitTextInput")
            .field("companion", &self.companion)
            .field("round", &self.round)
            .field("local_id", &self.local_id)
            .field("body", &"[redacted]")
            .finish()
    }
}

/// Message body: bounded text plus language tag. A transient expression:
/// the Host canonicalizes accepted text into History.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextBodyWire {
    /// Body text. Redacted from [`core::fmt::Debug`].
    pub text: String,
    /// Opaque language tag.
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

/// Intake outcome: an Ok-side domain outcome, never an error. An old round
/// maps back to its own round; nothing is rebound onto a new one, and a
/// [`None`] request is never auto-resent to work around a rejection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RoundIntakeOutcomeWire {
    /// Accepted into this Host-issued round.
    AcceptedForRound {
        /// Round the input joined.
        round: RoundWireId,
    },
    /// The premise round is not current.
    StaleRound {
        /// Current round, if one is open.
        current_round: Option<RoundWireId>,
        /// Current generation value the sender should observe next time.
        current_generation: u64,
    },
    /// A presence transition holds intake for now.
    HeldForTransition,
    /// The premise needs refreshing before intake.
    NeedsRevalidation {
        /// Opaque reason, matched against a known set at Host ingress.
        reason: RevalidationReasonWire,
    },
}

/// Response stream opening: stream key, round, and generation together.
/// Opening is neither presentation nor achievement.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextStreamOpen {
    /// Stream this opening starts, Host-issued by default.
    pub stream: StreamWireId,
    /// Round the stream answers.
    pub round: RoundWireId,
    /// Generation the stream belongs to.
    pub generation: u64,
}

/// One stream frame: per-stream order by `seq`, partial text, final flag.
/// A frame without `is_final` never completes anything; gaps are never
/// guessed over.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextStreamFrameWire {
    /// Owning stream.
    pub stream: StreamWireId,
    /// In-stream order.
    pub seq: u64,
    /// Partial text. Redacted from [`core::fmt::Debug`].
    pub delta: String,
    /// Whether this frame closes the stream.
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

/// How a stream ended, kept distinct: completion, interruption,
/// cancellation, and staleness are different facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StreamClose {
    /// Stream completed normally.
    Completed,
    /// Stream interrupted before completion.
    Interrupted,
    /// Stream cancelled.
    Cancelled,
    /// Stream went stale (old stream, never rebound).
    Stale,
}

/// Stream close record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextStreamClose {
    /// Stream that ended.
    pub stream: StreamWireId,
    /// How it ended.
    pub status: StreamClose,
}

/// Presentation confirmation: an observation, not a report of completion.
/// Sending never equals reported.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConfirmPresentationWire {
    /// Round the confirmation covers.
    pub round: RoundWireId,
    /// Stream the confirmation covers, if stream-scoped.
    pub stream: Option<StreamWireId>,
    /// Presentation status.
    pub status: PresentationStatus,
    /// Display reason. Operational metadata only: never a secret or a body
    /// copy, and redacted from [`core::fmt::Debug`] in depth.
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

/// Presentation status vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PresentationStatus {
    /// Presented.
    Presented,
    /// Presentation unknown. Sticky: never upgraded by resend.
    Unknown,
    /// Presentation failed.
    Failed,
}

/// Who produced a history item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HistoryRole {
    /// The Owner.
    Owner,
    /// The Companion.
    Companion,
}

/// One timeline item: Host-filtered display fact for restart restore.
/// Stage 1 gap-fill (no dedicated restore message in the referenced
/// design): filtered facts only, never undelivered reporting.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HistoryItem {
    /// Round this item belongs to.
    pub round: RoundWireId,
    /// Who produced it.
    pub role: HistoryRole,
    /// Item text. Redacted from [`core::fmt::Debug`].
    pub text: String,
    /// Wall-clock rendering (RFC 3339 with offset), display only.
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

/// Timeline restore request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HistoryRequest {
    /// Opaque Companion reference.
    pub companion: CompanionWireRef,
    /// Items at or after this wall-clock rendering, if bounded.
    pub since: Option<String>,
    /// Maximum items to return.
    pub limit: u64,
}

/// Timeline restore answer: filtered display facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryView {
    /// Restored items, oldest first.
    pub items: Vec<HistoryItem>,
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
        assert!(
            rendered.contains("companion-1"),
            "refs stay visible: {rendered}"
        );
        assert!(
            rendered.contains("round-1"),
            "refs stay visible: {rendered}"
        );
        assert!(
            rendered.contains("local-1"),
            "refs stay visible: {rendered}"
        );
        assert!(
            !rendered.contains("hello companion"),
            "body redacted: {rendered}"
        );
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
        assert!(
            !rendered.contains("partial words"),
            "delta redacted: {rendered}"
        );
        assert!(rendered.contains("seq"), "non-body fields stay: {rendered}");
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
        assert!(
            !rendered.contains("private words"),
            "text redacted: {rendered}"
        );
        assert!(
            rendered.contains("round-9"),
            "refs stay visible: {rendered}"
        );
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
        assert!(
            !rendered.contains("downstream said why"),
            "detail redacted: {rendered}"
        );
    }
}
