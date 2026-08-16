//! Anthropic Claude plugin: Messages API streaming and completion.
//!
//! Implements [`LlmPlugin`] for the Anthropic Messages API, supporting
//! both SSE streaming (`create_chat_stream`) and non-streaming
//! (`chat_completion`) chat completions with tool use and vision.
//! Structured output (`json_schema`) is emulated by forcing a synthetic tool
//! whose input schema matches the requested schema. All HTTP traffic is
//! mediated by the host through the `Network` broker (SSRF guard, origin
//! approval, size caps, `x-api-key` credential injection).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use ene_plugin::prelude::*;
use serde_json::{Value, json};
use tokio_stream::wrappers::ReceiverStream;

use crate::broker::{StreamSession, broker};
use crate::convert;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 8192;
/// Name of the synthetic tool used to force structured output.
const STRUCTURED_OUTPUT_TOOL: &str = "structured_output";

/// Configuration delivered by the host at handshake time
/// (`plugins.list.anthropic.config`), stored per process. The API key is no
/// longer part of this blob's contract — the host injects it into broker
/// requests by key name.
///
/// `Mutex` (rather than `OnceLock`) so tests can reset it between cases; in
/// production the handshake is a one-shot and reconnects resend the same
/// blob, so last-writer-wins is equivalent.
static PLUGIN_CONFIG: Mutex<Option<Value>> = Mutex::new(None);

/// Anthropic Claude plugin providing streaming and non-streaming chat
/// completions via the Anthropic Messages API.
///
/// The static capability data (`llm_spec()` / `LLM_PROVIDER_KIND`) is
/// generated from the `#[provider(...)]` attribute; the async handlers below
/// stay hand-written.
#[derive(LlmPlugin)]
#[provider(
    kind = "anthropic",
    models = "claude-sonnet-4-20250514, claude-haiku-4-20250514, claude-3-5-sonnet-20241022",
    streaming,
    vision,
    // A stateless HTTP proxy to Anthropic's cloud API, not a local model —
    // safe to run many requests concurrently. Explicit, per the
    // `ConcurrencyHint` design: opting into more than the serial default
    // requires stating so, which this does.
    concurrency = 8,
    queue_depth = 16,
    // Claude models expose a 200k-token context window.
    context_window = 200_000,
)]
pub(crate) struct AnthropicPlugin;

impl ene_plugin::ConfigurablePlugin for AnthropicPlugin {
    /// Receives the plugin configuration blob from the host at handshake
    /// time (`plugins.list.anthropic.config`).
    fn set_config(&self, config: &Value) {
        *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = Some(config.clone());
    }

    /// Captures the broker socket/token so every request is host-mediated.
    fn set_sandbox(&self, sandbox: &ene_plugin_proto::SandboxConfigData) {
        crate::broker::configure_broker(sandbox);
    }

    /// Advertises the config schema; `api_key` is marked `x-ene-secret: true`
    /// so the host masks/redacts it. The key itself is unused by the plugin:
    /// the host injects it into broker requests by key name.
    fn config_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "api_key": {
                    "oneOf": [
                        { "type": "string" },
                        {
                            "type": "object",
                            "properties": {
                                "source": {
                                    "type": "string",
                                    "enum": ["inline", "env", "auto"]
                                },
                                "inline": { "type": "string" },
                                "env": { "type": "string" }
                            }
                        }
                    ],
                    "x-ene-secret": true,
                    "description": "Anthropic API key, or a {source: inline|env|auto} descriptor"
                },
                "base_url": {
                    "type": "string",
                    "description": "API base URL override (unused today; reserved)"
                }
            }
        }))
    }
}

