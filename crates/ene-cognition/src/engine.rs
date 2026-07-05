use ene_memory::MemoryStore;

use crate::character::CharacterProcessor;
use crate::commitments::CommitmentLedger;
use crate::config::CognitionConfig;
use crate::error::CognitionError;
use crate::lifecycle::{ComposedPrompt, PostTurnInput, PreTurnOutput, TurnContext};
use crate::memory_writer::MemoryWriter;
use crate::prompt_packet::PromptPacket;
use crate::recall::{ExecuteRecallInput, execute_hybrid_recall};

/// Central cognitive engine facade.
pub struct CognitionEngine {
    /// Pre-turn input analysis.
    pub pre_turn: crate::pre_turn::PreTurnAnalyzer,
    /// Context budget and compression management.
    pub context: crate::context::ContextManager,
    /// Memory extraction and arbitration.
    pub memory_writer: MemoryWriter,
    /// Memory recall planning.
    pub recall: crate::recall::RecallPlanner,
    /// Deterministic and LLM emotion computation.
    pub emotion: crate::emotion::EmotionEngine,
    /// Character identity and lorebook processing.
    pub character: CharacterProcessor,
    /// Sectioned prompt composition.
    pub prompt_packet: PromptPacket,
    /// Expression arbitration and output validation.
    pub output: crate::output::OutputArbiter,
    /// Companion promise and task tracking.
    pub commitments: CommitmentLedger,
}

impl Default for CognitionEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl CognitionEngine {
    /// Create a new cognitive engine with default components.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pre_turn: crate::pre_turn::PreTurnAnalyzer,
            context: crate::context::ContextManager,
            memory_writer: MemoryWriter,
            recall: crate::recall::RecallPlanner,
            emotion: crate::emotion::EmotionEngine,
            character: CharacterProcessor,
            prompt_packet: PromptPacket::default(),
            output: crate::output::OutputArbiter,
            commitments: CommitmentLedger,
        }
    }

    /// Sync CCv3 lorebook and style indices when the card or config changes.
    pub async fn sync_character_memories(
        &self,
        ctx: TurnContext<'_>,
        previous_hash: Option<u64>,
    ) -> Result<(crate::character::CharacterMemorySyncReport, u64), CognitionError> {
        let store = ctx.store.ok_or_else(|| {
            CognitionError::Other("Memory store required for character memory sync".into())
        })?;
        let embedder = ctx.embedder.ok_or_else(|| {
            CognitionError::Other("Embedding provider required for character memory sync".into())
        })?;

        CharacterProcessor::sync_card_memories(
            store,
            embedder,
            ctx.character_id,
            ctx.user_name,
            ctx.card,
            &ctx.config.character,
            previous_hash,
        )
        .await
    }

    /// Pre-turn: load affect, plan recall, execute hybrid search.
    pub async fn before_turn(&self, ctx: TurnContext<'_>) -> Result<PreTurnOutput, CognitionError> {
        let store = ctx.store.ok_or_else(|| {
            CognitionError::Other("Memory store required for cognitive path".into())
        })?;

        let affect = store
            .get_affect_state(ctx.character_id)
            .await
            .map_err(CognitionError::Memory)?;

        let recent_turns = ctx.recent_recall_turns();
        let embedding = ctx.query_embedding.ok_or_else(|| {
            CognitionError::Other("Query embedding required for cognitive recall".into())
        })?;
        let embedder = ctx.embedder.ok_or_else(|| {
            CognitionError::Other("Embedding provider required for cognitive recall".into())
        })?;

        let recall_input = ExecuteRecallInput {
            store,
            character_id: ctx.character_id,
            user_id: ctx.user_name,
            user_input: ctx.user_input,
            recent_turns: &recent_turns,
            query_embedding: embedding,
            embedding_model: embedder.model_name(),
            llm_provider: ctx.llm_provider.clone(),
            affect: Some(&affect),
            card: Some(ctx.card),
        };

        let (recall_plan, recalled) = execute_hybrid_recall(ctx.config, &recall_input).await?;

        let commitment_rows =
            match CommitmentLedger::list_active(store, ctx.character_id, Some(ctx.user_name), 16)
                .await
            {
                Ok(rows) => rows,
                Err(error) => {
                    tracing::warn!(
                        component = "CognitionEngine",
                        error = %error,
                        "Failed to list active commitments for pre-turn recall"
                    );
                    vec![]
                }
            };
        let commitments = CommitmentLedger::active_prompt_candidates(&commitment_rows);

        Ok(PreTurnOutput {
            recall_plan,
            affect,
            recalled,
            commitments,
        })
    }

    /// Compose a sectioned prompt packet into LLM messages.
    pub async fn compose_prompt_packet(
        &self,
        ctx: TurnContext<'_>,
        pre: &PreTurnOutput,
    ) -> Result<ComposedPrompt, CognitionError> {
        let max_kernel_tokens = ctx.config.character.identity_kernel_max_tokens;
        let kernel = if ctx.config.character.always_include_identity_kernel {
            CharacterProcessor::compile_kernel(ctx.card, ctx.user_name, max_kernel_tokens)
        } else {
            crate::character::IdentityKernel {
                name: ctx.card.data.get_character_name().to_string(),
                text: String::new(),
                post_history_instructions: None,
            }
        };

        let style_examples = CharacterProcessor::select_style_examples(
            ctx.card,
            ctx.user_name,
            ctx.user_input,
            ctx.history,
            ctx.store,
            ctx.embedder,
            &ctx.config.character,
            2,
        )
        .await;

        let affect_summary = Some(format!(
            "mood={}; valence={:.2}; arousal={:.2}",
            pre.affect.mood_label, pre.affect.valence, pre.affect.arousal
        ));

        let history = ctx.history.to_vec();
        let packet = PromptPacket::compose(
            kernel,
            style_examples,
            pre.recalled.clone(),
            pre.commitments.clone(),
            affect_summary,
            history,
            ctx.post_history_block.map(str::to_string),
            ctx.user_input,
            ctx.config.context.max_prompt_tokens,
            ctx.config.context.style_example_budget_tokens,
        );

        let (messages, meta) = packet.to_llm_messages();
        Ok(ComposedPrompt { messages, meta })
    }

    /// Post-turn: memory extraction, forgetting lifecycle, affect persistence.
    pub async fn after_turn(
        &self,
        store: &MemoryStore,
        config: &CognitionConfig,
        input: PostTurnInput<'_>,
    ) -> Result<(), CognitionError> {
        MemoryWriter::after_turn(store, config, input).await
    }
}
