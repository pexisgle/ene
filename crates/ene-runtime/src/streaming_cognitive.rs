//! Cognitive runtime streaming path (#100).

use ene_ai::LlmToolCallChunk;
use ene_config::PromptLibrary;
use ene_mind::memory_writer::candidate::{ToolResultSummary, TurnInput};
use ene_mind::{
    CognitionEngine, ComposePrefetch, HistoryEntry, MindConfig, OwnedPostTurnInput, OwnedTurnInput,
    PostTurnInput, TurnContext, character::CharacterProcessor, character::compute_card_memory_hash,
    interruption_note, load_active_scene_summary,
};
use tokio_stream::StreamExt;

use std::sync::Arc;

use crate::diagnostics::{DiagnosticEvent, emit_diag};
use crate::empty_response_log::{EmptyResponseContext, log_empty_response_if_needed};
use crate::handle::{EneEvent, TerminalReason};
use crate::message_builder::build_cognitive_output_contract;
use crate::streaming::{
    PHASE_CONTEXT_SEARCH, PHASE_EMBEDDING, PHASE_PROMPT_BUILDING, StreamContext, StreamOutcome,
    accumulate_tool_calls, finalize_tool_calls, perform_tool_executions, select_relevant_tools,
    stream_finish,
};
use crate::types::TurnOrigin;
use ene_ai::{LlmMessage, UserMessagePart};
use ene_mind::{
    CueSource, PerfKind, PerformanceArbiter, PerformanceCue, cue_source_priority, strip_markers,
};
use tracing::Instrument;

/// Maximum sentence buffer length before forcing a TTS flush (chars).
const TTS_MAX_BUFFER_CHARS: usize = 100;

/// Finds the index just past the first sentence boundary in `buf`.
///
/// A boundary is:
/// - A CJK sentence-ending punctuation character (`。！？`, U+3002 / U+FF01 /
///   U+FF1F) unconditionally — Japanese text has no trailing whitespace after
///   these marks.
/// - An ASCII sentence-ending punctuation character (`.!?`) followed by
///   whitespace or end-of-buffer.
/// - The buffer exceeding [`TTS_MAX_BUFFER_CHARS`] characters.
///
/// `char_count` is the caller-maintained character count of `buf`, tracked
/// incrementally to avoid an O(n) rescan on every streaming delta (#L8).
/// Returns `None` when no boundary is found.
fn find_tts_sentence_boundary(buf: &str, char_count: usize) -> Option<usize> {
    if char_count > TTS_MAX_BUFFER_CHARS {
        return Some(buf.len());
    }
    let chars: Vec<(usize, char)> = buf.char_indices().collect();
    for (i, &(_, ch)) in chars.iter().enumerate() {
        match ch {
            // CJK punctuation: boundary unconditionally (no trailing space in Japanese).
            '。' | '！' | '？' => {
                let end = chars.get(i + 1).map_or(buf.len(), |&(offset, _)| offset);
                return Some(end);
            }
            // ASCII punctuation: boundary only at end or before whitespace.
            '.' | '!' | '?' => {
                let next = chars.get(i + 1).map(|&(_, c)| c);
                if next.is_none_or(char::is_whitespace) {
                    let end = chars.get(i + 1).map_or(buf.len(), |&(offset, _)| offset);
                    return Some(end);
                }
            }
            _ => {}
        }
    }
    None
}

#[expect(
    clippy::ref_option,
    reason = "optional callback kept as Option reference for zero-copy dispatch"
)]
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

/// Finish a cancelled turn: record the partial response as an interruption so
/// the next turn can acknowledge/resume it, then emit `TerminalReason::Cancelled` (#206).
///
/// When the trimmed partial text is empty (e.g. cancel arrived before any
/// visible text), the interruption record is skipped entirely — an empty
/// snapshot carries no useful context for the next turn (#M9).
fn finish_cancelled(
    mut session: ene_mind::ConversationSession,
    event_tx: &tokio::sync::broadcast::Sender<EneEvent>,
    terminal_emitted: &std::sync::atomic::AtomicBool,
    turn: &crate::types::TurnId,
    origin: TurnOrigin,
    spoken_text: &str,
) -> StreamOutcome {
    if !spoken_text.trim().is_empty() {
        let spoken_chars = spoken_text.chars().count();
        session.mark_interrupted(&turn.to_string(), spoken_text, spoken_chars);
    }
    stream_finish(
        session,
        event_tx,
        terminal_emitted,
        turn,
        origin,
        TerminalReason::Cancelled,
    )
}

