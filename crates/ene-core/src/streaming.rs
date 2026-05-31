use crate::handle::{ConversationEntry, EneEvent};
use crate::message_builder::{MessageBuildContext, build_messages};
use crate::types::RequestId;
use ene_config::EneConfig;
use ene_memory::RecalledSummary;
use ene_provider::{LlmMessage, LlmToolCall, LlmToolCallChunk, UserMessagePart};
use ene_session::ConversationSession;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

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

/// Configuration for tool RAG (retrieval-augmented generation) filtering.
#[derive(Debug, Clone)]
pub(crate) struct ToolRagConfig {
    /// Whether tool calling is enabled at all.
    pub tool_calling_enabled: bool,
    /// Whether RAG-based tool filtering is enabled.
    pub tool_rag_enabled: bool,
    /// Maximum number of tools to return via RAG.
    pub tool_rag_limit: usize,
    /// Tool names that should always be included regardless of RAG.
    pub tool_rag_always_include: Vec<String>,
}

/// Configuration for a single AI streaming run.
pub(crate) struct StreamContext {
    pub(crate) config: EneConfig,
    pub(crate) session: ConversationSession,
    pub(crate) user_input: String,
    pub(crate) registry: Arc<dyn ene_tool_host::ToolRegistry>,
    pub(crate) provider: Arc<dyn ene_provider::LlmProvider>,
    pub(crate) event_tx: broadcast::Sender<EneEvent>,
    pub(crate) cancel_token: CancellationToken,
    pub(crate) pending_permissions:
        Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
}

/// Runs the full AI streaming completion loop with tool calling, memory
/// retrieval, and session management. Sends events through the broadcast channel.
pub(crate) async fn run_stream(ctx: StreamContext) -> ConversationSession {
    let StreamContext {
        config,
        mut session,
        user_input,
        registry,
        provider,
        event_tx,
        cancel_token,
        pending_permissions,
    } = ctx;
    session.reset_display_buffer();

    let mem_config = config
        .get_section::<ene_memory::MemoryConfig>()
        .unwrap_or_default();

    // 1. Fetch memory context
    let (recalled_summaries, key_facts) = fetch_memory_context(&session, &config).await;

    // 2. Insert user log if memory enabled
    if mem_config.enabled {
        if let Some(store) = &session.memory.memory_store {
            let session_id_log = session.memory.session_id.clone();
            let card_name = session.card_name().to_string();
            ene_memory::MemoryStore::spawn_insert_log(
                store,
                session_id_log.as_str(),
                &card_name,
                "user",
                &user_input,
            );
        }
    }

    // 3. Build initial messages
    let mut messages = match build_chat_messages_list(
        &session,
        &config,
        &user_input,
        &recalled_summaries,
        &key_facts,
    ) {
        Ok(msgs) => msgs,
        Err(e) => {
            let _ = event_tx.send(EneEvent::Failed {
                message: e.to_string(),
            });
            return session;
        }
    };

    let memory_enabled = mem_config.enabled;
    let mem_store = if memory_enabled {
        session.memory.memory_store.clone()
    } else {
        None
    };
    let card_name = session.card_name().to_string();
    let session_id = session.memory.session_id.clone();
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

    // 4. Select relevant tools
    let tools = select_relevant_tools(
        registry.as_ref(),
        &embedding_provider,
        &user_input,
        &ToolRagConfig {
            tool_calling_enabled,
            tool_rag_enabled,
            tool_rag_limit,
            tool_rag_always_include,
        },
    )
    .await;

    let mut round = 0usize;

    loop {
        if cancel_token.is_cancelled() {
            return session;
        }

        if round >= max_rounds {
            let _ = event_tx.send(EneEvent::Failed {
                message: "Max tool call rounds exceeded".to_string(),
            });
            return session;
        }

        let mut stream = match provider.create_chat_stream(&messages, &tools).await {
            Ok(s) => s,
            Err(e) => {
                let _ = event_tx.send(EneEvent::Failed {
                    message: e.to_string(),
                });
                return session;
            }
        };

        let mut current_tool_calls: Vec<LlmToolCallChunk> = Vec::new();
        let mut assistant_content = String::new();

        while let Some(chunk_res) = stream.next().await {
            if cancel_token.is_cancelled() {
                let _ = event_tx.send(EneEvent::Done);
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
                    let _ = event_tx.send(EneEvent::Failed {
                        message: e.to_string(),
                    });
                    return session;
                }
            }
        }

        if current_tool_calls.is_empty() {
            // No tool calls - stream is done
            if !assistant_content.is_empty() {
                if let Some(store) = &mem_store {
                    ene_memory::MemoryStore::spawn_insert_log(
                        store,
                        session_id.as_str(),
                        &card_name,
                        "assistant",
                        &assistant_content,
                    );
                }
            }

            session.finalize_response();
            session.record_assistant_response();
            let _ = event_tx.send(EneEvent::Done);
            return session;
        }

        // Tool calls needed
        let tool_calls = finalize_tool_calls(current_tool_calls);

        let tx_messages = perform_tool_executions(
            registry.as_ref(),
            session_id_for_tools.as_str(),
            tool_calls,
            &assistant_content,
            &event_tx,
            &pending_permissions,
            tool_config.tool_call_timeout_ms,
        )
        .await;

        match tx_messages {
            Ok(msgs) => {
                messages.extend(msgs);
                round += 1;
            }
            Err(e) => {
                let _ = event_tx.send(EneEvent::Failed {
                    message: e.to_string(),
                });
                return session;
            }
        }
    }
}

