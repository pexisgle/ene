//! # ene-mind
//!
//! Conversation history management, character card loading, and performance-marker parsing.
//!
//! ## Key Types
//!
//! - [`ConversationSession`] — Central session holder: history, display buffer, memory context, character card
//! - [`SplitReason`] / [`SplitResult`] — Outcome types for session splits (which issue a new [`SessionId`]) and diagnostics
//! - [`crate::context::CompressionResult`] — Outcome of a compression-only pass (manual compression)
//!
//! ## Performance Markers
//!
//! The session layer parses `<|perf:…|>` tokens from LLM output streams:
//! - [`split_text_and_special_tokens`] — Splits streaming text into content and special tokens
//! - [`parse_performance_marker`] — Parses a token into a [`crate::output::PerformanceCue`]
//!
//! Also re-exports character card types ([`CharacterCardV3`], [`expand_cbs_macros`], etc.) from `ene_config`.

pub mod error;
#[expect(
    clippy::module_inception,
    reason = "session module re-exports session types by design"
)]
pub mod session;
pub mod session_split;
/// Performance-marker (`<|perf:…|>`) parsing.
pub mod special_token;
/// Topic-boundary detection via topic centroid + composite score.
pub mod topic_boundary;
pub mod types;

#[doc(no_inline)]
pub use ene_ai::Role;
#[doc(no_inline)]
pub use ene_card::{
    CharacterAsset, CharacterCardData, CharacterCardV3, ExpressionDefinition, ResolvedExpression,
    expand_cbs_macros, resolve_expressions,
};
pub use ene_util::truncate::Truncate;
pub use error::EneSessionError;
pub use session::{ConversationSession, InterruptedState};
pub use session_split::{SplitReason, SplitResult, generate_session_id};
pub use special_token::{
    StreamPiece, parse_performance_marker, split_text_and_special_tokens,
    split_text_and_special_tokens_ordered, strip_markers,
};
pub use topic_boundary::{TopicBoundarySignal, TopicBoundaryTracker};
pub use types::{CardName, SessionId};