/// Builds the Anthropic Messages API request body.
///
/// When `forced_tool` is `Some`, the synthetic tool is appended to the tool
/// list and `tool_choice` forces the model to call it (structured output).
fn build_request_body(
    model: &str,
    max_tokens: Option<u32>,
    messages: &[Value],
    tools: &[Value],
    stream: bool,
    forced_tool: Option<&Value>,
) -> Value {
    let (system, anthropic_messages) = convert::to_anthropic_messages(messages);
    let mut anthropic_tools = convert::to_anthropic_tools(tools);
    if let Some(forced) = forced_tool {
        anthropic_tools.push(forced.clone());
    }

    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "messages": anthropic_messages,
        "stream": stream,
    });

    if let Some(obj) = body.as_object_mut() {
        if let Some(system) = system {
            obj.insert("system".to_string(), json!(system));
        }

        if !anthropic_tools.is_empty() {
            obj.insert("tools".to_string(), json!(anthropic_tools));
        }

        if let Some(forced) = forced_tool {
            let name = forced
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(STRUCTURED_OUTPUT_TOOL);
            obj.insert(
                "tool_choice".to_string(),
                json!({ "type": "tool", "name": name }),
            );
        }
    }

    body
}

/// Converts a requested JSON Schema into an Anthropic tool definition that
/// the model is forced to call, emulating structured output.
///
/// Accepts either a raw JSON Schema object or a
/// `{ "schema": ..., "description": ... }` wrapper (matching the built-in
/// `OpenAI` provider's contract). Non-object schemas cannot be represented as
/// Anthropic tool inputs and are rejected with
/// [`PluginError::NotSupported`].
fn schema_to_forced_tool(schema: &Value) -> Result<Value, PluginError> {
    let inner = schema.get("schema").unwrap_or(schema);
    let is_object_schema = inner.is_object()
        && inner
            .get("type")
            .and_then(Value::as_str)
            .is_none_or(|ty| ty == "object");
    if !is_object_schema {
        return Err(PluginError::not_supported(
            "json_schema must be an object schema for structured output with Anthropic",
        ));
    }
    let description = schema
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("Respond only with JSON matching the provided schema.");
    Ok(json!({
        "name": STRUCTURED_OUTPUT_TOOL,
        "description": description,
        "input_schema": inner,
    }))
}

fn extract_text_content(body: &Value) -> Result<String, PluginError> {
    let content = body
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| PluginError::provider("response missing content array"))?;

    let text: String = content
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");

    Ok(text)
}

/// Extracts the serialized `input` of the first `tool_use` content block from
/// a non-streaming response (used for schema-forced structured output).
fn extract_tool_input(body: &Value) -> Result<String, PluginError> {
    let content = body
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| PluginError::provider("response missing content array"))?;

    let input = content
        .iter()
        .find(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .and_then(|block| block.get("input"))
        .ok_or_else(|| {
            PluginError::provider("model did not produce the forced structured-output tool call")
        })?;

    serde_json::to_string(input)
        .map_err(|e| PluginError::provider(format!("failed to serialize structured output: {e}")))
}

