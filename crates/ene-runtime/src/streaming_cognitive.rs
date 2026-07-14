//! Cognitive runtime streaming path (#100).

use ene_ai::LlmToolCallChunk;
use ene_config::PromptLibrary;
use ene_mind::memory_writer::candidate::{ToolResultSummary, TurnInput};
use ene_mind::{CognitionEngine, EngineMode, HistoryEntry, MindConfig, PostTurnInput, TurnContext};
use tokio_stream::StreamExt;

use crate::diagnostics::{DiagnosticEvent, emit_diag};
use crate::empty_response_log::{EmptyResponseContext, log_empty_response_if_needed};
use crate::handle::{EneEvent, TerminalReason};
use crate::message_builder::build_cognitive_output_contract;
use crate::streaming::{
    PHASE_CONTEXT_SEARCH, PHASE_EMBEDDING, PHASE_PROMPT_BUILDING, StreamContext,
    accumulate_tool_calls, emit_terminal, finalize_tool_calls, perform_tool_executions,
    select_relevant_tools,
};
use ene_mind::{CueSource, PerfKind, PerformanceArbiter, PerformanceCue, cue_source_priority};

#[expect(clippy::ref_option)]
fn build_turn_context<'a>(
    mind: &'a MindConfig,
    card: &'a ene_config::CharacterCardV3,
    card_name: &'a str,
    user_name: &'a str,
    session_id: &'a str,
    user_input: &'a str,
    history: &'a [HistoryEntry],
    mem_store: &'a Option<std::sync::Arc<ene_store::MemoryStore>>,
    query_embedding: Option<&'a [f32]>,
    embedder: Option<&'a std::sync::Arc<dyn ene_ai::EmbeddingProvider>>,
    provider: &std::sync::Arc<dyn ene_ai::LlmProvider>,
    post_history_block: Option<&'a str>,
) -> TurnContext<'a> {
    TurnContext {
        config: mind,
        card,
        character_id: card_name,
        user_name,
        session_id,
        user_input,
        history,
        store: mem_store.as_deref(),
        query_embedding,
        embedder,
        llm_provider: Some(provider.clone()),
        post_history_block,
    }
}

