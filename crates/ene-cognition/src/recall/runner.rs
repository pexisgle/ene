//! End-to-end hybrid recall execution for the cognitive runtime (#100).

use std::sync::Arc;

use chrono::Utc;
use ene_config::CharacterCardV3;
use ene_memory::MemoryStore;
use ene_provider::LlmProvider;

use super::lorebook_boost::merge_lorebook_recall;
use crate::commitments::CommitmentLedger;
use crate::config::CognitionConfig;
use crate::error::CognitionError;
use crate::recall::{
    MemoryDiversifyOptions, MemoryDiversifyPipeline, MemoryRerankOptions, MemoryRerankPipeline,
    RecallPlanner, RecallPlannerInput, RecallPlannerOptions, RecallResultMapper, RecallTurn,
    RecalledMemory, legacy,
};

/// Input for executing hybrid typed-memory recall.
pub struct ExecuteRecallInput<'a> {
    /// Memory store handle.
    pub store: &'a MemoryStore,
    /// Character card name.
    pub character_id: &'a str,
    /// User name / id for scoping.
    pub user_id: &'a str,
    /// Current user message.
    pub user_input: &'a str,
    /// Recent conversation turns for planning.
    pub recent_turns: &'a [RecallTurn<'a>],
    /// Query embedding vector.
    pub query_embedding: &'a [f32],
    /// Embedding model name.
    pub embedding_model: &'a str,
    /// Optional LLM provider for reranking.
    pub llm_provider: Option<Arc<dyn LlmProvider>>,
    /// Loaded affect state (optional).
    pub affect: Option<&'a ene_memory::AffectState>,
    /// Character card for lorebook key-trigger recall (#83).
    pub card: Option<&'a CharacterCardV3>,
}

/// Execute hybrid typed recall and return plan + recalled memories.
pub async fn execute_hybrid_recall(
    config: &CognitionConfig,
    input: &ExecuteRecallInput<'_>,
) -> Result<(crate::recall::RecallPlan, Vec<RecalledMemory>), CognitionError> {
    if !config.enabled {
        let options = RecallPlannerOptions::from_config(&config.context, &config.memory);
        let plan = RecallPlanner::plan(
            &RecallPlannerInput {
                user_input: input.user_input,
                recent_turns: input.recent_turns,
                scene_summary: None,
                affect: input.affect,
                commitments: &[],
                character_id: input.character_id,
                user_id: Some(input.user_id),
            },
            &options,
        )?;
        return Ok((plan, vec![]));
    }

    let commitments = match CommitmentLedger::list_active(
        input.store,
        input.character_id,
        Some(input.user_id),
        16,
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(
                component = "RecallRunner",
                error = %error,
                "Failed to list active commitments for recall planning"
            );
            vec![]
        }
    };
    let commitment_prompts = CommitmentLedger::active_prompt_candidates(&commitments);

    let planner_input = RecallPlannerInput {
        user_input: input.user_input,
        recent_turns: input.recent_turns,
        scene_summary: None,
        affect: input.affect,
        commitments: &commitment_prompts,
        character_id: input.character_id,
        user_id: Some(input.user_id),
    };

    let options = RecallPlannerOptions::from_config(&config.context, &config.memory);
    let plan = RecallPlanner::plan(&planner_input, &options)?;

    if !config.memory.hybrid_search {
        let recalled = maybe_merge_lorebook_recall(config, input, vec![]).await?;
        return Ok((plan, recalled));
    }

    input
        .store
        .ensure_legacy_migration_allowed(input.character_id, config.memory.require_migration)
        .await
        .map_err(CognitionError::Memory)?;

    let search_options = RecallPlanner::to_memory_search_options(
        &plan,
        input.query_embedding,
        input.embedding_model,
        Utc::now(),
    );

    let scored = input
        .store
        .search_typed_memories_hybrid(&search_options)
        .await
        .map_err(CognitionError::Memory)?;

    let diversify_options = MemoryDiversifyOptions::from_config(&config.memory);
    let diversified = MemoryDiversifyPipeline::diversify(scored, &plan, diversify_options);

    let rerank_options = MemoryRerankOptions::from_config(&config.memory);
    let recall_question = plan.search.primary_query_text.clone();
    let llm_provider = if rerank_options.enabled {
        input.llm_provider.clone()
    } else {
        None
    };
    let pipeline = MemoryRerankPipeline::new(llm_provider);
    let reranked = pipeline
        .rerank(&recall_question, diversified, rerank_options)
        .await;

    let mut recalled = RecallResultMapper::map(reranked);
    recalled = legacy::merge_legacy_recall(
        input.store,
        input.character_id,
        input.query_embedding,
        plan.budget.result_limit,
        config.memory.recall_similarity_threshold,
        recalled,
    )
    .await
    .map_err(CognitionError::Memory)?;

    recalled = maybe_merge_lorebook_recall(config, input, recalled).await?;

    bump_recalled_memory_access(input.store, &recalled).await;

    Ok((plan, recalled))
}

