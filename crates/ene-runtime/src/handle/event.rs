//! Events broadcast from the actor to all consumers, plus actor status and
//! read-only state snapshot types.
//!
//! ## Three-channel event bus
//!
//! Chat events, heavyweight `AudioChunk` PCM payloads, and turn-independent
//! lifecycle notifications ride separate channels because
//! `tokio::sync::broadcast` retains a per-subscriber buffer: mixing a burst
//! of audio chunks into the chat channel would inflate every subscriber's
//! buffer and `Lagged` a slow chat subscriber for reasons entirely
//! unrelated to chat volume. The bus is split by traffic class:
//!
//! - **Chat bus** ([`EneEvent`] / [`EneEventReceiver`]) — `broadcast`,
//!   capacity 1024. Lightweight, ordered, turn-scoped events. Multiple
//!   subscribers via [`crate::EneHandle::subscribe`].
//! - **Audio channel** ([`AudioChunk`] / [`AudioStreamReceiver`]) — bounded
//!   `mpsc`, single-consumer by construction:
//!   [`crate::EneHandle::take_audio_stream`] transfers ownership of the
//!   receiver and returns `None` on every call after the first.
//! - **Lifecycle bus** ([`LifecycleEvent`] / [`LifecycleReceiver`]) —
//!   `broadcast`, small capacity. Turn-independent notifications. Multiple
//!   subscribers via [`crate::EneHandle::subscribe_lifecycle`].

use crate::types::{RequestId, TurnId};
use chrono::{DateTime, Utc};
use ene_config::EneConfig;
use ene_mind::{CardName, SessionId};
use tokio::sync::{broadcast, mpsc};

/// Lightweight, ordered, turn-scoped chat events emitted from the actor via
/// the chat broadcast channel.
///
/// Consumers (CLI, Bevy systems, logging) receive these through
/// [`crate::EneHandle::subscribe`] which returns an [`EneEventReceiver`].
/// `AudioChunk` (see [`crate::handle::AudioChunk`]) and the lifecycle
/// variants (see [`LifecycleEvent`]) are intentionally not part of this
/// enum — they ride their own dedicated channels.
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
    /// A turn has started streaming (after provider open succeeds).
    TurnStarted {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
    },
}

/// A chunk of synthesized PCM audio from the TTS pipeline.
///
/// Delivered over a dedicated bounded `mpsc` channel — not the chat
/// broadcast bus — because the PCM payload is heavyweight relative to chat
/// events and the playback path is single-consumer by nature. Obtain the
/// receiving end via [`crate::EneHandle::take_audio_stream`], which hands
/// over ownership and returns `None` on every call after the first.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// Active turn.
    pub turn: TurnId,
    /// Who initiated this turn.
    pub origin: crate::types::TurnOrigin,
    /// Interleaved mono PCM samples normalized to `[-1.0, 1.0]`.
    pub pcm: Vec<f32>,
    /// Sample rate in Hz (e.g. 24000).
    pub sample_rate: u32,
    /// Whether this is the final audio chunk for the turn.
    pub is_final: bool,
    /// Expression cues attributed to the TTS sentence this chunk belongs to.
    ///
    /// Non-empty only on the first PCM chunk of a sentence, so the playback
    /// consumer can switch the expression when that sentence's audio starts
    /// playing. Each cue carries its [`ene_mind::PerformanceCue::text_offset`]
    /// in the spoken text.
    pub cues: Vec<ene_mind::PerformanceCue>,
}

/// Turn-independent lifecycle notifications emitted from the actor via the
/// lifecycle broadcast channel.
///
/// Consumers receive these through [`crate::EneHandle::subscribe_lifecycle`]
/// which returns a [`LifecycleReceiver`]. Separated from [`EneEvent`]
/// because these are not turn-scoped chat traffic: `StatusChanged` and
/// `PendingCandidateAvailable` can fire between turns, and
/// `ToolBackgroundCompleted` fires asynchronously after the originating
/// turn has already completed.
#[derive(Debug, Clone)]
pub enum LifecycleEvent {
    /// The actor's status changed.
    StatusChanged {
        /// New status value.
        status: EneStatus,
    },
    /// New pending memory candidates are available for review.
    PendingCandidateAvailable {
        /// Number of pending candidates.
        count: usize,
    },
    /// A pending memory candidate was approved, rejected, or edited.
    ///
    /// Audit event emitted from the actor after the mutation committed;
    /// consumers refetch the queue via
    /// [`crate::EneHandle::candidates`] rather than trusting this snapshot.
    CandidateChanged {
        /// Candidate row id.
        id: i64,
        /// Status after the mutation (`pending` for edits, `approved` /
        /// `rejected` for resolutions).
        status: ene_store::PendingCandidateStatus,
        /// Active turn context at mutation time, when any.
        turn: Option<TurnId>,
    },
    /// A deferred (background) tool task has reached a terminal state.
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
    /// A proactive run ended before any visible text because the main model
    /// declined with the `<|silent|>` token (integrated confirmation).
    Declined,
}

/// A snapshot of the current actor state for read-only queries.
///
/// Fetched via [`crate::EneDiagnostics::get_snapshot`] (mailbox-based). For
/// small per-frame state prefer the mailbox-free accessors on
/// [`crate::EneHandle`] — [`crate::EneHandle::card_name`],
/// [`crate::EneHandle::session_id`], [`crate::EneHandle::session_started_at`],
/// [`crate::EneHandle::turn_count`], [`crate::EneHandle::config`], and
/// [`crate::EneHandle::character_card`] — which never queue behind an
/// in-flight `Run` turn. History is the one deliberately mailbox-based
/// read: [`crate::EneHandle::history`] ships the large payload over the
/// command mailbox, so unlike the accessors above it *does* queue behind an
/// in-flight `Run` turn. Memory access lives on [`crate::EneDiagnostics::memory`]
/// (a [`crate::diagnostics::MemoryHandle`]), not on this snapshot.
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
    /// Current conversation turn count.
    pub current_turn_count: u32,
    /// When the session started (UTC).
    pub session_started_at: DateTime<Utc>,
}

