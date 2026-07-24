use crate::diagnostics::DiagnosticEvent;
use crate::handle::{EneEvent, TerminalReason};
use crate::types::{RequestId, TurnId, TurnOrigin};
use ene_ai::{LlmMessage, LlmToolCall, LlmToolCallChunk, UserMessagePart};
use ene_config::EneConfig;
use ene_mind::ConversationSession;
use ene_mind::memory_writer::{ToolResultSummary, tool_grounding};
use ene_plugin_host::PluginHostError;
use ene_plugin_proto::ToolError;
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

/// How long a granted permission scope remains in effect (#177).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantType {
    /// Granted for a single call only (not tracked as a standing scope).
    Once,
    /// Granted for the remainder of the session.
    Session,
    /// Granted persistently across sessions.
    Permanent,
}

/// A standing permission grant tracked by the host (#177).
///
/// Session grants are recorded when the user approves an action with
/// [`PermissionDecision::AllowSession`] so the permission center can
/// list and revoke them. The `target_pattern` is a path/glob prefix
/// interpreted by the owning tool.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PermissionScope {
    /// Monotonic host-side identifier for revocation.
    pub id: u64,
    /// Action label (e.g. `FileOverwrite`).
    pub action: String,
    /// Target glob/path prefix the grant applies to.
    pub target_pattern: String,
    /// How long the grant lasts.
    pub grant_type: GrantType,
    /// When the grant was created.
    pub granted_at: chrono::DateTime<chrono::Utc>,
}

/// Re-exported from `ene-tool-proto` so consumers of `ene-runtime` only need to
/// import one crate.
#[doc(no_inline)]
pub use ene_plugin_proto::MultiAnswer;

/// Represents a user's response to an interactive tool's input request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserInputResponse {
    /// One answer per sub-question in the prompt, in the same order.
    /// For a single-question prompt this is `Multi(vec![..])`.
    Multi(Vec<MultiAnswer>),
    /// The user dismissed or cancelled the entire prompt.
    Cancel,
}

/// Shared context for a tool execution round, reducing parameter count
/// in [`perform_tool_executions`].
pub(crate) struct ToolExecutionContext<'a> {
    /// Tool registry for dispatching calls.
    pub registry: &'a dyn ene_plugin_host::ToolRegistry,
    /// Optional RAG pipeline for semantic tool search.
    pub tool_rag: Option<&'a ToolRag>,
    /// Conversation session identifier.
    pub session_id: &'a str,
    /// Broadcast channel for emitting tool events.
    pub event_tx: &'a broadcast::Sender<EneEvent>,
    /// Active turn id.
    pub turn: &'a TurnId,
    /// Who initiated the turn.
    pub origin: TurnOrigin,
    /// Pending permission decision senders.
    pub pending_permissions:
        &'a Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    /// Pending user input senders.
    pub pending_user_inputs: &'a Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
    /// Per-tool call timeout in milliseconds.
    pub timeout_ms: u64,
    /// Maximum characters for tool result summaries.
    pub max_summary_chars: usize,
    /// Optional memory store for audit logging.
    pub audit_store: Option<&'a Arc<ene_store::MemoryStore>>,
    /// Session-wide permission grants (#177).
    pub permission_scopes: &'a Arc<Mutex<Vec<PermissionScope>>>,
    /// Undo stack for reversible operations (#178).
    pub undo_stack: &'a Arc<Mutex<crate::undo::UndoStack>>,
    /// Channel for spawning deferred tool tasks (#196).
    pub deferred_tool_tx: &'a mpsc::UnboundedSender<crate::handle::DeferredToolTask>,
}

/// Output of one tool execution round.
#[derive(Debug, Default)]
pub(crate) struct ToolExecutionOutput {
    /// Assistant/tool messages fed back to the LLM loop.
    pub messages: Vec<LlmMessage>,
    /// Bounded summaries for cognitive memory grounding.
    pub summaries: Vec<ToolResultSummary>,
}

/// Result of a cognitive streaming run (session snapshot + terminal reason).
#[derive(Debug)]
#[doc(hidden)]
pub struct StreamOutcome {
    /// Updated conversation session.
    pub session: ene_mind::ConversationSession,
    /// Why the stream ended (for proactive cooldown accounting).
    pub terminal: TerminalReason,
}