/// Run the streaming loop using the cognitive runtime lifecycle.
pub async fn run_stream_cognitive(ctx: StreamContext) -> ene_mind::ConversationSession {
    let StreamContext {
        config,
        mut session,
        user_input,
        embedder,
        registry,
        tool_rag,
        provider,
        event_tx,
        diag_tx,
        cancel_token,
        pending_permissions,
        pending_user_inputs,
        terminal_emitted,
        turn,
        classifier_tx,
    } = ctx;

    session.reset_display_buffer();

    let mind = config.get_section::<MindConfig>().unwrap_or_default();
    let engine = CognitionEngine::new();

    let tool_config = config
        .get_section::<ene_tool_host::ToolConfig>()
        .unwrap_or_default();
    let tool_calling_enabled = tool_config.enabled;

    let mem_config = config
        .get_section::<ene_store::StoreConfig>()
        .unwrap_or_default();

    let Some(card) = session.character_card.clone() else {
        emit_terminal(
            &event_tx,
            &terminal_emitted,
            &turn,
            TerminalReason::Failed {
                message: "No character card loaded".into(),
            },
        );
        return session;
    };

    let card_name = session.card_name().to_string();
    let user_name = config.user_name.clone();
    let session_id = session.memory.session_id.clone();
    let mem_store = session.memory.memory_store.clone();

    emit_diag(
        &diag_tx,
        DiagnosticEvent::PipelinePhase {
            turn: turn.clone(),
            phase: PHASE_EMBEDDING.to_string(),
        },
    );

    // Embeddings are optional: without an embedder (typical when store is off),
    // skip recall/write that needs vectors and continue with chat + tools.
    let query_embedding = if let Some(emb_prov) = &embedder {
        match ene_ai::embed_query(emb_prov.as_ref(), &user_input).await {
            Ok(emb) => {
                session.set_pending_embedding(emb.clone());
                session.set_last_input_embedding(emb.clone());
                Some(emb)
            }
            Err(e) => {
                emit_terminal(
                    &event_tx,
                    &terminal_emitted,
                    &turn,
                    TerminalReason::Failed {
                        message: format!("Embedding failed: {e}"),
                    },
                );
                return session;
            }
        }
    } else {
        None
    };

    let history: Vec<HistoryEntry> = session.history().to_vec();

    let prompts = PromptLibrary::load(&mind.emotion.classifier_language);
    let post_history_phi =
        build_cognitive_output_contract(&card, &prompts, mind.emotion.enabled, &user_name);

    emit_diag(
        &diag_tx,
        DiagnosticEvent::PipelinePhase {
            turn: turn.clone(),
            phase: PHASE_CONTEXT_SEARCH.to_string(),
        },
    );

    if let (Some(_store), Some(embedder)) = (mem_store.as_deref(), embedder.as_ref()) {
        let sync_ctx = build_turn_context(
            &mind,
            &card,
            &card_name,
            &user_name,
            session_id.as_str(),
            &user_input,
            &history,
            &mem_store,
            query_embedding.as_deref(),
            Some(embedder),
            &provider,
            post_history_phi.as_deref(),
        );
        match engine
            .sync_character_memories(sync_ctx, session.memory.ccv3_memory_hash)
            .await
        {
            Ok((report, hash)) => {
                session.memory.ccv3_memory_hash = Some(hash);
                tracing::debug!(
                    component = "CognitionEngine",
                    skipped = report.skipped,
                    lorebook_inserted = report.lorebook_inserted,
                    lorebook_updated = report.lorebook_updated,
                    style_inserted = report.style_inserted,
                    style_updated = report.style_updated,
                    archived = report.archived,
                    "CCv3 character memory sync complete"
                );
            }
            Err(error) => {
                tracing::warn!(
                    component = "CognitionEngine",
                    error = %error,
                    "CCv3 character memory sync failed; continuing turn"
                );
            }
        }
    }

    let turn_ctx = build_turn_context(
        &mind,
        &card,
        &card_name,
        &user_name,
        session_id.as_str(),
        &user_input,
        &history,
        &mem_store,
        query_embedding.as_deref(),
        embedder.as_ref(),
        &provider,
        post_history_phi.as_deref(),
    );

    let (pre_turn_result, tools) = tokio::join!(
        engine.before_turn(turn_ctx),
        select_relevant_tools(
            registry.as_ref(),
            tool_rag.as_deref(),
            &user_input,
            query_embedding.as_deref(),
            tool_calling_enabled,
        )
    );

    let pre_turn = match pre_turn_result {
        Ok(v) => v,
        Err(e) => {
            emit_terminal(
                &event_tx,
                &terminal_emitted,
                &turn,
                TerminalReason::Failed {
                    message: e.to_string(),
                },
            );
            return session;
        }
    };

    let mut turn_affect = pre_turn.affect.clone();

    if mind.emotion.enabled
        && let Some(store) = mem_store.as_deref()
        && let Err(error) = CognitionEngine::persist_affect_snapshot(store, &turn_affect).await
    {
        tracing::warn!(
            component = "CognitionEngine",
            error = %error,
            "Failed to persist pre-turn affect snapshot"
        );
    }

    if mem_config.enabled
        && let Some(store) = &mem_store
    {
        ene_store::MemoryStore::spawn_insert_log(
            store,
            session_id.as_str(),
            &card_name,
            "user",
            &user_input,
        );
    }

    emit_diag(
        &diag_tx,
        DiagnosticEvent::PipelinePhase {
            turn: turn.clone(),
            phase: PHASE_PROMPT_BUILDING.to_string(),
        },
    );

    let compose_ctx = build_turn_context(
        &mind,
        &card,
        &card_name,
        &user_name,
        session_id.as_str(),
        &user_input,
        &history,
        &mem_store,
        query_embedding.as_deref(),
        embedder.as_ref(),
        &provider,
        post_history_phi.as_deref(),
    );

    let composed = match engine.compose_prompt_packet(compose_ctx, &pre_turn).await {
        Ok(v) => v,
        Err(e) => {
            emit_terminal(
                &event_tx,
                &terminal_emitted,
                &turn,
                TerminalReason::Failed {
                    message: e.to_string(),
                },
            );
            return session;
        }
    };

    let mut messages = composed.messages;
    let max_rounds = tool_config.max_rounds;
    let session_id_for_tools = session.memory.session_id.clone();
    let mut round = 0usize;
    let mut turn_tool_results: Vec<ToolResultSummary> = Vec::new();

    loop {
        if cancel_token.is_cancelled() {
            emit_terminal(
                &event_tx,
                &terminal_emitted,
                &turn,
                TerminalReason::Cancelled,
            );
            return session;
        }

        if round >= max_rounds {
            emit_terminal(
                &event_tx,
                &terminal_emitted,
                &turn,
                TerminalReason::Failed {
                    message: "Max tool call rounds exceeded".into(),
                },
            );
            return session;
        }

        let mut stream = match provider.create_chat_stream(&messages, &tools).await {
            Ok(s) => s,
            Err(e) => {
                emit_terminal(
                    &event_tx,
                    &terminal_emitted,
                    &turn,
                    TerminalReason::Failed {
                        message: e.to_string(),
                    },
                );
                return session;
            }
        };

        let mut current_tool_calls: Vec<LlmToolCallChunk> = Vec::new();
        let mut assistant_content = String::new();
        let mut accumulated_emotion_tokens: Vec<String> = Vec::new();
        let mut perf_arbiter = PerformanceArbiter::default();
        let suppress_stream_tokens =
            mind.emotion.enabled && mind.emotion.llm_expression_is_advisory;

        while let Some(chunk_res) = stream.next().await {
            if cancel_token.is_cancelled() {
                emit_terminal(
                    &event_tx,
                    &terminal_emitted,
                    &turn,
                    TerminalReason::Cancelled,
                );
                return session;
            }

            match chunk_res {
                Ok(chunk) => {
                    if let Some(content_delta) = &chunk.text_delta {
                        assistant_content.push_str(content_delta);
                        let (text_deltas, special_tokens) = session.process_delta(content_delta);
                        for text in text_deltas {
                            let _ = event_tx.send(EneEvent::TextDelta {
                                turn: turn.clone(),
                                delta: text,
                            });
                        }
                        for token in special_tokens {
                            accumulated_emotion_tokens.push(token.clone());

                            if !suppress_stream_tokens {
                                // Try `<|perf:…|>` first (#128).
                                if let Some(cue) = ene_mind::parse_performance_marker(&token) {
                                    let source = match cue.kind {
                                        PerfKind::Expression
                                        | PerfKind::Motion
                                        | PerfKind::LookAt => {
                                            if mind.emotion.llm_expression_is_advisory {
                                                CueSource::LlmAdvisory
                                            } else {
                                                CueSource::LlmCommand
                                            }
                                        }
                                        PerfKind::Cancel => CueSource::LlmCommand,
                                    };
                                    perf_arbiter.accept(cue.clone(), source);
                                } else if let Some(name) =
                                    ene_mind::extract_emotion_from_token(&token)
                                {
                                    // Backward compat: `<|emo:NAME|>`.
                                    let source = if mind.emotion.llm_expression_is_advisory {
                                        CueSource::LlmAdvisory
                                    } else {
                                        CueSource::LlmCommand
                                    };
                                    let cue = PerformanceCue::expression(name);
                                    perf_arbiter.accept(cue.clone(), source);
                                }
                            }
                        }
                    }
                    if let Some(tool_calls_delta) = &chunk.tool_calls_delta {
                        accumulate_tool_calls(&mut current_tool_calls, tool_calls_delta);
                    }
                }
                Err(e) => {
                    emit_terminal(
                        &event_tx,
                        &terminal_emitted,
                        &turn,
                        TerminalReason::Failed {
                            message: e.to_string(),
                        },
                    );
                    return session;
                }
            }
        }

        if current_tool_calls.is_empty() {
            if !assistant_content.is_empty()
                && let Some(store) = &mem_store
            {
                ene_store::MemoryStore::spawn_insert_log(
                    store,
                    session_id.as_str(),
                    &card_name,
                    "assistant",
                    &assistant_content,
                );
            }

            if mind.emotion.enabled {
                let llm_proposal = accumulated_emotion_tokens
                    .iter()
                    .find_map(|token| ene_mind::extract_emotion_from_token(token))
                    .or_else(|| pre_turn.classifier_expression_hint.clone());
                let (previous_expression, elapsed_since_change) =
                    session.expression_context(&turn_affect);
                let (decision, updated_affect) = engine.resolve_expression_turn(
                    &mind,
                    &card,
                    &turn_affect,
                    &assistant_content,
                    llm_proposal.as_deref(),
                    previous_expression.as_ref(),
                    elapsed_since_change,
                );
                turn_affect = updated_affect;
                session.record_expression_change(&decision.expression);
                tracing::debug!(
                    component = "CognitionEngine",
                    event = "expression selected",
                    session_id = %session_id,
                    character_id = %card_name,
                    user_id = %user_name,
                    turn_id = history.len() + 1,
                    expression = %decision.expression,
                    source = %decision.source.as_str(),
                    "Expression arbiter selected expression"
                );
                // Feed the final expression decision into arbiter
                // and consolidate with mid-turn cue accumulations (#129).
                let expr_cue = PerformanceCue::expression(decision.expression.clone());
                let expr_source = CueSource::from(decision.source);
                perf_arbiter.accept(expr_cue, expr_source);
                // Fill any gaps with the affect-derived default.
                perf_arbiter.set_affect_default(&turn_affect);
                let resolved = perf_arbiter.resolve();
                if !resolved.is_empty() {
                    let (cues, sources): (Vec<_>, Vec<_>) = resolved.into_iter().unzip();
                    let primary_source =
                        sources.into_iter().max_by_key(|s| cue_source_priority(*s));
                    if let Some(source) = primary_source {
                        let _ = event_tx.send(EneEvent::Performance {
                            turn: turn.clone(),
                            cues,
                            source,
                        });
                    }
                }
            }

            if let Some(store) = mem_store.as_deref() {
                let post = PostTurnInput {
                    turn: TurnInput {
                        user_message: &user_input,
                        assistant_message: Some(&assistant_content),
                        tool_results: &turn_tool_results,
                    },
                    affect: turn_affect,
                    character_id: &card_name,
                    user_id: &user_name,
                };
                // Sole runtime write entry: CognitionEngine::after_turn (#121).
                // write_every_turn is gated inside mind's after_turn path.
                if let Err(error) = engine
                    .after_turn(
                        store,
                        &mind,
                        post,
                        ene_mind::MemoryWriteProviders {
                            llm: Some(provider.as_ref()),
                            embedder: embedder.as_ref().map(std::convert::AsRef::as_ref),
                        },
                    )
                    .await
                {
                    tracing::warn!(
                        component = "CognitionEngine",
                        error = %error,
                        "Post-turn after_turn failed"
                    );
                }
            }

            log_empty_response_if_needed(&EmptyResponseContext {
                pipeline: "cognitive",
                config: &config,
                session_id: session_id.as_str(),
                character_id: &card_name,
                user_input: &user_input,
                round,
                tool_count: tools.len(),
                messages: &messages,
                raw_assistant_content: &assistant_content,
                display_buffer: &session.display.display_buffer,
                emotion_tokens: &accumulated_emotion_tokens,
                suppress_stream_tokens,
                prompt_meta: Some(&composed.meta),
            });

            // Finalize and emit Terminal before the affect classifier so the
            // chat UI is not blocked for up to classifier_timeout_secs, and so
            // a cancel abort cannot drop an already-streamed assistant reply
            // from history.
            session.finalize_response();
            session.record_assistant_response();
            emit_terminal(&event_tx, &terminal_emitted, &turn, TerminalReason::Done);

            if let Some(classifier_store) = mem_store.clone()
                && mind.emotion.enabled
                && matches!(mind.emotion.engine, EngineMode::Llm | EngineMode::Hybrid)
                && !assistant_content.trim().is_empty()
            {
                let classifier_config = config.clone();
                let classifier_model = mind.emotion.classifier_model.clone();
                let classifier_max_tokens = mind.emotion.classifier_max_tokens;
                let classifier_lang = mind.emotion.classifier_language.clone();
                let classifier_timeout_secs = mind.emotion.classifier_timeout_secs;
                let classifier_character_id = card_name.clone();
                let classifier_user_id = user_name.clone();
                let classifier_turn_id =
                    ene_mind::engine::completed_user_turn_at_post_turn(&history);
                let classifier_context = ene_mind::engine::build_classifier_context(
                    &history,
                    &assistant_content,
                    &pre_turn.affect,
                    mind.context.recent_turns,
                );

                // Fire-and-forget: must not delay Terminal (already emitted).
                // The JoinHandle is sent to the actor for lifecycle management.
                let classifier_handle = tokio::spawn(async move {
                    tracing::info!(
                        component = "EmotionEngine",
                        turn_id = classifier_turn_id,
                        "Starting post-turn affect classifier"
                    );
                    let started = std::time::Instant::now();
                    match ene_mind::emotion::classifier::classify_for_config(
                        &classifier_config,
                        classifier_model.as_deref(),
                        classifier_max_tokens,
                        &classifier_context,
                        classifier_timeout_secs,
                        &classifier_lang,
                    )
                    .await
                    {
                        Ok(proposal) => {
                            let pending = ene_store::PendingAffectProposal {
                                character_id: classifier_character_id,
                                user_id: classifier_user_id,
                                source_turn_id: classifier_turn_id,
                                user_emotion: proposal.user_emotion,
                                user_intent: proposal.user_intent,
                                valence: proposal.valence,
                                arousal: proposal.arousal,
                                irritation: proposal.irritation,
                                affinity: proposal.affinity,
                                recommended_expression: proposal.recommended_expression,
                                confidence: proposal.confidence,
                                reason: proposal.reason,
                                created_at: chrono::Utc::now(),
                            };
                            if let Err(error) = classifier_store
                                .upsert_pending_affect_proposal(&pending)
                                .await
                            {
                                tracing::warn!(
                                    component = "EmotionEngine",
                                    error = %error,
                                    turn_id = classifier_turn_id,
                                    "Failed to persist post-turn classifier proposal"
                                );
                            } else {
                                tracing::info!(
                                    component = "EmotionEngine",
                                    turn_id = classifier_turn_id,
                                    elapsed_ms = started.elapsed().as_millis(),
                                    user_emotion = %pending.user_emotion,
                                    user_intent = %pending.user_intent,
                                    estimated_valence = pending.valence,
                                    estimated_arousal = pending.arousal,
                                    estimated_irritation = pending.irritation,
                                    estimated_affinity = pending.affinity,
                                    recommended_expression = %pending.recommended_expression,
                                    confidence = pending.confidence,
                                    reason = %pending.reason,
                                    "Post-turn affect classifier estimate complete"
                                );
                            }
                        }
                        Err(error) => {
                            tracing::warn!(
                                component = "EmotionEngine",
                                error = %error,
                                failure_reason =
                                    ene_mind::emotion::classifier::classify_failure_reason(&error),
                                turn_id = classifier_turn_id,
                                elapsed_ms = started.elapsed().as_millis(),
                                "Post-turn affect classifier failed"
                            );
                        }
                    }
                });
                // Send handle to actor for lifecycle management.
                // A send failure means the actor has shut down; the
                // classifier task runs as a detached orphan until
                // completion, which is acceptable at shutdown.
                let _ = classifier_tx.send(classifier_handle);
            }

            return session;
        }

        let tool_calls = finalize_tool_calls(current_tool_calls);
        let tx_messages = perform_tool_executions(
            registry.as_ref(),
            session_id_for_tools.as_str(),
            tool_calls,
            &assistant_content,
            &event_tx,
            &turn,
            &pending_permissions,
            &pending_user_inputs,
            tool_config.timeout_ms,
            mind.memory.tool_grounding.max_summary_chars,
        )
        .await;

        match tx_messages {
            Ok(output) => {
                messages.extend(output.messages);
                turn_tool_results.extend(output.summaries);
                round += 1;
            }
            Err(e) => {
                emit_terminal(
                    &event_tx,
                    &terminal_emitted,
                    &turn,
                    TerminalReason::Failed {
                        message: e.to_string(),
                    },
                );
                return session;
            }
        }
    }
}
