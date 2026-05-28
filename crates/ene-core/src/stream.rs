use crate::prompt_builder::build_messages;
use async_stream::stream;
use ene_config::EneConfig;
use ene_memory::RecalledSummary;
use ene_provider::{LlmMessage, LlmToolCall, LlmToolCallChunk, UserMessagePart};
use ene_session::ConversationSession;
use std::sync::Arc;
use tokio_stream::Stream;

/// Represents a user's permission decision for a destructive operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Allow the operation this one time.
    AllowOnce,
    /// Allow the operation for the rest of the session.
    AllowSession,
    /// Deny the operation.
    Deny,
}

static PENDING_DECISIONS: std::sync::OnceLock<
    std::sync::Mutex<
        std::collections::HashMap<String, tokio::sync::oneshot::Sender<PermissionDecision>>,
    >,
> = std::sync::OnceLock::new();

/// Registers a pending permission request that will be resolved when the user
/// makes a decision.
pub fn register_permission_request(
    request_id: String,
    tx: tokio::sync::oneshot::Sender<PermissionDecision>,
) {
    let map =
        PENDING_DECISIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Ok(mut guard) = map.lock() {
        guard.insert(request_id, tx);
    }
}

/// Submits a user's permission decision for a pending request.
pub fn submit_permission_decision(
    request_id: &str,
    decision: PermissionDecision,
) -> Result<(), &'static str> {
    let map =
        PENDING_DECISIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Ok(mut guard) = map.lock() {
        if let Some(tx) = guard.remove(request_id) {
            let _ = tx.send(decision);
            Ok(())
        } else {
            Err("Request ID not found or already processed")
        }
    } else {
        Err("Failed to lock global decision map")
    }
}

/// Events emitted during an AI streaming completion session.
#[derive(Debug)]
pub enum EneStreamEvent {
    /// A chunk of generated text.
    TextDelta(String),
    /// A special token like `<|emo:happy|>`.
    SpecialToken(String),
    /// A tool call has been initiated.
    ToolCallStart {
        /// The name of the tool being called.
        name: String,
        /// The arguments passed to the tool.
        arguments: String,
    },
    /// A tool call has completed with its result.
    ToolCallResult {
        /// The name of the tool that was called.
        name: String,
        /// The result returned by the tool.
        result: String,
    },
    /// A destructive operation requires user permission.
    PermissionRequired {
        /// Unique identifier for the permission request.
        request_id: String,
        /// The action requesting permission.
        action: String,
        /// The target of the action (e.g. file path).
        target: String,
        /// Human-readable description of the action.
        description: String,
    },
    /// Progress update for a background task.
    TaskProgress {
        /// Unique identifier for the task.
        task_id: String,
        /// Current step number.
        step: usize,
        /// Total number of steps.
        total_steps: usize,
        /// Description of the current step.
        description: String,
    },
    /// The session has been split with a new summary.
    SessionSplit {
        /// The generated conversation summary.
        summary: String,
        /// The reason the session was split.
        reason: String,
    },
    /// The AI stream has completed.
    Finished,
    /// An error occurred during streaming.
    Error(String),
}

/// Selects relevant tools using embedding-based RAG if enabled, otherwise
/// returns all tools.
pub async fn select_relevant_tools(
    registry: &dyn ene_tool_host::ToolRegistry,
    embedding_provider: &Option<Arc<dyn ene_provider::EmbeddingProvider>>,

    user_input: &str,
    tool_calling_enabled: bool,
    tool_rag_enabled: bool,
    tool_rag_limit: usize,
    tool_rag_always_include: &[String],
) -> Vec<ene_tool_host::ToolDefinition> {
    if !tool_calling_enabled {
        return vec![];
    }

    let relevant = if tool_rag_enabled {
        if let Some(embedder) = embedding_provider.as_ref() {
            registry
                .select_tools(embedder.as_ref(), user_input, tool_rag_limit)
                .await
        } else {
            registry.list_tools()
        }
    } else {
        registry.list_tools()
    };

    if !tool_rag_always_include.is_empty() {
        let all_tools = registry.list_tools();
        let all_map: std::collections::HashMap<String, _> =
            all_tools.into_iter().map(|t| (t.name.clone(), t)).collect();
        let mut result = relevant;
        for name in tool_rag_always_include {
            if !result.iter().any(|t| &t.name == name) {
                if let Some(tool) = all_map.get(name) {
                    result.push(tool.clone());
                }
            }
        }
        result
    } else {
        relevant
    }
}

