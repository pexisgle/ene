//! # ene-session
//!
//! Conversation history management, character card loading, emotion token parsing, and automatic session splitting.
//!
//! ## Key Types
//!
//! - [`ConversationSession`] — Central session holder: history, display buffer, memory context, character card
//! - [`SessionConfig`] — Configuration for auto-split thresholds and timing
//! - [`SplitReason`] — Why a session split was triggered (Timeout, TopicChange, Manual)
//!
//! ## Session Splitting
//!
//! Sessions are automatically split based on:
//! 1. **Timeout** — elapsed time exceeds `session_timeout_minutes`
//! 2. **Topic change** — cosine similarity between consecutive user messages drops below `topic_change_threshold`
//!
//! See [`spawn_split_task`] and [`execute_split`] for the async split lifecycle.
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
/// Session split lifecycle (boundary check, async split, polling).
pub mod session_split;
/// Session error types.
pub mod error;
/// Core session holder.
pub mod session;
/// Emotion-token (`<|emo:name|>`) parsing.
pub mod special_token;
/// Internal utilities.
pub mod utils;
/// Role enum for conversation history.
pub mod role;

/// Session auto-split configuration.
pub use config::SessionConfig;
/// Split lifecycle types and entry-points.
pub use session_split::{
    PendingSplitTask, SessionBoundary, SplitReason, SplitResult, SplitTaskInput, check_boundary,
    execute_split, generate_session_id, poll_split_result, spawn_split_task,
};
/// Character-card types re-exported from `ene-config`.
#[doc(no_inline)]
pub use ene_config::{
    CharacterAsset, CharacterCardData, CharacterCardV3, ExpressionDefinition, ResolvedExpression,
    expand_cbs_macros, resolve_expressions,
};
/// Session error type.
pub use error::SessionError;
/// Role enum for conversation history.
pub use role::Role;
/// Central session holder.
pub use session::ConversationSession;
/// Emotion-token parsing utilities.
pub use special_token::{extract_emotion_from_token, split_text_and_special_tokens};
/// Truncation utility re-exported from `ene-tools-common`.
pub use utils::truncate;
