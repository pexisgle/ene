use crate::client::build_openai_client;
use crate::prompt_builder::build_messages;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall as ToolCall, ChatCompletionMessageToolCallChunk as ToolCallChunk,
    ChatCompletionMessageToolCalls as ToolCalls,
    ChatCompletionRequestAssistantMessageArgs as AssistantMsgArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestMessageContentPartImageArgs as ImagePartArgs,
    ChatCompletionRequestMessageContentPartTextArgs as TextPartArgs,
    ChatCompletionRequestToolMessageArgs as ToolMsgArgs,
    ChatCompletionRequestUserMessageArgs as UserMsgArgs, CreateChatCompletionRequestArgs,
    FunctionCall, FunctionCallStream, ImageUrlArgs,
};
use async_stream::stream;
use ene_config::AiSettings;
use ene_memory::RecalledSummary;
use ene_session::ConversationSession;
use std::sync::Arc;
use tokio_stream::Stream;

#[derive(Debug)]
pub enum AiStreamEvent {
    TextDelta(String),
    SpecialToken(String),
    ToolCallStart {
        name: String,
        arguments: String,
    },
    ToolCallResult {
        name: String,
        result: String,
    },
    /// パーミッション要求（Phase 2: UI 連携）
    PermissionRequired {
        request_id: String,
        action: String,
        target: String,
        description: String,
    },
    /// タスク進捗（Phase 2: バックグラウンド実行）
    TaskProgress {
        task_id: String,
        step: usize,
        total_steps: usize,
        description: String,
    },
    SessionSplit {
        summary: String,
        reason: String,
    },
    Finished,
    Error(String),
}

pub async fn select_relevant_tools(
    registry: &dyn ene_tool_host::ToolRegistry,
    embedding_provider: &Option<Arc<dyn ene_embedding::EmbeddingProvider>>,
    user_input: &str,
    tool_calling_enabled: bool,
    tool_rag_enabled: bool,
    tool_rag_limit: usize,
    tool_rag_always_include: &[String],
    mem_store: Option<&ene_memory::MemoryStore>,
) -> Vec<ene_tool_host::ToolDefinition> {
    if tool_rag_enabled && tool_calling_enabled {
        if let Some(embedder) = embedding_provider.as_ref() {
            if let Err(e) = registry
                .ensure_index_built(embedder.as_ref(), mem_store)
                .await
            {
                tracing::warn!("[ToolRAG] Failed to build index: {}", e);
            }
            let query_embedding = match embedder.embed_query(user_input).await {
                Ok(emb) => Some(emb),
                Err(e) => {
                    tracing::warn!("[ToolRAG] Failed to embed query: {}", e);
                    None
                }
            };
            let mut relevant =
                registry.list_relevant_tools(query_embedding.as_deref(), tool_rag_limit);
            // 常に含めるツールが漏れていれば追加
            let all_tools = registry.list_tools();
            let all_map: std::collections::HashMap<String, _> =
                all_tools.into_iter().map(|t| (t.name.clone(), t)).collect();
            for name in tool_rag_always_include {
                if !relevant.iter().any(|t| &t.name == name) {
                    if let Some(tool) = all_map.get(name) {
                        relevant.push(tool.clone());
                    }
                }
            }
            relevant
        } else {
            registry.list_tools()
        }
    } else {
        registry.list_tools()
    }
}