/// Fetches recalled summaries and key facts from the memory store for the
/// current session context.
pub async fn fetch_memory_context(
    session: &ConversationSession,
    config: &EneConfig,
) -> (Vec<RecalledSummary>, Vec<ene_memory::KeyFact>) {
    let mem_config = config
        .get_section::<ene_memory::MemoryConfig>()
        .unwrap_or_default();
    let session_config = config
        .get_section::<ene_session::SessionConfig>()
        .unwrap_or_default();

    if !mem_config.enabled {
        return (vec![], vec![]);
    }

    let Some(store) = &session.memory.memory_store else {
        return (vec![], vec![]);
    };

    let Some(pending_embedding) = &session.memory.pending_embedding else {
        return (vec![], vec![]);
    };

    store
        .recall_context(
            session.card_name(),
            pending_embedding,
            session_config.summary_recall_limit,
            mem_config.similarity_threshold,
        )
        .unwrap_or_else(|e| {
            tracing::error!("[Memory] Context recall error: {}", e);
            (vec![], vec![])
        })
}

/// Builds the full list of chat completion request messages for the AI stream.
pub fn build_chat_messages_list(
    session: &ConversationSession,
    config: &EneConfig,
    user_input: &str,
    recalled_summaries: &[RecalledSummary],
    key_facts: &[ene_memory::KeyFact],
) -> Result<Vec<LlmMessage>, crate::error::EneCoreError> {
    let Some(card) = session.character_card.as_ref() else {
        return Err(crate::error::EneCoreError::NoCharacterCard);
    };
    let history = session.history.conversation_history.clone();
    let runtime_rules = config.runtime_rules.clone();
    let user_name = config.user_name.clone();

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
    .map_err(|e| crate::error::EneCoreError::PromptBuildError(e))
}

/// Executes a batch of tool calls and sends result events through the channel.
pub async fn perform_tool_executions(
    registry: &dyn ene_tool_host::ToolRegistry,
    session_id: &str,
    tool_calls: Vec<LlmToolCall>,
    assistant_content: &str,
    tx: tokio::sync::mpsc::UnboundedSender<EneStreamEvent>,
) -> Result<Vec<LlmMessage>, crate::error::EneCoreError> {
    let mut round_messages = Vec::new();

    round_messages.push(LlmMessage::Assistant {
        content: if assistant_content.is_empty() {
            None
        } else {
            Some(assistant_content.to_string())
        },
        tool_calls: Some(tool_calls.clone()),
    });

    registry.set_session_id(session_id).await;

    for call in tool_calls {
        let name = call.name.clone();
        let args = call.arguments.clone();

        let _ = tx.send(EneStreamEvent::ToolCallStart {
            name: name.clone(),
            arguments: args.clone(),
        });

        let tool_timeout = std::time::Duration::from_secs(60);
        let mut result =
            match tokio::time::timeout(tool_timeout, registry.call_tool(&name, &args)).await {
                Ok(Ok(res)) => Ok(res),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(ene_tool_host::error::ToolError::Other(format!(
                    "Tool '{}' timed out after {} seconds",
                    name,
                    tool_timeout.as_secs()
                ))),
            };

        if let Err(ene_tool_host::error::ToolError::PermissionRequired {
            request_id,
            action,
            target,
            description,
        }) = &result
        {
            let _ = tx.send(EneStreamEvent::PermissionRequired {
                request_id: request_id.clone(),
                action: action.clone(),
                target: target.clone(),
                description: description.clone(),
            });

            let (tx_decision, rx_decision) = tokio::sync::oneshot::channel::<PermissionDecision>();
            register_permission_request(request_id.clone(), tx_decision);

            match rx_decision.await {
                Ok(PermissionDecision::AllowOnce) => {
                    registry.approve_permission(request_id).await;
                    result = registry.call_tool(&name, &args).await;
                }
                Ok(PermissionDecision::AllowSession) => {
                    registry.allow_pattern(action, target).await;
                    registry.approve_permission(request_id).await;
                    result = registry.call_tool(&name, &args).await;
                }
                _ => {
                    result = Err(ene_tool_host::error::ToolError::PermissionDenied(
                        "Permission denied by user".to_string(),
                    ));
                }
            }
        }

        let result_str = match result {
            Ok(res) => res,
            Err(e) => format!("Error executing tool: {}", e),
        };

        let _ = tx.send(EneStreamEvent::ToolCallResult {
            name: name.clone(),
            result: result_str.clone(),
        });

        let (final_tool_text, screenshot_data) = extract_screenshot(&result_str);

        round_messages.push(LlmMessage::Tool {
            tool_call_id: call.id.clone(),
            content: final_tool_text,
        });

        if let Some(b64_data) = screenshot_data {
            round_messages.push(LlmMessage::User {
                parts: vec![
                    UserMessagePart::Text {
                        text: "Here is the screenshot.".to_string(),
                    },
                    UserMessagePart::Image {
                        base64_image_data: b64_data,
                    },
                ],
            });
        }
    }

    Ok(round_messages)
}

