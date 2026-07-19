//! # ene-mind
//!
//! Conversation history management, character card loading, and performance-marker parsing.
//!
//! ## Key Types
//!
//! - [`ConversationSession`] — Central session holder: history, display buffer, memory context, character card
//! - [`SplitReason`] / [`SplitResult`] — Outcome types for manual compression and diagnostics
//!
//! ## Performance Markers
//!
//! The session layer parses `<|perf:…|>` tokens from LLM output streams:
//! - [`split_text_and_special_tokens`] — Splits streaming text into content and special tokens
//! - [`parse_performance_marker`] — Parses a token into a [`crate::output::PerformanceCue`]
//!
//! Also re-exports character card types ([`CharacterCardV3`], [`expand_cbs_macros`], etc.) from `ene_config`.
#![warn(missing_docs)]

/// Session error types.
pub mod error;
/// Core session holder.
#[expect(
    clippy::module_inception,
    reason = "session module re-exports session types by design"
)]
pub mod session;
/// Session ID generation and split/compression result types.
pub mod session_split;
/// Performance-marker (`<|perf:…|>`) parsing.
pub mod special_token;
/// Type-safe identifiers for sessions and cards.
pub mod types;

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
/// Session ID and split/compression result types.
pub use session_split::{SplitReason, SplitResult, generate_session_id};
/// Performance-marker parsing utilities.
pub use special_token::{
    parse_performance_marker, split_text_and_special_tokens, strip_markers,
};
/// Type-safe identifiers.
pub use types::{CardName, SessionId};
