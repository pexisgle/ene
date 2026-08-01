//! Diagnostics / CLI / desktop memory journal facade (#123).
//!
//! Scored search always goes through mind policy (`MindMemoryConfig`) and
//! `MemoryStore::search`. Persistence CRUD (list/pin/transition) may call the
//! store directly.

use chrono::Utc;
use ene_ai::EmbeddingProvider;
use ene_core::{MemoryPort, Query, ScoredMemory};

use crate::config::MindMemoryConfig;
use crate::error::CognitionError;
use crate::recall::{RecalledMemory, explain_scored_memories};

/// Thin facade for journal-style memory search using `mind.memory.*` weights.
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryJournal;

impl MemoryJournal {
    /// Embed `query_text`, build a [`Query`] from mind memory policy, and search.
    pub async fn search(
        store: &dyn MemoryPort,
        embedder: &dyn EmbeddingProvider,
        mind_memory: &MindMemoryConfig,
        character_id: &str,
        user_id: Option<&str>,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<ScoredMemory>, CognitionError> {
        let embedding = ene_ai::embed_query(embedder, query_text)
            .await
            .map_err(CognitionError::Embedding)?;
        let query = Self::to_query(
            mind_memory,
            query_text,
            Some(embedding.as_slice()),
            character_id,
            user_id,
            embedder.model_name(),
            limit,
        );
        let gathered = store
            .search(&query)
            .await
            .map_err(CognitionError::MemoryPort)?;
        Ok(ene_rag::score_and_rank(&query, gathered))
    }

    /// [`search`] plus explainable recall reasons.
    pub async fn search_explained(
        store: &dyn MemoryPort,
        embedder: &dyn EmbeddingProvider,
        mind_memory: &MindMemoryConfig,
        character_id: &str,
        user_id: Option<&str>,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<RecalledMemory>, CognitionError> {
        let scored = Self::search(
            store,
            embedder,
            mind_memory,
            character_id,
            user_id,
            query_text,
            limit,
        )
        .await?;
        Ok(explain_scored_memories(scored))
    }

    /// Build a store [`Query`] from mind memory policy (embedding optional).
    ///
    /// Uses **journal-specific** thresholds (`journal_similarity_threshold`,
    /// `journal_min_score`) which differ from the cognitive recall thresholds
    /// (`recall_similarity_threshold`, `recall_min_score`). This is intentional:
    /// journal search is user-facing diagnostics (broader sweep, lower score
    /// floor), while recall is cognitive (stricter quality for LLM context).
    ///
    /// [`Query::query_affect`] is deliberately left `None`: like the thresholds
    /// above, this keeps journal search a purely diagnostic, broader sweep —
    /// affect matching stays 0 so cognitive recall remains the only path where
    /// the `emotional_match` weight (default 0.05) influences scores.
    pub fn to_query<'a>(
        mind_memory: &MindMemoryConfig,
        query_text: &'a str,
        embedding: Option<&'a [f32]>,
        character_id: &'a str,
        user_id: Option<&'a str>,
        model_name: &'a str,
        limit: usize,
    ) -> Query<'a> {
        let limit = limit.max(1);
        Query {
            query_text,
            embedding,
            character_id,
            user_id,
            model_name,
            limit,
            similarity_threshold: mind_memory.journal_similarity_threshold.clamp(0.0, 1.0),
            candidate_pool_size: mind_memory.journal_candidate_pool_size.max(limit).max(16),
            query_affect: None,
            weights: mind_memory.hybrid_weights,
            decay_half_life_days: mind_memory.default_forgetting_half_life_days.max(0.0),
            now: Utc::now(),
            min_score: mind_memory.journal_min_score.clamp(0.0, 1.0),
            commitment_boost: mind_memory.commitment_boost,
            recent_fallback_limit: mind_memory.recent_fallback_limit,
            time_range: None,
            exclude_kinds: Vec::new(),
        }
    }
}
