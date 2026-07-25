//! Anthropic Claude plugin: Messages API streaming and completion.
//!
//! Implements the [`Plugin`] trait for the Anthropic Messages API, supporting
//! both SSE streaming (`create_chat_stream`) and non-streaming
//! (`chat_completion`) chat completions with tool use and vision.
//! Structured output (`json_schema`) is emulated by forcing a synthetic tool
//! whose input schema matches the requested schema.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use ene_plugin::{LlmPlugin, LlmProviderSpec, PluginError, PluginStream, PluginStreamChunk};
use serde_json::{Value, json};
use tokio_stream::wrappers::ReceiverStream;

use crate::convert;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 8192;
/// Name of the synthetic tool used to force structured output.
const STRUCTURED_OUTPUT_TOOL: &str = "structured_output";
/// Timeout for establishing an HTTP connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Total timeout for a single HTTP request (covers streamed bodies).
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);

/// Shared HTTP client, built once with timeouts and reused for all requests.
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Anthropic Claude plugin providing streaming and non-streaming chat
/// completions via the Anthropic Messages API.
pub(crate) struct AnthropicPlugin;

/// Returns the shared HTTP client, building it on first use.
fn http_client() -> Result<&'static reqwest::Client, PluginError> {
    if let Some(client) = HTTP_CLIENT.get() {
        return Ok(client);
    }
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| PluginError::provider(format!("failed to build HTTP client: {e}")))?;
    // A racing task may have initialized first; either client is equivalent.
    let _ = HTTP_CLIENT.set(client);
    HTTP_CLIENT
        .get()
        .ok_or_else(|| PluginError::provider("HTTP client initialization failed"))
}

/// Resolves the API key from the provider configuration.
///
/// Accepted shapes for `config["api_key"]`:
/// 1. A plain JSON string (current host contract) — used directly.
/// 2. `{"source": "inline", "inline": "..."}` — the inline value.
/// 3. `{"source": "env", "env": "VAR"}` — the named environment variable
///    (defaults to `ANTHROPIC_API_KEY` when `env` is empty).
/// 4. `{"source": "auto"}`, any other shape, or missing — falls back to the
///    `ANTHROPIC_API_KEY` environment variable.
fn resolve_api_key(config: &Value) -> Result<String, PluginError> {
    match config.get("api_key") {
        Some(Value::String(key)) if !key.trim().is_empty() => return Ok(key.trim().to_string()),
        Some(Value::Object(obj)) => {
            let source = obj.get("source").and_then(Value::as_str).unwrap_or("auto");
            match source {
                "inline" => {
                    if let Some(inline) = obj.get("inline").and_then(Value::as_str)
                        && !inline.trim().is_empty()
                    {
                        return Ok(inline.trim().to_string());
                    }
                }
                "env" => {
                    let var_name = obj
                        .get("env")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .unwrap_or("ANTHROPIC_API_KEY");
                    if let Ok(key) = std::env::var(var_name)
                        && !key.is_empty()
                    {
                        return Ok(key);
                    }
                }
                // "auto" (or unrecognized) falls through to the env fallback.
                _ => {}
            }
        }
        _ => {}
    }

    std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
        PluginError::provider(
            "no API key found: set api_key, api_key.inline, api_key.env, or ANTHROPIC_API_KEY",
        )
    })
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

