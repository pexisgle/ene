//! OpenAI-compatible provider plugin: chat, streaming, and embeddings.
//!
//! Implements [`LlmPlugin`] (SSE streaming + non-streaming chat completions
//! with tool use, vision, and structured output) and [`EmbedPlugin`] (batch
//! embeddings) against any OpenAI-compatible `/v1` endpoint, using plain
//! HTTP rather than the `async-openai` client so the transport tweaks
//! (thinking-disabled bodies, `stream_options.include_usage`,
//! `Retry-After` handling) have a single code path.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use ene_plugin::prelude::*;
use serde_json::{Value, json};
use tokio_stream::wrappers::ReceiverStream;

use crate::convert;

const DEFAULT_API_BASE: &str = "https://api.openai.com/v1";
/// Timeout for establishing an HTTP connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Total timeout for a single HTTP request (covers streamed bodies).
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);
/// Retry budget for transient upstream failures (429 / network), matching
/// the defaults `ene-ai` applies to its in-process retry policy.
const MAX_ATTEMPTS: u32 = 3;
const BASE_DELAY: Duration = Duration::from_millis(500);
const MAX_DELAY: Duration = Duration::from_secs(30);

/// Shared HTTP client, built once with timeouts and reused for all requests.
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Configuration delivered by the host at handshake time
/// (`plugins.list.openai.config`), stored per process.
///
/// `Mutex` (rather than `OnceLock`) so tests can reset it between cases; in
/// production the handshake is a one-shot and reconnects resend the same
/// blob, so last-writer-wins is equivalent.
static PLUGIN_CONFIG: Mutex<Option<Value>> = Mutex::new(None);

/// OpenAI-compatible provider plugin serving chat, streaming, and embeddings.
///
/// The static capability data (`llm_spec()` / `LLM_PROVIDER_KIND`) is
/// generated from the `#[provider(...)]` attribute; the async handlers below
/// stay hand-written.
#[derive(LlmPlugin)]
#[provider(
    kind = "openai",
    models = "gpt-4o-mini, gpt-4o, gpt-4.1-mini, gpt-4.1, o3-mini",
    streaming,
    vision,
    // A stateless HTTP proxy to a cloud API, not a local model — safe to
    // run many requests concurrently, mirroring the anthropic plugin's
    // explicit concurrency declaration.
    concurrency = 8,
    queue_depth = 16,
    // Modern OpenAI-compatible models expose a 128k-token context window.
    context_window = 128_000,
)]
pub(crate) struct OpenAiPlugin;

impl ene_plugin::ConfigurablePlugin for OpenAiPlugin {
    /// Receives the plugin configuration blob from the host at handshake
    /// time (`plugins.list.openai.config`).
    fn set_config(&self, config: &Value) {
        *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = Some(config.clone());
    }

    /// Advertises the config schema; `api_key` is marked `x-ene-secret: true`
    /// so the host masks/redacts it.
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
                    "description": "OpenAI API key, or a {source: inline|env|auto} descriptor"
                },
                "base_url": {
                    "type": "string",
                    "description": "API base URL override (defaults to https://api.openai.com/v1)"
                }
            }
        }))
    }
}

#[async_trait]
impl EmbedPlugin for OpenAiPlugin {
    fn embed_providers(&self) -> Vec<String> {
        vec![Self::LLM_PROVIDER_KIND.to_string()]
    }

    async fn embed_batch(
        &self,
        kind: &str,
        config: Value,
        model: String,
        _dimensions: Option<u32>,
        items: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, PluginError> {
        if kind != Self::LLM_PROVIDER_KIND {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }
        if items.is_empty() {
            return Ok(Vec::new());
        }
        for text in &items {
            if text.trim().is_empty() {
                return Err(PluginError::provider(
                    "cannot embed empty text; refusing to pollute the vector store",
                ));
            }
        }

        let api_key = resolve_api_key(&config)?;
        let base_url = resolve_base_url(&config);
        let body = json!({ "model": model, "input": items });

        let response = post_with_retry(&base_url, &api_key, "embeddings", &body).await?;
        let raw = response
            .text()
            .await
            .map_err(|e| PluginError::provider(format!("failed to read response: {e}")))?;
        let parsed: Value = serde_json::from_str(&raw)
            .map_err(|e| PluginError::provider(format!("failed to parse response: {e}")))?;

        let data = parsed
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| PluginError::provider("embedding response missing data array"))?;
        let mut embeddings = Vec::with_capacity(data.len());
        for item in data {
            let embedding = item
                .get("embedding")
                .and_then(Value::as_array)
                .ok_or_else(|| PluginError::provider("embedding response missing vector"))?
                .iter()
                .map(|v| v.as_f64().unwrap_or_default() as f32)
                .collect::<Vec<f32>>();
            embeddings.push(embedding);
        }
        Ok(embeddings)
    }
}

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
    drop(HTTP_CLIENT.set(client));
    HTTP_CLIENT
        .get()
        .ok_or_else(|| PluginError::provider("HTTP client initialization failed"))
}