#[async_trait]
impl LlmPlugin for AnthropicPlugin {
    fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
        vec![Self::llm_spec()]
    }

    async fn create_chat_stream(
        &self,
        kind: &str,
        _config: Value,
        model: String,
        max_tokens: Option<u32>,
        messages: Vec<Value>,
        tools: Vec<Value>,
    ) -> Result<PluginStream, PluginError> {
        if kind != Self::LLM_PROVIDER_KIND {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }

        let body = build_request_body(&model, max_tokens, &messages, &tools, true, None);

        let session = broker()
            .stream(
                ene_plugin_broker::HttpMethod::Post,
                ANTHROPIC_API_URL,
                vec![(
                    "anthropic-version".to_string(),
                    ANTHROPIC_VERSION.to_string(),
                )],
                Some("api_key"),
                Some("x-api-key"),
                Some(serde_json::to_vec(&body).map_err(|e| {
                    PluginError::provider(format!("failed to serialize request: {e}"))
                })?),
            )
            .await?;

        let (tx, rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(stream_sse_response(session, tx));

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn chat_completion(
        &self,
        kind: &str,
        _config: Value,
        model: String,
        max_tokens: Option<u32>,
        messages: Vec<Value>,
        json_schema: Option<Value>,
    ) -> Result<PluginCompletion, PluginError> {
        if kind != Self::LLM_PROVIDER_KIND {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }

        let forced_tool = json_schema
            .as_ref()
            .map(schema_to_forced_tool)
            .transpose()?;
        let body = build_request_body(
            &model,
            max_tokens,
            &messages,
            &[],
            false,
            forced_tool.as_ref(),
        );

        let response = broker()
            .fetch(
                ene_plugin_broker::HttpMethod::Post,
                ANTHROPIC_API_URL,
                vec![(
                    "anthropic-version".to_string(),
                    ANTHROPIC_VERSION.to_string(),
                )],
                Some("api_key"),
                Some("x-api-key"),
                Some(serde_json::to_vec(&body).map_err(|e| {
                    PluginError::provider(format!("failed to serialize request: {e}"))
                })?),
            )
            .await
            .map_err(|e| PluginError::provider(format!("broker request failed: {e}")))?;

        if !(200..300).contains(&response.status) {
            let error_body = String::from_utf8_lossy(&response.body);
            return Err(PluginError::provider(format!(
                "Anthropic API error (HTTP {}): {error_body}",
                response.status
            )));
        }

        let body: Value = serde_json::from_slice(&response.body)
            .map_err(|e| PluginError::provider(format!("failed to parse response: {e}")))?;

        let text = if forced_tool.is_some() {
            extract_tool_input(&body)?
        } else {
            extract_text_content(&body)?
        };
        Ok(PluginCompletion {
            text,
            usage: usage_from_anthropic_body(&body),
        })
    }
}

/// Extract [`TokenUsage`] from an Anthropic message response body.
///
/// Anthropic reports `usage.input_tokens` and `usage.output_tokens` (it has no
/// single `total` field, so the total is derived as their sum). Returns `None`
/// when the body carries no `usage` object.
fn usage_from_anthropic_body(body: &Value) -> Option<TokenUsage> {
    let usage = body.get("usage")?;
    let input = usage.get("input_tokens").and_then(Value::as_u64);
    let output = usage.get("output_tokens").and_then(Value::as_u64);
    if input.is_none() && output.is_none() {
        return None;
    }
    let prompt = input.and_then(token_count_u32);
    let completion = output.and_then(token_count_u32);
    let total = match (prompt, completion) {
        (Some(p), Some(c)) => Some(p.saturating_add(c)),
        _ => None,
    };
    Some(TokenUsage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: total,
    })
}

/// Narrow a provider-reported token count to `u32`.
///
/// Values above [`u32::MAX`] are treated as absent rather than silently
/// truncated — a bad provider sentinel must not enter usage accounting as a
/// plausible-looking number.
fn token_count_u32(n: u64) -> Option<u32> {
    if let Ok(v) = u32::try_from(n) {
        Some(v)
    } else {
        tracing::warn!(
            tokens = n,
            "token usage count exceeds u32::MAX; treating as absent"
        );
        None
    }
}

/// Maps Anthropic content-block indices to dense tool-call indices.
///
/// Anthropic assigns `index` values across *all* content blocks (text blocks
/// included), so a response of text + two tool calls uses block indices
/// 0, 1, 2 while consumers expect contiguous tool-call indices 0 and 1.
/// Only `tool_use` blocks are registered here, at `content_block_start` time.
#[derive(Debug, Default)]
struct ToolCallState {
    /// Content-block index → dense tool-call index.
    block_to_tool: HashMap<u64, u64>,
    /// Next dense tool-call index to assign.
    next_tool_index: u64,
}