/// Extracts concatenated text content from a non-streaming Anthropic response.
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
        vec![LlmProviderSpec {
            kind: "anthropic".to_string(),
            supported_models: vec![
                "claude-sonnet-4-20250514".to_string(),
                "claude-haiku-4-20250514".to_string(),
                "claude-3-5-sonnet-20241022".to_string(),
            ],
            supports_streaming: true,
            supports_vision: true,
        }]
    }

    async fn create_chat_stream(
        &self,
        kind: &str,
        config: Value,
        model: String,
        max_tokens: Option<u32>,
        messages: Vec<Value>,
        tools: Vec<Value>,
    ) -> Result<PluginStream, PluginError> {
        if kind != "anthropic" {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }

        let api_key = resolve_api_key(&config)?;
        let body = build_request_body(&model, max_tokens, &messages, &tools, true, None);

        let response = http_client()?
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| PluginError::provider(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(PluginError::provider(format!(
                "Anthropic API error (HTTP {status}): {error_body}"
            )));
        }

        let (tx, rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(stream_sse_response(response, tx));

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn chat_completion(
        &self,
        kind: &str,
        config: Value,
        model: String,
        max_tokens: Option<u32>,
        messages: Vec<Value>,
        json_schema: Option<Value>,
    ) -> Result<String, PluginError> {
        if kind != "anthropic" {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }

        let api_key = resolve_api_key(&config)?;
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

        let response = http_client()?
            .post(ANTHROPIC_API_URL)
            .header("x-api-key", &api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| PluginError::provider(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(PluginError::provider(format!(
                "Anthropic API error (HTTP {status}): {error_body}"
            )));
        }

        let body: Value = response
            .json()
            .await
            .map_err(|e| PluginError::provider(format!("failed to parse response: {e}")))?;

        if forced_tool.is_some() {
            extract_tool_input(&body)
        } else {
            extract_text_content(&body)
        }
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
/// Raw bytes are accumulated and decoded only up to complete UTF-8 character
/// boundaries, so multi-byte characters split across network chunks are not
/// corrupted. Processes `content_block_start` (`tool_use`),
/// `content_block_delta` (`text_delta`, `input_json_delta`), and `error`
/// events; content-block indices are remapped to dense tool-call indices.
/// Other event types (`message_start`, `content_block_stop`, `message_delta`,
/// `message_stop`) are acknowledged but produce no output chunks.
async fn stream_sse_response(
    mut response: reqwest::Response,
    tx: tokio::sync::mpsc::Sender<Result<PluginStreamChunk, PluginError>>,
) {
    let mut raw_buf: Vec<u8> = Vec::new();
    let mut line_buf = String::new();
    let mut event_type = String::new();
    let mut data_buf = String::new();
    let mut tool_state = ToolCallState::default();

    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(e) => {
                let _ = tx
                    .send(Err(PluginError::provider(format!(
                        "stream read error: {e}"
                    ))))
                    .await;
                return;
            }
        };

        raw_buf.extend_from_slice(&chunk);

        // Decode only complete UTF-8 characters; a multi-byte character split
        // across chunks stays in `raw_buf` until the remaining bytes arrive
        // instead of being replaced with U+FFFD by a lossy per-chunk decode.
        let valid_len = match std::str::from_utf8(&raw_buf) {
            Ok(_) => raw_buf.len(),
            Err(e) => e.valid_up_to(),
        };
        let (valid, rest) = raw_buf.split_at(valid_len);
        line_buf.push_str(&String::from_utf8_lossy(valid));
        raw_buf = rest.to_vec();

        while let Some(newline_pos) = line_buf.find('\n') {
            let rest = line_buf.split_off(newline_pos.saturating_add(1));
            let line = line_buf.trim_end_matches(['\r', '\n']).to_string();
            line_buf = rest;

            if line.is_empty() {
                // Empty line marks the end of an SSE event.
                if !data_buf.is_empty() {
                    match process_sse_event(&event_type, &data_buf, &mut tool_state) {
                        Some(Ok(chunk)) => {
                            if tx.send(Ok(chunk)).await.is_err() {
                                return;
                            }
                        }
                        Some(Err(e)) => {
                            let _ = tx.send(Err(e)).await;
                            return;
                        }
                        None => {}
                    }
                }
                event_type.clear();
                data_buf.clear();
            } else if let Some(event) = line.strip_prefix("event:") {
                event_type = event.trim().to_string();
            } else if let Some(data) = line.strip_prefix("data:") {
                if !data_buf.is_empty() {
                    data_buf.push('\n');
                }
                data_buf.push_str(data.trim());
            }
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
                    }))
                }
                _ => None,
            }
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
    clippy::unwrap_used,
    reason = "unit tests use unwrap for concise assertions"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "test assertions index into known-length vectors"
)]
mod tests {
    use std::sync::{Mutex, PoisonError};

    use super::*;