pub async fn fetch_memory_context(
    session: &ConversationSession,
    settings: &AiSettings,
) -> (Vec<RecalledSummary>, Vec<ene_memory::KeyFact>) {
    let memory_enabled = settings.memory.enabled;
    let mem_store = if memory_enabled {
        session.memory.memory_store.clone()
    } else {
        None
    };
    let card_name = session.card_name().to_string();
    let pending_embedding = session.memory.pending_embedding.clone();
    let summary_recall_limit = settings.memory.summary_recall_limit;
    let similarity_threshold = settings.memory.similarity_threshold;

    let recalled_summaries = search_summaries(
        memory_enabled,
        &mem_store,
        &card_name,
        &pending_embedding,
        summary_recall_limit,
        similarity_threshold,
    )
    .await;

    let key_facts = if settings.memory.enabled {
        if let Some(store) = &session.memory.memory_store {
            store
                .get_all_keyfacts(session.card_name())
                .unwrap_or_default()
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    (recalled_summaries, key_facts)
}

pub fn build_chat_messages_list(
    session: &ConversationSession,
    settings: &AiSettings,
    user_input: &str,
    recalled_summaries: &[RecalledSummary],
    key_facts: &[ene_memory::KeyFact],
) -> Result<Vec<ChatCompletionRequestMessage>, crate::error::AiCoreError> {
    let Some(card) = session.character_card.as_ref() else {
        return Err(crate::error::AiCoreError::NoCharacterCard);
    };
    let history = session.history.conversation_history.clone();
    let runtime_rules = settings.runtime_rules.clone();
    let user_name = settings.user_name.clone();

    build_messages(
        card,
        user_input,
        &history,
        "",
        &runtime_rules,
        &user_name,
        recalled_summaries,
        key_facts,
    )
    .map_err(|e| crate::error::AiCoreError::PromptBuildError(e))
}

pub async fn perform_tool_executions(
    registry: &dyn ene_tool_host::ToolRegistry,
    session_id: &str,
    tool_calls: Vec<ToolCalls>,
    assistant_content: &str,
    tx: tokio::sync::mpsc::UnboundedSender<AiStreamEvent>,
) -> Result<Vec<ChatCompletionRequestMessage>, crate::error::AiCoreError> {
    let mut round_messages = Vec::new();

    let mut asst_msg_builder = AssistantMsgArgs::default();
    asst_msg_builder.tool_calls(tool_calls.clone());
    if !assistant_content.is_empty() {
        asst_msg_builder.content(assistant_content);
    }

    let asst_msg = asst_msg_builder.build().map_err(|e| {
        crate::error::AiCoreError::ToolExecutionError(format!(
            "Failed to build assistant message: {}",
            e
        ))
    })?;
    round_messages.push(asst_msg.into());

    registry.set_session_id(session_id).await;

    for tool_call_enum in tool_calls {
        if let ToolCalls::Function(call) = tool_call_enum {
            let name = call.function.name.clone();
            let args = call.function.arguments.clone();

            let _ = tx.send(AiStreamEvent::ToolCallStart {
                name: name.clone(),
                arguments: args.clone(),
            });

            let tool_timeout = std::time::Duration::from_secs(30);
            let result = match tokio::time::timeout(tool_timeout, registry.call_tool(&name, &args))
                .await
            {
                Ok(Ok(res)) => res,
                Ok(Err(e)) => format!("Error executing tool: {}", e),
                Err(_) => format!(
                    "Tool '{}' timed out after {} seconds (the operation may not be supported on this environment)",
                    name,
                    tool_timeout.as_secs()
                ),
            };

            let _ = tx.send(AiStreamEvent::ToolCallResult {
                name: name.clone(),
                result: result.clone(),
            });

            let (final_tool_text, screenshot_data) = extract_screenshot(&result);

            let tool_msg = ToolMsgArgs::default()
                .tool_call_id(call.id.clone())
                .content(final_tool_text)
                .build()
                .map_err(|e| {
                    crate::error::AiCoreError::ToolExecutionError(format!(
                        "Failed to build tool message: {}",
                        e
                    ))
                })?;
            round_messages.push(tool_msg.into());

            if let Some(b64_data) = screenshot_data {
                if let Some(user_msg) = build_screenshot_message(b64_data) {
                    round_messages.push(user_msg);
                }
            }
        }
    }

    Ok(round_messages)
}

pub async fn run_ai_with_tools(
    settings: &AiSettings,
    session: &ConversationSession,
    user_input: &str,
    registry: std::sync::Arc<dyn ene_tool_host::ToolRegistry>,
) -> Result<impl Stream<Item = AiStreamEvent> + 'static, crate::error::AiCoreError> {
    let base_url = settings.resolve_base_url()?;
    let api_key = settings.resolve_api_key();

    let client = build_openai_client(&base_url, &api_key);
    let model = settings.provider.model.clone();

    // 1. Fetch memory context (summaries and keyfacts)
    let (recalled_summaries, key_facts) = fetch_memory_context(session, settings).await;

    // 2. Insert user log if memory is enabled
    if settings.memory.enabled {
        if let Some(store) = &session.memory.memory_store {
            let store_clone = store.clone();
            let card_name = session.card_name().to_string();
            let session_id = session.memory.session_id.clone();
            let input_text = user_input.to_string();
            tokio::spawn(async move {
                if let Err(e) = store_clone.insert_log(&session_id, &card_name, "user", &input_text)
                {
                    tracing::error!("[Memory] Failed to save user log: {}", e);
                }
            });
        }
    }

    // 3. Build initial OpenAI chat messages list
    let mut messages = build_chat_messages_list(
        session,
        settings,
        user_input,
        &recalled_summaries,
        &key_facts,
    )?;

    let memory_enabled = settings.memory.enabled;
    let mem_store = if memory_enabled {
        session.memory.memory_store.clone()
    } else {
        None
    };
    let card_name = session.card_name().to_string();
    let session_id = session.memory.session_id.clone();
    let user_input = user_input.to_string();
    let tool_calling_enabled = settings.tool_calling_enabled;
    let max_rounds = settings.max_tool_call_rounds;
    let session_id_for_tools = session.memory.session_id.clone();
    let tool_rag_enabled = settings.memory.tool_rag_enabled;
    let tool_rag_limit = settings.memory.tool_rag_limit;
    let tool_rag_always_include = settings.memory.tool_rag_always_include.clone();
    let embedding_provider = session.memory.embedding_provider.clone();

    Ok(stream! {
        // 4. Select relevant tools (Tool RAG)
        let tools = select_relevant_tools(
            registry.as_ref(),
            &embedding_provider,
            &user_input,
            tool_calling_enabled,
            tool_rag_enabled,
            tool_rag_limit,
            &tool_rag_always_include,
            mem_store.as_ref().map(|s| s.as_ref()),
        )
        .await;

        let oa_tools = if tool_calling_enabled && !tools.is_empty() {
            match crate::prompt_builder::build_tools(&tools) {
                Ok(t) => Some(t),
                Err(e) => {
                    yield AiStreamEvent::Error(e);
                    return;
                }
            }
        } else {
            None
        };

        let mut round = 0;

        loop {
            if round >= max_rounds {
                yield AiStreamEvent::Error("Max tool call rounds exceeded".to_string());
                return;
            }

            let mut req_builder = CreateChatCompletionRequestArgs::default();
            req_builder.model(model.clone()).messages(messages.clone());
            if let Some(t) = &oa_tools {
                req_builder.tools(t.clone());
            }

            let request = match req_builder.build() {
                Ok(req) => req,
                Err(e) => {
                    yield AiStreamEvent::Error(format!("failed to build request: {e}"));
                    return;
                }
            };

            let mut stream = match client.chat().create_stream(request).await {
                Ok(s) => s,
                Err(e) => {
                    yield AiStreamEvent::Error(e.to_string());
                    return;
                }
            };

            let mut current_tool_calls: Vec<ToolCallChunk> = Vec::new();
            let mut assistant_content = String::new();

            use tokio_stream::StreamExt;
            while let Some(chunk_res) = stream.next().await {
                match chunk_res {
                    Ok(chunk) => {
                        if let Some(choice) = chunk.choices.first() {
                            if let Some(content_delta) = &choice.delta.content {
                                assistant_content.push_str(content_delta);
                                yield AiStreamEvent::TextDelta(content_delta.clone());
                            }

                            if let Some(tool_calls_delta) = &choice.delta.tool_calls {
                                accumulate_tool_calls(&mut current_tool_calls, tool_calls_delta);
                            }
                        }
                    }
                    Err(e) => {
                        yield AiStreamEvent::Error(e.to_string());
                        return;
                    }
                }
            }

            if !current_tool_calls.is_empty() {
                let tool_calls = finalize_tool_calls(current_tool_calls);

                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let registry_clone = registry.clone();
                let session_id_clone = session_id_for_tools.clone();
                let tool_calls_clone = tool_calls.clone();
                let assistant_content_clone = assistant_content.clone();

                let handle = tokio::spawn(async move {
                    perform_tool_executions(
                        registry_clone.as_ref(),
                        &session_id_clone,
                        tool_calls_clone,
                        &assistant_content_clone,
                        tx,
                    )
                    .await
                });

                while let Some(event) = rx.recv().await {
                    yield event;
                }

                let tx_messages = match handle.await {
                    Ok(Ok(msgs)) => msgs,
                    Ok(Err(e)) => {
                        yield AiStreamEvent::Error(e.to_string());
                        return;
                    }
                    Err(e) => {
                        yield AiStreamEvent::Error(format!("Task panicked: {}", e));
                        return;
                    }
                };

                messages.extend(tx_messages);
                round += 1;
                continue;
            } else {
                if !assistant_content.is_empty() {
                    if let Some(store) = mem_store.clone() {
                        let card_name = card_name.clone();
                        let session_id = session_id.clone();
                        let content = assistant_content.clone();
                        tokio::spawn(async move {
                            if let Err(e) = store.insert_log(&session_id, &card_name, "assistant", &content) {
                                tracing::error!("[Memory] Failed to save assistant log: {}", e);
                            }
                        });
                    }
                }

                yield AiStreamEvent::Finished;
                return;
            }
        }
    })
}

async fn search_summaries(
    memory_enabled: bool,
    mem_store: &Option<std::sync::Arc<ene_memory::MemoryStore>>,
    card_name: &str,
    pending_embedding: &Option<Vec<f32>>,
    summary_recall_limit: usize,
    similarity_threshold: f32,
) -> Vec<RecalledSummary> {
    if !memory_enabled || summary_recall_limit == 0 {
        return vec![];
    }

    let Some(store) = mem_store else {
        return vec![];
    };

    let query_vec = match pending_embedding {
        Some(v) => v.clone(),
        None => {
            tracing::warn!("[Memory] No embedding available for summary search");
            return vec![];
        }
    };

    if query_vec.is_empty() {
        return vec![];
    }

    match store.search_summaries(
        &query_vec,
        card_name,
        summary_recall_limit,
        similarity_threshold,
    ) {
        Ok(summaries) => summaries,
        Err(e) => {
            tracing::error!("[Memory] Summary search error: {}", e);
            vec![]
        }
    }
}

fn accumulate_tool_calls(
    current_tool_calls: &mut Vec<ToolCallChunk>,
    tool_calls_delta: &[ToolCallChunk],
) {
    for tc_delta in tool_calls_delta {
        let idx = tc_delta.index as usize;
        if idx >= current_tool_calls.len() {
            current_tool_calls.resize(
                idx + 1,
                ToolCallChunk {
                    index: tc_delta.index,
                    id: None,
                    r#type: None,
                    function: None,
                },
            );
        }
        let target = &mut current_tool_calls[idx];
        if let Some(id) = &tc_delta.id {
            target.id = Some(id.clone());
        }
        if let Some(ty) = &tc_delta.r#type {
            target.r#type = Some(ty.clone());
        }
        if let Some(func_delta) = &tc_delta.function {
            if target.function.is_none() {
                target.function = Some(FunctionCallStream {
                    name: None,
                    arguments: None,
                });
            }
            if let Some(target_func) = &mut target.function {
                if let Some(name) = &func_delta.name {
                    target_func.name = Some(target_func.name.clone().unwrap_or_default() + name);
                }
                if let Some(args) = &func_delta.arguments {
                    target_func.arguments =
                        Some(target_func.arguments.clone().unwrap_or_default() + args);
                }
            }
        }
    }
}

