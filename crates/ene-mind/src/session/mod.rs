//! # ene-mind
//!
//! Conversation history management, character card loading, emotion token parsing, and automatic session splitting.
//!
//! ## Key Types
//!
//! - [`ConversationSession`] — Central session holder: history, display buffer, memory context, character card
//! - [`SessionConfig`] — Configuration for auto-split thresholds and timing
//! - [`SplitReason`] — Why a session split was triggered (Timeout, `TopicChange`, Manual)
//!
//! ## Session Splitting
//!
//! Sessions are automatically split using a **composite score function** that
//! aggregates four normalised signals:
//!
//! | Factor | 0 = no pressure | 1 = max pressure |
//! |--------|-----------------|-------------------|
//! | Time | No time elapsed | `timeout_minutes` elapsed |
//! | Topic  | Same topic | Completely different topic |
//! | Context | Empty history | History full |
//! | Turns | 0 turns | 100+ turns |
//!
//! A split is triggered when the weighted score ≥ `split_weights.threshold` (default 0.65).
//! See [`compute_split_score`] and [`spawn_split_task`] for details.
//!
//! **Compression preferred:** this hard-split machinery ([`execute_split`], [`check_boundary`],
//! [`spawn_split_task`]) is the *hard-split* context-management strategy — it discards trimmed
//! history behind a single rolling summary and mints a brand-new session ID. When
//! `ene-mind`'s `MindConfig::context.compression_enabled` is set, `ene-runtime`'s actor
//! (`EneHandle::manual_split()` and the auto-split check) routes to `crate::context::execute_compression`
//! instead, which keeps the session ID stable and trims history more gradually. See
//! [Streaming Events: the `SessionSplit` / compression gap](../../docs/core/streaming-events.md#the-sessionsplit--compression-gap)
//! for the event-visibility difference between the two strategies. This module is not being
//! removed — it remains the active path whenever compression is disabled — but new integrations
//! should prefer enabling compression rather than relying on hard splits.
//!
//! ## Emotion Tokens
//!
//! The session layer parses `<|emo:name|>` tokens from LLM output streams:
//! - [`split_text_and_special_tokens`] — Splits streaming text into content and special tokens
//! - [`extract_emotion_from_token`] — Extracts the emotion name from a token
//!
//! Also re-exports character card types ([`CharacterCardV3`], [`expand_cbs_macros`], etc.) from `ene_config`.
#![warn(missing_docs)]

/// Session configuration types.
pub mod config;
/// Session error types.
pub mod error;
/// Core session holder.
#[expect(clippy::module_inception)]
pub mod session;
/// Session split lifecycle (boundary check, async split, polling).
pub mod session_split;
/// Emotion-token (`<|emo:name|>`) parsing.
pub mod special_token;
/// Type-safe identifiers for sessions and cards.
pub mod types;

/// Session auto-split configuration.
pub use config::{SessionConfig, SplitScoreWeights, SummarizationConfig};
/// Role enum for conversation history.
#[doc(no_inline)]
pub use ene_ai::Role;
/// Truncation utility.
pub use ene_config::truncate::Truncate;
/// Character-card types re-exported from `ene-config`.
#[doc(no_inline)]
pub use ene_config::{
    CharacterAsset, CharacterCardData, CharacterCardV3, ExpressionDefinition, ResolvedExpression,
    expand_cbs_macros, resolve_expressions,
};
/// Session error type.
pub use error::EneSessionError;
/// Central session holder.
pub use session::ConversationSession;
/// Split lifecycle types and entry-points.
pub use session_split::{
    PendingSplitTask, SessionBoundary, SplitReason, SplitResult, SplitScore, SplitTaskInput,
    check_boundary, compute_split_score, execute_split, generate_session_id, poll_split_result,
    spawn_split_task,
};
/// Emotion-token parsing utilities.
pub use special_token::{
    extract_emotion_from_token, parse_performance_marker, split_text_and_special_tokens,
    strip_markers,
};
/// Type-safe identifiers.
pub use types::{CardName, SessionId};
