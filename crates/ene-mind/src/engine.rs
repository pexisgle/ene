#![expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "mind pipeline uses intentional turn/score/index arithmetic; history/token helpers index into bounds-checked conversational buffers"
)]
use std::time::Duration;

use chrono::Utc;
use ene_config::{CharacterCardV3, PromptLibrary};
use ene_core::MemoryPort;
use tracing::Instrument;

use crate::character::CharacterProcessor;
use crate::commitments::CommitmentLedger;
use crate::config::MindConfig;
use crate::context::{
    ContextBudget, ContextManager, PackInput, load_active_scene_summary, pack_prompt,
};
use crate::emotion::TurnAffectInput;
use crate::error::CognitionError;
use crate::lifecycle::{
    ComposePrefetch, ComposedPrompt, PostTurnInput, PreTurnOutput, TurnContext,
};
use crate::memory_writer::MemoryWriter;
use crate::recall::{ExecuteRecallInput, execute_hybrid_recall};

/// Central cognitive engine facade.
#[expect(
    dead_code,
    reason = "facade fields are constructed and held for sub-component lifecycle; access is through engine methods"
)]
pub struct CognitionEngine {
    /// Context budget and compression management.
    pub(crate) context: ContextManager,
    /// Memory extraction and arbitration.
    pub(crate) memory_writer: MemoryWriter,
    /// Memory recall planning.
    pub(crate) recall: crate::recall::RecallPlanner,
    /// Deterministic and LLM emotion computation.
    pub(crate) emotion: crate::emotion::EmotionEngine,
    /// Character identity and lorebook processing.
    pub(crate) character: CharacterProcessor,
    /// Sectioned prompt composition.
    pub(crate) prompt_packet: crate::prompt_packet::PromptPacket,
    /// Expression arbitration and output validation.
    pub(crate) output: crate::output::OutputArbiter,
    /// Companion promise and task tracking.
    pub(crate) commitments: CommitmentLedger,
}

impl Default for CognitionEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of a deferred memory-write task.
#[derive(Debug, Clone)]
pub enum MemoryWriteOutcome {
    /// Write and forgetting completed successfully.
    Ok {
        /// Number of memory candidates deferred to the user-approval queue.
        deferred_candidates: usize,
    },
    /// Write failed; a retry row was enqueued (or marked permanent).
    Failed {
        /// Human-readable error.
        message: String,
        /// Pending queue id when enqueued.
        pending_id: Option<i64>,
        /// Whether the failure is permanent.
        permanent: bool,
        /// Character id for diagnostics.
        character_id: String,
    },
}

impl CognitionEngine {
    /// Create a new cognitive engine with default components.
    #[must_use]
    pub fn new() -> Self {
        Self {
            context: ContextManager::default(),
            memory_writer: MemoryWriter,
            recall: crate::recall::RecallPlanner,
            emotion: crate::emotion::EmotionEngine,
            character: CharacterProcessor,
            prompt_packet: crate::prompt_packet::PromptPacket::default(),
            output: crate::output::OutputArbiter,
            commitments: CommitmentLedger,
        }
    }

