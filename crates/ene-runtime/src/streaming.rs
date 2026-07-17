use crate::diagnostics::DiagnosticEvent;
use crate::handle::{EneEvent, TerminalReason};
use crate::types::{RequestId, TurnId};
use ene_ai::{LlmMessage, LlmToolCall, LlmToolCallChunk, UserMessagePart};
use ene_config::EneConfig;
use ene_mind::ConversationSession;
use ene_mind::memory_writer::{ToolResultSummary, tool_grounding};
use ene_tool_host::ToolHostError;
use ene_tool_proto::ToolError;
use ene_tool_rag::ToolRag;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Phase name for vectorization (embedding generation).
pub const PHASE_EMBEDDING: &str = "Embedding";
/// Phase name for context search (memory and tools retrieval).
pub const PHASE_CONTEXT_SEARCH: &str = "Context Search";
/// Phase name for prompt building.
pub const PHASE_PROMPT_BUILDING: &str = "Prompt Building";

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

/// Re-exported from `ene-tool-proto` so consumers of `ene-runtime` only need to
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

/// Output of one tool execution round.
#[derive(Debug, Default)]
pub(crate) struct ToolExecutionOutput {
    /// Assistant/tool messages fed back to the LLM loop.
    pub messages: Vec<LlmMessage>,
    /// Bounded summaries for cognitive memory grounding.
    pub summaries: Vec<ToolResultSummary>,
}

/// Atomically claim the right to emit the terminal event for the
/// current run, and emit [`EneEvent::Terminal`] if the claim
/// succeeds. If the cancel command (or another emit site) has
/// already emitted a terminal, this is a no-op so exactly one
/// terminal event is delivered per run.
pub(crate) fn emit_terminal(
    event_tx: &broadcast::Sender<EneEvent>,
    guard: &AtomicBool,
    turn: &TurnId,
    reason: TerminalReason,
) {
    if guard
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let _ = event_tx.send(EneEvent::Terminal {
            turn: turn.clone(),
            reason,
        });
    }
}

/// Configuration for a single AI streaming run.
#[doc(hidden)]
pub struct StreamContext {
    pub config: EneConfig,
    pub session: ConversationSession,
    pub user_input: String,
    pub embedder: Option<Arc<dyn ene_ai::EmbeddingProvider>>,
    pub registry: Arc<dyn ene_tool_host::ToolRegistry>,
    pub tool_rag: Option<Arc<ToolRag>>,
    pub provider: Arc<dyn ene_ai::LlmProvider>,
    pub event_tx: broadcast::Sender<EneEvent>,
    pub diag_tx: broadcast::Sender<DiagnosticEvent>,
    pub cancel_token: CancellationToken,
    pub pending_permissions: Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    pub pending_user_inputs: Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
    /// Shared with the actor; first side to flip emits Terminal.
    pub terminal_emitted: Arc<std::sync::atomic::AtomicBool>,
    /// Active turn id for all turn-scoped events.
    pub turn: TurnId,
    /// Sender for classifier `JoinHandles` spawned after Terminal emission.
    /// The actor drains this into its classifier `JoinSet` for lifecycle management.
    pub classifier_tx: mpsc::UnboundedSender<tokio::task::JoinHandle<()>>,
}

/// Runs the full AI streaming completion loop with tool calling, optional memory
/// retrieval, and session management. Sends events through the broadcast channel.
///
/// Chat works with `store.enabled = false` (text + tools only). When the store
/// and embedder are present, recall/write run on the cognitive path. There is
/// no legacy streaming fallback. Opening with memory enabled still fails closed
/// if the embedder cannot be initialized.
#[doc(hidden)]
pub use crate::streaming_cognitive::run_stream_cognitive;

