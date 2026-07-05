//! Typed memory recall execution with optional LLM reranking.
//!
//! Bridges cognition recall planning, hybrid search, reranking, and prompt
//! injection via legacy [`KeyFact`] rows.

use std::sync::Arc;

use chrono::Utc;
use ene_cognition::{
    CognitionConfig, CommitmentLedger, MemoryDiversifyOptions, MemoryDiversifyPipeline,
    MemoryRerankOptions, MemoryRerankPipeline, RecallPlanner, RecallPlannerInput,
    RecallPlannerOptions, RecallResultMapper, RecallTurn, RecalledMemory, format_recalled_content,
};
use ene_config::EneConfig;
use ene_memory::KeyFact;
use ene_provider::{EmbeddingProvider, LlmProvider, Role};
use ene_session::ConversationSession;

/// Execute typed hybrid recall for the current turn and map results into key facts.
///
/// Returns an empty vector when cognition is disabled, hybrid search is off, or
/// recall execution prerequisites are unavailable.
pub(crate) async fn recall_typed_memories_for_prompt(
    session: &ConversationSession,
    config: &EneConfig,
    user_input: &str,
    query_embedding: &[f32],
    provider: Option<Arc<dyn LlmProvider>>,
    embedder: Option<&Arc<dyn EmbeddingProvider>>,
) -> Vec<KeyFact> {
    let cognition = config.get_section::<CognitionConfig>().unwrap_or_default();
    if !cognition.enabled || !cognition.memory.hybrid_search {
        return vec![];
    }

    let Some(store) = session.memory.memory_store.as_ref() else {
        return vec![];
    };

    let Some(embedder) = embedder else {
        return vec![];
    };

    let model_name = embedder.model_name();
    let card_name = session.card_name();
    let user_name = config.user_name.as_str();

    let recent_limit = cognition.context.recent_turns.max(1);
    let history = session.history();
    let recent_turns: Vec<RecallTurn<'_>> = history
        .iter()
        .rev()
        .take(recent_limit.saturating_mul(2))
        .rev()
        .map(|(role, content)| RecallTurn {
            role: match role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
            },
            content: content.as_str(),
        })
        .collect();

    let commitments =
        match CommitmentLedger::list_active(store, card_name, Some(user_name), 16).await {
            Ok(rows) => CommitmentLedger::active_prompt_candidates(&rows),
            Err(error) => {
                tracing::warn!(
                    component = "Memory",
                    error = %error,
                    "Failed to list active commitments for typed recall"
                );
                vec![]
            }
        };

    let planner_input = RecallPlannerInput {
        user_input,
        recent_turns: &recent_turns,
        scene_summary: None,
        affect: None,
        commitments: &commitments,
        character_id: card_name,
        user_id: Some(user_name),
    };

    let options = RecallPlannerOptions::from_config(&cognition.context, &cognition.memory);
    let plan = match RecallPlanner::plan(&planner_input, &options) {
        Ok(plan) => plan,
        Err(error) => {
            tracing::warn!(
                component = "Memory",
                error = %error,
                "Typed recall planning failed"
            );
            return vec![];
        }
    };

    let search_options =
        RecallPlanner::to_memory_search_options(&plan, query_embedding, model_name, Utc::now());

    let scored = match store.search_typed_memories_hybrid(&search_options).await {
        Ok(results) => results,
        Err(error) => {
            tracing::error!(
                component = "Memory",
                error = %error,
                "Typed hybrid recall failed"
            );
            return vec![];
        }
    };

    let rerank_options = MemoryRerankOptions::from_config(&cognition.memory);

    let diversify_options = MemoryDiversifyOptions::from_config(&cognition.memory);
    let diversified = MemoryDiversifyPipeline::diversify(scored, &plan, diversify_options);

    let recall_question = plan.search.primary_query_text;

    let llm_provider = if rerank_options.enabled {
        provider
    } else {
        None
    };
    let pipeline = MemoryRerankPipeline::new(llm_provider);
    let reranked = pipeline
        .rerank(&recall_question, diversified, rerank_options)
        .await;

    let recalled = RecallResultMapper::map(reranked);
    bump_recalled_memory_access(store, &recalled).await;
    recalled_memories_to_key_facts(recalled)
}

