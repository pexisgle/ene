//! End-to-end hybrid recall execution for the cognitive runtime (#100).

use std::sync::Arc;

use chrono::Utc;
use ene_memory::MemoryStore;
use ene_provider::LlmProvider;

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
}

/// Execute hybrid typed recall and return plan + recalled memories.
pub async fn execute_hybrid_recall(
    config: &CognitionConfig,
    input: &ExecuteRecallInput<'_>,
) -> Result<(crate::recall::RecallPlan, Vec<RecalledMemory>), CognitionError> {
    if !config.enabled || !config.memory.hybrid_search {
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

    input
        .store
        .ensure_legacy_migration_allowed(input.character_id, config.memory.require_migration)
        .await
        .map_err(CognitionError::Memory)?;

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
                "Failed to list active commitments for hybrid recall"
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

    bump_recalled_memory_access(input.store, &recalled).await;

    Ok((plan, recalled))
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
