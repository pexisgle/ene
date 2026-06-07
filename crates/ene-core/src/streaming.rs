use crate::handle::{ConversationEntry, EneEvent};
use crate::message_builder::{MessageBuildContext, build_messages};
use crate::types::RequestId;
use ene_config::EneConfig;
use ene_memory::RecalledSummary;
use ene_provider::{LlmMessage, LlmToolCall, LlmToolCallChunk, UserMessagePart};
use ene_session::ConversationSession;
use ene_tool_proto::ToolError;
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

/// Re-exported from `ene-tool-proto` so consumers of `ene-core` only need to
/// import one crate.
#[doc(no_inline)]
pub use ene_tool_proto::MultiAnswer;

/// Represents a user's response to an interactive tool's input request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserInputResponse {
    /// One answer per sub-question in the prompt, in the same order.
    /// For a single-question prompt this is `Multi(vec![..])`.
    Multi(Vec<MultiAnswer>),
    /// The user dismissed or cancelled the entire prompt.
    Cancel,
}

/// Configuration for a single AI streaming run.
pub(crate) struct StreamContext {
    pub(crate) config: EneConfig,
    pub(crate) session: ConversationSession,
    pub(crate) user_input: String,
    pub(crate) registry: Arc<dyn ene_tool_host::ToolRegistry>,
    pub(crate) tool_rag: Option<Arc<ene_tool_host::ToolRag>>,
    pub(crate) provider: Arc<dyn ene_provider::LlmProvider>,
    pub(crate) event_tx: broadcast::Sender<EneEvent>,
    pub(crate) cancel_token: CancellationToken,
    pub(crate) pending_permissions:
        Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    pub(crate) pending_user_inputs:
        Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
}

/// Runs the full AI streaming completion loop with tool calling, memory
/// retrieval, and session management. Sends events through the broadcast channel.
pub(crate) async fn run_stream(ctx: StreamContext) -> ConversationSession {
    let StreamContext {
        config,
        mut session,
        user_input,
        registry,
        tool_rag,
        provider,
        event_tx,
        cancel_token,
        pending_permissions,
        pending_user_inputs,
    } = ctx;
    session.reset_display_buffer();

    let mem_config = config
        .get_section::<ene_memory::MemoryConfig>()
        .unwrap_or_default();

    // 1. Fetch memory context
    let (recalled_summaries, key_facts) = fetch_memory_context(&session, &config).await;

    // 2. Insert user log if memory enabled
    if mem_config.enabled
        && let Some(store) = &session.memory.memory_store
    {
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

    // 4. Select relevant tools
    let tools = select_relevant_tools(
        registry.as_ref(),
        tool_rag.as_deref(),
        &user_input,
        tool_calling_enabled,
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
                let _ = event_tx.send(EneEvent::Failed { message: e.clone() });
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
                    let _ = event_tx.send(EneEvent::Failed { message: e.clone() });
                    return session;
                }
            }
        }

        if current_tool_calls.is_empty() {
            // No tool calls - stream is done
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
            &pending_user_inputs,
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

/// Selects relevant tools using the `ToolRag` pipeline if available,
/// otherwise falls back to the registry's `select_tools` or `list_tools`.
pub(crate) async fn select_relevant_tools(
    registry: &dyn ene_tool_host::ToolRegistry,
    tool_rag: Option<&ene_tool_host::ToolRag>,
    user_input: &str,
    tool_calling_enabled: bool,
) -> Vec<ene_tool_proto::ToolSpec> {
    if !tool_calling_enabled {
        return vec![];
    }

    // Use the new ToolRag pipeline if available.
    if let Some(rag) = tool_rag {
        // Get all tools from the registry.
        let all_tools = registry.list_tools();

        // Ensure the index is up-to-date (no-op for already-indexed fields).
        if let Err(e) = rag.ensure_index(&all_tools).await {
            tracing::warn!("[ToolRag] ensure_index failed: {}", e);
        }

        // Select relevant tools via the RAG pipeline.
        return rag.select(user_input).await;
    }

    // Fallback: no ToolRag, return all tools from the registry.
    registry.list_tools()
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

    let store = Arc::clone(store);
    let card_name = session.card_name().to_string();
    let pending_embedding = pending_embedding.clone();

    tokio::task::spawn_blocking(move || {
        store
            .recall_context(
                &card_name,
                &pending_embedding,
                session_config.summary_recall_limit,
                mem_config.similarity_threshold,
            )
            .unwrap_or_else(|e| {
                tracing::error!("[Memory] Context recall error: {}", e);
                (vec![], vec![])
            })
    })
    .await
    .unwrap_or_else(|_| (vec![], vec![]))
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
    pending_user_inputs: &Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
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
                Err(_) => Err(ToolError::Other {
                    message: format!(
                        "Tool '{}' timed out after {:.2} seconds",
                        name,
                        tool_timeout.as_secs_f64()
                    ),
                }),
            };

        if let Err(ToolError::PermissionRequired {
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
                    result = Err(ToolError::PermissionDenied {
                        message: "Permission denied by user".to_string(),
                    });
                }
            }
        }

        if let Err(ToolError::UserInputRequired { request_id, prompt }) = &result {
            let req_id = RequestId::from(request_id.clone());
            let _ = event_tx.send(EneEvent::UserInputRequired {
                request_id: req_id.clone(),
                prompt: prompt.clone(),
            });

            let (resp_tx, resp_rx) = oneshot::channel::<UserInputResponse>();
            {
                let mut guard = pending_user_inputs.lock().await;
                guard.insert(req_id.clone(), resp_tx);
            }

            match resp_rx.await {
                Ok(UserInputResponse::Multi(answers)) => {
                    let new_args = inject_user_answers(&args, &answers);
                    result = registry.call_tool(&name, &new_args).await;
                }
                Ok(UserInputResponse::Cancel) | Err(_) => {
                    result = Err(ToolError::ExecutionFailed {
                        message: "User cancelled the question".to_string(),
                    });
                }
            }
        }

        let result_str = match result {
            Ok(res) => res,
            Err(e) => format!("Error executing tool: {e}"),
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
    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(result)
        && json_val.get("type").and_then(|v| v.as_str()) == Some("screenshot")
        && let Some(data) = json_val.get("data").and_then(|v| v.as_str())
    {
        return (
            "[Screenshot successfully captured and sent to vision system]".to_string(),
            Some(data.to_string()),
        );
    }
    (result.to_string(), None)
}

