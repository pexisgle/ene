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
    pub companion: CompanionWireRef,
    /// Target round, or [`None`] to join-or-mint. Old rounds are never
    /// rebound from this field. With [`fresh`](Self::fresh) set this field
    /// must be [`None`]: a force-new request is the design's round-less
    /// new-round request (IPC §13.1), so a round premise here makes the
    /// frame self-contradictory and the Host declines it stale instead of
    /// interpreting either value.
    pub round: Option<RoundWireId>,
    /// Force a fresh round: the Host mints instead of joining any open
    /// round. Defaults to `false` when absent, preserving the join-or-mint
    /// meaning of a bare `round: None`. A force-new request carries no
    /// round premise: [`round`](Self::round) and the envelope `round_view`
    /// must both be absent, and a contradictory frame is declined stale,
    /// never silently reinterpreted.
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

/// Message body: bounded text plus language tag. A transient expression:
/// the Host canonicalizes accepted text into History.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextBodyWire {
    /// Redacted from [`core::fmt::Debug`].
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

/// Intake outcome: an Ok-side domain outcome, never an error. An old round
/// maps back to its own round; nothing is rebound onto a new one, and a
/// [`None`] request is never auto-resent to work around a rejection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RoundIntakeOutcomeWire {
    AcceptedForRound {
        round: RoundWireId,
    },
    /// The premise round is not current.
    StaleRound {
        current_round: Option<RoundWireId>,
        /// Current generation the sender should observe next time.
        current_generation: u64,
    },
    /// A presence transition holds intake for now.
    HeldForTransition,
    NeedsRevalidation {
        /// Opaque reason, matched against a known set at Host ingress.
        reason: RevalidationReasonWire,
    },
}

/// Opening is neither presentation nor achievement.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextStreamOpen {
    pub stream: StreamWireId,
    pub round: RoundWireId,
    pub generation: u64,
}

/// Per-stream order by `seq`. A frame without `is_final` never completes
/// anything; gaps are never guessed over.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextStreamFrameWire {
    pub stream: StreamWireId,
    pub seq: u64,
    /// Redacted from [`core::fmt::Debug`].
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

/// Completion, interruption, cancellation, and staleness are different
/// facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StreamClose {
    Completed,
    Interrupted,
    Cancelled,
    /// Old streams are never rebound.
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextStreamClose {
    pub stream: StreamWireId,
    pub status: StreamClose,
}

/// Presentation confirmation: an observation, not a report of completion.
/// Sending never equals reported.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConfirmPresentationWire {
    pub round: RoundWireId,
    pub stream: Option<StreamWireId>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PresentationStatus {
    Presented,
    /// Sticky: never upgraded by resend.
    Unknown,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HistoryRole {
    Owner,
    Companion,
}

/// One timeline item: Host-filtered display fact for restart restore.
/// Stage 1 gap-fill (no dedicated restore message in the referenced
/// design): filtered facts only, never undelivered reporting.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HistoryItem {
    pub round: RoundWireId,
    pub role: HistoryRole,
    /// Redacted from [`core::fmt::Debug`].
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HistoryRequest {
    pub companion: CompanionWireRef,
    /// Items at or after this wall-clock rendering, if bounded. The Host
    /// parses it as RFC 3339 and compares instants; a different UTC offset is
    /// therefore respected, never compared as plain text.
    pub since: Option<String>,
    /// Maximum number of items, oldest first. Zero requests no items; the
    /// Host applies the bound to the storage query, not after reading.
    pub limit: u64,
    /// Restrict to one Host-issued round projection, or [`None`] for the
    /// whole companion timeline. The projection travels opaquely: the Host
    /// resolves it against stored history, so a round stays addressable
    /// across restarts even though the transient wire map is gone.
    #[serde(default)]
    pub round: Option<RoundWireId>,
}

/// Outcome of an explicit History read (owner-requested, never the optional
/// background retrieval used while assembling a dialogue prompt).
///
/// A successful read may be empty: empty is a fact about the timeline, not a
/// failure. Failure variants are typed and operation-level only and carry no
/// History body, secret, or raw backend error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryResponse {
    /// The read succeeded; `items` is oldest first and may be empty.
    Items(Vec<HistoryItem>),
    /// The request itself is unusable (for example `since` is not an RFC 3339
    /// instant). Retrying the identical bytes fails identically; the Client
    /// corrects the request.
    InvalidRequest,
    /// The Host could not read the requested timeline. The same request may
    /// succeed later; nothing about the stored timeline is implied.
    Unavailable,
    /// The companion projection is unknown or rotated. The Client re-reads
    /// presence/the current projection and retries instead of showing an
    /// empty timeline.
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
