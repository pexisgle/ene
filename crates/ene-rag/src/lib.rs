//! # ene-rag
//!
//! RAG (Retrieval-Augmented Generation) **policy** layer for the ene AI
//! companion (#302).
//!
//! ## Why this crate exists
//!
//! Before this crate existed, RAG policy was scattered across two crates:
//!
//! - `ene-store` — the persistence crate — hosted pure scoring functions
//!   (`search.rs`: `score_candidate`, `recency_score`, ...) and decay policy
//!   (`forgetting.rs`: `decay_score`, `FADE_THRESHOLD`, ...), even though none
//!   of them touch the database.
//! - `ene-tool-rag` — a standalone crate for tool selection — depended directly
//!   on the concrete `ene_store::MemoryStore` for embedding persistence.
//!
//! Both violated the "store is only about persistence" principle, and the tool
//! RAG's direct store dependency made a future dependency cycle possible.
//!
//! `ene-rag` consolidates the policy layer and depends on `ene-core` **only**
//! (plus generic deps) for its scoring/decay core. Because the
//! `ene-rag → ene-store` edge does not exist, a cycle is compile-time
//! impossible.
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
//! - [`tool`] *(feature = `tool`)* — the tool-selection RAG pipeline (absorbed
//!   from `ene-tool-rag`): multi-vector embedding, weighted field similarity,
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
//! This crate is a **structural** separation (#302). On top of that structure,
//! the hybrid-score combination was redesigned from an additive weighted sum to
//! a relevance-driven multiplicative form (#346), and the tool-selection score
//! from an unnormalized field sum to a normalized, field-count-independent
//! weighted average with a negative-example gate (#436). Both policies live
//! here so the memory and tool sides cannot diverge again.
#![warn(missing_docs)]

/// Decay scoring and lifecycle thresholds.
pub mod decay;
/// Hybrid memory search scoring.
pub mod scoring;
/// Tool-selection RAG pipeline (absorbed from `ene-tool-rag`).
#[cfg(feature = "tool")]
pub mod tool;

pub use decay::{
    ARCHIVE_THRESHOLD, FADE_THRESHOLD, active_decay_anchor, decay_score, emotional_impact,
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
