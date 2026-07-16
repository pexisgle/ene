//! End-to-end hybrid recall execution for the cognitive runtime (#100).

use std::sync::Arc;

use chrono::Utc;
use ene_ai::{EmbeddingKind, EmbeddingProvider, LlmProvider};
use ene_config::CharacterCardV3;
use ene_store::MemoryStore;

use super::lorebook_boost::merge_lorebook_recall;
use crate::commitments::CommitmentLedger;
use crate::config::MindConfig;
use crate::error::CognitionError;
use crate::recall::{
    MemoryDiversifyOptions, MemoryDiversifyPipeline, MemoryRerankOptions, MemoryRerankPipeline,
    RecallPlanner, RecallPlannerInput, RecallPlannerOptions, RecallResultMapper, RecallTurn,
    RecalledMemory,
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
    /// Optional embedder used when [`crate::config::MindMemoryConfig::use_hyde`]
    /// is enabled to embed the hypothetical document.
    pub embedder: Option<&'a dyn EmbeddingProvider>,
    /// Optional LLM provider for `HyDE` document generation and reranking.
    pub llm_provider: Option<Arc<dyn LlmProvider>>,
    /// Loaded affect state (optional).
    pub affect: Option<&'a ene_store::AffectState>,
    /// Character card for lorebook key-trigger recall (#83).
    pub card: Option<&'a CharacterCardV3>,
}

/// Execute hybrid typed recall and return plan + recalled memories.
pub async fn execute_hybrid_recall(
    config: &MindConfig,
    input: &ExecuteRecallInput<'_>,
) -> Result<(crate::recall::RecallPlan, Vec<RecalledMemory>), CognitionError> {
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

    let search_embedding = resolve_search_embedding(
        config,
        input,
        plan.use_hyde,
        &plan.search.primary_query_text,
    )
    .await;
    let search_options = RecallPlanner::to_memory_search_options(
        &plan,
        &search_embedding,
        input.embedding_model,
        Utc::now(),
        &config.memory,
    );

    let scored = input
        .store
        .search(&search_options)
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

    recalled = maybe_merge_lorebook_recall(config, input, recalled).await?;

    bump_recalled_memory_access(input.store, &recalled).await;

    Ok((plan, recalled))
}

/// Resolve the embedding used for hybrid search, optionally blending `HyDE`.
async fn resolve_search_embedding(
    config: &MindConfig,
    input: &ExecuteRecallInput<'_>,
    use_hyde: bool,
    primary_query: &str,
) -> Vec<f32> {
    if !use_hyde {
        return input.query_embedding.to_vec();
    }

    let Some(embedder) = input.embedder else {
        tracing::warn!(
            component = "RecallRunner",
            "use_hyde is enabled but no embedder was provided; using query embedding"
        );
        return input.query_embedding.to_vec();
    };

    let llm = input.llm_provider.as_deref();
    let hyde_text = match ene_ai::hyde_document(llm, primary_query).await {
        Ok(text) => text,
        Err(error) => {
            tracing::warn!(
                component = "RecallRunner",
                error = %error,
                "HyDE document generation failed; using query embedding"
            );
            return input.query_embedding.to_vec();
        }
    };

    if hyde_text == primary_query {
        // No LLM (or echo fallback): skip the redundant embed + blend.
        return input.query_embedding.to_vec();
    }

    let hyde_vec = match ene_ai::embed(embedder, &hyde_text, EmbeddingKind::Hyde).await {
        Ok(vec) => vec,
        Err(error) => {
            tracing::warn!(
                component = "RecallRunner",
                error = %error,
                "HyDE embedding failed; using query embedding"
            );
            return input.query_embedding.to_vec();
        }
    };

    if hyde_vec.len() != input.query_embedding.len() {
        tracing::warn!(
            component = "RecallRunner",
            hyde_dims = hyde_vec.len(),
            query_dims = input.query_embedding.len(),
            "HyDE embedding dimension mismatch; using query embedding"
        );
        return input.query_embedding.to_vec();
    }

    blend_embeddings(input.query_embedding, &hyde_vec, config.memory.hyde_blend)
}

/// Linearly blend query and `HyDE` embeddings, then L2-normalize.
fn blend_embeddings(query: &[f32], hyde: &[f32], blend: f32) -> Vec<f32> {
    let blend = blend.clamp(0.0, 1.0);
    let mut out: Vec<f32> = query
        .iter()
        .zip(hyde.iter())
        .map(|(q, h)| h.mul_add(blend, q * (1.0 - blend)))
        .collect();

    let norm = out.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in &mut out {
            *x /= norm;
        }
    }
    out
}