/// Resolves a single `api_key` value per the `{"source": ...}` contract.
///
/// Accepted shapes for `value`:
/// 1. A plain JSON string — used directly.
/// 2. `{"source": "inline", "inline": "..."}` — the inline value.
/// 3. `{"source": "env", "env": "VAR"}` — the named environment variable
///    (defaults to `OPENAI_API_KEY` when `env` is empty).
/// 4. Anything else (including `{"source": "auto"}`) — `None`, so the caller
///    falls back to the process environment.
fn resolve_key_value(value: &Value) -> Option<String> {
    match value {
        Value::String(key) if !key.trim().is_empty() => Some(key.trim().to_string()),
        Value::Object(obj) => {
            let source = obj.get("source").and_then(Value::as_str).unwrap_or("auto");
            match source {
                "inline" => obj
                    .get("inline")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|key| !key.is_empty())
                    .map(str::to_string),
                "env" => {
                    let var_name = obj
                        .get("env")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .unwrap_or("OPENAI_API_KEY");
                    std::env::var(var_name).ok().filter(|key| !key.is_empty())
                }
                // "auto" (or unrecognized) falls through to the caller's
                // process-env fallback.
                _ => None,
            }
        }
        _ => None,
    }
}

/// Resolves the effective API key for a request.
///
/// Precedence:
/// 1. The config delivered by the host at handshake time via
///    [`ConfigurablePlugin::set_config`] (`plugins.list.openai.config`),
///    resolved with the same `{"source": ...}` contract.
/// 2. The per-request `config` argument (`ai.providers.<name>.api_key`).
/// 3. The `OPENAI_API_KEY` environment variable.
fn resolve_api_key(config: &Value) -> Result<String, PluginError> {
    let host_config = PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner);
    resolve_api_key_with_host(host_config.as_ref(), config)
}

/// The precedence logic behind [`resolve_api_key`], parameterized over the
/// host-delivered config so tests can exercise it deterministically.
fn resolve_api_key_with_host(
    host_config: Option<&Value>,
    config: &Value,
) -> Result<String, PluginError> {
    if let Some(key) = host_config
        .and_then(|cfg| cfg.get("api_key"))
        .and_then(resolve_key_value)
    {
        return Ok(key);
    }
    if let Some(key) = config.get("api_key").and_then(resolve_key_value) {
        return Ok(key);
    }
    std::env::var("OPENAI_API_KEY").map_err(|_| {
        PluginError::provider(
            "no API key found: set api_key, api_key.inline, api_key.env, or OPENAI_API_KEY",
        )
    })
}