    /// Serializes tests that read or mutate the process environment:
    /// `set_var`/`remove_var` are process-global and would otherwise race
    /// with concurrent env reads in other tests.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_api_key_plain_string() {
        let config = json!({"api_key": "sk-ant-plain-789"});
        let key = resolve_api_key(&config).unwrap();
        assert_eq!(key, "sk-ant-plain-789");
    }

    #[test]
    fn resolve_api_key_plain_string_empty_falls_through() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        let config = json!({"api_key": ""});
        // An empty string must not be used as the key; resolution falls
        // through to ANTHROPIC_API_KEY (which may or may not be set).
        if let Ok(key) = resolve_api_key(&config) {
            assert!(!key.is_empty());
        }
    }

    #[test]
    fn resolve_api_key_inline() {
        let config = json!({"api_key": {"source": "inline", "inline": "sk-ant-test-123"}});
        let key = resolve_api_key(&config).unwrap();
        assert_eq!(key, "sk-ant-test-123");
    }

    #[test]
    fn resolve_api_key_inline_empty_falls_through() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        let config = json!({"api_key": {"source": "inline", "inline": ""}});
        // Should fall through to ANTHROPIC_API_KEY (which may or may not be set).
        let result = resolve_api_key(&config);
        // We can't assert success/failure without controlling the env, but
        // it should not return the empty string.
        if let Ok(key) = result {
            assert!(!key.is_empty());
        }
    }

    #[test]
    fn resolve_api_key_from_env_var() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: Test-only env var mutation, serialized by `ENV_MUTEX`.
        unsafe {
            std::env::set_var("ENE_TEST_ANTHROPIC_KEY", "sk-env-test-456");
        }
        let config = json!({"api_key": {"source": "env", "env": "ENE_TEST_ANTHROPIC_KEY"}});
        let key = resolve_api_key(&config).unwrap();
        assert_eq!(key, "sk-env-test-456");
        // SAFETY: Test-only cleanup, serialized by `ENV_MUTEX`.
        unsafe {
            std::env::remove_var("ENE_TEST_ANTHROPIC_KEY");
        }
    }

    #[test]
    fn resolve_api_key_auto_source_uses_env_fallback() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        let original = std::env::var("ANTHROPIC_API_KEY").ok();
        // SAFETY: Test-only env var mutation, serialized by `ENV_MUTEX`.
        unsafe {
            std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-auto-000");
        }

        let config = json!({"api_key": {"source": "auto"}});
        assert_eq!(resolve_api_key(&config).unwrap(), "sk-ant-auto-000");
        // A missing api_key entry also falls back to the environment.
        assert_eq!(resolve_api_key(&json!({})).unwrap(), "sk-ant-auto-000");

        if let Some(value) = original {
            // SAFETY: Test-only env restore, serialized by `ENV_MUTEX`.
            unsafe {
                std::env::set_var("ANTHROPIC_API_KEY", value);
            }
        } else {
            // SAFETY: Test-only env restore, serialized by `ENV_MUTEX`.
            unsafe {
                std::env::remove_var("ANTHROPIC_API_KEY");
            }
        }
    }

    #[test]
    fn resolve_api_key_no_config_no_env_fails() {
        let _guard = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        let config = json!({});
        // Without ANTHROPIC_API_KEY set, this should fail.
        // (If the env var happens to be set in CI, this test still passes
        // because we only check the error case when it's absent.)
        if std::env::var("ANTHROPIC_API_KEY").is_err() {
            assert!(resolve_api_key(&config).is_err());
        }
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
        // Content-block index 1 is the first tool → dense index 0.
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
        // Content-block index 1 is the first tool → dense index 0.
        assert_eq!(deltas[0]["index"], 0);
        assert_eq!(deltas[0]["id"], "toolu_abc");
        assert_eq!(deltas[0]["name"], "get_weather");
        assert_eq!(deltas[0]["arguments"], "");
    }

    #[test]
    fn sse_tool_indices_are_dense_across_text_blocks() {
        let mut state = ToolCallState::default();

        // Block 0 is text → ignored and consumes no tool index.
        let text_start =
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        assert!(process_sse_event("content_block_start", text_start, &mut state).is_none());

        // Block 1 is the first tool → dense index 0.
        let tool1_start = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_a","name":"a"}}"#;
        let chunk = process_sse_event("content_block_start", tool1_start, &mut state)
            .unwrap()
            .unwrap();
        assert_eq!(chunk.tool_calls_delta.unwrap()[0]["index"], 0);

        // Block 2 is the second tool → dense index 1.
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
    }
}
