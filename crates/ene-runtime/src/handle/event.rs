//! Events broadcast from the actor to all consumers, plus actor status and
//! read-only state snapshot types.
//!
//! This module's contents are unaffected by the #271 actor decomposition —
//! moved here verbatim from the former monolithic `handle.rs` as part of the
//! file split. The event bus itself (chat/audio/lifecycle channel split) is
//! a separate issue (#272); [`EneEvent`]'s variant set and the channel it
//! flows through are intentionally left as-is here.

use crate::diagnostics::MemoryQueryHandle;
use crate::types::{RequestId, TurnId};
use chrono::{DateTime, Utc};
use ene_config::EneConfig;
use ene_mind::{CardName, SessionId};
use tokio::sync::broadcast;

/// Events emitted from the actor to all consumers via broadcast channel.
///
/// Consumers (CLI, Bevy systems, logging) receive these through
/// [`crate::EneHandle::subscribe`] which returns an [`EneEventReceiver`].
#[derive(Debug, Clone)]
pub enum EneEvent {
    /// A chunk of generated text from the LLM (markers stripped).
    TextDelta {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// The raw text delta.
        delta: String,
    },
    /// Presentation cues (expression / emote) for the active turn.
    Performance {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// Cue list (usually one expression).
        cues: Vec<ene_mind::PerformanceCue>,
        /// How the cues were chosen.
        source: ene_mind::CueSource,
    },
    /// A tool call has been requested by the LLM.
    ToolCallStart {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// The tool name (e.g. "fs.write").
        name: String,
        /// JSON-encoded arguments.
        arguments: String,
    },
    /// A tool call has completed with its result.
    ToolCallResult {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// The tool name.
        name: String,
        /// The tool's output as a string.
        result: String,
    },
    /// A deferred (background) tool task has reached a terminal state (#196).
    ///
    /// Emitted asynchronously after the originating turn has completed, once
    /// the background-capable tool reports that the task finished, failed, or
    /// was cancelled. Consumers can use `task_id` to correlate this with the
    /// earlier `DeferredAccepted` result returned to the LLM.
    ToolBackgroundCompleted {
        /// The tool name that owns the background task.
        tool_name: String,
        /// The `task_id` returned by the deferred call acceptance.
        task_id: String,
        /// Terminal status of the background task.
        status: ene_plugin_proto::DeferredStatus,
    },
    /// A destructive operation requires user approval before execution.
    PermissionRequired {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// Unique identifier for this permission request.
        request_id: RequestId,
        /// The category of operation (e.g. "write", "delete").
        action: String,
        /// The target resource path.
        target: String,
        /// Human-readable description of what will be done.
        description: String,
    },
    /// An interactive tool needs user input (e.g. a clarifying question).
    UserInputRequired {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// Unique identifier for this input request.
        request_id: RequestId,
        /// The prompt describing the question, options, and free-text allowance.
        prompt: ene_plugin_proto::UserInputPrompt,
    },
    /// Thin signal that rolling context compression completed for this turn.
    ContextCompressed {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// Compression level label (e.g. "scene").
        level: String,
    },
    /// Terminal event for a run: emitted exactly once after `after_turn` completes.
    Terminal {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// Why the run terminated.
        reason: TerminalReason,
    },
    /// The actor's status changed.
    StatusChanged {
        /// New status value.
        status: EneStatus,
    },
    /// A turn has started streaming (after provider open succeeds).
    TurnStarted {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
    },
    /// A chunk of synthesized PCM audio from the TTS pipeline.
    ///
    /// NOTE (#L6): `AudioChunk` carries a `Vec<f32>` PCM payload through the
    /// same 1024-capacity broadcast channel as lightweight chat events. A burst
    /// of audio chunks can therefore crowd out chat events (causing `Lagged`
    /// for slow subscribers) and inflates every subscriber's buffer. This is
    /// acceptable for the current single-consumer playback path, but a
    /// dedicated bounded audio channel should be introduced before adding more
    /// broadcast subscribers or higher sample rates (#272).
    AudioChunk {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// Interleaved mono PCM samples normalized to `[-1.0, 1.0]`.
        pcm: Vec<f32>,
        /// Sample rate in Hz (e.g. 24000).
        sample_rate: u32,
        /// Whether this is the final audio chunk for the turn.
        is_final: bool,
    },
    /// New pending memory candidates are available for review (#174).
    PendingCandidateAvailable {
        /// Number of pending candidates.
        count: usize,
    },
}

