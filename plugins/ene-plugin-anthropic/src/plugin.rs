//! Anthropic Claude plugin: Messages API streaming and completion.
//!
//! Implements the [`Plugin`] trait for the Anthropic Messages API, supporting
//! both SSE streaming (`create_chat_stream`) and non-streaming
//! (`chat_completion`) chat completions with tool use and vision.

use async_trait::async_trait;
use ene_plugin::{
    LlmProviderSpec, Plugin, PluginCapabilities, PluginError, PluginStream, PluginStreamChunk,
};
use serde_json::{Value, json};
use tokio_stream::wrappers::ReceiverStream;

use crate::convert;

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 8192;

/// Anthropic Claude plugin providing streaming and non-streaming chat
/// completions via the Anthropic Messages API.
pub(crate) struct AnthropicPlugin;

/// Resolves the API key from the provider configuration.
///
/// Resolution order:
/// 1. `config["api_key"]["inline"]` (non-empty string)
/// 2. Environment variable named by `config["api_key"]["env"]`
/// 3. `ANTHROPIC_API_KEY` environment variable
fn resolve_api_key(config: &Value) -> Result<String, PluginError> {
    let api_key = config.get("api_key");

    if let Some(inline) = api_key
        .and_then(|k| k.get("inline"))
        .and_then(Value::as_str)
        && !inline.is_empty()
    {
        return Ok(inline.to_string());
    }

    if let Some(env_name) = api_key.and_then(|k| k.get("env")).and_then(Value::as_str)
        && !env_name.is_empty()
        && let Ok(key) = std::env::var(env_name)
    {
        return Ok(key);
    }

    std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
        PluginError::provider(
            "no API key found: set api_key.inline, api_key.env, or ANTHROPIC_API_KEY",
        )
    })
}

/// Builds the Anthropic Messages API request body.
fn build_request_body(
    model: &str,
    max_tokens: Option<u32>,
    messages: &[Value],
    tools: &[Value],
    stream: bool,
) -> Value {
    let (system, anthropic_messages) = convert::to_anthropic_messages(messages);
    let anthropic_tools = convert::to_anthropic_tools(tools);

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
    }

    body
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

#[async_trait]
impl Plugin for AnthropicPlugin {
    fn capabilities(&self) -> PluginCapabilities {
        PluginCapabilities {
            llm_providers: vec![LlmProviderSpec {
                kind: "anthropic".to_string(),
                supported_models: vec![
                    "claude-sonnet-4-20250514".to_string(),
                    "claude-haiku-4-20250514".to_string(),
                    "claude-3-5-sonnet-20241022".to_string(),
                ],
                supports_streaming: true,
                supports_vision: true,
            }],
            ..Default::default()
        }
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
        let body = build_request_body(&model, max_tokens, &messages, &tools, true);

        let response = reqwest::Client::new()
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
        _json_schema: Option<Value>,
    ) -> Result<String, PluginError> {
        if kind != "anthropic" {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }

        let api_key = resolve_api_key(&config)?;
        let body = build_request_body(&model, max_tokens, &messages, &[], false);

        let response = reqwest::Client::new()
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

        extract_text_content(&body)
    }
}