/// Selects relevant tools using the `ToolRag` pipeline if available,
/// otherwise falls back to the registry's `select_tools` or `list_tools`.
pub(crate) async fn select_relevant_tools(
    registry: &dyn ene_tool_host::ToolRegistry,
    tool_rag: Option<&ToolRag>,
    user_input: &str,
    query_embedding: Option<&[f32]>,
    tool_calling_enabled: bool,
) -> Vec<ene_tool_proto::ToolSpec> {
    if !tool_calling_enabled {
        return vec![];
    }

    // Use the new ToolRag pipeline if available.
    if let Some(rag) = tool_rag {
        // Get all tools and RAG profiles from the registry.
        let all_tools = registry.list_tools();
        let profiles = registry.list_rag_profiles();

        // Ensure the index is up-to-date (no-op for already-indexed fields).
        if let Err(e) = rag.ensure_index(&all_tools, &profiles).await {
            tracing::warn!(component = "ToolRag", error = %e, "ensure_index failed");
        }

        // Select relevant tools via the RAG pipeline.
        let res = if let Some(emb) = query_embedding {
            rag.select_with_embedding(user_input, emb).await
        } else {
            rag.select(user_input).await
        };
        return res;
    }

    // Fallback: no ToolRag, return all tools from the registry.
    registry.list_tools()
}

/// Executes a batch of tool calls and sends result events through the broadcast channel.
pub(crate) async fn perform_tool_executions(
    registry: &dyn ene_tool_host::ToolRegistry,
    session_id: &str,
    tool_calls: Vec<LlmToolCall>,
    assistant_content: &str,
    event_tx: &broadcast::Sender<EneEvent>,
    turn: &crate::types::TurnId,
    pending_permissions: &Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    pending_user_inputs: &Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
    timeout_ms: u64,
    max_summary_chars: usize,
) -> Result<ToolExecutionOutput, crate::error::EneRuntimeError> {
    let mut round_messages = Vec::new();
    let mut summaries = Vec::new();

    round_messages.push(LlmMessage::Assistant {
        content: if assistant_content.is_empty() {
            None
        } else {
            Some(assistant_content.to_string())
        },
        tool_calls: Some(tool_calls.clone()),
    });

    registry
        .set_call_context(&ene_tool_proto::CallContext {
            conversation_id: session_id.to_string(),
            turn_id: turn.to_string(),
        })
        .await;

    for call in tool_calls {
        let name = call.name.clone();
        let args = call.arguments.clone();

        let _ = event_tx.send(EneEvent::ToolCallStart {
            turn: turn.clone(),
            name: name.clone(),
            arguments: args.clone(),
        });

        let tool_timeout = std::time::Duration::from_millis(timeout_ms);
        let mut result =
            match tokio::time::timeout(tool_timeout, registry.call_tool(&name, &args)).await {
                Ok(Ok(res)) => Ok(res),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(ToolHostError::ExecutionFailed {
                    message: format!(
                        "Tool '{}' timed out after {:.2} seconds",
                        name,
                        tool_timeout.as_secs_f64()
                    ),
                }),
            };

        // A tool may chain a permission prompt followed by a
        // user-input prompt (or vice-versa) on a single call.
        // Resolve pending requests in a loop with a hard cap
        // so a buggy tool that ping-pongs between the two
        // does not lock up the stream.
        const MAX_PENDING_ROUNDS: usize = 8;
        for _ in 0..MAX_PENDING_ROUNDS {
            match &result {
                Err(ToolHostError::Protocol(ToolError::PermissionRequired {
                    request_id,
                    action,
                    target,
                    description,
                })) => {
                    let req_id = RequestId::from(request_id.clone());

                    // Register the oneshot::Sender BEFORE
                    // emitting EneEvent::PermissionRequired. A
                    // consumer that synchronously replies to
                    // the event (e.g. an automated/headless
                    // test) can otherwise race ahead of this
                    // task, send EneCommand::PermissionDecision,
                    // and hit the lookup in handle.rs when the
                    // map is still empty — the decision would
                    // then be silently dropped and the stream
                    // would await forever below.
                    let (decide_tx, decide_rx) = oneshot::channel::<PermissionDecision>();
                    {
                        let mut guard = pending_permissions.lock().await;
                        guard.insert(req_id.clone(), decide_tx);
                    }

                    let _ = event_tx.send(EneEvent::PermissionRequired {
                        turn: turn.clone(),
                        request_id: req_id.clone(),
                        action: action.clone(),
                        target: target.clone(),
                        description: description.clone(),
                    });

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
                            result = Err(ToolHostError::Protocol(ToolError::PermissionDenied {
                                message: "Permission denied by user".to_string(),
                            }));
                            // Decision resolved; no further
                            // pending rounds needed.
                            break;
                        }
                    }
                }
                Err(ToolHostError::Protocol(ToolError::UserInputRequired {
                    request_id,
                    prompt,
                })) => {
                    let req_id = RequestId::from(request_id.clone());

                    // Register the oneshot::Sender BEFORE
                    // emitting EneEvent::UserInputRequired for
                    // the same race reason as the permission
                    // branch above.
                    let (resp_tx, resp_rx) = oneshot::channel::<UserInputResponse>();
                    {
                        let mut guard = pending_user_inputs.lock().await;
                        guard.insert(req_id.clone(), resp_tx);
                    }

                    let _ = event_tx.send(EneEvent::UserInputRequired {
                        turn: turn.clone(),
                        request_id: req_id.clone(),
                        prompt: prompt.clone(),
                    });

                    match resp_rx.await {
                        Ok(UserInputResponse::Multi(answers)) => {
                            let new_args = inject_user_answers(&args, &answers);
                            result = registry.call_tool(&name, &new_args).await;
                        }
                        Ok(UserInputResponse::Cancel) | Err(_) => {
                            result = Err(ToolHostError::ExecutionFailed {
                                message: "User cancelled the question".to_string(),
                            });
                            // Decision resolved; no further
                            // pending rounds needed.
                            break;
                        }
                    }
                }
                _ => break, // Ok or other Err; stop resolving.
            }
        }

        let (result_str, success) = match result {
            Ok(res) => (res, true),
            Err(e) => (format!("Error executing tool: {e}"), false),
        };

        let _ = event_tx.send(EneEvent::ToolCallResult {
            turn: turn.clone(),
            name: name.clone(),
            result: result_str.clone(),
        });

        summaries.push(tool_grounding::summarize_tool_result(
            &name,
            &result_str,
            success,
            max_summary_chars,
        ));

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

    Ok(ToolExecutionOutput {
        messages: round_messages,
        summaries,
    })
}