/// Selects relevant tools using embedding-based RAG if enabled, otherwise
/// returns all tools.
pub(crate) async fn select_relevant_tools(
    registry: &dyn ene_tool_host::ToolRegistry,
    embedding_provider: &Option<Arc<dyn ene_provider::EmbeddingProvider>>,
    user_input: &str,
    rag_config: &ToolRagConfig,
) -> Vec<ene_tool_host::ToolDefinition> {
    if !rag_config.tool_calling_enabled {
        return vec![];
    }

    let relevant = if rag_config.tool_rag_enabled {
        if let Some(embedder) = embedding_provider.as_ref() {
            registry
                .select_tools(embedder.as_ref(), user_input, rag_config.tool_rag_limit)
                .await
        } else {
            registry.list_tools()
        }
    } else {
        registry.list_tools()
    };

    if !rag_config.tool_rag_always_include.is_empty() {
        let all_tools = registry.list_tools();
        let all_map: std::collections::HashMap<String, _> =
            all_tools.into_iter().map(|t| (t.name.clone(), t)).collect();
        let mut result = relevant;
        for name in &rag_config.tool_rag_always_include {
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
pub(crate) async fn fetch_memory_context(
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
pub(crate) fn build_chat_messages_list(
    session: &ConversationSession,
    config: &EneConfig,
    user_input: &str,
    recalled_summaries: &[RecalledSummary],
    key_facts: &[ene_memory::KeyFact],
) -> Result<Vec<LlmMessage>, crate::error::EneCoreError> {
    let Some(card) = session.character_card.as_ref() else {
        return Err(crate::error::EneCoreError::NoCharacterCard);
    };
    let history: Vec<ConversationEntry> = session
        .history()
        .iter()
        .map(|(role, content)| ConversationEntry {
            role: *role,
            content: content.clone(),
        })
        .collect();
    let runtime_rules = config.runtime_rules.clone();
    let user_name = config.user_name.clone();

    build_messages(&MessageBuildContext {
        card,
        user_input,
        history: &history,
        runtime_context: None,
        runtime_rules: &runtime_rules,
        user_name: &user_name,
        recalled_summaries,
        key_facts,
    })
}

/// Executes a batch of tool calls and sends result events through the broadcast channel.
async fn perform_tool_executions(
    registry: &dyn ene_tool_host::ToolRegistry,
    session_id: &str,
    tool_calls: Vec<LlmToolCall>,
    assistant_content: &str,
    event_tx: &broadcast::Sender<EneEvent>,
    pending_permissions: &Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    timeout_ms: u64,
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

        let _ = event_tx.send(EneEvent::ToolCallStart {
            name: name.clone(),
            arguments: args.clone(),
        });

        let tool_timeout = std::time::Duration::from_millis(timeout_ms);
        let mut result =
            match tokio::time::timeout(tool_timeout, registry.call_tool(&name, &args)).await {
                Ok(Ok(res)) => Ok(res),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(ene_tool_host::error::ToolError::Other(format!(
                    "Tool '{}' timed out after {:.2} seconds",
                    name,
                    tool_timeout.as_secs_f64()
                ))),
            };

        if let Err(ene_tool_host::error::ToolError::PermissionRequired {
            request_id,
            action,
            target,
            description,
        }) = &result
        {
            let req_id = RequestId::from(request_id.clone());
            let _ = event_tx.send(EneEvent::PermissionRequired {
                request_id: req_id.clone(),
                action: action.clone(),
                target: target.clone(),
                description: description.clone(),
            });

            let (decide_tx, decide_rx) = oneshot::channel::<PermissionDecision>();
            {
                let mut guard = pending_permissions.lock().await;
                guard.insert(req_id.clone(), decide_tx);
            }

            match decide_rx.await {
                Ok(PermissionDecision::AllowOnce) => {
                    registry.approve_permission(req_id.as_str()).await;
                    result = registry.call_tool(&name, &args).await;
                }
                Ok(PermissionDecision::AllowSession) => {
                    registry.allow_pattern(action, target).await;
                    registry.approve_permission(req_id.as_str()).await;
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

        let _ = event_tx.send(EneEvent::ToolCallResult {
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