/// Reads an Anthropic SSE stream and sends parsed chunks through the channel.
///
/// Processes `content_block_start` (`tool_use`), `content_block_delta`
/// (`text_delta`, `input_json_delta`), and `error` events. Other event types
/// (`message_start`, `content_block_stop`, `message_delta`, `message_stop`)
/// are acknowledged but produce no output chunks.
async fn stream_sse_response(
    mut response: reqwest::Response,
    tx: tokio::sync::mpsc::Sender<Result<PluginStreamChunk, PluginError>>,
) {
    let mut buffer = String::new();
    let mut event_type = String::new();
    let mut data_buf = String::new();

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

        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(newline_pos) = buffer.find('\n') {
            let rest = buffer.split_off(newline_pos.saturating_add(1));
            let line = buffer.trim_end_matches(['\r', '\n']).to_string();
            buffer = rest;

            if line.is_empty() {
                // Empty line marks the end of an SSE event.
                if !data_buf.is_empty() {
                    match process_sse_event(&event_type, &data_buf) {
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
/// Returns `None` for events that don't produce output (e.g. `message_start`,
/// `content_block_stop`, `message_delta`, `message_stop`).
fn process_sse_event(
    event_type: &str,
    data: &str,
) -> Option<Result<PluginStreamChunk, PluginError>> {
    let parsed: Value = serde_json::from_str(data).ok()?;

    match event_type {
        "content_block_start" => {
            let index = parsed.get("index")?.as_u64()?;
            let block = parsed.get("content_block")?;
            if block.get("type")?.as_str()? == "tool_use" {
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
                        "index": index,
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
                    let partial = delta
                        .get("partial_json")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    Some(Ok(PluginStreamChunk {
                        text_delta: None,
                        tool_calls_delta: Some(vec![json!({
                            "index": index,
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
    use super::*;

    #[test]
    fn resolve_api_key_inline() {
        let config = json!({"api_key": {"source": "inline", "inline": "sk-ant-test-123"}});
        let key = resolve_api_key(&config).unwrap();
        assert_eq!(key, "sk-ant-test-123");
    }

    #[test]
    fn resolve_api_key_inline_empty_falls_through() {
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
        // SAFETY: Test-only env var manipulation; single-threaded test context.
        unsafe {
            std::env::set_var("ENE_TEST_ANTHROPIC_KEY", "sk-env-test-456");
        }
        let config = json!({"api_key": {"source": "env", "env": "ENE_TEST_ANTHROPIC_KEY"}});
        let key = resolve_api_key(&config).unwrap();
        assert_eq!(key, "sk-env-test-456");
        // SAFETY: Test-only cleanup.
        unsafe {
            std::env::remove_var("ENE_TEST_ANTHROPIC_KEY");
        }
    }

    #[test]
    fn resolve_api_key_no_config_no_env_fails() {
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
        let body = build_request_body("claude-sonnet-4-20250514", Some(1024), &messages, &[], true);
        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["stream"], true);
        assert!(body.get("system").is_none());
        assert!(body.get("tools").is_none());
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
        let body = build_request_body("claude-sonnet-4-20250514", None, &messages, &tools, false);
        assert_eq!(body["system"], "Be helpful.");
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(body["stream"], false);
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert_eq!(body["tools"][0]["name"], "test.tool");
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
        let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let result = process_sse_event("content_block_delta", data)
            .unwrap()
            .unwrap();
        assert_eq!(result.text_delta.as_deref(), Some("Hello"));
        assert!(result.tool_calls_delta.is_none());
    }

    #[test]
    fn sse_input_json_delta_event() {
        let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"loc"}}"#;
        let result = process_sse_event("content_block_delta", data)
            .unwrap()
            .unwrap();
        assert!(result.text_delta.is_none());
        let deltas = result.tool_calls_delta.unwrap();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["index"], 1);
        assert_eq!(deltas[0]["arguments"], "{\"loc");
    }

    #[test]
    fn sse_tool_use_start_event() {
        let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_abc","name":"get_weather"}}"#;
        let result = process_sse_event("content_block_start", data)
            .unwrap()
            .unwrap();
        assert!(result.text_delta.is_none());
        let deltas = result.tool_calls_delta.unwrap();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["index"], 1);
        assert_eq!(deltas[0]["id"], "toolu_abc");
        assert_eq!(deltas[0]["name"], "get_weather");
        assert_eq!(deltas[0]["arguments"], "");
    }

    #[test]
    fn sse_text_block_start_ignored() {
        let data =
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
        assert!(process_sse_event("content_block_start", data).is_none());
    }

    #[test]
    fn sse_error_event() {
        let data = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let result = process_sse_event("error", data).unwrap();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Overloaded"));
    }

    #[test]
    fn sse_message_start_ignored() {
        let data = r#"{"type":"message_start","message":{"id":"msg_123"}}"#;
        assert!(process_sse_event("message_start", data).is_none());
    }

    #[test]
    fn sse_message_stop_ignored() {
        let data = r#"{"type":"message_stop"}"#;
        assert!(process_sse_event("message_stop", data).is_none());
    }

    #[test]
    fn sse_malformed_json_skipped() {
        assert!(process_sse_event("content_block_delta", "not json").is_none());
    }

    #[test]
    fn capabilities_advertises_anthropic() {
        let plugin = AnthropicPlugin;
        let caps = plugin.capabilities();
        assert_eq!(caps.llm_providers.len(), 1);
        let provider = &caps.llm_providers[0];
        assert_eq!(provider.kind, "anthropic");
        assert!(provider.supports_streaming);
        assert!(provider.supports_vision);
        assert_eq!(provider.supported_models.len(), 3);
        assert!(caps.tools.is_empty());
    }
}
