use std::time::Duration;

use chrono::Utc;
use ene_config::{CharacterCardV3, PromptLibrary};
use ene_memory::MemoryStore;

use crate::character::CharacterProcessor;
use crate::commitments::CommitmentLedger;
use crate::config::{CognitionConfig, EngineMode};
use crate::context::{
    ContextBudget, ContextManager, PackInput, load_active_scene_summary, pack_prompt,
    validate_context_config,
};
use crate::emotion::TurnAffectInput;
use crate::error::CognitionError;
use crate::lifecycle::{ComposedPrompt, PostTurnInput, PreTurnOutput, TurnContext};
use crate::memory_writer::MemoryWriter;
use crate::recall::{ExecuteRecallInput, execute_hybrid_recall};

/// Central cognitive engine facade.
pub struct CognitionEngine {
    /// Pre-turn input analysis.
    pub pre_turn: crate::pre_turn::PreTurnAnalyzer,
    /// Context budget and compression management.
    pub context: ContextManager,
    /// Memory extraction and arbitration.
    pub memory_writer: MemoryWriter,
    /// Memory recall planning.
    pub recall: crate::recall::RecallPlanner,
    /// Deterministic and LLM emotion computation.
    pub emotion: crate::emotion::EmotionEngine,
    /// Character identity and lorebook processing.
    pub character: CharacterProcessor,
    /// Sectioned prompt composition.
    pub prompt_packet: crate::prompt_packet::PromptPacket,
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
            context: ContextManager,
            memory_writer: MemoryWriter,
            recall: crate::recall::RecallPlanner,
            emotion: crate::emotion::EmotionEngine,
            character: CharacterProcessor,
            prompt_packet: crate::prompt_packet::PromptPacket::default(),
            output: crate::output::OutputArbiter,
            commitments: CommitmentLedger,
        }
    }

    /// Validate cognitive configuration, including context sub-budgets.
    pub fn validate_config(config: &CognitionConfig) -> Result<(), CognitionError> {
        validate_context_config(&config.context)
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

        let mut affect = store
            .get_affect_state(ctx.character_id)
            .await
            .map_err(CognitionError::Memory)?;

        let mut classifier_expression_hint = None;

        if ctx.config.emotion.enabled {
            let elapsed = affect
                .updated_at
                .map(|ts| {
                    Utc::now()
                        .signed_duration_since(ts)
                        .to_std()
                        .unwrap_or(Duration::ZERO)
                })
                .unwrap_or(Duration::ZERO);

            let recent_turn_count = ctx
                .history
                .iter()
                .filter(|entry| entry.role == "user")
                .count();

            if ctx.config.emotion.engine == EngineMode::Llm && ctx.llm_provider.is_none() {
                tracing::warn!(
                    component = "EmotionEngine",
                    "LLM emotion mode enabled but no LLM provider; only decay will apply"
                );
            }

            let mut turn_input = TurnAffectInput {
                state: &mut affect,
                user_message: ctx.user_input,
                elapsed_since_update: elapsed,
                recent_turn_count,
                classifier_proposal: None,
                classifier_min_confidence: ctx.config.emotion.classifier_min_confidence,
                llm_only: ctx.config.emotion.engine == EngineMode::Llm,
            };

            let classifier_lang = ctx.config.emotion.classifier_language.as_str();

            // Optional LLM classifier (#88) — advisory merge inside update_turn.
            if matches!(
                ctx.config.emotion.engine,
                EngineMode::Llm | EngineMode::Hybrid
            ) && let Some(provider) = ctx.llm_provider.as_deref()
            {
                let snippet = build_classifier_snippet(ctx.user_input, ctx.history);
                match crate::emotion::classifier::classify_with_timeout(
                    provider,
                    &snippet,
                    ctx.config.emotion.classifier_timeout_secs,
                    classifier_lang,
                )
                .await
                {
                    Ok(proposal)
                        if proposal.confidence >= ctx.config.emotion.classifier_min_confidence =>
                    {
                        classifier_expression_hint = Some(proposal.recommended_expression.clone());
                        turn_input = turn_input.with_proposal(proposal);
                    }
                    Ok(proposal) if proposal.confidence > 0.0 => {
                        turn_input = turn_input.with_proposal(proposal);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(
                            component = "EmotionEngine",
                            error = %error,
                            "LLM affect classifier failed; using deterministic path"
                        );
                    }
                }
            }

            let update = self
                .emotion
                .update_turn(&ctx.config.emotion, &mut turn_input);
            tracing::debug!(
                component = "CognitionEngine",
                mood = %update.mood_label,
                reasons = update.reasons.len(),
                "Pre-turn affect update complete"
            );
        }

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
            classifier_expression_hint,
        })
    }

    /// Persist affect state after pre-turn update (survives stream cancel/failure).
    pub async fn persist_affect_snapshot(
        store: &MemoryStore,
        affect: &ene_memory::AffectState,
    ) -> Result<(), CognitionError> {
        store
            .upsert_affect_state(affect)
            .await
            .map_err(CognitionError::Memory)
    }

    /// Compose a sectioned prompt packet into LLM messages.
    pub async fn compose_prompt_packet(
        &self,
        ctx: TurnContext<'_>,
        pre: &PreTurnOutput,
    ) -> Result<ComposedPrompt, CognitionError> {
        Self::validate_config(ctx.config)?;

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

        let scene_summary = if let Some(store) = ctx.store {
            load_active_scene_summary(store, ctx.session_id)
                .await?
                .map(|s| s.text)
        } else {
            None
        };

        let prompts = PromptLibrary::load(&ctx.config.emotion.classifier_language);
        let char_name = ctx.card.data.get_character_name();
        let platform_contract = Some(
            prompts
                .system()
                .render_mascot_context(char_name, ctx.user_name),
        );
        let behavior_contract = build_behavior_contract(ctx.card, ctx.user_name);

        let recent_limit = ctx.config.context.recent_turns.saturating_mul(2).max(2);
        let history: Vec<_> = if ctx.history.len() > recent_limit {
            ctx.history[ctx.history.len() - recent_limit..].to_vec()
        } else {
            ctx.history.to_vec()
        };

        let pack_input = PackInput {
            platform_contract,
            identity_kernel: kernel,
            behavior_contract,
            style_examples,
            recalled: pre.recalled.clone(),
            commitments: pre.commitments.clone(),
            affect_summary,
            scene_summary,
            history,
            output_contract: ctx.post_history_block.map(str::to_string),
            user_input: ctx.user_input.to_string(),
        };

        let budget =
            ContextBudget::from_config_and_hints(&ctx.config.context, &pre.recall_plan.budget);
        let packed = pack_prompt(pack_input, &budget);
        let (messages, mut meta) = packed.packet.to_llm_messages();
        meta.dropped_sections = packed.meta.dropped.clone();
        meta.packed_tokens = packed.meta.packed_tokens;

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

    /// Resolve the final character expression after an assistant turn (#89).
    #[must_use]
    pub fn resolve_expression_turn(
        &self,
        config: &CognitionConfig,
        card: &CharacterCardV3,
        affect: &ene_memory::AffectState,
        response_text: &str,
        llm_proposal: Option<&str>,
        previous_expression: &str,
        elapsed_since_change: Option<Duration>,
    ) -> (crate::output::ExpressionDecision, ene_memory::AffectState) {
        use ene_config::resolve_expressions;

        let expressions = resolve_expressions(card);
        let irritation_spike = affect.irritation >= 0.6;
        let input = crate::output::ExpressionInput {
            affect,
            available: &expressions,
            llm_proposal,
            previous_expression,
            elapsed_since_change,
            response_text,
            irritation_spike,
        };
        let decision = self.output.resolve(&config.emotion, &input);
        let mut updated = affect.clone();
        updated.last_expression = decision.expression.clone();
        (decision, updated)
    }
}

fn build_behavior_contract(card: &CharacterCardV3, user_name: &str) -> Option<String> {
    let data = &card.data;
    let char_name = data.get_character_name();
    let mut parts = Vec::new();
    if !data.creator_notes.trim().is_empty() {
        parts.push(format!(
            "## Creator Notes\n{}",
            ene_config::expand_cbs_macros(&data.creator_notes, char_name, user_name)
        ));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

fn build_classifier_snippet(
    user_input: &str,
    history: &[crate::lifecycle::HistoryEntry],
) -> String {
    let mut lines = Vec::new();
    let tail: Vec<_> = history.iter().rev().take(4).collect();
    for entry in tail.into_iter().rev() {
        lines.push(format!("{}: {}", entry.role, entry.content));
    }
    lines.push(format!("user: {user_input}"));
    lines.join("\n")
}