/// Reads an Anthropic SSE stream and sends parsed chunks through the channel.
///
/// Processes `content_block_start` (`tool_use`), `content_block_delta`
/// (`text_delta`, `input_json_delta`), and `error` events; content-block
/// indices are remapped to dense tool-call indices. Other event types
/// (`message_start`, `content_block_stop`, `message_delta`, `message_stop`)
/// are acknowledged but produce no output chunks.
async fn stream_sse_response(
    mut session: StreamSession,
    tx: tokio::sync::mpsc::Sender<Result<PluginStreamChunk, PluginError>>,
) {
    use eventsource_stream::Eventsource;
    use tokio_stream::StreamExt as _;

    if !(200..300).contains(&session.status) {
        let mut raw = Vec::new();
        while let Some(chunk) = session.chunks.next().await {
            let Ok(bytes) = chunk else { break };
            raw.extend_from_slice(&bytes);
            if raw.len() > 64 * 1024 {
                break;
            }
        }
        let error_body = String::from_utf8_lossy(&raw);
        drop(
            tx.send(Err(PluginError::provider(format!(
                "Anthropic API error (HTTP {}): {error_body}",
                session.status
            ))))
            .await,
        );
        return;
    }

    let mut tool_state = ToolCallState::default();
    let mut events = session.chunks.eventsource();
    while let Some(event) = events.next().await {
        let event = match event {
            Ok(event) => event,
            Err(e) => {
                drop(
                    tx.send(Err(PluginError::provider(format!(
                        "stream read error: {e}"
                    ))))
                    .await,
                );
                return;
            }
        };
        match process_sse_event(&event.event, &event.data, &mut tool_state) {
            Some(Ok(chunk)) => {
                if tx.send(Ok(chunk)).await.is_err() {
                    return;
                }
            }
            Some(Err(e)) => {
                drop(tx.send(Err(e)).await);
                return;
            }
            None => {}
        }
    }
}

/// Processes a single SSE event and returns a stream chunk if applicable.
///
/// Tool-call deltas carry dense tool-call indices (0, 1, 2, … counting only
/// `tool_use` blocks) rather than raw Anthropic content-block indices; the
/// mapping is maintained in `state`.
///
/// Returns `None` for events that don't produce output (e.g. `message_start`,
/// `content_block_stop`, `message_delta`, `message_stop`).
fn process_sse_event(
    event_type: &str,
    data: &str,
    state: &mut ToolCallState,
) -> Option<Result<PluginStreamChunk, PluginError>> {
    let parsed: Value = serde_json::from_str(data).ok()?;

    match event_type {
        "content_block_start" => {
            let index = parsed.get("index")?.as_u64()?;
            let block = parsed.get("content_block")?;
            if block.get("type")?.as_str()? == "tool_use" {
                let tool_index = state.next_tool_index;
                state.next_tool_index = state.next_tool_index.saturating_add(1);
                state.block_to_tool.insert(index, tool_index);
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                Some(Ok(PluginStreamChunk {
                    text_delta: None,
                    tool_calls_delta: Some(vec![json!({
                        "index": tool_index,
                        "id": id,
                        "name": name,
                        "arguments": "",
                    })]),
                    usage: None,
                }))
            } else {
                None
            }
        }
        "content_block_delta" => {
            let index = parsed.get("index")?.as_u64()?;
            let delta = parsed.get("delta")?;
            match delta.get("type")?.as_str()? {
                "text_delta" => {
                    let text = delta.get("text")?.as_str()?.to_string();
                    Some(Ok(PluginStreamChunk {
                        text_delta: Some(text),
                        tool_calls_delta: None,
                        usage: None,
                    }))
                }
                "input_json_delta" => {
                    let tool_index = *state.block_to_tool.get(&index)?;
                    let partial = delta
                        .get("partial_json")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    Some(Ok(PluginStreamChunk {
                        text_delta: None,
                        tool_calls_delta: Some(vec![json!({
                            "index": tool_index,
                            "arguments": partial,
                        })]),
                        usage: None,
                    }))
                }
                _ => None,
            }
        }
        "message_delta" => {
            // Anthropic reports the cumulative output token count on the final
            // `message_delta` event; emit it as a usage-only chunk so
            // the host can attach it to the stream's final chunk.
            let usage = parsed.get("usage")?;
            let output = usage.get("output_tokens").and_then(Value::as_u64)?;
            Some(Ok(PluginStreamChunk {
                text_delta: None,
                tool_calls_delta: None,
                usage: Some(TokenUsage {
                    prompt_tokens: None,
                    completion_tokens: token_count_u32(output),
                    total_tokens: None,
                }),
            }))
        }
        "error" => {
            let message = parsed
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown Anthropic API error");
            Some(Err(PluginError::provider(message.to_string())))
        }
        _ => None,
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit tests use expect/unwrap for concise assertions"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "test assertions index into known-length vectors"
)]
mod tests {
    use super::*;
    use ene_plugin::ConfigurablePlugin;

