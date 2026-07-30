//! Hybrid memory search scoring — re-exported from `ene-rag` (#302).
//!
//! The scoring policy (weighted hybrid scoring, recency decay, lexical overlap)
//! moved to `ene-rag` as part of the RAG policy-layer separation. This module
//! stays so existing `ene_store::search::*` import paths keep working.

pub use ene_rag::{
    access_boost_score, contradiction_penalty, document_lexical_similarity, emotional_match_score,
    is_recallable_status, lexical_overlap_score, relationship_score, score_and_rank,
    score_candidate, stale_penalty, tokenize, within_time_range,
};
