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

pub mod config;
pub mod conversation_manager;
pub mod error;
pub mod session;
pub mod special_token;
pub mod utils;

pub use conversation_manager::{
    PendingSplitTask, SessionBoundary, SplitReason, SplitResult, SplitTaskInput, check_boundary,
    execute_split, generate_session_id, poll_split_result, spawn_split_task,
};
pub use error::SessionError;
pub use session::ConversationSession;
pub use special_token::{extract_emotion_from_token, split_text_and_special_tokens};
pub use utils::truncate;
pub use ene_config::{
    CharacterAsset, CharacterCardData, CharacterCardV3, ExpressionDefinition, ResolvedExpression,
    expand_cbs_macros, resolve_expressions,
};
pub use config::SessionConfig;