async fn bump_recalled_memory_access(store: &ene_memory::MemoryStore, recalled: &[RecalledMemory]) {
    for memory in recalled {
        let Some(id) = memory.item.id else {
            continue;
        };
        if let Err(error) = store.bump_typed_memory_access(id).await {
            tracing::warn!(
                component = "Memory",
                memory_id = id,
                error = %error,
                "Failed to bump typed memory access after recall"
            );
        }
    }
}

fn recalled_memories_to_key_facts(recalled: Vec<RecalledMemory>) -> Vec<KeyFact> {
    recalled
        .into_iter()
        .map(|memory| {
            let key = if memory.item.title.trim().is_empty() {
                memory.item.kind.as_str().to_string()
            } else {
                memory.item.title.clone()
            };
            KeyFact {
                key,
                value: format_recalled_content(&memory),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_memory::{
        AffectAnnotation, MemoryConfidence, MemoryItem, MemoryKind, MemorySalience, MemoryScope,
        MemoryScoreBreakdown, MemorySource, MemoryStatus,
    };

    #[test]
    fn recalled_memories_map_to_key_facts_using_title_or_kind() {
        let recalled = vec![RecalledMemory {
            item: MemoryItem {
                id: Some(1),
                scope: MemoryScope::Character,
                character_id: "ene".into(),
                user_id: String::new(),
                kind: MemoryKind::Preference,
                title: "favorite drink".into(),
                content: "matcha latte".into(),
                source: MemorySource::Conversation,
                source_ref: None,
                confidence: MemoryConfidence::default(),
                salience: MemorySalience::default(),
                affect: AffectAnnotation::default(),
                relationship_impact: 0.0,
                access_count: 0,
                last_accessed_at: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Active,
                supersedes_id: None,
                pinned: false,
                faded_at: None,
            },
            reason: ene_cognition::RecallReason::UserPreference,
            score_breakdown: MemoryScoreBreakdown {
                vector_similarity: 0.8,
                lexical_score: 0.0,
                recency_score: 0.0,
                salience: 0.0,
                confidence: 0.0,
                emotional_match: 0.0,
                relationship: 0.0,
                access_boost: 0.0,
                contradiction_penalty: 0.0,
                stale_penalty: 0.0,
                commitment_boost: 0.0,
                total: 0.8,
            },
            sources: vec![ene_memory::MemoryCandidateSource::Vector],
        }];

        let facts = recalled_memories_to_key_facts(recalled);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].key, "favorite drink");
        assert_eq!(facts[0].value, "matcha latte");
    }

    #[test]
    fn faded_recalled_memories_get_uncertain_marker() {
        let recalled = vec![RecalledMemory {
            item: MemoryItem {
                id: Some(2),
                scope: MemoryScope::Character,
                character_id: "ene".into(),
                user_id: String::new(),
                kind: MemoryKind::Semantic,
                title: "old fact".into(),
                content: "maybe remembers this".into(),
                source: MemorySource::Conversation,
                source_ref: None,
                confidence: MemoryConfidence::new(0.8),
                salience: MemorySalience::default(),
                affect: AffectAnnotation::default(),
                relationship_impact: 0.0,
                access_count: 0,
                last_accessed_at: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                valid_from: None,
                valid_until: None,
                status: MemoryStatus::Faded,
                supersedes_id: None,
                pinned: false,
                faded_at: None,
            },
            reason: ene_cognition::RecallReason::SimilarTopic,
            score_breakdown: MemoryScoreBreakdown {
                vector_similarity: 0.0,
                lexical_score: 0.0,
                recency_score: 0.0,
                salience: 0.0,
                confidence: 0.0,
                emotional_match: 0.0,
                relationship: 0.0,
                access_boost: 0.0,
                contradiction_penalty: 0.0,
                stale_penalty: 0.1,
                commitment_boost: 0.0,
                total: 0.0,
            },
            sources: vec![],
        }];

        let facts = recalled_memories_to_key_facts(recalled);
        assert_eq!(facts[0].value, "[uncertain] maybe remembers this");
    }
}