async fn maybe_merge_lorebook_recall(
    config: &MindConfig,
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
    use crate::config::{CharacterMemoryConfig, MindConfig};
    use ene_config::{CharacterCardV3, Lorebook, LorebookEntry};
    use ene_store::{MemoryConfidence, MemoryKind, MemorySalience, MemorySource, MemoryStatus};
    use ene_store::{MemoryScope, NewMemoryItem};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockEmbedder {
        hyde_calls: AtomicUsize,
    }

    impl MockEmbedder {
        fn new() -> Self {
            Self {
                hyde_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ene_ai::EmbeddingProvider for MockEmbedder {
        fn model_name(&self) -> &'static str {
            "mock"
        }

        fn dimensions(&self) -> usize {
            4
        }

        async fn embed_batch(
            &self,
            items: &[(&str, ene_ai::EmbeddingKind)],
        ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
            Ok(items
                .iter()
                .map(|(_, kind)| match kind {
                    EmbeddingKind::Hyde => {
                        self.hyde_calls.fetch_add(1, Ordering::SeqCst);
                        vec![0.0, 1.0, 0.0, 0.0]
                    }
                    _ => vec![1.0, 0.0, 0.0, 0.0],
                })
                .collect())
        }
    }

    struct MockHydeLlm;

    #[async_trait::async_trait]
    impl LlmProvider for MockHydeLlm {
        fn name(&self) -> &'static str {
            "mock-hyde"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[ene_ai::LlmMessage],
            _tools: &[ene_tool_proto::ToolSpec],
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn tokio_stream::Stream<
                            Item = Result<ene_ai::LlmResponseChunk, ene_ai::LlmProviderError>,
                        > + Send,
                >,
            >,
            ene_ai::LlmProviderError,
        > {
            Err(ene_ai::LlmProviderError::Provider(
                "mock: stream not implemented".into(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[ene_ai::LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<String, ene_ai::LlmProviderError> {
            Ok("A sunny world where friends meet outdoors.".into())
        }
    }

    #[test]
    fn blend_embeddings_mixes_and_normalizes() {
        let query = [1.0, 0.0, 0.0, 0.0];
        let hyde = [0.0, 1.0, 0.0, 0.0];
        let blended = blend_embeddings(&query, &hyde, 0.5);
        assert!((blended[0] - blended[1]).abs() < 1e-5);
        let norm: f32 = blended.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[tokio::test]
    async fn hyde_blends_when_enabled_with_llm_and_embedder() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        // Memory aligned with HyDE axis [0,1,0,0] so blended search prefers it.
        let hyde_item = NewMemoryItem {
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: "User".into(),
            kind: MemoryKind::Semantic,
            title: "Sunny lore".into(),
            content: "A sunny world where friends meet outdoors.".into(),
            source: MemorySource::Ccv3,
            source_ref: Some("test:hyde".into()),
            confidence: MemoryConfidence::new(1.0),
            salience: MemorySalience::new(1.0),
            affect: Default::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        };
        let hyde_id = store.insert_typed_memory(&hyde_item).await.unwrap();
        store
            .upsert_memory_embedding(hyde_id, "mock", "content", &[0.0, 1.0, 0.0, 0.0])
            .await
            .unwrap();

        let query_item = NewMemoryItem {
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: "User".into(),
            kind: MemoryKind::Semantic,
            title: "Query-aligned".into(),
            content: "Something about pizza.".into(),
            source: MemorySource::Conversation,
            source_ref: Some("test:query".into()),
            confidence: MemoryConfidence::new(1.0),
            salience: MemorySalience::new(1.0),
            affect: Default::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        };
        let query_id = store.insert_typed_memory(&query_item).await.unwrap();
        store
            .upsert_memory_embedding(query_id, "mock", "content", &[1.0, 0.0, 0.0, 0.0])
            .await
            .unwrap();

        let embedder = MockEmbedder::new();
        let mut config = MindConfig::default();
        config.memory.use_hyde = true;
        config.memory.hyde_blend = 1.0; // pure HyDE vector
        config.memory.hybrid_search = true;
        config.memory.mmr_enabled = false;
        config.memory.rerank_enabled = false;
        config.memory.recall_similarity_threshold = 0.0;
        config.memory.recall_min_score = 0.0;

        let input = ExecuteRecallInput {
            store: &store,
            character_id: "Ene",
            user_id: "User",
            user_input: "How is the weather?",
            recent_turns: &[],
            query_embedding: &[1.0, 0.0, 0.0, 0.0],
            embedding_model: "mock",
            embedder: Some(&embedder),
            llm_provider: Some(Arc::new(MockHydeLlm)),
            affect: None,
            card: None,
        };

        let (_, recalled) = execute_hybrid_recall(&config, &input)
            .await
            .expect("recall");
        assert!(
            embedder.hyde_calls.load(Ordering::SeqCst) >= 1,
            "HyDE embedding should be requested"
        );
        assert!(
            recalled
                .first()
                .is_some_and(|m| m.item.content.contains("sunny")),
            "pure HyDE blend should rank the HyDE-aligned memory first, got: {:?}",
            recalled
                .iter()
                .map(|m| m.item.title.clone())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn hyde_skipped_without_llm_echo() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let embedder = MockEmbedder::new();
        let mut config = MindConfig::default();
        config.memory.use_hyde = true;
        config.memory.hybrid_search = true;

        let input = ExecuteRecallInput {
            store: &store,
            character_id: "Ene",
            user_id: "User",
            user_input: "hello",
            recent_turns: &[],
            query_embedding: &[1.0, 0.0, 0.0, 0.0],
            embedding_model: "mock",
            embedder: Some(&embedder),
            llm_provider: None, // hyde_document echoes query → skip embed
            affect: None,
            card: None,
        };

        let _ = execute_hybrid_recall(&config, &input)
            .await
            .expect("recall");
        assert_eq!(
            embedder.hyde_calls.load(Ordering::SeqCst),
            0,
            "echo HyDE should not re-embed"
        );
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
            commitment_id: None,
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

        let mut config = MindConfig::default();
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
            embedder: None,
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