/// Spawn deferred memory work for an interrupted (barge-in / cancelled) turn (#M7).
///
/// Tags the turn as `interrupted` and includes the partial `spoken_text` so
/// downstream memory extraction can distinguish partial episodes.
fn spawn_interrupted_memory_work(
    mem_store: Option<&Arc<ene_store::MemoryStore>>,
    mind: &MindConfig,
    provider: &Arc<dyn ene_ai::LlmProvider>,
    embedder: Option<&Arc<dyn ene_ai::EmbeddingProvider>>,
    memory_writer_tx: &tokio::sync::mpsc::UnboundedSender<
        tokio::task::JoinHandle<ene_mind::MemoryWriteOutcome>,
    >,
    user_input: &str,
    spoken_text: &str,
    turn_tool_results: &[ToolResultSummary],
    turn_affect: &ene_store::AffectState,
    card_name: &str,
    user_name: &str,
) {
    let Some(store) = mem_store.cloned() else {
        return;
    };
    let deferred_input = OwnedPostTurnInput {
        turn: OwnedTurnInput {
            user_message: user_input.to_string(),
            assistant_message: Some(spoken_text.to_string()),
            tool_results: turn_tool_results.to_vec(),
        },
        affect: turn_affect.clone(),
        character_id: card_name.to_string(),
        user_id: user_name.to_string(),
        interrupted: true,
        spoken_text: Some(spoken_text.to_string()),
    };
    let handle = CognitionEngine::spawn_deferred_memory_work(
        store,
        mind.clone(),
        deferred_input,
        provider.clone(),
        embedder.cloned(),
    );
    let _ = memory_writer_tx.send(handle);
}

/// Mutates `messages` in-place for a proactive turn: strips trailing empty
/// user messages, injects the companion directive as a system message, and
/// appends a synthetic user prompt (with optional screenshot) so the chat
/// API always ends with a user-role message.
fn apply_proactive_prompt(
    messages: &mut Vec<LlmMessage>,
    directive: Option<&str>,
    screen_image_data_uri: Option<&str>,
) {
    if let Some(ene_ai::LlmMessage::User { parts }) = messages.last() {
        let empty = parts.iter().all(|p| match p {
            UserMessagePart::Text { text } => text.trim().is_empty(),
            UserMessagePart::Image { .. } => true,
        });
        if empty {
            messages.pop();
        }
    }
    if let Some(dir) = directive.filter(|s| !s.trim().is_empty()) {
        messages.push(LlmMessage::System {
            content: format!("[Companion directive]\n{dir}"),
        });
    }
    // OpenAI-compatible chat APIs expect the last message to be user-role.
    // Keep this cue ephemeral (not written to ConversationSession history).
    let mut parts = vec![UserMessagePart::Text {
        text: if screen_image_data_uri.is_some() {
            "(Proactive turn — respond per the companion directive. A screenshot from the decision moment is attached.)"
                .to_string()
        } else {
            "(Proactive turn — respond per the companion directive.)".to_string()
        },
    }];
    if let Some(uri) = screen_image_data_uri.filter(|s| !s.trim().is_empty()) {
        parts.push(UserMessagePart::Image {
            base64_image_data: uri.to_string(),
        });
    }
    messages.push(LlmMessage::User { parts });
}