async fn maybe_merge_lorebook_recall(
    config: &CognitionConfig,
    input: &ExecuteRecallInput<'_>,
    recalled: Vec<RecalledMemory>,
) -> Result<Vec<RecalledMemory>, CognitionError> {
    if !config.character.compile_ccv3_to_semantic_memory || input.card.is_none() {
        return Ok(recalled);
    }

    merge_lorebook_recall(
        input.store,
        input.character_id,
        input.card,
        input.user_input,
        input.recent_turns,
        recalled,
    )
    .await
}

async fn bump_recalled_memory_access(store: &MemoryStore, recalled: &[RecalledMemory]) {
    for memory in recalled {
        let Some(id) = memory.item.id else {
            continue;
        };
        if let Err(error) = store.bump_typed_memory_access(id).await {
            tracing::warn!(
                component = "RecallRunner",
                memory_id = id,
                error = %error,
                "Failed to bump typed memory access after recall"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CharacterMemoryConfig, CognitionConfig};
    use ene_config::{CharacterCardV3, Lorebook, LorebookEntry};
    use ene_memory::{MemoryConfidence, MemoryKind, MemorySalience, MemorySource, MemoryStatus};
    use ene_memory::{MemoryScope, NewMemoryItem};

    struct MockEmbedder;

    #[async_trait::async_trait]
    impl ene_provider::EmbeddingProvider for MockEmbedder {
        fn model_name(&self) -> &str {
            "mock"
        }

        fn dimensions(&self) -> usize {
            4
        }

        async fn embed(
            &self,
            _text: &str,
            _kind: ene_provider::EmbeddingKind,
        ) -> Result<Vec<f32>, ene_provider::EmbeddingError> {
            Ok(vec![1.0, 0.0, 0.0, 0.0])
        }
    }

    #[tokio::test]
    async fn lorebook_recall_runs_when_hybrid_search_disabled() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let item = NewMemoryItem {
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: String::new(),
            kind: MemoryKind::Semantic,
            title: "World tone".into(),
            content: "The world is always sunny.".into(),
            source: MemorySource::Ccv3,
            source_ref: Some("ccv3:lorebook:constant".into()),
            confidence: MemoryConfidence::new(1.0),
            salience: MemorySalience::new(1.0),
            affect: Default::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: true,
            created_at: None,
        };
        let id = store.insert_typed_memory(&item).await.unwrap();
        store
            .upsert_memory_embedding(id, "mock", "content", &[1.0, 0.0, 0.0, 0.0])
            .await
            .unwrap();

        let mut card = CharacterCardV3::default();
        card.data.name = "Ene".into();
        card.data.character_book = Some(Lorebook {
            entries: vec![LorebookEntry {
                keys: vec![],
                content: "The world is always sunny.".into(),
                extensions: Default::default(),
                enabled: true,
                insertion_order: 20,
                case_sensitive: None,
                use_regex: false,
                constant: Some(true),
                name: Some("World tone".into()),
                priority: None,
                id: Some(serde_json::json!("constant")),
                comment: None,
                selective: None,
                secondary_keys: None,
                position: None,
            }],
            ..Default::default()
        });

        let mut config = CognitionConfig::default();
        config.memory.hybrid_search = false;
        config.character = CharacterMemoryConfig {
            compile_ccv3_to_semantic_memory: true,
            ..CharacterMemoryConfig::default()
        };

        let input = ExecuteRecallInput {
            store: &store,
            character_id: "Ene",
            user_id: "User",
            user_input: "How is the weather?",
            recent_turns: &[],
            query_embedding: &[1.0, 0.0, 0.0, 0.0],
            embedding_model: "mock",
            llm_provider: None,
            affect: None,
            card: Some(&card),
        };

        let (_, recalled) = execute_hybrid_recall(&config, &input)
            .await
            .expect("recall");
        assert!(
            recalled
                .iter()
                .any(|m| m.item.content.contains("always sunny")),
            "constant lorebook should merge even when hybrid search is disabled"
        );
    }
}