/// Resolves the effective API base URL, with the same precedence as the API
/// key, falling back to the OpenAI default.
fn resolve_base_url(config: &Value) -> String {
    let host_config = PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner);
    host_config
        .and_then(|cfg| cfg.get("base_url"))
        .or_else(|| config.get("base_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map_or_else(|| DEFAULT_API_BASE.to_string(), str::to_string)
}

/// Heuristic: models whose name contains "mimo" (case-insensitive) fill
/// `reasoning_content` instead of `content` unless thinking is disabled.
/// Extend the match if other reasoning models exhibit the same behavior.
fn model_wants_thinking_disabled(model: &str) -> bool {
    model.to_ascii_lowercase().contains("mimo")
}

/// An upstream OpenAI-compatible API failure, before mapping to
/// [`PluginError`]. Transport failures and HTTP 429 are retryable;
/// everything else is terminal.
enum UpstreamError {
    /// Transport-level failure (DNS, connect, timeout, mid-read).
    Network(String),
    /// Non-success HTTP status with the (possibly truncated) response body.
    Http {
        /// HTTP status code.
        status: u16,
        /// Response body snippet.
        body: String,
        /// `Retry-After` header value, if present and parseable.
        retry_after: Option<u64>,
    },
}

impl UpstreamError {
    /// Whether the retry budget should spend an attempt on this error.
    fn is_retryable(&self) -> bool {
        matches!(self, Self::Network(_) | Self::Http { status: 429, .. })
    }

    /// Maps to a [`PluginError::Provider`] with a status-aware message.
    fn into_plugin_error(self) -> PluginError {
        match self {
            Self::Network(message) => PluginError::provider(message),
            Self::Http { status, body, .. } => {
                let snippet: String = body.chars().take(280).collect();
                match status {
                    401 | 403 => PluginError::provider(format!("authentication failed: {snippet}")),
                    429 => PluginError::provider(format!("rate limited: {snippet}")),
                    402 => PluginError::provider(format!(
                        "HTTP 402 Payment Required (often OpenRouter credit collateral for max_tokens): {snippet}"
                    )),
                    _ => PluginError::provider(format!("HTTP {status}: {snippet}")),
                }
            }
        }
    }
}

/// Parses a `Retry-After` response header (seconds) if present and valid.
fn retry_after_secs(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
}

/// POSTs `body` to `{base_url}/{endpoint}`, retrying transient failures
/// (network / HTTP 429) with exponential backoff and jitter.
///
/// Retries happen before the response body is consumed; a 2xx response is
/// returned as-is so the caller can stream or parse it. Non-transient
/// statuses fail immediately with the body snippet in the message.
async fn post_with_retry(
    base_url: &str,
    api_key: &str,
    endpoint: &str,
    body: &Value,
) -> Result<reqwest::Response, PluginError> {
    let url = format!("{}/{}", base_url.trim_end_matches('/'), endpoint);
    let client = http_client()?;

    let mut attempt: u32 = 0;
    loop {
        let sent = client
            .post(&url)
            .bearer_auth(api_key)
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await;

        let err = match sent {
            Ok(response) => {
                if response.status().is_success() {
                    return Ok(response);
                }
                let retry_after = retry_after_secs(&response);
                let raw = response.text().await.map_err(|e| {
                    PluginError::provider(format!("failed to read error response: {e}"))
                })?;
                UpstreamError::Http {
                    status: response.status().as_u16(),
                    body: raw,
                    retry_after,
                }
            }
            Err(e) => UpstreamError::Network(format!("HTTP request failed: {e}")),
        };

        let next = attempt.saturating_add(1);
        if !err.is_retryable() || next >= MAX_ATTEMPTS {
            return Err(err.into_plugin_error());
        }
        let delay = match &err {
            UpstreamError::Http {
                retry_after: Some(secs),
                ..
            } => Duration::from_secs(*secs),
            _ => backoff_delay(attempt),
        }
        .min(MAX_DELAY);
        tracing::warn!(
            component = "ene-plugin-openai",
            attempt = next,
            delay_ms = delay.as_millis() as u64,
            error = %err.into_plugin_error(),
            "retryable upstream failure; backing off"
        );
        tokio::time::sleep(delay).await;
        attempt = next;
    }
}

/// Jittered exponential backoff for retry attempt `retry_index` (0-indexed).
fn backoff_delay(retry_index: u32) -> Duration {
    let base_ms = BASE_DELAY.as_millis() as u64;
    let exp_ms = base_ms.saturating_mul(2u64.saturating_pow(retry_index));
    let capped = exp_ms.min(MAX_DELAY.as_millis() as u64);
    let jittered = if capped == 0 {
        0
    } else {
        rand::random_range(0..=capped)
    };
    Duration::from_millis(jittered)
}

/// Builds the chat completion request body.
///
/// `stream` and `thinking_disabled` insert the transport tweaks that the
/// plain-HTTP path needs: `stream_options.include_usage` (usage on the final
/// SSE chunk) and `thinking: {type: disabled}` (MiMo-class models).
fn build_chat_body(
    model: &str,
    max_tokens: Option<u32>,
    messages: Vec<Value>,
    tools: Vec<Value>,
    json_schema: Option<Value>,
    stream: bool,
    thinking_disabled: bool,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
    });
    if let Some(obj) = body.as_object_mut() {
        if let Some(max_tokens) = max_tokens {
            obj.insert("max_tokens".to_string(), json!(max_tokens));
        }
        if !tools.is_empty() {
            obj.insert("tools".to_string(), Value::Array(tools));
        }
        if let Some(schema) = json_schema {
            // Accept either a raw JSON Schema object or a `{ "schema": ... }`
            // wrapper.
            let schema = schema.get("schema").cloned().unwrap_or(schema);
            obj.insert(
                "response_format".to_string(),
                json!({
                    "type": "json_schema",
                    "json_schema": {
                        "description": "Structured output",
                        "name": "StructuredOutput",
                        "schema": schema,
                    }
                }),
            );
        }
        if stream {
            // Ask the provider to append a `usage` object to the final SSE
            // chunk. Providers that do not recognize the option ignore it;
            // those that do report token counts there, which the stream
            // parser picks up.
            obj.insert("stream".to_string(), json!(true));
            obj.insert("stream_options".to_string(), json!({"include_usage": true}));
        }
        if thinking_disabled {
            obj.insert("thinking".to_string(), json!({"type": "disabled"}));
        }
    }
    body
}