fn finalize_tool_calls(current_tool_calls: Vec<ToolCallChunk>) -> Vec<ToolCalls> {
    let mut tool_calls = Vec::new();
    for tc in current_tool_calls {
        if let Some(func) = tc.function {
            tool_calls.push(ToolCalls::Function(ToolCall {
                id: tc.id.unwrap_or_default(),
                function: FunctionCall {
                    name: func.name.unwrap_or_default(),
                    arguments: func.arguments.unwrap_or_default(),
                },
            }));
        }
    }
    tool_calls
}

fn extract_screenshot(result: &str) -> (String, Option<String>) {
    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(result) {
        if json_val.get("type").and_then(|v| v.as_str()) == Some("screenshot") {
            if let Some(data) = json_val.get("data").and_then(|v| v.as_str()) {
                return (
                    "[Screenshot successfully captured and sent to vision system]".to_string(),
                    Some(data.to_string()),
                );
            }
        }
    }
    (result.to_string(), None)
}

fn build_screenshot_message(b64_data: String) -> Option<ChatCompletionRequestMessage> {
    let img_url = ImageUrlArgs::default().url(b64_data).build().ok()?;
    let text_part = TextPartArgs::default()
        .text("Here is the screenshot.")
        .build()
        .ok()?;
    let img_part = ImagePartArgs::default().image_url(img_url).build().ok()?;
    let user_msg = UserMsgArgs::default()
        .content(vec![text_part.into(), img_part.into()])
        .build()
        .ok()?;
    Some(user_msg.into())
}