/// Current status of the actor.
///
/// Status only answers "is a turn running?" — it is deliberately *not* an
/// error channel. Failures are reported through the turn's
/// [`EneEvent::Terminal`] with [`TerminalReason::Failed`], so there is no
/// `Error` status variant: nothing emits it, and its presence would invite
/// consumers (e.g. the `minimal_chat` example) to wait on a condition that
/// can never fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EneStatus {
    /// Not currently processing anything.
    Idle,
    /// An AI stream is running.
    Running,
}

/// Event receiver handle obtained from [`crate::EneHandle::subscribe`].
///
/// Wraps the broadcast receiver and provides a ergonomic interface for
/// consuming events from the actor. On lag, [`Self::recv`] / [`Self::try_recv`]
/// return [`tokio::sync::broadcast::error::RecvError::Lagged`] and emit
/// [`crate::diagnostics::DiagnosticEvent::Lagged`] so gaps are never silent.
///
/// ## Recovering from a lag
///
/// `Lagged` means one or more chat events — possibly the active turn's
/// [`EneEvent::Terminal`] — were dropped, so the streamed view of the
/// in-flight turn is no longer trustworthy. The uniform recovery is to call
/// [`crate::EneHandle::active_turn`] (a cheap, mailbox-free query) and, when
/// it returns `Some(turn)`, [`crate::EneHandle::cancel`] that turn so the
/// single-flight gate is released; see [`crate::EneHandle::active_turn`] for
/// the full procedure. Both the CLI and desktop follow it.
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
        drop(
            self.diag_tx
                .send(crate::diagnostics::DiagnosticEvent::Lagged {
                    channel: "events".to_string(),
                    skipped,
                }),
        );
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

/// Single-consumer receiver for the audio channel, obtained from
/// [`crate::EneHandle::take_audio_stream`].
///
/// Wraps a bounded `mpsc::Receiver<AudioChunk>`. Unlike [`EneEventReceiver`]
/// / [`LifecycleReceiver`] there is no lag/lossiness to report here: the
/// bounded channel applies back-pressure to the TTS pipeline instead
/// (dropping only non-final chunks under sustained back-pressure — see
/// `send_audio_chunk` in `streaming_cognitive.rs`), so a consumer that keeps
/// draining never silently loses the terminal `is_final` marker.
pub struct AudioStreamReceiver {
    pub(super) inner: mpsc::Receiver<AudioChunk>,
}

impl std::fmt::Debug for AudioStreamReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioStreamReceiver").finish()
    }
}

impl AudioStreamReceiver {
    /// Non-blocking poll of the audio stream.
    pub fn try_recv(&mut self) -> Result<AudioChunk, mpsc::error::TryRecvError> {
        self.inner.try_recv()
    }

    /// Async receive, waiting for the next audio chunk. Returns `None` once
    /// every sender has been dropped (actor shutdown).
    pub async fn recv(&mut self) -> Option<AudioChunk> {
        self.inner.recv().await
    }
}

/// Lifecycle event receiver handle obtained from
/// [`crate::EneHandle::subscribe_lifecycle`].
///
/// Mirrors [`EneEventReceiver`]'s lag-reporting behavior on its own
/// `"lifecycle"` diagnostics channel tag so gaps here are never silent
/// either, even though lifecycle traffic is low-frequency and unlikely to
/// ever overflow its small buffer.
///
/// ## Recovering from a lag
///
/// Unlike a chat-bus lag, a lifecycle lag never strands an in-flight turn:
/// lifecycle notifications are turn-independent, so there is no
/// [`crate::EneHandle::cancel`] step. The consumer simply re-derives the
/// state the missed notification would have carried — e.g. re-query
/// [`crate::EneHandle::candidates`] for the pending-candidate count after
/// missing a `PendingCandidateAvailable`.
pub struct LifecycleReceiver {
    pub(super) inner: broadcast::Receiver<LifecycleEvent>,
    pub(super) diag_tx: broadcast::Sender<crate::diagnostics::DiagnosticEvent>,
}

impl std::fmt::Debug for LifecycleReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LifecycleReceiver").finish()
    }
}

impl LifecycleReceiver {
    fn note_lag(&self, skipped: u64) {
        drop(
            self.diag_tx
                .send(crate::diagnostics::DiagnosticEvent::Lagged {
                    channel: "lifecycle".to_string(),
                    skipped,
                }),
        );
    }

    /// Non-blocking poll of the lifecycle stream.
    pub fn try_recv(&mut self) -> Result<LifecycleEvent, broadcast::error::TryRecvError> {
        match self.inner.try_recv() {
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                self.note_lag(n);
                Err(broadcast::error::TryRecvError::Lagged(n))
            }
            other => other,
        }
    }

    /// Async receive, waiting for the next lifecycle event.
    pub async fn recv(&mut self) -> Result<LifecycleEvent, broadcast::error::RecvError> {
        match self.inner.recv().await {
            Err(broadcast::error::RecvError::Lagged(n)) => {
                self.note_lag(n);
                Err(broadcast::error::RecvError::Lagged(n))
            }
            other => other,
        }
    }
}