/// Reads an SSE response body and sends parsed chunks through the channel.
///
/// Skips malformed payloads (debug-logged), stops at `[DONE]`, and emits a
/// usage-only chunk when the final payload carries one.
async fn stream_sse_response(
    response: reqwest::Response,
    name_mapping: HashMap<String, String>,
    tx: tokio::sync::mpsc::Sender<Result<PluginStreamChunk, PluginError>>,
) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio_util::io::StreamReader;

    let bytes_stream = response
        .bytes_stream()
        .map(|res| res.map_err(std::io::Error::other));
    let reader = StreamReader::new(bytes_stream);
    let mut lines = BufReader::new(reader).lines();

    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(e) => {
                drop(
                    tx.send(Err(PluginError::provider(format!(
                        "read stream line failed: {e}"
                    ))))
                    .await,
                );
                return;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(payload) = trimmed.strip_prefix("data: ") else {
            continue;
        };
        let payload = payload.trim();
        if payload == "[DONE]" {
            break;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(payload) else {
            tracing::debug!(
                component = "ene-plugin-openai",
                payload = %payload,
                "skipping malformed SSE chunk"
            );
            continue;
        };
        if let Some(chunk) = convert::process_sse_chunk(&chunk, &name_mapping)
            && tx.send(Ok(chunk)).await.is_err()
        {
            return;
        }
    }
}

#[async_trait]
impl LlmPlugin for OpenAiPlugin {
    fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
        vec![Self::llm_spec()]
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
        if kind != Self::LLM_PROVIDER_KIND {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }

        let api_key = resolve_api_key(&config)?;
        let base_url = resolve_base_url(&config);
        let oa_messages = convert::to_openai_messages(&messages)?;
        let oa_tools = convert::to_openai_tools(&tools);
        let name_mapping = convert::tool_name_mapping(&tools);
        let body = build_chat_body(
            &model,
            max_tokens,
            oa_messages,
            oa_tools,
            None,
            true,
            model_wants_thinking_disabled(&model),
        );

        let response = post_with_retry(&base_url, &api_key, "chat/completions", &body).await?;

        let (tx, rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(stream_sse_response(response, name_mapping, tx));

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
    ) -> Result<PluginCompletion, PluginError> {
        if kind != Self::LLM_PROVIDER_KIND {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }

        let api_key = resolve_api_key(&config)?;
        let base_url = resolve_base_url(&config);
        let oa_messages = convert::to_openai_messages(&messages)?;
        let body = build_chat_body(
            &model,
            max_tokens,
            oa_messages,
            Vec::new(),
            json_schema,
            false,
            model_wants_thinking_disabled(&model),
        );

        let response = post_with_retry(&base_url, &api_key, "chat/completions", &body).await?;
        let raw = response
            .text()
            .await
            .map_err(|e| PluginError::provider(format!("failed to read response: {e}")))?;
        let parsed: Value = serde_json::from_str(&raw)
            .map_err(|e| PluginError::provider(format!("failed to parse response: {e}")))?;

        let choice = parsed
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .ok_or_else(|| {
                tracing::warn!(component = "ene-plugin-openai", "no choices in response");
                PluginError::provider("provider returned no choices")
            })?;

        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            match reason {
                "length" => {
                    return Err(PluginError::provider(
                        "finish_reason=length: configured token limit reached",
                    ));
                }
                "content_filter" => {
                    return Err(PluginError::provider(
                        "finish_reason=content_filter: provider blocked the response",
                    ));
                }
                _ => {}
            }
        }

