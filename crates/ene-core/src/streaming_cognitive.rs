//! Cognitive runtime streaming path (#100).

use ene_cognition::memory_writer::candidate::TurnInput;
use ene_cognition::{CognitionConfig, CognitionEngine, HistoryEntry, PostTurnInput, TurnContext};
use ene_provider::LlmToolCallChunk;
use tokio_stream::StreamExt;

use crate::handle::{EneEvent, TerminalReason};
use crate::streaming::{
    PHASE_CONTEXT_SEARCH, PHASE_EMBEDDING, PHASE_PROMPT_BUILDING, StreamContext,
    accumulate_tool_calls, emit_terminal, finalize_tool_calls, perform_tool_executions,
    select_relevant_tools,
};

fn build_turn_context<'a>(
    cognition: &'a CognitionConfig,
    card: &'a ene_config::CharacterCardV3,
    card_name: &'a str,
    user_name: &'a str,
    session_id: &'a str,
    user_input: &'a str,
    history: &'a [HistoryEntry],
    mem_store: &'a Option<std::sync::Arc<ene_memory::MemoryStore>>,
    query_embedding: Option<&'a [f32]>,
    embedder: Option<&'a std::sync::Arc<dyn ene_provider::EmbeddingProvider>>,
    provider: &std::sync::Arc<dyn ene_provider::LlmProvider>,
) -> TurnContext<'a> {
    TurnContext {
        config: cognition,
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
    }
}