/// Runs the full AI streaming completion loop with tool calling, memory
/// retrieval, and session management.
pub async fn run_ene_with_tools(
    config: &EneConfig,
    session: &ConversationSession,
    user_input: &str,
    registry: std::sync::Arc<dyn ene_tool_host::ToolRegistry>,
    provider: std::sync::Arc<dyn ene_provider::LlmProvider>,
) -> Result<impl Stream<Item = EneStreamEvent> + 'static, crate::error::EneCoreError> {
    let mem_config = config
        .get_section::<ene_memory::MemoryConfig>()
        .unwrap_or_default();

    // 1. Fetch memory context (summaries and keyfacts)
    let (recalled_summaries, key_facts) = fetch_memory_context(session, config).await;

    // 2. Insert user log if memory is enabled
    if mem_config.enabled {
        if let Some(store) = &session.memory.memory_store {
            let session_id_log = session.memory.session_id.clone();
            let card_name = session.card_name().to_string();
            ene_memory::MemoryStore::spawn_insert_log(
                store,
                &session_id_log,
                &card_name,
                "user",
                user_input,
            );
        }
    }

    // 3. Build initial messages list
    let mut messages =
        build_chat_messages_list(session, config, user_input, &recalled_summaries, &key_facts)?;

    let memory_enabled = mem_config.enabled;
    let mem_store = if memory_enabled {
        session.memory.memory_store.clone()
    } else {
        None
    };
    let card_name = session.card_name().to_string();
    let session_id = session.memory.session_id.clone();
    let user_input = user_input.to_string();
    let tool_config = config
        .get_section::<ene_tool_host::ToolConfig>()
        .unwrap_or_default();
    let tool_calling_enabled = tool_config.tool_calling_enabled;
    let max_rounds = tool_config.max_tool_call_rounds;
    let session_id_for_tools = session.memory.session_id.clone();
    let tool_rag_enabled = mem_config.tool_rag_enabled;
    let tool_rag_limit = mem_config.tool_rag_limit;
    let tool_rag_always_include = mem_config.tool_rag_always_include.clone();
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
        )
        .await;

        let mut round = 0;

        loop {
            if round >= max_rounds {
                yield EneStreamEvent::Error("Max tool call rounds exceeded".to_string());
                return;
            }

            let mut stream = match provider.create_chat_stream(&messages, &tools).await {
                Ok(s) => s,
                Err(e) => {
                    yield EneStreamEvent::Error(e.to_string());
                    return;
                }
            };

            let mut current_tool_calls: Vec<LlmToolCallChunk> = Vec::new();
            let mut assistant_content = String::new();

            use tokio_stream::StreamExt;
            while let Some(chunk_res) = stream.next().await {
                match chunk_res {
                    Ok(chunk) => {
                        if let Some(content_delta) = &chunk.text_delta {
                            assistant_content.push_str(content_delta);
                            yield EneStreamEvent::TextDelta(content_delta.clone());
                        }

                        if let Some(tool_calls_delta) = &chunk.tool_calls_delta {
                            accumulate_tool_calls(&mut current_tool_calls, tool_calls_delta);
                        }
                    }
                    Err(e) => {
                        yield EneStreamEvent::Error(e.to_string());
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
                        yield EneStreamEvent::Error(e.to_string());
                        return;
                    }
                    Err(e) => {
                        yield EneStreamEvent::Error(format!("Task panicked: {}", e));
                        return;
                    }
                };

                messages.extend(tx_messages);
                round += 1;
                continue;
            } else {
                if !assistant_content.is_empty() {
                    if let Some(store) = &mem_store {
                        ene_memory::MemoryStore::spawn_insert_log(
                            store,
                            &session_id,
                            &card_name,
                            "assistant",
                            &assistant_content,
                        );
                    }
                }

                yield EneStreamEvent::Finished;
                return;
            }
        }
    })
}

fn accumulate_tool_calls(
    current_tool_calls: &mut Vec<LlmToolCallChunk>,
    tool_calls_delta: &[LlmToolCallChunk],
) {
    for tc_delta in tool_calls_delta {
        let idx = tc_delta.index;
        if idx >= current_tool_calls.len() {
            current_tool_calls.resize(
                idx + 1,
                LlmToolCallChunk {
                    index: tc_delta.index,
                    id: None,
                    name: None,
                    arguments: None,
                },
            );
        }
        let target = &mut current_tool_calls[idx];
        if let Some(id) = &tc_delta.id {
            target.id = Some(id.clone());
        }
        if let Some(name) = &tc_delta.name {
            target.name = Some(target.name.clone().unwrap_or_default() + name);
        }
        if let Some(args) = &tc_delta.arguments {
            target.arguments = Some(target.arguments.clone().unwrap_or_default() + args);
        }
    }
}

fn finalize_tool_calls(current_tool_calls: Vec<LlmToolCallChunk>) -> Vec<LlmToolCall> {
    let mut tool_calls = Vec::new();
    for tc in current_tool_calls {
        tool_calls.push(LlmToolCall {
            id: tc.id.unwrap_or_default(),
            name: tc.name.unwrap_or_default(),
            arguments: tc.arguments.unwrap_or_default(),
        });
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
