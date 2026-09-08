//! Text rounds, streams, presentation confirmations, and history views.
//!
//! Intake outcomes that sound negative are still `Ok`-side domain outcomes,
//! never errors and never implicit retries:
//! [`crate::round::RoundIntakeOutcome::StaleRound`]
//! tells the Client which round is current,
//! [`crate::round::RoundIntakeOutcome::HeldForTransition`]
//! tells it to wait out a presence move, and
//! [`crate::round::RoundIntakeOutcome::NeedsRevalidation`] tells it what to
//! fix. The Client
//! decides what to do next; the transport retries nothing on its own.
//!
//! [`crate::round::HistoryView`] carries Host-filtered display facts only: the
//! Host selects
//! which items the Client may see, and the Client presents them without
//! treating them as durable or complete.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One text input submitted by the Client.
///
/// All references are opaque Host-minted strings: the Client echoes them and
/// never parses, synthesizes, or stores them as keys. `round` is [`None`] for
/// a new-round request; the Host decides the round and reports it back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitTextInput {
    /// Opaque companion reference to address; echo-only.
    pub companion: String,
    /// Opaque round reference to append to, or [`None`] to request a new round.
    pub round: Option<String>,
    /// Client-local identifier binding this submission to its outcome.
    pub local_id: String,
    /// Submitted text body.
    pub text: String,
    /// Language tag for the submitted text, as a plain string.
    pub lang: String,
}

/// `Ok`-side domain outcome of round intake.
///
/// None of these variants is an error and none authorizes a retry by itself:
/// stale, held, and needs-revalidation are answers the Client acts on, not
/// failures the transport replays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundIntakeOutcome {
    /// Taken in for `round`; the Client may stream against it.
    AcceptedForRound {
        /// Opaque round reference the input joined; echo-only.
        round: String,
    },
    /// The addressed round is no longer current.
    StaleRound {
        /// Opaque current-round reference, if there is one; echo-only.
        current_round: Option<String>,
        /// Current presence generation the Client should observe next.
        current_generation: u64,
    },
    /// Intake is paused for an ongoing presence transition; the Client waits,
    /// it does not resend.
    HeldForTransition,
    /// The input needs Client-side correction before the Host will take it.
    NeedsRevalidation {
        /// Human-readable explanation for display only.
        reason: String,
    },
}

/// Opening of a Host-to-Client text stream for one round.
///
/// `stream` is minted by the opener for this stream alone and is never
/// continued across reconnects: a new connection opens new streams.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextStreamOpen {
    /// Stream identifier minted for this stream alone.
    pub stream: Uuid,
    /// Opaque round reference the stream belongs to; echo-only.
    pub round: String,
    /// Presence generation the stream was opened under.
    pub generation: u64,
}

/// One frame of streamed text.
///
/// Frames carry order (`seq`) and content (`delta`) only; completion is
/// reported with [`TextStreamClose`], never inferred from a missing frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextStreamFrame {
    /// Stream this frame belongs to.
    pub stream: Uuid,
    /// Frame order within the stream, starting where the opener started.
    pub seq: u64,
    /// Text added by this frame.
    pub delta: String,
    /// Whether the sender will send no further frames on this stream.
    pub is_final: bool,
}

/// How a text stream ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamClose {
    /// Stream ran to its final frame.
    Completed,
    /// Stream stopped early by interruption.
    Interrupted,
    /// Stream stopped early by cancellation.
    Cancelled,
    /// Stream belongs to a round that is no longer current.
    Stale,
}

/// Closing notice for a text stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextStreamClose {
    /// Stream being closed.
    pub stream: Uuid,
    /// How the stream ended.
    pub status: StreamClose,
}

/// Whether presented output reached the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresentationStatus {
    /// Shown to the user.
    Presented,
    /// Delivery outcome is not known.
    Unknown,
    /// Showing failed; `detail` on [`ConfirmPresentation`] may say more.
    Failed,
}

/// Client confirmation of what was presented for a round.
///
/// A confirmation is a fact report, not a settlement: it never completes
/// Host-side work by itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmPresentation {
    /// Opaque round reference confirmed; echo-only.
    pub round: String,
    /// Stream confirmed, if the confirmation is stream-scoped.
    pub stream: Option<Uuid>,
    /// Whether the output reached the user.
    pub status: PresentationStatus,
    /// Extra display text about the outcome, if any.
    pub detail: Option<String>,
}

/// Who produced one history item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryRole {
    /// The Owner side.
    Owner,
    /// The companion side.
    Companion,
}

/// One Host-filtered display fact in a [`HistoryView`].
///
/// Items are display facts selected by the Host, not a durable or complete
/// record; the Client presents them and keeps no canonical copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryItem {
    /// Opaque round reference this item belongs to; echo-only.
    pub round: String,
    /// Who produced the text.
    pub role: HistoryRole,
    /// Display text of the item.
    pub text: String,
    /// Wall-clock time with creation offset, as an `RFC3339+offset` string.
    pub at: String,
}

/// Request for a filtered history slice.
///
/// `since` bounds the slice as an `RFC3339+offset` timestamp string; the Host
/// applies it as a filter and decides what the Client may see.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRequest {
    /// Opaque companion reference whose history is asked for; echo-only.
    pub companion: String,
    /// Lower time bound as an `RFC3339+offset` string, if any.
    pub since: Option<String>,
    /// Maximum number of items to return.
    pub limit: u64,
}

/// Host-filtered history slice answering a [`HistoryRequest`].
///
/// Display facts only: the Host selects the items, and the Client presents
/// them without treating them as durable or complete.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryView {
    /// Items the Host chose to show, in Host-chosen order.
    pub items: Vec<HistoryItem>,
}
