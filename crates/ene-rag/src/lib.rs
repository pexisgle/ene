//! # ene-rag
//!
//! RAG (Retrieval-Augmented Generation) **policy** layer for the ene AI
//! companion.
//!
//! ## Why this crate exists
//!
//! RAG policy — pure scoring, decay, and tool selection — is a distinct
//! layer from persistence, and keeping it here is what makes "the store is
//! only about persistence" hold: the scoring functions (`score_candidate`,
//! `recency_score`, ...) and decay policy (`decay_score`,
//! `FADE_THRESHOLD`, ...) touch no database, and a tool-selection pipeline
//! that reached for the concrete `ene_store::MemoryStore` directly would
//! make a future dependency cycle possible.
//!
//! `ene-rag` depends on `ene-core` **only** (plus generic deps) for its
//! scoring/decay core. Because the `ene-rag → ene-store` edge does not
//! exist, a cycle is compile-time impossible.
//!
//! The tool-selection pipeline (the [`tool`] module) is behind the `tool`
//! Cargo feature because it needs embedding/LLM machinery (`ene-ai`).
//! Persistence (`ene-store`) and cognitive (`ene-mind`) callers use the default
//! feature set — pure scoring/decay only — so the embedding stack never leaks
//! into the persistence layer (AGENTS.md: `ene-store` ↛ `ene-ai`). Only
//! `ene-runtime` enables `tool` to build the selection pipeline.
//!
//! ```text
//! ene-core  ←  ene-rag            (scoring, decay)
//! ene-core  ←  ene-rag[tool]      (+ tool selection; needs ene-ai)
//! ene-core  ←  ene-store          (persistence; uses ene-rag scoring core)
//! ```
//!
//! ## What lives here
//!
//! - [`decay`] — half-life exponential decay, unified into a single
//!   [`half_life_decay`](decay::half_life_decay) primitive behind both recall
//!   recency and lifecycle decay; lifecycle thresholds (`FADE_THRESHOLD`,
//!   `ARCHIVE_THRESHOLD`).
//! - [`scoring`] — hybrid memory scoring (`score_candidate` and its component
//!   functions) plus [`score_and_rank`](scoring::score_and_rank), the
//!   gather→score composition entry point.
//! - [`tool`] *(feature = `tool`)* — the tool-selection RAG pipeline:
//!   multi-vector embedding, weighted field similarity,
//!   rerank, per-category limits. Persistence goes through
//!   [`ene_core::EmbeddingStorePort`].
//!
//! ## What does NOT live here
//!
//! - DB I/O, SQL, entities, migrations — those stay in `ene-store`.
//! - Pure state-machine validators (`validate_transition`,
//!   `validate_user_restore`) — those stay in `ene-store` as they are lifecycle
//!   policy, not scoring policy.
//!
//! ## Scope note
//!
//! This crate is a **structural** separation. The hybrid-score combination is
//! a relevance-driven multiplicative form, and the tool-selection score is a
//! normalized, field-count-independent weighted average with a negative-example
//! gate. Both policies live here so the memory and tool sides cannot diverge.
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(clippy::unwrap_used, reason = "unit tests use unwrap for assertions")
)]

/// Decay scoring and lifecycle thresholds.
pub mod decay;
/// Hybrid memory search scoring.
pub mod scoring;
/// Tool-selection RAG pipeline.
#[cfg(feature = "tool")]
pub mod tool;
/// Document/workspace indexing policy — chunking, ignore rules, scoring.
pub mod workspace;

pub use decay::{
    ARCHIVE_THRESHOLD, DECAY_CONFIDENCE_BIAS, DECAY_CONFIDENCE_WEIGHT, DECAY_EMOTIONAL_BIAS,
    DECAY_EMOTIONAL_WEIGHT, DECAY_LN2, DECAY_SALIENCE_BIAS, DECAY_SALIENCE_WEIGHT,
    EMOTIONAL_MAGNITUDE_SCALE, FADE_THRESHOLD, active_decay_anchor, decay_score, emotional_impact,
    faded_decay_anchor, half_life_decay, recency_score, target_status_after_decay,
};
pub use scoring::{
    ACCESS_BOOST_HALF_LIFE_DAYS, access_boost_score, contradiction_penalty,
    document_lexical_similarity, emotional_match_score, is_recallable_status,
    lexical_overlap_score, penalty_multiplier, quality_factor, relationship_score, relevance_score,
    score_and_rank, score_candidate, stale_penalty, tokenize, within_time_range,
};
#[cfg(feature = "tool")]
pub use tool::{
    FieldWeights, HybridRerankProvider, ToolRag, ToolRagConfig, ToolRagError, ToolRagOptions,
    ToolRagStats, hybrid_embed, hyde_document, rerank_tool_specs,
};
pub use workspace::{
    ChunkOptions, ChunkedDocument, DocumentChunk, WorkspaceRagConfig, chunk_document, glob_matches,
    score_chunk,
};