/// Run the streaming loop using the cognitive runtime lifecycle.
pub async fn run_stream_cognitive(ctx: StreamContext) -> StreamOutcome {
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
        permission_scopes,
        undo_stack,
        terminal_emitted,
        turn,
        origin,
        allow_tools,
        runtime_directive,
        proactive_screen_image,
        generation_timeout,
        classifier_tx,
        memory_writer_tx,
        deferred_tool_tx,
        tts_provider,
        partial_text,
    } = ctx;

    let is_proactive = origin == TurnOrigin::Proactive;
    if let Some(timeout) = generation_timeout {
        let token = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            token.cancel();
        });
    }

    session.reset_display_buffer();

    let mind = config.get_section::<MindConfig>().unwrap_or_default();
    let engine = CognitionEngine::new();

    let tool_config = config
        .get_section::<ene_tool_host::ToolConfig>()
        .unwrap_or_default();
    let tool_calling_enabled = tool_config.enabled && allow_tools;

    let mem_config = config
        .get_section::<ene_store::StoreConfig>()
        .unwrap_or_default();

    let Some(card) = session.character_card.clone() else {
        return stream_finish(
            session,
            &event_tx,
            &terminal_emitted,
            &turn,
            origin,
            TerminalReason::Failed {
                message: "No character card loaded".into(),
            },
        );
    };

    let card_name = session.card_name().to_string();
    let user_name = config.user_name.clone();
    let session_id = session.memory.session_id.clone();
    let mem_store = session.memory.memory_store.clone();

    let history: Vec<HistoryEntry> = session.history().to_vec();
    let recall_query = if is_proactive {
        ""
    } else {
        user_input.as_str()
    };
    let compose_query = recall_query;

    let prompts = PromptLibrary::load(&mind.emotion.classifier_language);
    let post_history_phi =
        build_cognitive_output_contract(&card, &prompts, mind.emotion.enabled, &user_name);

    // Phase A: query embedding || CCv3 sync (when hash mismatches).
    emit_diag(
        &diag_tx,
        DiagnosticEvent::PipelinePhase {
            turn: turn.clone(),
            phase: PHASE_EMBEDDING.to_string(),
        },
    );

    let card_hash = compute_card_memory_hash(&card);
    let sync_needed = !is_proactive
        && mem_store.is_some()
        && embedder.is_some()
        && session.memory.ccv3_memory_hash != Some(card_hash);

    let span_pre_a = tracing::info_span!("pre_turn.phase_a");
    let embed_span = tracing::info_span!(parent: &span_pre_a, "embedding");
    let sync_span = tracing::info_span!(parent: &span_pre_a, "ccv3_sync");

    let embed_fut = async {
        if is_proactive {
            return Ok::<Option<Vec<f32>>, String>(None);
        }
        let Some(emb_prov) = embedder.as_ref() else {
            return Ok(None);
        };
        tracing::info!(%turn, "Generating user query embedding...");
        match ene_ai::embed_query(emb_prov.as_ref(), &user_input).await {
            Ok(emb) => {
                tracing::info!(%turn, "User query embedding generated successfully");
                Ok(Some(emb))
            }
            Err(e) => Err(format!("Embedding failed: {e}")),
        }
    }
    .instrument(embed_span);

    let sync_fut = async {
        if !sync_needed {
            if !is_proactive && session.memory.ccv3_memory_hash == Some(card_hash) {
                tracing::info!(%turn, "Character card memories already up-to-date");
            }
            return None;
        }
        let (Some(_store), Some(sync_embedder)) = (mem_store.as_deref(), embedder.as_ref()) else {
            return None;
        };
        let sync_ctx = build_turn_context(
            &mind,
            &card,
            &card_name,
            &user_name,
            session_id.as_str(),
            compose_query,
            &history,
            &mem_store,
            None,
            Some(sync_embedder),
            &provider,
            post_history_phi.as_deref(),
        );
        tracing::info!(%turn, "Synchronizing character card memories...");
        match engine
            .sync_character_memories(sync_ctx, session.memory.ccv3_memory_hash)
            .await
        {
            Ok((report, hash)) => {
                if report.skipped {
                    tracing::info!(%turn, "Character card memories already up-to-date");
                } else {
                    tracing::info!(
                        %turn,
                        inserted_lorebook = report.lorebook_inserted,
                        updated_lorebook = report.lorebook_updated,
                        inserted_style = report.style_inserted,
                        updated_style = report.style_updated,
                        archived = report.archived,
                        "Character card memories synchronization complete"
                    );
                }
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
                Some(hash)
            }
            Err(error) => {
                tracing::warn!(
                    component = "CognitionEngine",
                    error = %error,
                    "CCv3 character memory sync failed; continuing turn"
                );
                None
            }
        }
    }
    .instrument(sync_span);

    let (embed_result, sync_hash) = async { tokio::join!(embed_fut, sync_fut) }
        .instrument(span_pre_a)
        .await;
    let query_embedding = match embed_result {
        Ok(emb) => {
            if let Some(ref emb) = emb {
                session.set_pending_embedding(emb.clone());
                session.set_last_input_embedding(emb.clone());
            }
            emb
        }
        Err(message) => {
            return stream_finish(
                session,
                &event_tx,
                &terminal_emitted,
                &turn,
                origin,
                TerminalReason::Failed { message },
            );
        }
    };
    if let Some(hash) = sync_hash {
        session.memory.ccv3_memory_hash = Some(hash);
    }

    // Phase B: recall || tools || style examples || scene summary.
    emit_diag(
        &diag_tx,
        DiagnosticEvent::PipelinePhase {
            turn: turn.clone(),
            phase: PHASE_CONTEXT_SEARCH.to_string(),
        },
    );

    tracing::info!(%turn, "Retrieving memory recall context and selecting relevant tools...");
    let (pre_turn_result, tools, style_examples, scene_summary) = if is_proactive {
        let span_pre_b = tracing::info_span!("pre_turn.phase_b");
        let turn_ctx = build_turn_context(
            &mind,
            &card,
            &card_name,
            &user_name,
            session_id.as_str(),
            compose_query,
            &history,
            &mem_store,
            query_embedding.as_deref(),
            embedder.as_ref(),
            &provider,
            post_history_phi.as_deref(),
        );
        let recall_span = tracing::info_span!(parent: &span_pre_b, "recall");
        let style_span = tracing::info_span!(parent: &span_pre_b, "style_examples");
        let scene_span = tracing::info_span!(parent: &span_pre_b, "scene_summary");
        let (pre, style, scene) = async {
            tokio::join!(
                engine
                    .before_proactive_turn(turn_ctx)
                    .instrument(recall_span),
                CharacterProcessor::select_style_examples(
                    &card,
                    &user_name,
                    compose_query,
                    &history,
                    mem_store.as_deref(),
                    embedder.as_ref(),
                    &mind.character,
                    2,
                )
                .instrument(style_span),
                async {
                    if let Some(store) = mem_store.as_deref() {
                        load_active_scene_summary(store, session_id.as_str())
                            .await
                            .ok()
                            .flatten()
                            .map(|s| s.text)
                    } else {
                        None
                    }
                }
                .instrument(scene_span),
            )
        }
        .instrument(span_pre_b)
        .await;
        (pre, Vec::new(), style, scene)
    } else {
        let span_pre_b = tracing::info_span!("pre_turn.phase_b");
        let turn_ctx = build_turn_context(
            &mind,
            &card,
            &card_name,
            &user_name,
            session_id.as_str(),
            compose_query,
            &history,
            &mem_store,
            query_embedding.as_deref(),
            embedder.as_ref(),
            &provider,
            post_history_phi.as_deref(),
        );
        let recall_span = tracing::info_span!(parent: &span_pre_b, "recall");
        let tools_span = tracing::info_span!(parent: &span_pre_b, "tools");
        let style_span = tracing::info_span!(parent: &span_pre_b, "style_examples");
        let scene_span = tracing::info_span!(parent: &span_pre_b, "scene_summary");
        async {
            tokio::join!(
                engine.before_turn(turn_ctx).instrument(recall_span),
                select_relevant_tools(
                    registry.as_ref(),
                    tool_rag.as_deref(),
                    recall_query,
                    query_embedding.as_deref(),
                    tool_calling_enabled,
                )
                .instrument(tools_span),
                CharacterProcessor::select_style_examples(
                    &card,
                    &user_name,
                    recall_query,
                    &history,
                    mem_store.as_deref(),
                    embedder.as_ref(),
                    &mind.character,
                    2,
                )
                .instrument(style_span),
                async {
                    if let Some(store) = mem_store.as_deref() {
                        load_active_scene_summary(store, session_id.as_str())
                            .await
                            .ok()
                            .flatten()
                            .map(|s| s.text)
                    } else {
                        None
                    }
                }
                .instrument(scene_span),
            )
        }
        .instrument(span_pre_b)
        .await
    };

    let pre_turn = match pre_turn_result {
        Ok(v) => {
            tracing::info!(%turn, tools_selected = tools.len(), "Memory recall and tool selection complete");
            v
        }
        Err(e) => {
            return stream_finish(
                session,
                &event_tx,
                &terminal_emitted,
                &turn,
                origin,
                TerminalReason::Failed {
                    message: e.to_string(),
                },
            );
        }
    };

    let mut turn_affect = pre_turn.affect.clone();

    if !is_proactive
        && mem_config.enabled
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

    // Phase C: affect persist || prompt pack (with prefetched style/scene).
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
        compose_query,
        &history,
        &mem_store,
        query_embedding.as_deref(),
        embedder.as_ref(),
        &provider,
        post_history_phi.as_deref(),
    );
    let prefetch = ComposePrefetch {
        style_examples: Some(style_examples),
        scene_summary: Some(scene_summary),
        // Consume any pending interruption so the model can resume it (#206).
        interruption_note: Some(
            session
                .take_interruption()
                .map(|state| interruption_note(&state)),
        ),
    };

    // Extract the small fields still needed after composition, then move
    // `pre_turn` by value into `compose_prompt_packet` to avoid cloning the
    // recalled/commitment vectors (#review M2).
    let classifier_expression_hint = pre_turn.classifier_expression_hint.clone();
    let pre_turn_affect = pre_turn.affect.clone();

    tracing::info!(%turn, "Building prompt packet context...");
    let span_pre_c = tracing::info_span!("pre_turn.phase_c");
    let persist_span = tracing::info_span!(parent: &span_pre_c, "persist_affect");
    let compose_span = tracing::info_span!(parent: &span_pre_c, "compose_prompt");
    let (persist_result, composed_result) = async {
        tokio::join!(
            async {
                if mind.emotion.enabled
                    && let Some(store) = mem_store.as_deref()
                {
                    CognitionEngine::persist_affect_snapshot(store, &turn_affect).await
                } else {
                    Ok(())
                }
            }
            .instrument(persist_span),
            engine
                .compose_prompt_packet(compose_ctx, pre_turn, prefetch)
                .instrument(compose_span),
        )
    }
    .instrument(span_pre_c)
    .await;

    if let Err(error) = persist_result {
        tracing::warn!(
            component = "CognitionEngine",
            error = %error,
            "Failed to persist pre-turn affect snapshot"
        );
    }

    let composed = match composed_result {
        Ok(v) => {
            tracing::info!(%turn, "Prompt packet context assembled successfully");
            v
        }
        Err(e) => {
            return stream_finish(
                session,
                &event_tx,
                &terminal_emitted,
                &turn,
                origin,
                TerminalReason::Failed {
                    message: e.to_string(),
                },
            );
        }
    };

    let mut messages = composed.messages;
    if is_proactive {
        apply_proactive_prompt(
            &mut messages,
            runtime_directive.as_deref(),
            proactive_screen_image.as_deref(),
        );
    }
    let max_rounds = tool_config.max_rounds;
    let session_id_for_tools = session.memory.session_id.clone();
    let mut round = 0usize;
    let mut turn_tool_results: Vec<ToolResultSummary> = Vec::new();

    // TTS pipeline: spawn a background worker that synthesizes sentences into
    // PCM audio chunks and emits them through the broadcast channel.
    // The worker monitors the turn's CancellationToken so barge-in can stop
    // synthesis immediately instead of finishing the current sentence (#H4).
    let (tts_tx, tts_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let tts_tx: Option<tokio::sync::mpsc::UnboundedSender<String>> = if tts_provider.is_some() {
        Some(tts_tx)
    } else {
        None
    };
    if let Some(ref provider) = tts_provider {
        let tts_provider = Arc::clone(provider);
        let event_tx = event_tx.clone();
        let turn = turn.clone();
        let tts_cancel = cancel_token.clone();
        tokio::spawn(
            async move {
                let mut rx = tts_rx;
                loop {
                    // Wait for the next sentence or cancellation (#H4).
                    let sentence = tokio::select! {
                        biased;
                        () = tts_cancel.cancelled() => break,
                        recv = rx.recv() => match recv {
                            Some(s) => s,
                            None => break, // channel closed: all sentences flushed
                        },
                    };
                    let trimmed = sentence.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match tts_provider.synthesize_stream(trimmed).await {
                        Ok(mut stream) => {
                            use tokio_stream::StreamExt as _;
                            loop {
                                // Consume synthesis chunks or bail on cancel (#H4).
                                let chunk_res = tokio::select! {
                                    biased;
                                    () = tts_cancel.cancelled() => break,
                                    next = stream.next() => match next {
                                        Some(res) => res,
                                        None => break, // stream exhausted
                                    },
                                };
                                match chunk_res {
                                    Ok(chunk) => {
                                        let _ = event_tx.send(EneEvent::AudioChunk {
                                            turn: turn.clone(),
                                            origin,
                                            pcm: chunk.pcm,
                                            sample_rate: chunk.sample_rate,
                                            is_final: false,
                                        });
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            component = "TtsPipeline",
                                            error = %e,
                                            "TTS synthesis chunk error"
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                component = "TtsPipeline",
                                error = %e,
                                "TTS synthesis stream open failed"
                            );
                        }
                    }
                    // If cancelled mid-synthesis, stop processing further sentences.
                    if tts_cancel.is_cancelled() {
                        break;
                    }
                }
                // Emit final marker only on clean completion (not on cancel).
                if !tts_cancel.is_cancelled() {
                    let _ = event_tx.send(EneEvent::AudioChunk {
                        turn: turn.clone(),
                        origin,
                        pcm: Vec::new(),
                        sample_rate: 0,
                        is_final: true,
                    });
                }
            }
            .instrument(tracing::info_span!("tts_pipeline")),
        );
    }
    let mut tts_sentence_buf = String::new();
    // Incremental char count for `tts_sentence_buf` to avoid O(n) rescans (#L8).
    let mut tts_sentence_buf_chars: usize = 0;

    loop {
        if cancel_token.is_cancelled() {
            let spoken = session.display.display_buffer.clone();
            spawn_interrupted_memory_work(
                mem_store.as_ref(),
                &mind,
                &provider,
                embedder.as_ref(),
                &memory_writer_tx,
                user_input.as_str(),
                &spoken,
                &turn_tool_results,
                &turn_affect,
                &card_name,
                &user_name,
            );
            return finish_cancelled(
                session,
                &event_tx,
                &terminal_emitted,
                &turn,
                origin,
                &spoken,
            );
        }

        if round >= max_rounds {
            return stream_finish(
                session,
                &event_tx,
                &terminal_emitted,
                &turn,
                origin,
                TerminalReason::Failed {
                    message: "Max tool call rounds exceeded".into(),
                },
            );
        }

        tracing::info!(%turn, round, "Requesting LLM response stream...");
        let mut stream = match provider.create_chat_stream(&messages, &tools).await {
            Ok(s) => s,
            Err(e) => {
                return stream_finish(
                    session,
                    &event_tx,
                    &terminal_emitted,
                    &turn,
                    origin,
                    TerminalReason::Failed {
                        message: e.to_string(),
                    },
                );
            }
        };

        let mut current_tool_calls: Vec<LlmToolCallChunk> = Vec::new();
        let mut assistant_content = String::new();
        let mut accumulated_emotion_tokens: Vec<String> = Vec::new();
        let mut perf_arbiter = PerformanceArbiter::default();
        let suppress_stream_tokens =
            mind.emotion.enabled && mind.emotion.llm_expression_is_advisory;

        let mut is_first_chunk = true;
        while let Some(chunk_res) = stream.next().await {
            if cancel_token.is_cancelled() {
                let spoken = session.display.display_buffer.clone();
                spawn_interrupted_memory_work(
                    mem_store.as_ref(),
                    &mind,
                    &provider,
                    embedder.as_ref(),
                    &memory_writer_tx,
                    user_input.as_str(),
                    &spoken,
                    &turn_tool_results,
                    &turn_affect,
                    &card_name,
                    &user_name,
                );
                return finish_cancelled(
                    session,
                    &event_tx,
                    &terminal_emitted,
                    &turn,
                    origin,
                    &spoken,
                );
            }

            if is_first_chunk {
                is_first_chunk = false;
                if chunk_res.is_ok() {
                    tracing::info!(%turn, "LLM streaming response started");
                } else {
                    tracing::warn!(%turn, "LLM streaming response failed on first chunk");
                }
            }

            match chunk_res {
                Ok(chunk) => {
                    if let Some(content_delta) = &chunk.text_delta {
                        assistant_content.push_str(content_delta);
                        let (text_deltas, special_tokens) = session.process_delta(content_delta);
                        for text in text_deltas {
                            // Mirror streamed text into the shared buffer so a
                            // hard-aborted turn can recover its partial response
                            // for interruption recording (#H5).
                            partial_text.lock().push_str(&text);
                            let _ = event_tx.send(EneEvent::TextDelta {
                                turn: turn.clone(),
                                origin,
                                delta: text.clone(),
                            });
                            if let Some(ref tx) = tts_tx {
                                tts_sentence_buf.push_str(&text);
                                tts_sentence_buf_chars =
                                    tts_sentence_buf_chars.saturating_add(text.chars().count());
                                while let Some(end) = find_tts_sentence_boundary(
                                    &tts_sentence_buf,
                                    tts_sentence_buf_chars,
                                ) {
                                    let sentence: String = tts_sentence_buf.drain(..end).collect();
                                    tts_sentence_buf_chars = tts_sentence_buf_chars
                                        .saturating_sub(sentence.chars().count());
                                    let _ = tx.send(sentence);
                                }
                            }
                        }
                        for token in special_tokens {
                            accumulated_emotion_tokens.push(token.clone());

                            if !suppress_stream_tokens
                                && let Some(cue) = ene_mind::parse_performance_marker(&token)
                            {
                                let source = match cue.kind {
                                    PerfKind::Expression | PerfKind::Motion | PerfKind::LookAt => {
                                        if mind.emotion.llm_expression_is_advisory {
                                            CueSource::LlmAdvisory
                                        } else {
                                            CueSource::LlmCommand
                                        }
                                    }
                                    PerfKind::Cancel => CueSource::LlmCommand,
                                };
                                perf_arbiter.accept(cue.clone(), source);
                            }
                        }
                    }
                    if let Some(tool_calls_delta) = &chunk.tool_calls_delta {
                        accumulate_tool_calls(&mut current_tool_calls, tool_calls_delta);
                    }
                }
                Err(e) => {
                    return stream_finish(
                        session,
                        &event_tx,
                        &terminal_emitted,
                        &turn,
                        origin,
                        TerminalReason::Failed {
                            message: e.to_string(),
                        },
                    );
                }
            }
        }

        let clean_content = if !mind.emotion.enabled && !assistant_content.is_empty() {
            strip_markers(&assistant_content)
        } else {
            assistant_content.clone()
        };

        if current_tool_calls.is_empty() {
            if !assistant_content.is_empty()
                && let Some(store) = &mem_store
            {
                ene_store::MemoryStore::spawn_insert_log(
                    store,
                    session_id.as_str(),
                    &card_name,
                    "assistant",
                    &clean_content,
                );
            }

            if mind.emotion.enabled {
                let llm_proposal = accumulated_emotion_tokens
                    .iter()
                    .find_map(|token| {
                        ene_mind::parse_performance_marker(token)
                            .and_then(|cue| (cue.kind == PerfKind::Expression).then_some(cue.name))
                    })
                    .or_else(|| classifier_expression_hint.clone());
                let (previous_expression, elapsed_since_change) =
                    session.expression_context(&turn_affect);
                let (decision, updated_affect) = engine.resolve_expression_turn(
                    &mind,
                    &card,
                    &turn_affect,
                    &clean_content,
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
            }

            let resolved = perf_arbiter.resolve();
            if !resolved.is_empty() {
                let (cues, sources): (Vec<_>, Vec<_>) = resolved.into_iter().unzip();
                let primary_source = sources.into_iter().max_by_key(|s| cue_source_priority(*s));
                if let Some(source) = primary_source {
                    let _ = event_tx.send(EneEvent::Performance {
                        turn: turn.clone(),
                        origin,
                        cues,
                        source,
                    });
                }
            }

            let post_user = if is_proactive {
                ""
            } else {
                user_input.as_str()
            };
            let post = PostTurnInput {
                turn: TurnInput {
                    user_message: post_user,
                    assistant_message: Some(&clean_content),
                    tool_results: &turn_tool_results,
                },
                affect: &turn_affect,
                character_id: &card_name,
                user_id: &user_name,
                interrupted: false,
                spoken_text: None,
            };

            if let Some(store) = mem_store.as_deref()
                && let Err(error) = engine.finalize_turn(store, &mind, &post).await
            {
                tracing::warn!(
                    component = "CognitionEngine",
                    error = %error,
                    "Post-turn finalize_turn failed"
                );
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

            // Commit assistant history before Terminal so the next turn's prompt
            // includes this exchange even when memory extraction is deferred.
            session.finalize_response();
            session.record_assistant_response();

            if let Some(store) = mem_store.clone() {
                let deferred_input = OwnedPostTurnInput {
                    turn: OwnedTurnInput {
                        user_message: post_user.to_string(),
                        assistant_message: Some(clean_content.clone()),
                        tool_results: turn_tool_results.clone(),
                    },
                    affect: turn_affect,
                    character_id: card_name.clone(),
                    user_id: user_name.clone(),
                    interrupted: false,
                    spoken_text: None,
                };
                let memory_writer_handle = CognitionEngine::spawn_deferred_memory_work(
                    store,
                    mind.clone(),
                    deferred_input,
                    provider.clone(),
                    embedder.clone(),
                );
                let _ = memory_writer_tx.send(memory_writer_handle);
            }

            if !is_proactive
                && let Some(classifier_store) = mem_store.clone()
                && mind.emotion.enabled
                && !assistant_content.trim().is_empty()
            {
                let classifier_config = config.clone();
                let classifier_lang = mind.emotion.classifier_language.clone();
                let classifier_timeout_secs = mind.emotion.classifier_timeout_secs;
                let classifier_character_id = card_name.clone();
                let classifier_user_id = user_name.clone();
                let classifier_turn_id =
                    ene_mind::engine::completed_user_turn_at_post_turn(&history);
                let classifier_context = ene_mind::engine::build_classifier_context(
                    &history,
                    &clean_content,
                    &pre_turn_affect,
                    mind.context.recent_turns,
                );

                // Fire-and-forget: must not delay Terminal (already emitted).
                // The JoinHandle is sent to the actor for lifecycle management.
                let classifier_handle = tokio::spawn(
                    async move {
                        tracing::info!(
                            component = "EmotionEngine",
                            turn_id = classifier_turn_id,
                            "Starting post-turn affect classifier"
                        );
                        let started = std::time::Instant::now();
                        match ene_mind::emotion::classifier::classify_for_config(
                            &classifier_config,
                            None,
                            0,
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
                                        ene_mind::emotion::classifier::classify_failure_reason(
                                            &error
                                        ),
                                    turn_id = classifier_turn_id,
                                    elapsed_ms = started.elapsed().as_millis(),
                                    "Post-turn affect classifier failed"
                                );
                            }
                        }
                    }
                    .instrument(tracing::info_span!("post_turn.affect")),
                );
                // Send handle to actor for lifecycle management.
                // A send failure means the actor has shut down; the
                // classifier task runs as a detached orphan until
                // completion, which is acceptable at shutdown.
                let _ = classifier_tx.send(classifier_handle);
            }

            // Flush any remaining TTS buffer before Terminal.
            if let Some(ref tx) = tts_tx
                && !tts_sentence_buf.trim().is_empty()
            {
                let _ = tx.send(tts_sentence_buf.clone());
            }
            drop(tts_tx);

            return stream_finish(
                session,
                &event_tx,
                &terminal_emitted,
                &turn,
                origin,
                TerminalReason::Done,
            );
        }

        let tool_calls = finalize_tool_calls(current_tool_calls);
        let exec_ctx = crate::streaming::ToolExecutionContext {
            registry: registry.as_ref(),
            tool_rag: tool_rag.as_deref(),
            session_id: session_id_for_tools.as_str(),
            event_tx: &event_tx,
            turn: &turn,
            origin,
            pending_permissions: &pending_permissions,
            pending_user_inputs: &pending_user_inputs,
            timeout_ms: tool_config.timeout_ms,
            max_summary_chars: mind.memory.tool_grounding.max_summary_chars,
            audit_store: mem_store.as_ref(),
            permission_scopes: &permission_scopes,
            undo_stack: &undo_stack,
            deferred_tool_tx: &deferred_tool_tx,
        };
        let tx_messages = perform_tool_executions(&exec_ctx, tool_calls, &assistant_content).await;

        match tx_messages {
            Ok(output) => {
                messages.extend(output.messages);
                turn_tool_results.extend(output.summaries);
                round += 1;
            }
            Err(e) => {
                return stream_finish(
                    session,
                    &event_tx,
                    &terminal_emitted,
                    &turn,
                    origin,
                    TerminalReason::Failed {
                        message: e.to_string(),
                    },
                );
            }
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::string_slice,
    reason = "boundary indices come from char_indices and are always char-boundary safe"
)]
mod tests {
    use super::*;

    /// Helper: char count for the incremental-tracking tests (#L8).
    fn char_count(buf: &str) -> usize {
        buf.chars().count()
    }

    #[test]
    fn ja_sentence_splits_at_cjk_period_without_trailing_space() {
        // Japanese has no space after 。 — must split unconditionally (#M8).
        let buf = "こんにちは。元気ですか？";
        let end =
            find_tts_sentence_boundary(buf, char_count(buf)).expect("should find a boundary at 。");
        assert_eq!(&buf[..end], "こんにちは。");
    }

    #[test]
    fn ja_sentence_splits_at_fullwidth_exclamation() {
        let buf = "すごい！本当？";
        let end =
            find_tts_sentence_boundary(buf, char_count(buf)).expect("should find a boundary at ！");
        assert_eq!(&buf[..end], "すごい！");
    }

    #[test]
    fn ja_sentence_splits_at_fullwidth_question() {
        let buf = "本当？うん。";
        let end =
            find_tts_sentence_boundary(buf, char_count(buf)).expect("should find a boundary at ？");
        assert_eq!(&buf[..end], "本当？");
    }

    #[test]
    fn en_sentence_splits_at_period_with_trailing_space() {
        // The boundary index lands right after the punctuation; the trailing
        // space stays in the buffer and is trimmed before synthesis.
        let buf = "Hello. How are you?";
        let end = find_tts_sentence_boundary(buf, char_count(buf))
            .expect("should find a boundary at '. '");
        assert_eq!(&buf[..end], "Hello.");
    }

    #[test]
    fn en_sentence_splits_at_end_of_buffer() {
        // ASCII punctuation at end-of-buffer is a boundary even without space.
        let buf = "Are you there?";
        let end = find_tts_sentence_boundary(buf, char_count(buf))
            .expect("should find a boundary at trailing '?'");
        assert_eq!(&buf[..end], "Are you there?");
    }

    #[test]
    fn en_period_without_space_is_not_a_boundary() {
        // "3.14" — the period is followed by a digit, not whitespace/end.
        let buf = "Pi is 3.14 roughly";
        assert!(
            find_tts_sentence_boundary(buf, char_count(buf)).is_none(),
            "a period followed by a digit must not be treated as a boundary"
        );
    }

    #[test]
    fn mixed_ja_en_splits_at_first_boundary() {
        let buf = "OK。then let's go.";
        let end = find_tts_sentence_boundary(buf, char_count(buf))
            .expect("should find the first boundary at 。");
        assert_eq!(&buf[..end], "OK。");
    }

    #[test]
    fn no_boundary_returns_none() {
        let buf = "just some text without punctuation";
        assert!(find_tts_sentence_boundary(buf, char_count(buf)).is_none());
    }

    #[test]
    fn overlong_buffer_forces_flush() {
        // Exceeding TTS_MAX_BUFFER_CHARS forces a flush of the whole buffer.
        let buf: String = "あ".repeat(TTS_MAX_BUFFER_CHARS + 1);
        let end = find_tts_sentence_boundary(&buf, char_count(&buf))
            .expect("overlong buffer should force a flush");
        assert_eq!(end, buf.len());
    }

    #[test]
    fn incremental_char_count_matches_recount() {
        // The incremental count passed by the caller must agree with a fresh
        // recount, otherwise the overlong flush threshold would misfire (#L8).
        let buf = "こんにちは。Hello. ";
        assert_eq!(char_count(buf), buf.chars().count());
        let end = find_tts_sentence_boundary(buf, char_count(buf)).expect("boundary");
        assert_eq!(&buf[..end], "こんにちは。");
    }
}