/// Emit terminal and return [`StreamOutcome`] for the actor oneshot.
pub(crate) fn stream_finish(
    session: ene_mind::ConversationSession,
    event_tx: &broadcast::Sender<EneEvent>,
    guard: &AtomicBool,
    turn: &TurnId,
    origin: TurnOrigin,
    reason: TerminalReason,
) -> StreamOutcome {
    emit_terminal(event_tx, guard, turn, origin, reason.clone());
    StreamOutcome {
        session,
        terminal: reason,
    }
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
    origin: TurnOrigin,
    reason: TerminalReason,
) {
    if guard
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let _ = event_tx.send(EneEvent::Terminal {
            turn: turn.clone(),
            origin,
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
    pub registry: Arc<dyn ene_plugin_host::ToolRegistry>,
    pub tool_rag: Option<Arc<ToolRag>>,
    pub provider: Arc<dyn ene_ai::LlmProvider>,
    pub event_tx: broadcast::Sender<EneEvent>,
    pub diag_tx: broadcast::Sender<DiagnosticEvent>,
    pub cancel_token: CancellationToken,
    pub pending_permissions: Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    pub pending_user_inputs: Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
    /// Session-wide permission grants tracked for the permission center (#177).
    pub permission_scopes: Arc<Mutex<Vec<PermissionScope>>>,
    /// Actor-native undo stack of mutating tool calls (#178).
    pub undo_stack: Arc<Mutex<crate::undo::UndoStack>>,
    /// Shared with the actor; first side to flip emits Terminal.
    pub terminal_emitted: Arc<std::sync::atomic::AtomicBool>,
    /// Active turn id for all turn-scoped events.
    pub turn: TurnId,
    /// Whether this turn was user- or proactive-initiated.
    pub origin: TurnOrigin,
    /// When false, tool selection is skipped (proactive default).
    pub allow_tools: bool,
    /// Internal companion directive for proactive turns (never stored as user history).
    pub runtime_directive: Option<String>,
    /// Optional decision-time screen frame (JPEG data URI) for vision-capable generation.
    pub proactive_screen_image: Option<String>,
    /// Wall-clock cap for proactive generation (outer timeout wins over provider defaults).
    pub generation_timeout: Option<std::time::Duration>,
    /// Sender for classifier `JoinHandles` spawned after Terminal emission.
    /// The actor drains this into its classifier `JoinSet` for lifecycle management.
    pub classifier_tx: mpsc::UnboundedSender<tokio::task::JoinHandle<()>>,
    /// Sender for deferred memory-writer `JoinHandles` spawned after Terminal emission.
    pub memory_writer_tx:
        mpsc::UnboundedSender<tokio::task::JoinHandle<ene_mind::MemoryWriteOutcome>>,
    /// Sender for deferred tool tasks accepted during tool execution (#196).
    pub deferred_tool_tx: mpsc::UnboundedSender<crate::handle::DeferredToolTask>,
    /// Optional TTS provider for streaming audio synthesis.
    pub tts_provider: Option<Arc<dyn ene_ai::TtsProvider>>,
    /// Shared buffer of streamed assistant text deltas, updated live by the
    /// stream task so the actor can recover the partial response if the task
    /// is hard-aborted before it records the interruption itself (#H5).
    pub partial_text: Arc<parking_lot::Mutex<String>>,
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
    registry: &dyn ene_plugin_host::ToolRegistry,
    tool_rag: Option<&ToolRag>,
    user_input: &str,
    query_embedding: Option<&[f32]>,
    tool_calling_enabled: bool,
) -> Vec<ene_plugin_proto::ToolSpec> {
    if !tool_calling_enabled {
        return vec![];
    }

    // Use the new ToolRag pipeline if available.
    let mut res = if let Some(rag) = tool_rag {
        // Get all tools and RAG profiles from the registry.
        let all_tools = registry.list_tools();
        let profiles = registry.list_rag_profiles();

        // Ensure the index is up-to-date (no-op for already-indexed fields).
        if let Err(e) = rag.ensure_index(&all_tools, &profiles).await {
            tracing::warn!(component = "ToolRag", error = %e, "ensure_index failed");
        }

        // Select relevant tools via the RAG pipeline.
        if let Some(emb) = query_embedding {
            rag.select_with_embedding(user_input, emb).await
        } else {
            rag.select(user_input).await
        }
    } else {
        // Fallback: no ToolRag, return all tools from the registry.
        registry.list_tools()
    };

    res.push(search_tools_spec());
    res
}

/// Executes a batch of tool calls and sends result events through the broadcast channel.
pub(crate) async fn perform_tool_executions(
    ctx: &ToolExecutionContext<'_>,
    tool_calls: Vec<LlmToolCall>,
    assistant_content: &str,
) -> Result<ToolExecutionOutput, crate::error::EneRuntimeError> {
    let mut round_messages = Vec::new();
    let mut summaries = Vec::new();

    // Build a lookup of background-capable tool names so deferred calls
    // are only attempted for tools that advertise support (#196).
    let background_capable: std::collections::HashSet<String> = ctx
        .registry
        .list_tools()
        .into_iter()
        .filter(|spec| spec.background_capable)
        .map(|spec| spec.name.as_str().to_string())
        .collect();

    round_messages.push(LlmMessage::Assistant {
        content: if assistant_content.is_empty() {
            None
        } else {
            Some(assistant_content.to_string())
        },
        tool_calls: Some(tool_calls.clone()),
    });

    ctx.registry
        .set_call_context(&ene_plugin_proto::CallContext {
            conversation_id: ctx.session_id.to_string(),
            turn_id: ctx.turn.to_string(),
        })
        .await;

    for call in tool_calls {
        let name = call.name.clone();
        let args = call.arguments.clone();

        let _ = ctx.event_tx.send(EneEvent::ToolCallStart {
            turn: ctx.turn.clone(),
            origin: ctx.origin,
            name: name.clone(),
            arguments: args.clone(),
        });

        // Warn before executing an irreversible operation (#178). Such
        // actions are never placed on the undo stack, so the user is told
        // up front that they cannot be rolled back.
        if crate::undo::is_irreversible(&name) {
            tracing::warn!(
                component = "undo",
                tool = %name,
                "executing irreversible operation; it cannot be undone"
            );
        }

        let mut result = if name == "system.search_tools" {
            let query = serde_json::from_str::<serde_json::Value>(&args)
                .ok()
                .and_then(|v| v.get("query").and_then(|q| q.as_str()).map(String::from))
                .unwrap_or_default();
            execute_system_search_tool(ctx.registry, ctx.tool_rag, &query).await
        } else if background_capable.contains(&name) {
            // Try deferred execution for background-capable tools (#196).
            let tool_timeout = std::time::Duration::from_millis(ctx.timeout_ms);
            match tokio::time::timeout(tool_timeout, ctx.registry.call_tool_deferred(&name, &args))
                .await
            {
                Ok(Ok(ene_plugin_host::DeferredCallResult::Deferred { task_id })) => {
                    // Task accepted for background execution.
                    let _ = ctx.deferred_tool_tx.send(crate::handle::DeferredToolTask {
                        tool_name: name.clone(),
                        task_id: task_id.clone(),
                        arguments: args.clone(),
                        started_at: chrono::Utc::now(),
                    });
                    Ok(format!(
                        "Task queued for background execution with task_id: {task_id}"
                    ))
                }
                Ok(Ok(ene_plugin_host::DeferredCallResult::Sync(res))) => Ok(res),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(PluginHostError::ExecutionFailed {
                    message: format!(
                        "Tool '{}' timed out after {:.2} seconds",
                        name,
                        tool_timeout.as_secs_f64()
                    ),
                }),
            }
        } else {
            let tool_timeout = std::time::Duration::from_millis(ctx.timeout_ms);
            match tokio::time::timeout(tool_timeout, ctx.registry.call_tool(&name, &args)).await {
                Ok(Ok(res)) => Ok(res),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(PluginHostError::ExecutionFailed {
                    message: format!(
                        "Tool '{}' timed out after {:.2} seconds",
                        name,
                        tool_timeout.as_secs_f64()
                    ),
                }),
            }
        };

        // A tool may chain a permission prompt followed by a
        // user-input prompt (or vice-versa) on a single call.
        // Resolve pending requests in a loop with a hard cap
        // so a buggy tool that ping-pongs between the two
        // does not lock up the stream.
        const MAX_PENDING_ROUNDS: usize = 8;
        let mut audit_decision = ene_store::AuditDecision::NotRequired;
        let mut audit_action = String::new();
        let mut audit_target = String::new();
        for _ in 0..MAX_PENDING_ROUNDS {
            match &result {
                Err(PluginHostError::Protocol(ToolError::PermissionRequired {
                    request_id,
                    action,
                    target,
                    description,
                })) => {
                    let req_id = RequestId::from(request_id.clone());
                    audit_action = action.clone();
                    audit_target = target.clone();

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
                        let mut guard = ctx.pending_permissions.lock().await;
                        guard.insert(req_id.clone(), decide_tx);
                    }

                    let _ = ctx.event_tx.send(EneEvent::PermissionRequired {
                        turn: ctx.turn.clone(),
                        origin: ctx.origin,
                        request_id: req_id.clone(),
                        action: action.clone(),
                        target: target.clone(),
                        description: description.clone(),
                    });

                    match decide_rx.await {
                        Ok(PermissionDecision::AllowOnce) => {
                            audit_decision = ene_store::AuditDecision::AllowOnce;
                            ctx.registry.approve_permission(req_id.as_str()).await;
                            result = ctx.registry.call_tool(&name, &args).await;
                        }
                        Ok(PermissionDecision::AllowSession) => {
                            audit_decision = ene_store::AuditDecision::AllowSession;
                            ctx.registry.allow_pattern(action, target).await;
                            ctx.registry.approve_permission(req_id.as_str()).await;
                            {
                                let mut guard = ctx.permission_scopes.lock().await;
                                let next_id = guard
                                    .iter()
                                    .map(|s| s.id)
                                    .max()
                                    .unwrap_or(0)
                                    .saturating_add(1);
                                // De-duplicate: a repeated grant for the same
                                // action+target refreshes the existing scope
                                // rather than stacking duplicates.
                                if let Some(existing) = guard
                                    .iter_mut()
                                    .find(|s| s.action == *action && s.target_pattern == *target)
                                {
                                    existing.granted_at = chrono::Utc::now();
                                } else {
                                    guard.push(PermissionScope {
                                        id: next_id,
                                        action: action.clone(),
                                        target_pattern: target.clone(),
                                        grant_type: GrantType::Session,
                                        granted_at: chrono::Utc::now(),
                                    });
                                }
                            }
                            result = ctx.registry.call_tool(&name, &args).await;
                        }
                        _ => {
                            audit_decision = ene_store::AuditDecision::Denied;
                            result = Err(PluginHostError::Protocol(ToolError::permission_denied(
                                "Permission denied by user".to_string(),
                            )));
                            // Decision resolved; no further
                            // pending rounds needed.
                            break;
                        }
                    }
                }
                Err(PluginHostError::Protocol(ToolError::UserInputRequired {
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
                        let mut guard = ctx.pending_user_inputs.lock().await;
                        guard.insert(req_id.clone(), resp_tx);
                    }

                    let _ = ctx.event_tx.send(EneEvent::UserInputRequired {
                        turn: ctx.turn.clone(),
                        origin: ctx.origin,
                        request_id: req_id.clone(),
                        prompt: prompt.clone(),
                    });

                    match resp_rx.await {
                        Ok(UserInputResponse::Multi(answers)) => {
                            let new_args = inject_user_answers(&args, &answers);
                            result = ctx.registry.call_tool(&name, &new_args).await;
                        }
                        Ok(UserInputResponse::Cancel) | Err(_) => {
                            result = Err(PluginHostError::ExecutionFailed {
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

        // Record the tool call in the permission audit log (#177).
        // Arguments are redacted by the store before persistence so
        // secrets and raw prompt text never land in the audit trail.
        if let Some(store) = ctx.audit_store {
            ene_store::MemoryStore::spawn_insert_audit_entry(
                store,
                ene_store::NewAuditEntry {
                    turn_id: ctx.turn.to_string(),
                    tool_name: name.clone(),
                    action: std::mem::take(&mut audit_action),
                    target: std::mem::take(&mut audit_target),
                    decision: audit_decision,
                    success,
                    arguments: args.clone(),
                },
            );
        }

        // Record a successful mutating tool call on the undo stack (#178).
        // Only reversible/irreversible mutations are relevant; read-only and
        // meta tools are ignored by `UndoStack::record`.
        if success {
            let targets = crate::undo::extract_targets(&args);
            ctx.undo_stack
                .lock()
                .await
                .record(&name, &ctx.turn.to_string(), targets);
        }
        let _ = ctx.event_tx.send(EneEvent::ToolCallResult {
            turn: ctx.turn.clone(),
            origin: ctx.origin,
            name: name.clone(),
            result: result_str.clone(),
        });

        summaries.push(tool_grounding::summarize_tool_result(
            &name,
            &result_str,
            success,
            ctx.max_summary_chars,
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

/// Maximum number of tool calls per round to guard against unbounded memory growth.
const MAX_TOOL_CALLS_PER_ROUND: usize = 64;

pub(crate) fn accumulate_tool_calls(
    current_tool_calls: &mut Vec<LlmToolCallChunk>,
    tool_calls_delta: &[LlmToolCallChunk],
) {
    for tc_delta in tool_calls_delta {
        let idx = tc_delta.index;
        if idx >= MAX_TOOL_CALLS_PER_ROUND {
            tracing::warn!(
                component = "streaming",
                index = idx,
                limit = MAX_TOOL_CALLS_PER_ROUND,
                "tool call index exceeds limit; ignoring chunk"
            );
            continue;
        }
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
            tracing::warn!(
                component = "streaming",
                "tool arguments are not a JSON object; wrapping under _original_args"
            );
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

pub(crate) fn search_tools_spec() -> ene_plugin_proto::ToolSpec {
    ene_plugin_proto::ToolSpec {
        name: ene_plugin_proto::ToolName::new("system.search_tools"),
        description: "Search for registered tools by a semantic query. Returns a list of matching tool names, descriptions, and parameter schemas. Use this when you need to perform an action but the necessary tool is not in your active tool list.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Semantic query describing the goal or function of the tool you want to find."
                }
            },
            "required": ["query"]
        }),
        background_capable: false,
    }
}

pub(crate) async fn execute_system_search_tool(
    registry: &dyn ene_plugin_host::ToolRegistry,
    tool_rag: Option<&ToolRag>,
    query: &str,
) -> Result<String, PluginHostError> {
    if query.is_empty() {
        return Ok("Please provide a search query.".to_string());
    }

    let matching_tools = if let Some(rag) = tool_rag {
        let all_tools = registry.list_tools();
        let profiles = registry.list_rag_profiles();
        if let Err(e) = rag.ensure_index(&all_tools, &profiles).await {
            tracing::warn!(component = "ToolRag", error = %e, "ensure_index failed");
        }
        rag.select(query).await
    } else {
        let query_lower = query.to_lowercase();
        registry
            .list_tools()
            .into_iter()
            .filter(|t| {
                t.name.as_str().to_lowercase().contains(&query_lower)
                    || t.description.to_lowercase().contains(&query_lower)
            })
            .collect()
    };

    if matching_tools.is_empty() {
        Ok("No matching tools found.".to_string())
    } else {
        use std::fmt::Write as _;
        let mut output = String::new();
        let _ = writeln!(output, "Found the following tools matching your query:\n");
        for tool in matching_tools {
            if tool.name.as_str() == "system.search_tools" {
                continue;
            }
            let _ = writeln!(output, "- **{}**", tool.name.as_str());
            let _ = writeln!(output, "  *Description:* {}", tool.description);
            let _ = writeln!(
                output,
                "  *Parameters Schema:* {}\n",
                serde_json::to_string(&tool.parameters).unwrap_or_default()
            );
        }
        Ok(output)
    }
}

#[cfg(test)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "test intentionally keeps lock held across helper calls"
)]
mod tests {
    use super::*;
    use crate::types::TurnOrigin;

    #[tokio::test]
    async fn select_relevant_tools_includes_system_search_tool() {
        struct DummyRegistry;
        #[async_trait::async_trait]
        impl ene_plugin_host::ToolRegistry for DummyRegistry {
            fn list_tools(&self) -> Vec<ene_plugin_proto::ToolSpec> {
                vec![]
            }
            async fn call_tool(
                &self,
                _name: &str,
                _arguments: &str,
            ) -> Result<String, ene_plugin_host::PluginHostError> {
                Ok(String::new())
            }
        }
        let registry = DummyRegistry;
        let tools = select_relevant_tools(&registry, None, "test", None, true).await;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name.as_str(), "system.search_tools");
    }

    #[tokio::test]
    async fn perform_tool_executions_intercepts_system_search_tools() {
        use ene_ai::LlmToolCall;
        use std::collections::HashMap;
        use std::sync::Arc;
        use tokio::sync::{Mutex, broadcast};

        struct DummyRegistry;
        #[async_trait::async_trait]
        impl ene_plugin_host::ToolRegistry for DummyRegistry {
            fn list_tools(&self) -> Vec<ene_plugin_proto::ToolSpec> {
                vec![ene_plugin_proto::ToolSpec {
                    name: ene_plugin_proto::ToolName::new("filesystem.read"),
                    description: "Read files".to_string(),
                    parameters: serde_json::json!({}),
                    background_capable: false,
                }]
            }
            async fn call_tool(
                &self,
                _name: &str,
                _arguments: &str,
            ) -> Result<String, ene_plugin_host::PluginHostError> {
                Ok("fail".to_string())
            }
        }
        let registry = DummyRegistry;
        let (event_tx, _event_rx) = broadcast::channel(16);
        let (deferred_tool_tx, _deferred_tool_rx) = tokio::sync::mpsc::unbounded_channel();
        let pending_permissions = Arc::new(Mutex::new(HashMap::new()));
        let pending_user_inputs = Arc::new(Mutex::new(HashMap::new()));
        let turn = crate::types::TurnId::new();

        // Query for filesystem tool
        let tool_calls = vec![LlmToolCall {
            id: "call_123".to_string(),
            name: "system.search_tools".to_string(),
            arguments: serde_json::json!({ "query": "filesystem" }).to_string(),
        }];

        let output = perform_tool_executions(
            &ToolExecutionContext {
                registry: &registry,
                tool_rag: None,
                session_id: "session_123",
                event_tx: &event_tx,
                turn: &turn,
                origin: TurnOrigin::User,
                pending_permissions: &pending_permissions,
                pending_user_inputs: &pending_user_inputs,
                timeout_ms: 1000,
                max_summary_chars: 100,
                audit_store: None,
                permission_scopes: &Arc::new(Mutex::new(Vec::new())),
                undo_stack: &Arc::new(Mutex::new(crate::undo::UndoStack::new(8))),
                deferred_tool_tx: &deferred_tool_tx,
            },
            tool_calls,
            "assistant text",
        )
        .await
        .unwrap();

        assert_eq!(output.summaries.len(), 1);
        let summary = &output.summaries[0];
        assert_eq!(summary.tool_name.as_str(), "system.search_tools");
        // Check that it returned description of filesystem.read
        assert!(summary.summary.contains("filesystem.read"));
        assert!(summary.summary.contains("Read files"));
    }

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
        use ene_plugin_host::ToolRegistry;

        struct AlwaysOk {
            output: String,
        }

        #[async_trait::async_trait]
        impl ToolRegistry for AlwaysOk {
            fn list_tools(&self) -> Vec<ene_plugin_proto::ToolSpec> {
                Vec::new()
            }

            async fn call_tool(
                &self,
                _name: &str,
                _arguments: &str,
            ) -> Result<String, PluginHostError> {
                Ok(self.output.clone())
            }

            async fn approve_permission(&self, _request_id: &str) {}
        }

        let registry = AlwaysOk {
            output: "x".repeat(64),
        };
        let (event_tx, _event_rx) = tokio::sync::broadcast::channel::<EneEvent>(16);
        let (deferred_tool_tx, _deferred_tool_rx) = tokio::sync::mpsc::unbounded_channel();
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
        let scopes = Arc::new(Mutex::new(Vec::new()));
        let undo_stack = Arc::new(Mutex::new(crate::undo::UndoStack::new(8)));
        let output = perform_tool_executions(
            &ToolExecutionContext {
                registry: &registry,
                tool_rag: None,
                session_id: "session-1",
                event_tx: &event_tx,
                turn: &turn,
                origin: TurnOrigin::User,
                pending_permissions: &pending_permissions,
                pending_user_inputs: &pending_user_inputs,
                timeout_ms: 5_000,
                max_summary_chars: 10,
                audit_store: None,
                permission_scopes: &scopes,
                undo_stack: &undo_stack,
                deferred_tool_tx: &deferred_tool_tx,
            },
            tool_calls,
            "",
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
        use ene_plugin_host::ToolRegistry;
        use ene_plugin_proto::ToolError;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct PermissionThenOk {
            request_id: String,
            calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl ToolRegistry for PermissionThenOk {
            fn list_tools(&self) -> Vec<ene_plugin_proto::ToolSpec> {
                Vec::new()
            }
            async fn call_tool(
                &self,
                _name: &str,
                _arguments: &str,
            ) -> Result<String, PluginHostError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err(PluginHostError::Protocol(ToolError::PermissionRequired {
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
        let (deferred_tool_tx, _deferred_tool_rx) = tokio::sync::mpsc::unbounded_channel();
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
        let scopes = Arc::new(Mutex::new(Vec::new()));
        let undo_stack = Arc::new(Mutex::new(crate::undo::UndoStack::new(8)));
        let exec_ctx = ToolExecutionContext {
            registry: &registry,
            tool_rag: None,
            session_id: "session-1",
            event_tx: &event_tx,
            turn: &turn,
            origin: TurnOrigin::User,
            pending_permissions: &pending_permissions,
            pending_user_inputs: &pending_user_inputs,
            timeout_ms: 5_000,
            max_summary_chars: 500,
            audit_store: None,
            permission_scopes: &scopes,
            undo_stack: &undo_stack,
            deferred_tool_tx: &deferred_tool_tx,
        };
        let exec = perform_tool_executions(&exec_ctx, tool_calls, "");

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
        use ene_plugin_host::ToolRegistry;
        use ene_plugin_proto::ToolError;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Chained {
            perm_id: String,
            input_id: String,
            calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl ToolRegistry for Chained {
            fn list_tools(&self) -> Vec<ene_plugin_proto::ToolSpec> {
                Vec::new()
            }
            async fn call_tool(
                &self,
                _name: &str,
                _arguments: &str,
            ) -> Result<String, PluginHostError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                match n {
                    0 => Err(PluginHostError::Protocol(ToolError::PermissionRequired {
                        request_id: self.perm_id.clone(),
                        action: "fs.write".to_string(),
                        target: "/tmp/x".to_string(),
                        description: "write file".to_string(),
                    })),
                    1 => Err(PluginHostError::Protocol(ToolError::UserInputRequired {
                        request_id: self.input_id.clone(),
                        prompt: ene_plugin_proto::UserInputPrompt::new(vec![
                            ene_plugin_proto::QuestionItem {
                                question: "Pick a value".to_string(),
                                options: Vec::new(),
                                allow_free_text: true,
                            },
                        ])
                        .unwrap(),
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
        let (deferred_tool_tx, _deferred_tool_rx) = tokio::sync::mpsc::unbounded_channel();
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
        let scopes = Arc::new(Mutex::new(Vec::new()));
        let undo_stack = Arc::new(Mutex::new(crate::undo::UndoStack::new(8)));
        let exec_ctx = ToolExecutionContext {
            registry: &registry,
            tool_rag: None,
            session_id: "session-1",
            event_tx: &event_tx,
            turn: &turn,
            origin: TurnOrigin::User,
            pending_permissions: &pending_permissions,
            pending_user_inputs: &pending_user_inputs,
            timeout_ms: 5_000,
            max_summary_chars: 500,
            audit_store: None,
            permission_scopes: &scopes,
            undo_stack: &undo_stack,
            deferred_tool_tx: &deferred_tool_tx,
        };
        let exec = perform_tool_executions(&exec_ctx, tool_calls, "");

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
        emit_terminal(
            &event_tx,
            &guard,
            &turn,
            TurnOrigin::User,
            TerminalReason::Cancelled,
        );
        emit_terminal(
            &event_tx,
            &guard,
            &turn,
            TurnOrigin::User,
            TerminalReason::Done,
        );
        emit_terminal(
            &event_tx,
            &guard,
            &turn,
            TurnOrigin::User,
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