pub(crate) fn accumulate_tool_calls(
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

pub(crate) fn finalize_tool_calls(current_tool_calls: Vec<LlmToolCallChunk>) -> Vec<LlmToolCall> {
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
#[expect(
    clippy::significant_drop_tightening,
    reason = "test intentionally keeps lock held across helper calls"
)]
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

    #[tokio::test]
    async fn perform_tool_executions_respects_summary_char_limit() {
        use ene_ai::LlmToolCall;
        use ene_tool_host::ToolRegistry;

        struct AlwaysOk {
            output: String,
        }

        #[async_trait::async_trait]
        impl ToolRegistry for AlwaysOk {
            fn list_tools(&self) -> Vec<ene_tool_proto::ToolSpec> {
                Vec::new()
            }

            async fn call_tool(
                &self,
                _name: &str,
                _arguments: &str,
            ) -> Result<String, ToolHostError> {
                Ok(self.output.clone())
            }

            async fn approve_permission(&self, _request_id: &str) {}
        }

        let registry = AlwaysOk {
            output: "x".repeat(64),
        };
        let (event_tx, _event_rx) = tokio::sync::broadcast::channel::<EneEvent>(16);
        let pending_permissions: Arc<
            Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>,
        > = Arc::new(Mutex::new(HashMap::new()));
        let pending_user_inputs: Arc<
            Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>,
        > = Arc::new(Mutex::new(HashMap::new()));
        let tool_calls = vec![LlmToolCall {
            id: "call-1".to_string(),
            name: "fs.write".to_string(),
            arguments: "{}".to_string(),
        }];

        let turn = crate::types::TurnId::new();
        let output = perform_tool_executions(
            &registry,
            "session-1",
            tool_calls,
            "",
            &event_tx,
            &turn,
            &pending_permissions,
            &pending_user_inputs,
            5_000,
            10,
        )
        .await
        .expect("tool executions");

        assert_eq!(output.summaries.len(), 1);
        assert!(
            output.summaries[0].summary.chars().count() <= 13,
            "summary should be truncated to max+ellipsis, got: {:?}",
            output.summaries[0].summary
        );
    }

    /// Regression test for #35: a fast consumer that receives
    /// `EneEvent::PermissionRequired` and immediately sends a
    /// `PermissionDecision` must never race ahead of the executor's
    /// insert into `pending_permissions`. Before the fix the
    /// executor emitted the event before registering the oneshot,
    /// so a synchronous consumer's decision was dropped at the
    /// actor and the executor then awaited forever.
    #[tokio::test]
    async fn fast_consumer_does_not_lose_permission_decision() {
        use ene_ai::LlmToolCall;
        use ene_tool_host::ToolRegistry;
        use ene_tool_proto::ToolError;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct PermissionThenOk {
            request_id: String,
            calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl ToolRegistry for PermissionThenOk {
            fn list_tools(&self) -> Vec<ene_tool_proto::ToolSpec> {
                Vec::new()
            }
            async fn call_tool(
                &self,
                _name: &str,
                _arguments: &str,
            ) -> Result<String, ToolHostError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err(ToolHostError::Protocol(ToolError::PermissionRequired {
                        request_id: self.request_id.clone(),
                        action: "fs.delete".to_string(),
                        target: "/tmp/x".to_string(),
                        description: "delete file".to_string(),
                    }))
                } else {
                    Ok("ok".to_string())
                }
            }
            async fn approve_permission(&self, _request_id: &str) {}
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let registry = PermissionThenOk {
            request_id: "req-1".to_string(),
            calls: calls.clone(),
        };

        let (event_tx, _event_rx) = tokio::sync::broadcast::channel::<EneEvent>(16);
        let pending_permissions: Arc<
            Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>,
        > = Arc::new(Mutex::new(HashMap::new()));
        let pending_user_inputs: Arc<
            Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>,
        > = Arc::new(Mutex::new(HashMap::new()));

        let tool_calls = vec![LlmToolCall {
            id: "call-1".to_string(),
            name: "fs.delete".to_string(),
            arguments: "{}".to_string(),
        }];

        // Start a fast consumer that subscribes to the event stream
        // and replies with a decision as soon as the event is seen.
        // The decision path here mirrors the actor in
        // `ene-runtime::handle::EneActor::run`: remove the entry from
        // the shared map and send on the oneshot.
        let consumer_event_rx = event_tx.subscribe();
        let consumer_perms = pending_permissions.clone();
        let consumer = tokio::spawn(async move {
            let mut rx = consumer_event_rx;
            while let Ok(ev) = rx.recv().await {
                if let EneEvent::PermissionRequired { request_id, .. } = ev {
                    let mut guard = consumer_perms.lock().await;
                    if let Some(tx) = guard.remove(&request_id) {
                        let _ = tx.send(PermissionDecision::AllowOnce);
                    }
                    return;
                }
            }
        });

        let turn = crate::types::TurnId::new();
        let exec = perform_tool_executions(
            &registry,
            "session-1",
            tool_calls,
            "",
            &event_tx,
            &turn,
            &pending_permissions,
            &pending_user_inputs,
            5_000,
            500,
        );

        // Bound the test: if the fix regresses, the executor would
        // await forever on the dropped decision and this would
        // time out rather than hang the suite.
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), exec).await;
        let _ = consumer.await;

        let result = result.expect("executor hung: decision was dropped");
        let output = result.expect("executor returned error");
        assert_eq!(output.messages.len(), 2);
        assert_eq!(output.summaries.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// Regression test for #39: a tool that returns
    /// `PermissionRequired` on the first call and then
    /// `UserInputRequired` on the second call must have
    /// both pending requests resolved in sequence. Before
    /// the fix the second error was silently treated as a
    /// terminal tool result, so a chained prompt would be
    /// reported as a `Error executing tool:` line in the
    /// final history instead of being resolved.
    #[tokio::test]
    async fn chained_permission_then_user_input_is_resolved() {
        use ene_ai::LlmToolCall;
        use ene_tool_host::ToolRegistry;
        use ene_tool_proto::ToolError;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Chained {
            perm_id: String,
            input_id: String,
            calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl ToolRegistry for Chained {
            fn list_tools(&self) -> Vec<ene_tool_proto::ToolSpec> {
                Vec::new()
            }
            async fn call_tool(
                &self,
                _name: &str,
                _arguments: &str,
            ) -> Result<String, ToolHostError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                match n {
                    0 => Err(ToolHostError::Protocol(ToolError::PermissionRequired {
                        request_id: self.perm_id.clone(),
                        action: "fs.write".to_string(),
                        target: "/tmp/x".to_string(),
                        description: "write file".to_string(),
                    })),
                    1 => Err(ToolHostError::Protocol(ToolError::UserInputRequired {
                        request_id: self.input_id.clone(),
                        prompt: ene_tool_proto::UserInputPrompt {
                            items: vec![ene_tool_proto::QuestionItem {
                                question: "Pick a value".to_string(),
                                options: Vec::new(),
                                allow_free_text: true,
                            }],
                        },
                    })),
                    _ => Ok("ok".to_string()),
                }
            }
            async fn approve_permission(&self, _request_id: &str) {}
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let registry = Chained {
            perm_id: "perm-1".to_string(),
            input_id: "input-1".to_string(),
            calls: calls.clone(),
        };

        let (event_tx, _event_rx) = tokio::sync::broadcast::channel::<EneEvent>(16);
        let pending_permissions: Arc<
            Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>,
        > = Arc::new(Mutex::new(HashMap::new()));
        let pending_user_inputs: Arc<
            Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>,
        > = Arc::new(Mutex::new(HashMap::new()));

        let tool_calls = vec![LlmToolCall {
            id: "call-1".to_string(),
            name: "fs.write".to_string(),
            arguments: "{}".to_string(),
        }];

        // Fast consumer: reply to permission, then user input.
        let consumer_event_rx = event_tx.subscribe();
        let consumer_perms = pending_permissions.clone();
        let consumer_inputs = pending_user_inputs.clone();
        let consumer = tokio::spawn(async move {
            let mut rx = consumer_event_rx;
            let mut answered_perm = false;
            let mut answered_input = false;
            while let Ok(ev) = rx.recv().await {
                match ev {
                    EneEvent::PermissionRequired { request_id, .. } => {
                        let mut guard = consumer_perms.lock().await;
                        if let Some(tx) = guard.remove(&request_id) {
                            let _ = tx.send(PermissionDecision::AllowOnce);
                        }
                        answered_perm = true;
                    }
                    EneEvent::UserInputRequired { request_id, .. } => {
                        let mut guard = consumer_inputs.lock().await;
                        if let Some(tx) = guard.remove(&request_id) {
                            let _ = tx.send(UserInputResponse::Multi(vec![MultiAnswer::Answer {
                                text: "hello".to_string(),
                            }]));
                        }
                        answered_input = true;
                    }
                    _ => {}
                }
                if answered_perm && answered_input {
                    return;
                }
            }
        });

        let turn = crate::types::TurnId::new();
        let exec = perform_tool_executions(
            &registry,
            "session-1",
            tool_calls,
            "",
            &event_tx,
            &turn,
            &pending_permissions,
            &pending_user_inputs,
            5_000,
            500,
        );

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), exec).await;
        let _ = consumer.await;

        let output = result
            .expect("executor hung: pending request not resolved")
            .expect("executor returned error");
        assert_eq!(output.messages.len(), 2);
        assert_eq!(output.summaries.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    /// Regression test for #32: the terminal guard must guarantee
    /// exactly one [`EneEvent::Terminal`] per run, even when the
    /// cancel command and the stream task both reach a terminal
    /// emit site (which is the common race when the user cancels
    /// mid-stream).
    #[tokio::test]
    async fn terminal_guard_emits_exactly_once() {
        let (event_tx, mut event_rx) = tokio::sync::broadcast::channel::<EneEvent>(8);
        let guard = Arc::new(AtomicBool::new(false));
        let turn = crate::types::TurnId::new();

        // Both the cancel-side and the stream-side try to emit a
        // terminal; the guard must let only the first one through.
        emit_terminal(&event_tx, &guard, &turn, TerminalReason::Cancelled);
        emit_terminal(&event_tx, &guard, &turn, TerminalReason::Done);
        emit_terminal(
            &event_tx,
            &guard,
            &turn,
            TerminalReason::Failed {
                message: "late".to_string(),
            },
        );

        let mut got = 0usize;
        while let Ok(ev) = event_rx.try_recv() {
            if matches!(ev, EneEvent::Terminal { .. }) {
                got += 1;
            }
        }
        assert_eq!(got, 1, "expected exactly one Terminal event");
    }
}