/// Run the streaming loop using the cognitive runtime lifecycle.
pub(crate) async fn run_stream_cognitive(ctx: StreamContext) -> ene_session::ConversationSession {
    let StreamContext {
        config,
        mut session,
        user_input,
        embedder,
        registry,
        tool_rag,
        provider,
        event_tx,
        cancel_token,
        pending_permissions,
        pending_user_inputs,
        terminal_emitted,
    } = ctx;

    session.reset_display_buffer();

    let cognition = config.get_section::<CognitionConfig>().unwrap_or_default();
    let engine = CognitionEngine::new();

    let tool_config = config
        .get_section::<ene_tool_host::ToolConfig>()
        .unwrap_or_default();
    let tool_calling_enabled = tool_config.enabled;

    let mem_config = config
        .get_section::<ene_memory::MemoryConfig>()
        .unwrap_or_default();

    let Some(card) = session.character_card.clone() else {
        emit_terminal(
            &event_tx,
            &terminal_emitted,
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

    let _ = event_tx.send(EneEvent::PipelinePhase {
        phase: PHASE_EMBEDDING.to_string(),
    });

    let query_embedding = if let Some(emb_prov) = &embedder {
        match emb_prov.embed_query(&user_input).await {
            Ok(emb) => {
                session.set_pending_embedding(emb.clone());
                session.set_last_input_embedding(emb.clone());
                Some(emb)
            }
            Err(e) => {
                emit_terminal(
                    &event_tx,
                    &terminal_emitted,
                    TerminalReason::Failed {
                        message: format!("Embedding failed: {e}"),
                    },
                );
                return session;
            }
        }
    } else {
        emit_terminal(
            &event_tx,
            &terminal_emitted,
            TerminalReason::Failed {
                message: "Embedding provider required for cognitive path".into(),
            },
        );
        return session;
    };

    let history: Vec<HistoryEntry> = session
        .history()
        .iter()
        .map(|(role, content)| HistoryEntry {
            role: match role {
                ene_provider::Role::User => "user".into(),
                ene_provider::Role::Assistant => "assistant".into(),
                ene_provider::Role::System => "system".into(),
            },
            content: content.clone(),
        })
        .collect();

    let _ = event_tx.send(EneEvent::PipelinePhase {
        phase: PHASE_CONTEXT_SEARCH.to_string(),
    });

    let turn_ctx = build_turn_context(
        &cognition,
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
                TerminalReason::Failed {
                    message: e.to_string(),
                },
            );
            return session;
        }
    };

    if mem_config.enabled
        && let Some(store) = &mem_store
    {
        ene_memory::MemoryStore::spawn_insert_log(
            store,
            session_id.as_str(),
            &card_name,
            "user",
            &user_input,
        );
    }

    let _ = event_tx.send(EneEvent::PipelinePhase {
        phase: PHASE_PROMPT_BUILDING.to_string(),
    });

    let compose_ctx = build_turn_context(
        &cognition,
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
    );

    let composed = match engine.compose_prompt_packet(compose_ctx, &pre_turn) {
        Ok(v) => v,
        Err(e) => {
            emit_terminal(
                &event_tx,
                &terminal_emitted,
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

    loop {
        if cancel_token.is_cancelled() {
            emit_terminal(&event_tx, &terminal_emitted, TerminalReason::Cancelled);
            return session;
        }

        if round >= max_rounds {
            emit_terminal(
                &event_tx,
                &terminal_emitted,
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
                    TerminalReason::Failed {
                        message: e.to_string(),
                    },
                );
                return session;
            }
        };

        let mut current_tool_calls: Vec<LlmToolCallChunk> = Vec::new();
        let mut assistant_content = String::new();

        while let Some(chunk_res) = stream.next().await {
            if cancel_token.is_cancelled() {
                emit_terminal(&event_tx, &terminal_emitted, TerminalReason::Cancelled);
                return session;
            }

            match chunk_res {
                Ok(chunk) => {
                    if let Some(content_delta) = &chunk.text_delta {
                        assistant_content.push_str(content_delta);
                        let (text_deltas, special_tokens) = session.process_delta(content_delta);
                        for text in text_deltas {
                            let _ = event_tx.send(EneEvent::TextDelta { delta: text });
                        }
                        for token in special_tokens {
                            let _ = event_tx.send(EneEvent::SpecialToken { token });
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
                ene_memory::MemoryStore::spawn_insert_log(
                    store,
                    session_id.as_str(),
                    &card_name,
                    "assistant",
                    &assistant_content,
                );
            }

            if let Some(store) = mem_store.as_deref() {
                let post = PostTurnInput {
                    turn: TurnInput {
                        user_message: &user_input,
                        assistant_message: Some(&assistant_content),
                        tool_results: &[],
                    },
                    affect: pre_turn.affect.clone(),
                    character_id: &card_name,
                    user_id: &user_name,
                };
                if cognition.memory.write_every_turn
                    && let Err(error) = ene_cognition::memory_writer::MemoryWriter::write_memories(
                        store, &cognition, &post,
                    )
                    .await
                {
                    tracing::warn!(
                        component = "CognitionEngine",
                        error = %error,
                        "Post-turn memory write failed"
                    );
                }
                if let Err(error) = ene_cognition::memory_writer::MemoryWriter::finalize_turn(
                    store, &cognition, &post,
                )
                .await
                {
                    tracing::warn!(
                        component = "CognitionEngine",
                        error = %error,
                        "Post-turn finalize failed"
                    );
                }
            }

            session.finalize_response();
            session.record_assistant_response();
            emit_terminal(&event_tx, &terminal_emitted, TerminalReason::Done);
            return session;
        }

        let tool_calls = finalize_tool_calls(current_tool_calls);
        let tx_messages = perform_tool_executions(
            registry.as_ref(),
            session_id_for_tools.as_str(),
            tool_calls,
            &assistant_content,
            &event_tx,
            &pending_permissions,
            &pending_user_inputs,
            tool_config.timeout_ms,
        )
        .await;

        match tx_messages {
            Ok(msgs) => {
                messages.extend(msgs);
                round += 1;
            }
            Err(e) => {
                emit_terminal(
                    &event_tx,
                    &terminal_emitted,
                    TerminalReason::Failed {
                        message: e.to_string(),
                    },
                );
                return session;
            }
        }
    }
}