    /// Sync `CCv3` lorebook and style indices when the card or config changes.
    pub async fn sync_character_memories(
        &self,
        ctx: TurnContext<'_>,
        previous_hash: Option<u64>,
    ) -> Result<(crate::character::CharacterMemorySyncReport, u64), CognitionError> {
        let store = ctx.store.ok_or_else(|| {
            CognitionError::MissingProvider(
                "Memory store required for character memory sync".into(),
            )
        })?;
        let embedder = ctx.embedder.ok_or_else(|| {
            CognitionError::MissingProvider(
                "Embedding provider required for character memory sync".into(),
            )
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
            CognitionError::MissingProvider("Memory store required for cognitive path".into())
        })?;

        let mut affect = store
            .get_affect_state(ctx.character_id)
            .await
            .map_err(CognitionError::MemoryPort)?;

        let mut classifier_expression_hint = None;

        if ctx.config.emotion.enabled {
            let elapsed = affect.updated_at.map_or(Duration::ZERO, |ts| {
                Utc::now()
                    .signed_duration_since(ts)
                    .to_std()
                    .unwrap_or(Duration::ZERO)
            });

            let recent_turn_count = ctx
                .history
                .iter()
                .filter(|entry| entry.role == ene_ai::Role::User)
                .count();

            // `get_ene_extension` clones the extension; same per-turn cost as
            // the existing `expressions` flow.
            let affect_baseline = ctx
                .card
                .data
                .get_ene_extension()
                .and_then(|ext| ext.affect_baseline)
                .unwrap_or_default();

            let mut turn_input = TurnAffectInput {
                state: &mut affect,
                elapsed_since_update: elapsed,
                recent_turn_count,
                baseline: affect_baseline,
                classifier_proposal: None,
                classifier_min_confidence: ctx.config.emotion.classifier_min_confidence,
            };

            // Consume a pending post-turn classifier proposal from the previous turn.
            let mut classifier_estimate_for_log = None;
            let current_user_turn = count_user_turns(ctx.history);
            if let Some(pending) = store
                .take_pending_affect_proposal(ctx.character_id, ctx.user_name)
                .await
                .map_err(CognitionError::MemoryPort)?
            {
                if pending.source_turn_id >= current_user_turn {
                    tracing::warn!(
                        component = "EmotionEngine",
                        source_turn_id = pending.source_turn_id,
                        current_user_turn,
                        "Dropping future pending classifier proposal"
                    );
                } else if pending.source_turn_id < current_user_turn - 1 {
                    tracing::warn!(
                        component = "EmotionEngine",
                        source_turn_id = pending.source_turn_id,
                        current_user_turn,
                        "Dropping stale pending classifier proposal"
                    );
                } else {
                    let proposal = pending_to_affect_proposal(pending);
                    if proposal.confidence >= ctx.config.emotion.classifier_min_confidence {
                        classifier_expression_hint = Some(proposal.recommended_expression.clone());
                    }
                    if proposal.confidence > 0.0 {
                        classifier_estimate_for_log = Some(proposal.clone());
                        turn_input = turn_input.with_proposal(proposal);
                    }
                }
            }

            let update = self
                .emotion
                .update_turn(&ctx.config.emotion, &mut turn_input);

            if let Some(proposal) = classifier_estimate_for_log {
                tracing::info!(
                    component = "EmotionEngine",
                    user_emotion = %proposal.user_emotion,
                    user_intent = %proposal.user_intent,
                    estimated_valence = proposal.valence,
                    estimated_arousal = proposal.arousal,
                    estimated_irritation = proposal.irritation,
                    estimated_affinity = proposal.affinity,
                    recommended_expression = %proposal.recommended_expression,
                    confidence = proposal.confidence,
                    reason = %proposal.reason,
                    mood_after = %update.mood_label,
                    valence_after = affect.valence,
                    arousal_after = affect.arousal,
                    irritation_after = affect.irritation,
                    affinity_after = affect.affinity,
                    "Blended post-turn classifier estimate into affect"
                );
            }

            tracing::debug!(
                component = "CognitionEngine",
                mood = %update.mood_label,
                reasons = update.reasons.len(),
                "Pre-turn affect update complete"
            );
        }

        let recent_turns = ctx.recent_recall_turns();
        let embedding = ctx.query_embedding.ok_or_else(|| {
            CognitionError::MissingProvider("Query embedding required for cognitive recall".into())
        })?;
        let embedder = ctx.embedder.ok_or_else(|| {
            CognitionError::MissingProvider(
                "Embedding provider required for cognitive recall".into(),
            )
        })?;

        let recall_input = ExecuteRecallInput {
            store,
            character_id: ctx.character_id,
            user_id: ctx.user_name,
            user_input: ctx.user_input,
            recent_turns: &recent_turns,
            query_embedding: embedding,
            embedding_model: embedder.model_name(),
            affect: Some(&affect),
            card: Some(ctx.card),
        };

        let (recall_plan, recalled) = execute_hybrid_recall(ctx.config, &recall_input).await?;
        tracing::debug!(
            component = "CognitionEngine",
            event = "recall plan generated",
            session_id = %ctx.session_id,
            character_id = %ctx.character_id,
            user_id = %ctx.user_name,
            turn_id = ctx.history.len() + 1,
            result_limit = recall_plan.budget.result_limit,
            recalled_count = recalled.len(),
            "Cognitive recall plan generated"
        );

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

    /// Lightweight pre-turn for proactive speech: affect + commitments only (no recall / embedding).
    pub async fn before_proactive_turn(
        &self,
        ctx: TurnContext<'_>,
    ) -> Result<PreTurnOutput, CognitionError> {
        use crate::recall::{RecallBudgetHints, RecallPlan, RecallScopeFilter, RecallSearchHints};

        let recall_plan = RecallPlan {
            current_topic: "proactive".to_string(),
            semantic_queries: Vec::new(),
            episodic_queries: Vec::new(),
            required_kinds: Vec::new(),
            scope: RecallScopeFilter {
                character_id: ctx.character_id.to_string(),
                user_id: Some(ctx.user_name.to_string()),
            },
            budget: RecallBudgetHints { result_limit: 0 },
            search: RecallSearchHints {
                primary_query_text: String::new(),
                similarity_threshold: 0.0,
                min_score: 0.0,
                decay_half_life_days: 30.0,
                query_affect: None,
            },
        };

        let Some(store) = ctx.store else {
            return Ok(PreTurnOutput {
                recall_plan,
                affect: ene_core::AffectState::neutral(ctx.character_id),
                recalled: Vec::new(),
                commitments: Vec::new(),
                classifier_expression_hint: None,
            });
        };

        let affect = store
            .get_affect_state(ctx.character_id)
            .await
            .map_err(CognitionError::MemoryPort)?;

        let commitment_rows =
            match CommitmentLedger::list_active(store, ctx.character_id, Some(ctx.user_name), 16)
                .await
            {
                Ok(rows) => rows,
                Err(error) => {
                    tracing::warn!(
                        component = "CognitionEngine",
                        error = %error,
                        "Failed to list active commitments for proactive pre-turn"
                    );
                    vec![]
                }
            };
        let commitments = CommitmentLedger::active_prompt_candidates(&commitment_rows);

        Ok(PreTurnOutput {
            recall_plan,
            affect,
            recalled: Vec::new(),
            commitments,
            classifier_expression_hint: None,
        })
    }

    /// Persist affect state after pre-turn update (survives stream cancel/failure).
    pub async fn persist_affect_snapshot(
        store: &dyn MemoryPort,
        affect: &ene_core::AffectState,
    ) -> Result<(), CognitionError> {
        store
            .upsert_affect_state(affect)
            .await
            .map_err(CognitionError::MemoryPort)
    }

    /// Compose a sectioned prompt packet into LLM messages.
    ///
    /// Pass [`ComposePrefetch`] fields from a parallel pre-LLM phase to skip
    /// redundant style/scene fetches. Default prefetch fetches them here.
    pub async fn compose_prompt_packet(
        &self,
        ctx: TurnContext<'_>,
        pre: PreTurnOutput,
        prefetch: ComposePrefetch,
    ) -> Result<ComposedPrompt, CognitionError> {
        // Destructure to move the recalled/commitment vectors into the pack
        // input without cloning (#review M2). `recall_plan` is no longer needed
        // here now that packing budgets against the context window, not recall
        // hints.
        let PreTurnOutput {
            recall_plan: _,
            affect,
            recalled,
            commitments,
            classifier_expression_hint: _,
        } = pre;

        // Size the identity kernel from the model's available window rather
        // than a fixed token count, so larger models carry a fuller character
        // definition. Fall back to the conservative default window when
        // the caller does not know it (tests / legacy paths).
        let available_window = ctx.available_window.unwrap_or_else(|| {
            usize::try_from(ene_ai::DEFAULT_CONTEXT_WINDOW).unwrap_or(usize::MAX)
        });
        // Load user persona from global config so the identity kernel can expand
        // the `{{user_persona}}` CBS macro at compile time (#H-3).
        let user_persona = ene_config::get_global_config().user_persona;
        // Seed `{{pick}}` from the character+session so a trait chosen once
        // (hair colour, hometown, …) stays fixed across per-turn kernel
        // recompilations instead of re-rolling every turn.
        let pick_seed = Some(ene_config::session_pick_seed(&format!(
            "{}:{}",
            ctx.character_id, ctx.session_id
        )));
        let kernel = CharacterProcessor::compile_kernel(
            ctx.card,
            ctx.user_name,
            user_persona.as_ref(),
            pick_seed,
            available_window,
            ctx.config.resolved_language(),
        );

        let style_examples = if let Some(examples) = prefetch.style_examples {
            examples
        } else {
            CharacterProcessor::select_style_examples(
                ctx.card,
                ctx.user_name,
                ctx.user_input,
                ctx.history,
                ctx.store.map(|s| s as &dyn ene_core::MemoryPort),
                ctx.embedder,
                &ctx.config.character,
                2,
            )
            .await
        };

        // Affect is from before this user utterance (decay / prior-turn
        // classifier). Same-turn reaction belongs in the model's reply.
        let affect_summary = Some(format!(
            "mood={}; valence={:.2}; arousal={:.2}",
            affect.mood_label, affect.valence, affect.arousal
        ));

        let scene_summary = if let Some(prefetched) = prefetch.scene_summary {
            prefetched
        } else if let Some(store) = ctx.store {
            load_active_scene_summary(store, ctx.session_id)
                .await?
                .map(|s| s.text)
        } else {
            None
        };

        // `Some(None)` means the caller checked and there is no pending
        // interruption; `None` (legacy/test callers) injects nothing.
        let interruption_note = prefetch.interruption_note.flatten();

        let prompts = PromptLibrary::load(ctx.config.resolved_classifier_language());
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

        // Build author's note from character card data if present
        let authors_note = ctx
            .card
            .data
            .get_authors_note()
            .map(|(content, depth)| crate::character::AuthorsNote::new(content, depth));

        let pack_input = PackInput {
            platform_contract,
            identity_kernel: kernel,
            behavior_contract,
            style_examples,
            recalled,
            commitments,
            affect_summary,
            character_state_note: Some(prompts.system().affect_pre_utterance_note.clone()),
            scene_summary,
            history,
            output_contract: ctx.post_history_block.map(str::to_string),
            interruption_note,
            authors_note,
            user_persona,
            compression_pending: ctx.compression_pending,
            user_input: ctx.user_input.to_string(),
            lang: ctx.config.resolved_classifier_language().to_owned(),
            now: None,
        };

        // Budget the prompt against the model's effective context window
        //: `min(provider-advertised, operator override,
        // mind.context.max_prompt_tokens)` minus the response reserve and
        // safety margin. Packing then fills that window in priority order
        // rather than against per-section sub-budgets. Tests may inject a
        // deterministic budget via `packing_budget_override`.
        let budget = if let Some(tokens) = ctx.packing_budget_override {
            ContextBudget::with_capacity(tokens)
        } else {
            let ai_config = ene_config::get_global_config()
                .get_section::<ene_ai::AiConfig>()
                .unwrap_or_default();
            let chat_task = &ai_config.tasks.chat;
            let provider_advertised = ctx
                .llm_provider
                .as_deref()
                .and_then(ene_ai::LlmProvider::context_window);
            // Two operator shrinkage caps, combined as a min so configuration
            // can only ever narrow the model's stated window: the per-provider
            // `context_window` override and the mind-level `max_prompt_tokens`.
            // Either being `None` leaves the other (or the advertised
            // window) in force; both `None` defers entirely to the model, so
            // the prompt auto-follows the model's context size.
            let provider_override = ai_config
                .providers
                .get(&chat_task.provider)
                .and_then(|def| def.context_window);
            let max_prompt_cap = ctx
                .config
                .context
                .max_prompt_tokens
                .and_then(|tokens| u32::try_from(tokens).ok());
            let user_configured = match (provider_override, max_prompt_cap) {
                (Some(provider), Some(cap)) => Some(provider.min(cap)),
                (Some(provider), None) => Some(provider),
                (None, Some(cap)) => Some(cap),
                (None, None) => None,
            };
            let window = ene_ai::effective_window(
                provider_advertised,
                user_configured,
                chat_task.max_tokens,
                ene_ai::DEFAULT_SAFETY_MARGIN_FRACTION,
            );
            // Guard against the degenerate case where the response reserve eats
            // the entire window (e.g. no advertised window and max_tokens ==
            // DEFAULT_CONTEXT_WINDOW): guarantee at least half the effective
            // window for the prompt so packing always has room to work.
            let available = window.available.max(window.effective / 2);
            ContextBudget::with_capacity(usize::try_from(available).unwrap_or(usize::MAX))
        };
        let packed = pack_prompt(pack_input, &budget);
        let (messages, mut meta) = packed.packet.to_llm_messages();
        meta.dropped_sections.clone_from(&packed.meta.dropped);
        meta.history_messages_detached = packed.meta.history_messages_detached;
        meta.packed_tokens = packed.meta.packed_tokens;
        meta.injected_memory_ids
            .clone_from(&packed.meta.injected_memory_ids);

        // Bump access counters only for memories actually composed into the
        // message packet. The bump fires here, during prompt composition,
        // before the request is sent — "injected" means the memory survived
        // budget packing into the packet, not that the LLM has seen it. This
        // moved out of `execute_hybrid_recall` (which bumped every recalled
        // memory) so that being recalled-but-dropped no longer reinforces a
        // memory's score and shields it from forgetting.
        if let Some(store) = ctx.store {
            crate::recall::bump_injected_memory_access(store, &meta.injected_memory_ids).await;
        }
        tracing::debug!(
            component = "CognitionEngine",
            event = "prompt packet composed",
            session_id = %ctx.session_id,
            character_id = %ctx.character_id,
            user_id = %ctx.user_name,
            turn_id = ctx.history.len() + 1,
            message_count = messages.len(),
            packed_tokens = meta.packed_tokens,
            dropped_sections = meta.dropped_sections.len(),
            history_messages_detached = meta.history_messages_detached,
            injected_memory_ids = meta.injected_memory_ids.len(),
            "Prompt packet composed"
        );

        Ok(ComposedPrompt { messages, meta })
    }

    /// Post-turn: memory extraction, forgetting lifecycle, affect persistence.
    pub async fn after_turn(
        &self,
        store: &dyn MemoryPort,
        config: &MindConfig,
        input: PostTurnInput<'_>,
        providers: crate::memory_writer::MemoryWriteProviders<'_>,
    ) -> Result<(), CognitionError> {
        MemoryWriter::after_turn(store, config, input, providers).await
    }

    /// Synchronous post-turn finalize: persist affect state only.
    ///
    /// Forgetting runs on the deferred path via [`Self::spawn_deferred_memory_work`].
    pub async fn finalize_turn(
        &self,
        store: &dyn MemoryPort,
        config: &MindConfig,
        input: &PostTurnInput<'_>,
    ) -> Result<(), CognitionError> {
        MemoryWriter::finalize_turn(store, config, input).await
    }

    /// Spawn deferred post-turn memory extraction (LLM + arbiter) and forgetting lifecycle.
    ///
    /// On failure, enqueues a [`ene_core::PendingMemoryWrite`] for later retry.
    /// Returns the task handle so callers can track or abort it.
    pub fn spawn_deferred_memory_work(
        store: std::sync::Arc<dyn MemoryPort>,
        config: MindConfig,
        input: crate::lifecycle::OwnedPostTurnInput,
        llm: std::sync::Arc<dyn ene_ai::LlmProvider>,
        embedder: Option<std::sync::Arc<dyn ene_ai::EmbeddingProvider>>,
    ) -> tokio::task::JoinHandle<MemoryWriteOutcome> {
        tokio::spawn(
            async move {
                tracing::info!(
                    component = "MemoryWriter",
                    character_id = %input.character_id,
                    user_id = %input.user_id,
                    "Post-turn memory extraction and forgetting starting"
                );
                // Drain due retries before writing the new turn.
                Self::drain_pending_memory_writes(
                    store.as_ref(),
                    &config,
                    llm.as_ref(),
                    embedder.as_ref().map(std::convert::AsRef::as_ref),
                    3,
                )
                .await;

                let borrowed = input.as_borrowed();
                let providers = crate::memory_writer::MemoryWriteProviders {
                    llm: Some(llm.as_ref()),
                    embedder: embedder.as_ref().map(std::convert::AsRef::as_ref),
                };
                let result: Result<usize, CognitionError> = async {
                    let deferred =
                        MemoryWriter::write_memories(store.as_ref(), &config, &borrowed, providers)
                            .await?;
                    MemoryWriter::apply_forgetting(store.as_ref(), &config, &borrowed).await?;
                    Ok(deferred)
                }
                .await;
                match result {
                    Ok(deferred_candidates) => {
                        tracing::info!(
                            component = "MemoryWriter",
                            deferred_candidates,
                            "Post-turn memory extraction and forgetting completed"
                        );
                        MemoryWriteOutcome::Ok {
                            deferred_candidates,
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            component = "MemoryWriter",
                            error = %error,
                            "Post-turn memory extraction failed"
                        );
                        let message = error.to_string();
                        let payload = serde_json::to_string(&input).unwrap_or_else(|_| {
                            format!(
                                "{{\"character_id\":{},\"user_id\":{}}}",
                                serde_json::to_string(&input.character_id).unwrap_or_default(),
                                serde_json::to_string(&input.user_id).unwrap_or_default()
                            )
                        });
                        match store
                            .enqueue_pending_memory_write(
                                &input.character_id,
                                &input.user_id,
                                payload,
                                message.clone(),
                            )
                            .await
                        {
                            Ok(pending_id) => MemoryWriteOutcome::Failed {
                                message,
                                pending_id: Some(pending_id),
                                permanent: false,
                                character_id: input.character_id,
                            },
                            Err(enqueue_err) => {
                                tracing::error!(
                                    component = "MemoryWriter",
                                    error = %enqueue_err,
                                    "Failed to enqueue pending memory write"
                                );
                                MemoryWriteOutcome::Failed {
                                    message,
                                    pending_id: None,
                                    permanent: true,
                                    character_id: input.character_id,
                                }
                            }
                        }
                    }
                }
            }
            .instrument(tracing::info_span!("post_turn.memory")),
        )
    }

    /// Retry due pending memory writes from the persistent queue.
    pub async fn drain_pending_memory_writes(
        store: &dyn MemoryPort,
        config: &MindConfig,
        llm: &dyn ene_ai::LlmProvider,
        embedder: Option<&dyn ene_ai::EmbeddingProvider>,
        limit: usize,
    ) {
        let Ok(due) = store.take_due_pending_memory_writes(limit).await else {
            return;
        };
        for row in due {
            let Ok(input) =
                serde_json::from_str::<crate::lifecycle::OwnedPostTurnInput>(&row.payload_json)
            else {
                drop(
                    store
                        .fail_pending_memory_write(row.id, "invalid payload JSON".to_string())
                        .await,
                );
                continue;
            };
            let borrowed = input.as_borrowed();
            let providers = crate::memory_writer::MemoryWriteProviders {
                llm: Some(llm),
                embedder,
            };
            let result = async {
                MemoryWriter::write_memories(store, config, &borrowed, providers).await?;
                MemoryWriter::apply_forgetting(store, config, &borrowed).await
            }
            .await;
            match result {
                Ok(()) => {
                    drop(store.complete_pending_memory_write(row.id).await);
                    tracing::info!(
                        component = "MemoryWriter",
                        pending_id = row.id,
                        "Retried pending memory write succeeded"
                    );
                }
                Err(error) => {
                    match store
                        .fail_pending_memory_write(row.id, error.to_string())
                        .await
                    {
                        Ok(updated) => {
                            tracing::warn!(
                                component = "MemoryWriter",
                                pending_id = row.id,
                                status = updated.status.as_str(),
                                attempts = updated.attempts,
                                "Retried pending memory write failed"
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                component = "MemoryWriter",
                                pending_id = row.id,
                                error = %e,
                                "Failed to update pending memory write after retry"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Resolve the final character expression after an assistant turn.
    pub fn resolve_expression_turn(
        &self,
        config: &MindConfig,
        card: &CharacterCardV3,
        affect: &ene_core::AffectState,
        response_text: &str,
        llm_proposal: Option<&str>,
        explicit_proposal: bool,
        previous_expression: &str,
        elapsed_since_change: Option<Duration>,
    ) -> (crate::output::ExpressionDecision, ene_core::AffectState) {
        use ene_config::resolve_expressions;

        let expressions = resolve_expressions(card);
        let irritation_spike = affect.irritation >= 0.6;
        let input = crate::output::ExpressionInput {
            affect,
            available: &expressions,
            llm_proposal,
            explicit_proposal,
            previous_expression,
            elapsed_since_change,
            response_text,
            irritation_spike,
        };
        let decision = self.output.resolve(&config.emotion, &input);
        let mut updated = affect.clone();
        updated.last_expression.clone_from(&decision.expression);
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

fn pending_to_affect_proposal(
    pending: ene_core::PendingAffectProposal,
) -> crate::emotion::AffectProposal {
    crate::emotion::AffectProposal {
        user_emotion: pending.user_emotion,
        user_intent: pending.user_intent,
        valence: pending.valence,
        arousal: pending.arousal,
        irritation: pending.irritation,
        affinity: pending.affinity,
        recommended_expression: pending.recommended_expression,
        confidence: pending.confidence,
        reason: pending.reason,
    }
}

/// Count user messages in a history snapshot (current turn user is already included).
pub fn count_user_turns(history: &[crate::lifecycle::HistoryEntry]) -> i64 {
    let count = history
        .iter()
        .filter(|entry| entry.role == ene_ai::Role::User)
        .count();
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// Completed user-turn index for a post-turn classifier proposal.
///
/// Uses the stream-start history snapshot, which already includes the current
/// user message but not the assistant reply produced in this turn.
pub fn completed_user_turn_at_post_turn(history: &[crate::lifecycle::HistoryEntry]) -> i64 {
    count_user_turns(history)
}

/// Compact affect snapshot passed to the post-turn classifier prompt.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ClassifierAffectSnapshot {
    /// Pleasure–displeasure (-1.0 ..= 1.0).
    pub valence: f32,
    /// Excitement–calm (-1.0 ..= 1.0).
    pub arousal: f32,
    /// Irritation (0.0 ..= 1.0).
    pub irritation: f32,
    /// Affinity toward user (-1.0 ..= 1.0).
    pub affinity: f32,
    /// Human-readable mood label at turn start.
    pub mood_label: String,
}

impl ClassifierAffectSnapshot {
    /// Build a compact snapshot from persistent affect state.
    pub fn from_affect_state(affect: &ene_core::AffectState) -> Self {
        Self {
            valence: affect.valence,
            arousal: affect.arousal,
            irritation: affect.irritation,
            affinity: affect.affinity,
            mood_label: affect.mood_label.clone(),
        }
    }

    /// Serialize for the classifier user prompt.
    pub fn to_prompt_json(&self) -> String {
        serde_json::json!({
            "valence": self.valence,
            "arousal": self.arousal,
            "irritation": self.irritation,
            "affinity": self.affinity,
            "mood_label": self.mood_label,
        })
        .to_string()
    }
}

/// Input bundle for the post-turn affect classifier.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifierContext {
    /// Affect state at the start of the turn (before this exchange).
    pub current_affect: ClassifierAffectSnapshot,
    /// Recent conversation lines including the current assistant reply.
    pub conversation: String,
}

/// Build classifier input from history, current assistant output, and turn-start affect.
#[must_use]
pub fn build_classifier_context(
    history: &[crate::lifecycle::HistoryEntry],
    current_assistant: &str,
    affect: &ene_core::AffectState,
    max_turns: usize,
) -> ClassifierContext {
    let max_entries = max_turns.saturating_mul(2).max(2);
    let start = history.len().saturating_sub(max_entries);
    let mut lines = Vec::new();
    for entry in &history[start..] {
        let role = entry.role_label();
        if matches!(entry.role, ene_ai::Role::User | ene_ai::Role::Assistant) {
            lines.push(format!("{role}: {}", entry.content));
        }
    }
    if !current_assistant.trim().is_empty() {
        lines.push(format!("assistant: {current_assistant}"));
    }

    ClassifierContext {
        current_affect: ClassifierAffectSnapshot::from_affect_state(affect),
        conversation: lines.join("\n"),
    }
}

#[cfg(test)]
mod turn_id_tests {
    use super::*;
    use crate::lifecycle::HistoryEntry;

    #[test]
    fn count_user_turns_ignores_assistant_messages() {
        let history = vec![
            HistoryEntry {
                role: ene_ai::Role::User,
                content: "one".into(),
            },
            HistoryEntry {
                role: ene_ai::Role::Assistant,
                content: "reply".into(),
            },
            HistoryEntry {
                role: ene_ai::Role::User,
                content: "two".into(),
            },
        ];
        assert_eq!(count_user_turns(&history), 2);
        assert_eq!(completed_user_turn_at_post_turn(&history), 2);
    }

    #[test]
    fn build_classifier_context_includes_history_and_assistant() {
        use ene_core::AffectState;

        let history = vec![
            HistoryEntry {
                role: ene_ai::Role::User,
                content: "hi".into(),
            },
            HistoryEntry {
                role: ene_ai::Role::Assistant,
                content: "hello".into(),
            },
            HistoryEntry {
                role: ene_ai::Role::User,
                content: "thanks".into(),
            },
        ];
        let affect = AffectState::neutral("ene");
        let ctx = build_classifier_context(&history, "you're welcome", &affect, 8);
        assert!(ctx.conversation.contains("user: thanks"));
        assert!(ctx.conversation.contains("assistant: you're welcome"));
        assert!(ctx.current_affect.to_prompt_json().contains("valence"));
    }

    #[tokio::test]
    async fn compose_prompt_packet_uses_prefetch_without_store() {
        use crate::character::{StyleExample, StyleIntent};
        use crate::lifecycle::{ComposePrefetch, PreTurnOutput};
        use crate::recall::{RecallBudgetHints, RecallPlan, RecallScopeFilter, RecallSearchHints};
        use ene_config::CharacterCardV3;
        use ene_core::AffectState;

        let engine = CognitionEngine::new();
        let config = MindConfig::default();
        let card = CharacterCardV3::default();
        let pre = PreTurnOutput {
            recall_plan: RecallPlan {
                current_topic: "test".into(),
                semantic_queries: Vec::new(),
                episodic_queries: Vec::new(),
                required_kinds: Vec::new(),
                scope: RecallScopeFilter {
                    character_id: "ene".into(),
                    user_id: Some("user".into()),
                },
                budget: RecallBudgetHints { result_limit: 4 },
                search: RecallSearchHints {
                    primary_query_text: "hi".into(),
                    similarity_threshold: 0.0,
                    min_score: 0.0,
                    decay_half_life_days: 30.0,
                    query_affect: None,
                },
            },
            affect: AffectState::neutral("ene"),
            recalled: Vec::new(),
            commitments: Vec::new(),
            classifier_expression_hint: None,
        };
        let prefetch = ComposePrefetch {
            style_examples: Some(vec![StyleExample {
                text: "PREFETCHED_STYLE_MARKER".into(),
                intent: StyleIntent::Greeting,
            }]),
            scene_summary: Some(Some("PREFETCHED_SCENE_MARKER".into())),
            interruption_note: None,
        };
        let ctx = TurnContext {
            config: &config,
            card: &card,
            character_id: "ene",
            user_name: "user",
            session_id: "sess",
            user_input: "hi",
            history: &[],
            store: None,
            query_embedding: None,
            embedder: None,
            llm_provider: None,
            available_window: None,
            post_history_block: None,
            compression_pending: false,
            packing_budget_override: None,
        };
        let composed = engine
            .compose_prompt_packet(ctx, pre, prefetch)
            .await
            .expect("compose");
        let blob = format!("{composed:?}");
        assert!(
            blob.contains("PREFETCHED_STYLE_MARKER"),
            "prefetched style should appear in prompt: {blob}"
        );
        assert!(
            blob.contains("PREFETCHED_SCENE_MARKER"),
            "prefetched scene should appear in prompt: {blob}"
        );
        assert_eq!(composed.meta.style_example_count, 1);
        assert!(composed.meta.scene_summary_included);
    }
}