    #[test]
    fn config_schema_marks_api_key_secret() {
        let schema = AnthropicPlugin.config_schema().expect("schema");
        assert_eq!(
            schema.pointer("/properties/api_key/x-ene-secret"),
            Some(&json!(true))
        );
    }

    #[test]
    fn build_request_body_basic() {
        let messages = vec![json!({"role": "user", "parts": [{"Text": {"text": "Hello"}}]})];
        let body = build_request_body(
            "claude-sonnet-4-20250514",
            Some(1024),
            &messages,
            &[],
            true,
            None,
        );
        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["stream"], true);
        assert!(body.get("system").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn build_request_body_with_system_and_tools() {
        let messages = vec![
            json!({"role": "system", "content": "Be helpful."}),
            json!({"role": "user", "parts": [{"Text": {"text": "Hi"}}]}),
        ];
        let tools = vec![json!({
            "name": "test.tool",
            "description": "A test tool.",
            "parameters": {"type": "object"}
        })];
        let body = build_request_body(
            "claude-sonnet-4-20250514",
            None,
            &messages,
            &tools,
            false,
            None,
        );
        assert_eq!(body["system"], "Be helpful.");
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(body["stream"], false);
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert_eq!(body["tools"][0]["name"], "test.tool");
    }

    #[test]
    fn build_request_body_forced_tool_sets_tool_choice() {
        let messages = vec![json!({"role": "user", "parts": [{"Text": {"text": "Hi"}}]})];
        let forced = schema_to_forced_tool(&json!({"type": "object"})).unwrap();
        let body = build_request_body(
            "claude-sonnet-4-20250514",
            None,
            &messages,
            &[],
            false,
            Some(&forced),
        );
        assert_eq!(
            body["tool_choice"],
            json!({ "type": "tool", "name": STRUCTURED_OUTPUT_TOOL })
        );
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], STRUCTURED_OUTPUT_TOOL);
    }

    #[test]
    fn schema_to_forced_tool_raw_schema() {
        let schema = json!({
            "type": "object",
            "properties": {"answer": {"type": "number"}},
            "required": ["answer"]
        });
        let tool = schema_to_forced_tool(&schema).unwrap();
        assert_eq!(tool["name"], STRUCTURED_OUTPUT_TOOL);
        assert_eq!(tool["input_schema"], schema);
    }

    #[test]
    fn schema_to_forced_tool_wrapped_schema() {
        let schema = json!({
            "schema": {"type": "object", "properties": {}},
            "description": "Custom description."
        });
        let tool = schema_to_forced_tool(&schema).unwrap();
        assert_eq!(tool["description"], "Custom description.");
        assert_eq!(
            tool["input_schema"],
            json!({"type": "object", "properties": {}})
        );
    }

    #[test]
    fn schema_to_forced_tool_rejects_non_object_schema() {
        let schema = json!({"type": "array", "items": {"type": "string"}});
        assert!(matches!(
            schema_to_forced_tool(&schema),
            Err(PluginError::NotSupported(_))
        ));
    }

    #[test]
    fn extract_tool_input_serializes_input() {
        let body = json!({
            "content": [
                {"type": "text", "text": "thinking..."},
                {"type": "tool_use", "id": "toolu_1", "name": STRUCTURED_OUTPUT_TOOL,
                 "input": {"answer": 42}}
            ]
        });
        assert_eq!(extract_tool_input(&body).unwrap(), "{\"answer\":42}");
    }

    #[test]
    fn extract_tool_input_missing_tool_use() {
        let body = json!({"content": [{"type": "text", "text": "no tool call"}]});
        assert!(extract_tool_input(&body).is_err());
    }

    #[test]
    fn extract_text_content_single_block() {
        let body = json!({
            "content": [{"type": "text", "text": "Hello, world!"}]
        });
        assert_eq!(extract_text_content(&body).unwrap(), "Hello, world!");
    }

    #[test]
    fn extract_text_content_multiple_blocks() {
        let body = json!({
            "content": [
                {"type": "text", "text": "Hello, "},
                {"type": "tool_use", "id": "t1", "name": "test", "input": {}},
                {"type": "text", "text": "world!"}
            ]
        });
        assert_eq!(extract_text_content(&body).unwrap(), "Hello, world!");
    }

    #[test]
    fn extract_text_content_missing_content() {
        let body = json!({"id": "msg_123"});
        assert!(extract_text_content(&body).is_err());
    }

    #[test]
    fn extract_text_content_empty() {
        let body = json!({"content": []});
        assert_eq!(extract_text_content(&body).unwrap(), "");
    }

    #[test]
    fn sse_text_delta_event() {
        let mut state = ToolCallState::default();
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let result = process_sse_event("content_block_delta", data, &mut state)
            .unwrap()
            .unwrap();
        assert_eq!(result.text_delta.as_deref(), Some("Hello"));
        assert!(result.tool_calls_delta.is_none());
    }

    #[test]
    fn sse_input_json_delta_event() {
        let mut state = ToolCallState::default();
        let start = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"get_weather"}}"#;
        let _ = process_sse_event("content_block_start", start, &mut state);
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"loc"}}"#;
        let result = process_sse_event("content_block_delta", data, &mut state)
            .unwrap()
            .unwrap();
        assert!(result.text_delta.is_none());
        let deltas = result.tool_calls_delta.unwrap();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["index"], 0);
        assert_eq!(deltas[0]["arguments"], "{\"loc");
    }

    #[test]
    fn sse_input_json_delta_unknown_block_skipped() {
        let mut state = ToolCallState::default();
        let data = r#"{"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#;
        assert!(process_sse_event("content_block_delta", data, &mut state).is_none());
    }

    #[test]
    fn sse_tool_use_start_event() {
        let mut state = ToolCallState::default();
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"get_weather"}}"#;
        let result = process_sse_event("content_block_start", data, &mut state)
            .unwrap()
            .unwrap();
        assert!(result.text_delta.is_none());
        let deltas = result.tool_calls_delta.unwrap();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["index"], 0);
        assert_eq!(deltas[0]["id"], "toolu_abc");
        assert_eq!(deltas[0]["name"], "get_weather");
        assert_eq!(deltas[0]["arguments"], "");
    }

    #[test]
    fn sse_tool_indices_are_dense_across_text_blocks() {
        let mut state = ToolCallState::default();

        let text_start =
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        assert!(process_sse_event("content_block_start", text_start, &mut state).is_none());

        let tool1_start = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_a","name":"a"}}"#;
        let chunk = process_sse_event("content_block_start", tool1_start, &mut state)
            .unwrap()
            .unwrap();
        assert_eq!(chunk.tool_calls_delta.unwrap()[0]["index"], 0);

        let tool2_start = r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_b","name":"b"}}"#;
        let chunk = process_sse_event("content_block_start", tool2_start, &mut state)
            .unwrap()
            .unwrap();
        assert_eq!(chunk.tool_calls_delta.unwrap()[0]["index"], 1);

        // Deltas carry the dense tool index, not the content-block index.
        let delta = r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#;
        let chunk = process_sse_event("content_block_delta", delta, &mut state)
            .unwrap()
            .unwrap();
        assert_eq!(chunk.tool_calls_delta.unwrap()[0]["index"], 1);
    }

    #[test]
    fn sse_text_block_start_ignored() {
        let mut state = ToolCallState::default();
        let data =
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        assert!(process_sse_event("content_block_start", data, &mut state).is_none());
    }

    #[test]
    fn sse_error_event() {
        let mut state = ToolCallState::default();
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let result = process_sse_event("error", data, &mut state).unwrap();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Overloaded"));
    }

    #[test]
    fn usage_from_anthropic_body_maps_counts() {
        let body = json!({"usage": {"input_tokens": 10u64, "output_tokens": 20u64}});
        let usage = usage_from_anthropic_body(&body).unwrap();
        assert_eq!(usage.prompt_tokens, Some(10));
        assert_eq!(usage.completion_tokens, Some(20));
        assert_eq!(usage.total_tokens, Some(30));
    }

    #[test]
    fn usage_from_anthropic_body_drops_overflowing_counts() {
        let overflow = u64::from(u32::MAX) + 1;
        let body = json!({"usage": {"input_tokens": overflow, "output_tokens": 20u64}});
        let usage = usage_from_anthropic_body(&body).unwrap();
        assert_eq!(usage.prompt_tokens, None);
        assert_eq!(usage.completion_tokens, Some(20));
        assert_eq!(usage.total_tokens, None);
    }

    #[test]
    fn sse_message_delta_drops_overflowing_output_tokens() {
        let mut state = ToolCallState::default();
        let overflow = u64::from(u32::MAX) + 1;
        let data = format!(r#"{{"type":"message_delta","usage":{{"output_tokens":{overflow}}}}}"#);
        let chunk = process_sse_event("message_delta", &data, &mut state)
            .unwrap()
            .unwrap();
        let usage = chunk.usage.unwrap();
        assert_eq!(usage.completion_tokens, None);
        assert_eq!(usage.prompt_tokens, None);
        assert_eq!(usage.total_tokens, None);
    }

    #[test]
    fn sse_message_start_ignored() {
        let mut state = ToolCallState::default();
        let data = r#"{"type":"message_start","message":{"id":"msg_123"}}"#;
        assert!(process_sse_event("message_start", data, &mut state).is_none());
    }

    #[test]
    fn sse_message_stop_ignored() {
        let mut state = ToolCallState::default();
        let data = r#"{"type":"message_stop"}"#;
        assert!(process_sse_event("message_stop", data, &mut state).is_none());
    }

    #[test]
    fn sse_malformed_json_skipped() {
        let mut state = ToolCallState::default();
        assert!(process_sse_event("content_block_delta", "not json", &mut state).is_none());
    }

    #[test]
    fn capabilities_advertises_anthropic() {
        let plugin = AnthropicPlugin;
        let caps = plugin.llm_capabilities();
        assert_eq!(caps.len(), 1);
        let provider = &caps[0];
        assert_eq!(provider.kind, "anthropic");
        assert!(provider.supports_streaming);
        assert!(provider.supports_vision);
        assert_eq!(provider.supported_models.len(), 3);
        // A cloud HTTP proxy explicitly opts into higher-than-default
        // concurrency, per the `ConcurrencyHint` design (see the type's
        // docs): declaring it is evidence the choice was considered.
        assert_eq!(provider.concurrency.max_in_flight, 8);
        assert_eq!(provider.concurrency.queue_depth, 16);
    }
}