/// Injects the user's per-question answers into a tool's JSON argument string
/// under the `_user_answers` key. Each entry is a [`MultiAnswer`] and is
/// serialised in the order the user answered (i.e. the order of the prompt's
/// `items`). Falls back to wrapping the original string under `_original_args`
/// if the args are not a JSON object, so the re-invocation does not crash.
fn inject_user_answers(args_json: &str, answers: &[MultiAnswer]) -> String {
    let answers_value = match serde_json::to_value(answers) {
        Ok(v) => v,
        Err(_) => serde_json::Value::Array(Vec::new()),
    };
    let parsed: Option<serde_json::Value> = serde_json::from_str(args_json).ok();
    if let Some(mut value) = parsed {
        if let Some(obj) = value.as_object_mut() {
            obj.insert("_user_answers".to_string(), answers_value);
        } else {
            let mut obj = serde_json::Map::new();
            obj.insert("_user_answers".to_string(), answers_value);
            obj.insert("_original_args".to_string(), value);
            value = serde_json::Value::Object(obj);
        }
        serde_json::to_string(&value).unwrap_or_else(|_| args_json.to_string())
    } else {
        let mut obj = serde_json::Map::new();
        obj.insert("_user_answers".to_string(), answers_value);
        obj.insert(
            "_original_args".to_string(),
            serde_json::Value::String(args_json.to_string()),
        );
        serde_json::to_string(&serde_json::Value::Object(obj))
            .unwrap_or_else(|_| args_json.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_answers() -> Vec<MultiAnswer> {
        vec![
            MultiAnswer::Selected {
                option: "yes".into(),
            },
            MultiAnswer::Answer {
                text: "alice".into(),
            },
            MultiAnswer::Skip,
        ]
    }

    #[test]
    fn inject_into_object() {
        let out = inject_user_answers(r#"{"a":1}"#, &sample_answers());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["a"], 1);
        let arr = v["_user_answers"].as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["kind"], "selected");
        assert_eq!(arr[0]["option"], "yes");
        assert_eq!(arr[1]["kind"], "answer");
        assert_eq!(arr[1]["text"], "alice");
        assert_eq!(arr[2]["kind"], "skip");
    }

    #[test]
    fn inject_into_invalid_json() {
        let out = inject_user_answers("not-json", &sample_answers());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["_user_answers"].is_array());
        assert_eq!(v["_original_args"], "not-json");
    }

    #[test]
    fn inject_into_non_object_json() {
        let out = inject_user_answers("[1,2,3]", &sample_answers());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["_user_answers"].is_array());
        assert_eq!(v["_original_args"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn multi_answer_serde_roundtrip() {
        let answers = sample_answers();
        let json = serde_json::to_string(&answers).unwrap();
        let de: Vec<MultiAnswer> = serde_json::from_str(&json).unwrap();
        assert_eq!(de, answers);
    }
}