/// Reason a single run terminated.
///
/// Used in [`EneEvent::Terminal`]. Exactly one of these is emitted per
/// `EneCommand::Run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalReason {
    /// The LLM stream completed normally (no more tool calls and the
    /// provider finished the response).
    Done,
    /// The run terminated due to an error.
    Failed {
        /// Human-readable error description.
        message: String,
    },
    /// The run was cancelled by the user via `EneCommand::Cancel`.
    Cancelled,
}

/// A snapshot of the current actor state for read-only queries.
#[derive(Clone)]
pub struct EneStateSnapshot {
    /// The loaded character card, if any.
    pub character_card: Option<ene_config::CharacterCardV3>,
    /// Conversation history (`ene_mind::HistoryEntry`).
    pub history: Vec<ene_mind::HistoryEntry>,
    /// A copy of the current configuration.
    pub config: EneConfig,
    /// Current session ID.
    pub session_id: SessionId,
    /// Character card name.
    pub card_name: CardName,
    /// Memory query handle (enabled only if memory is configured).
    /// Prefer [`crate::EneDiagnostics::memory`] for new code.
    pub memory: MemoryQueryHandle,
    /// Current conversation turn count.
    pub current_turn_count: u32,
    /// When the session started (UTC).
    pub session_started_at: DateTime<Utc>,
}

/// Current status of the actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EneStatus {
    /// Not currently processing anything.
    Idle,
    /// An AI stream is running.
    Running,
    /// An error state (non-fatal).
    Error,
}

/// Event receiver handle obtained from [`crate::EneHandle::subscribe`].
///
/// Wraps the broadcast receiver and provides a ergonomic interface for
/// consuming events from the actor. On lag, emits
/// [`crate::diagnostics::DiagnosticEvent::Lagged`] /
/// [`crate::diagnostics::DiagnosticEvent::ResyncNeeded`] so gaps are never
/// silent (#189).
pub struct EneEventReceiver {
    pub(super) inner: broadcast::Receiver<EneEvent>,
    pub(super) diag_tx: broadcast::Sender<crate::diagnostics::DiagnosticEvent>,
}

impl std::fmt::Debug for EneEventReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EneEventReceiver").finish()
    }
}

impl EneEventReceiver {
    fn note_lag(&self, skipped: u64) {
        let _ = self
            .diag_tx
            .send(crate::diagnostics::DiagnosticEvent::Lagged {
                channel: "events".to_string(),
                skipped,
            });
        let _ = self
            .diag_tx
            .send(crate::diagnostics::DiagnosticEvent::ResyncNeeded {
                channel: "events".to_string(),
            });
    }

    /// Non-blocking poll of the event stream.
    pub fn try_recv(&mut self) -> Result<EneEvent, broadcast::error::TryRecvError> {
        match self.inner.try_recv() {
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                self.note_lag(n);
                Err(broadcast::error::TryRecvError::Lagged(n))
            }
            other => other,
        }
    }

    /// Async receive, waiting for the next event.
    pub async fn recv(&mut self) -> Result<EneEvent, broadcast::error::RecvError> {
        match self.inner.recv().await {
            Err(broadcast::error::RecvError::Lagged(n)) => {
                self.note_lag(n);
                Err(broadcast::error::RecvError::Lagged(n))
            }
            other => other,
        }
    }
}