        let text = choice
            .get("message")
            .map(convert::text_from_message)
            .unwrap_or_default();
        Ok(PluginCompletion {
            text,
            usage: convert::usage_from_json_value(parsed.get("usage")),
        })
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit tests use expect/unwrap for concise assertions"
)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use ene_plugin::ConfigurablePlugin;

    /// Serializes tests that read or mutate the process environment:
    /// `set_var`/`remove_var` are process-global and would otherwise race
    /// with concurrent env reads in other tests.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Serializes tests that read or write the process-wide `PLUGIN_CONFIG`
    /// static (every `resolve_api_key` / `resolve_base_url` call reads it).
    static TEST_SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_api_key_plain_string() {
        let _guard = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let config = json!({"api_key": "sk-plain-789"});
        assert_eq!(resolve_api_key(&config).unwrap(), "sk-plain-789");
    }

    #[test]
    fn resolve_api_key_inline() {
        let _guard = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let config = json!({"api_key": {"source": "inline", "inline": "sk-inline-123"}});
        assert_eq!(resolve_api_key(&config).unwrap(), "sk-inline-123");
    }

    #[test]
    fn resolve_api_key_from_env_var() {
        let _guard = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: Test-only env var mutation, serialized by `ENV_MUTEX`.
        unsafe {
            std::env::set_var("ENE_TEST_OPENAI_KEY", "sk-env-test-456");
        }
        let config = json!({"api_key": {"source": "env", "env": "ENE_TEST_OPENAI_KEY"}});
        assert_eq!(resolve_api_key(&config).unwrap(), "sk-env-test-456");
        // SAFETY: Test-only cleanup, serialized by `ENV_MUTEX`.
        unsafe {
            std::env::remove_var("ENE_TEST_OPENAI_KEY");
        }
    }

    #[test]
    fn resolve_api_key_auto_source_uses_env_fallback() {
        let _guard = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        let original = std::env::var("OPENAI_API_KEY").ok();
        // SAFETY: Test-only env var mutation, serialized by `ENV_MUTEX`.
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "sk-auto-000");
        }

        let config = json!({"api_key": {"source": "auto"}});
        assert_eq!(resolve_api_key(&config).unwrap(), "sk-auto-000");
        assert_eq!(resolve_api_key(&json!({})).unwrap(), "sk-auto-000");

        if let Some(value) = original {
            // SAFETY: Test-only env restore, serialized by `ENV_MUTEX`.
            unsafe {
                std::env::set_var("OPENAI_API_KEY", value);
            }
        } else {
            // SAFETY: Test-only env restore, serialized by `ENV_MUTEX`.
            unsafe {
                std::env::remove_var("OPENAI_API_KEY");
            }
        }
    }

    #[test]
    fn host_config_key_wins_over_request_config() {
        let host = json!({"api_key": "sk-host-wins"});
        let request = json!({"api_key": "sk-request-loses"});
        assert_eq!(
            resolve_api_key_with_host(Some(&host), &request).unwrap(),
            "sk-host-wins"
        );
    }

    #[test]
    fn host_config_supports_inline_shape() {
        let host = json!({"api_key": {"source": "inline", "inline": "sk-host-inline"}});
        assert_eq!(
            resolve_api_key_with_host(Some(&host), &json!({})).unwrap(),
            "sk-host-inline"
        );
    }

    #[test]
    fn missing_host_key_falls_back_to_request_config() {
        let host = json!({"base_url": "https://example.com"});
        let request = json!({"api_key": "sk-request-fallback"});
        assert_eq!(
            resolve_api_key_with_host(Some(&host), &request).unwrap(),
            "sk-request-fallback"
        );
    }

    #[test]
    fn set_config_stores_key_used_by_resolve() {
        let _guard = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
        OpenAiPlugin.set_config(&json!({"api_key": "sk-via-set-config"}));
        assert_eq!(
            resolve_api_key(&json!({})).unwrap(),
            "sk-via-set-config",
            "key delivered via set_config must be used by request resolution"
        );
        *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    #[test]
    fn resolve_base_url_precedence_and_default() {
        let _guard = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
        assert_eq!(resolve_base_url(&json!({})), DEFAULT_API_BASE);
        assert_eq!(
            resolve_base_url(&json!({"base_url": "https://example.com/v1"})),
            "https://example.com/v1"
        );
        OpenAiPlugin.set_config(&json!({"base_url": "https://host.example/v1"}));
        assert_eq!(
            resolve_base_url(&json!({"base_url": "https://request.example/v1"})),
            "https://host.example/v1"
        );
        *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    #[test]
    fn config_schema_marks_api_key_secret() {
        let schema = OpenAiPlugin.config_schema().expect("schema");
        assert_eq!(
            schema.pointer("/properties/api_key/x-ene-secret"),
            Some(&json!(true))
        );
    }

    #[test]
    fn embed_providers_advertises_openai_kind() {
        assert_eq!(OpenAiPlugin.embed_providers(), vec!["openai"]);
    }

    #[test]
    fn build_chat_body_carries_optional_fields() {
        let body = build_chat_body(
            "gpt-4o-mini",
            Some(512),
            vec![json!({"role": "user", "content": "Hi"})],
            Vec::new(),
            None,
            false,
            false,
        );
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["max_tokens"], 512);
        assert!(body.get("stream").is_none());
        assert!(body.get("stream_options").is_none());
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn build_chat_body_streaming_adds_usage_option() {
        let body = build_chat_body(
            "gpt-4o-mini",
            None,
            vec![json!({"role": "user", "content": "Hi"})],
            Vec::new(),
            None,
            true,
            false,
        );
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"], json!({"include_usage": true}));
    }

    #[test]
    fn build_chat_body_thinking_disabled() {
        let body = build_chat_body(
            "mimo-1",
            None,
            vec![json!({"role": "user", "content": "Hi"})],
            Vec::new(),
            None,
            false,
            true,
        );
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
    }

    #[test]
    fn build_chat_body_json_schema_unwraps_wrapper() {
        let schema = json!({"schema": {"type": "object", "properties": {}}});
        let body = build_chat_body(
            "gpt-4o-mini",
            None,
            vec![json!({"role": "user", "content": "Hi"})],
            Vec::new(),
            Some(schema),
            false,
            false,
        );
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["schema"],
            json!({"type": "object", "properties": {}})
        );
    }

    #[test]
    fn build_chat_body_includes_tools() {
        let tools = vec![json!({
            "type": "function",
            "function": {"name": "web_search", "description": "", "parameters": {}}
        })];
        let body = build_chat_body(
            "gpt-4o-mini",
            None,
            vec![json!({"role": "user", "content": "Hi"})],
            tools.clone(),
            None,
            false,
            false,
        );
        assert_eq!(body["tools"], Value::Array(tools));
    }

    #[test]
    fn model_wants_thinking_disabled_heuristic() {
        assert!(model_wants_thinking_disabled("openrouter/mimo-7b"));
        assert!(model_wants_thinking_disabled("MIMO-32B"));
        assert!(!model_wants_thinking_disabled("gpt-4o-mini"));
    }

    #[test]
    fn capabilities_advertise_openai_kind() {
        let plugin = OpenAiPlugin;
        let caps = plugin.llm_capabilities();
        assert_eq!(caps.len(), 1);
        let provider = &caps[0];
        assert_eq!(provider.kind, "openai");
        assert!(provider.supports_streaming);
        assert!(provider.supports_vision);
        assert_eq!(provider.concurrency.max_in_flight, 8);
        assert_eq!(provider.concurrency.queue_depth, 16);
        assert_eq!(provider.context_window, Some(128_000));
    }

    #[test]
    fn backoff_delay_respects_caps() {
        assert!(backoff_delay(0) <= MAX_DELAY);
        assert!(backoff_delay(10) <= MAX_DELAY);
    }
}
