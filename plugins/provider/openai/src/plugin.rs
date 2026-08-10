//! OpenAI-compatible provider plugin: chat, streaming, and embeddings.
//!
//! Implements [`LlmPlugin`] (SSE streaming + non-streaming chat completions
//! with tool use, vision, and structured output) and [`EmbedPlugin`] (batch
//! embeddings) against any OpenAI-compatible `/v1` endpoint. All HTTP
//! traffic is mediated by the host through the `Network` broker (SSRF
//! guard, origin approval, size caps, credential injection); the plugin
//! keeps a single code path for the transport tweaks (thinking-disabled
//! bodies, `stream_options.include_usage`, `Retry-After` handling).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use ene_plugin::prelude::*;
use serde_json::{Value, json};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use crate::broker::{FetchOutcome, StreamSession, broker};
use crate::convert;

const DEFAULT_API_BASE: &str = "https://api.openai.com/v1";
/// Retry budget for transient upstream failures (429 / network), matching
/// the defaults `ene-ai` applies to its in-process retry policy.
const MAX_ATTEMPTS: u32 = 3;
const BASE_DELAY: Duration = Duration::from_millis(500);
const MAX_DELAY: Duration = Duration::from_secs(30);

/// Configuration delivered by the host at handshake time
/// (`plugins.list.openai.config`), stored per process. The API key is no
/// longer part of this blob's contract — the host injects it into broker
/// requests by key name.
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
                    "description": "OpenAI API key, or a {source: inline|env|auto} descriptor"
                },
                "base_url": {
                    "type": "string",
                    "description": "API base URL override (defaults to https://api.openai.com/v1)"
                },
                "context_window": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Advertised context window override in tokens"
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

        let base_url = resolve_base_url(&config);
        let body = json!({ "model": model, "input": items });

        let response = post_with_retry(&base_url, "api_key", "embeddings", &body).await?;
        let raw = String::from_utf8_lossy(&response.body);
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

/// Resolves the effective API base URL, with the same precedence as the API
/// key: host config, then request config, then the `OPENAI_BASE_URL`
/// environment variable, falling back to the `OpenAI` default.
fn resolve_base_url(config: &Value) -> String {
    let host_config = PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner);
    host_config
        .as_ref()
        .and_then(|cfg| cfg.get("base_url"))
        .or_else(|| config.get("base_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map_or_else(
            || {
                std::env::var("OPENAI_BASE_URL")
                    .ok()
                    .map(|url| url.trim().to_string())
                    .filter(|url| !url.is_empty())
                    .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
            },
            str::to_string,
        )
}

/// Heuristic: models whose name contains "mimo" (case-insensitive) fill
/// `reasoning_content` instead of `content` unless thinking is disabled.
/// Extend the match if other reasoning models exhibit the same behavior.
fn model_wants_thinking_disabled(model: &str) -> bool {
    model.to_ascii_lowercase().contains("mimo")
}

/// Effective thinking-disabled decision for a request: the host's explicit
/// `thinking_disabled` instruction (forwarded from the task config by
/// `IpcLlmProviderFactory`) wins; absent, fall back to the model-name
/// heuristic.
fn effective_thinking_disabled(config: &Value, model: &str) -> bool {
    config
        .get("thinking_disabled")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| model_wants_thinking_disabled(model))
}

/// An upstream OpenAI-compatible API failure, before mapping to
/// [`PluginError`]. Transport failures and HTTP 429 are retryable;
/// everything else is terminal.
#[derive(Clone)]
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
                    401 | 403 => PluginError::provider_typed(
                        ProviderErrorKind::Auth,
                        format!("authentication failed: {snippet}"),
                    ),
                    429 => PluginError::provider_typed(
                        ProviderErrorKind::RateLimit,
                        format!("rate limited: {snippet}"),
                    ),
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
fn retry_after_secs(headers: &[(String, String)]) -> Option<u64> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
}

/// POSTs `body` to `{base_url}/{endpoint}` through the network broker,
/// retrying transient failures (network / HTTP 429) with exponential
/// backoff and jitter.
///
/// The host injects the credential named by `credential` (typically
/// `"api_key"`) as `Authorization: Bearer <value>`; the plugin never holds
/// the value. Non-transient statuses fail immediately with the body snippet
/// in the message.
async fn post_with_retry(
    base_url: &str,
    credential: &str,
    endpoint: &str,
    body: &Value,
) -> Result<FetchOutcome, PluginError> {
    let url = format!("{}/{}", base_url.trim_end_matches('/'), endpoint);
    let payload = serde_json::to_vec(body)
        .map_err(|e| PluginError::provider(format!("failed to serialize request: {e}")))?;

    let mut attempt: u32 = 0;
    loop {
        let sent = broker()
            .fetch(
                ene_plugin_broker::HttpMethod::Post,
                &url,
                vec![("Content-Type".to_string(), "application/json".to_string())],
                Some(credential),
                Some(payload.clone()),
            )
            .await;

        let err = match sent {
            Ok(response) if (200..300).contains(&response.status) => return Ok(response),
            Ok(response) => {
                let status = response.status;
                let retry_after = retry_after_secs(&response.headers);
                let raw = String::from_utf8_lossy(&response.body).into_owned();
                UpstreamError::Http {
                    status,
                    body: raw,
                    retry_after,
                }
            }
            Err(e) => UpstreamError::Network(format!("broker request failed: {e}")),
        };

        let Some(delay) = retry_delay(&err, attempt) else {
            return Err(err.into_plugin_error());
        };
        tokio::time::sleep(delay).await;
        attempt = attempt.saturating_add(1);
    }
}

/// POSTs `body` to `{base_url}/{endpoint}` as a streamed request, retrying
/// transient failures (network / HTTP 429) with the same policy as
/// [`post_with_retry`]. The host injects the `"api_key"` credential.
async fn stream_with_retry(
    base_url: &str,
    endpoint: &str,
    body: &Value,
) -> Result<StreamSession, PluginError> {
    let url = format!("{}/{}", base_url.trim_end_matches('/'), endpoint);
    let payload = serde_json::to_vec(body)
        .map_err(|e| PluginError::provider(format!("failed to serialize request: {e}")))?;

    let mut attempt: u32 = 0;
    loop {
        let sent = broker()
            .stream(
                ene_plugin_broker::HttpMethod::Post,
                &url,
                vec![("Content-Type".to_string(), "application/json".to_string())],
                Some("api_key"),
                Some(payload.clone()),
            )
            .await;

        let err = match sent {
            Ok(session) if (200..300).contains(&session.status) => return Ok(session),
            Ok(mut session) => {
                let status = session.status;
                let retry_after = retry_after_secs(&session.headers);
                let mut raw = Vec::new();
                while let Some(chunk) = session.chunks.next().await {
                    let Ok(bytes) = chunk else { break };
                    raw.extend_from_slice(&bytes);
                    if raw.len() > 64 * 1024 {
                        break;
                    }
                }
                UpstreamError::Http {
                    status,
                    body: String::from_utf8_lossy(&raw).into_owned(),
                    retry_after,
                }
            }
            Err(e) => UpstreamError::Network(format!("broker request failed: {e}")),
        };

        let Some(delay) = retry_delay(&err, attempt) else {
            return Err(err.into_plugin_error());
        };
        tokio::time::sleep(delay).await;
        attempt = attempt.saturating_add(1);
    }
}

/// The backoff delay for a retryable error, or `None` when the error is
/// terminal or the retry budget is spent.
fn retry_delay(err: &UpstreamError, attempt: u32) -> Option<Duration> {
    let next = attempt.saturating_add(1);
    if !err.is_retryable() || next >= MAX_ATTEMPTS {
        return None;
    }
    let delay = match err {
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
        error = %err.clone().into_plugin_error(),
        "retryable upstream failure; backing off"
    );
    Some(delay)
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
    messages: &[Value],
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

/// Reads a streamed SSE response and sends parsed chunks through the
/// channel.
///
/// Skips malformed payloads (debug-logged), stops at `[DONE]`, and emits a
/// usage-only chunk when the final payload carries one.
async fn stream_sse_response(
    mut session: StreamSession,
    name_mapping: HashMap<String, String>,
    tx: tokio::sync::mpsc::Sender<Result<PluginStreamChunk, PluginError>>,
) {
    use eventsource_stream::Eventsource;

    if !(200..300).contains(&session.status) {
        let mut raw = Vec::new();
        while let Some(chunk) = session.chunks.next().await {
            let Ok(bytes) = chunk else { break };
            raw.extend_from_slice(&bytes);
            if raw.len() > 64 * 1024 {
                break;
            }
        }
        let snippet: String = String::from_utf8_lossy(&raw).chars().take(280).collect();
        drop(
            tx.send(Err(match session.status {
                401 | 403 => PluginError::provider_typed(
                    ProviderErrorKind::Auth,
                    format!("authentication failed: {snippet}"),
                ),
                429 => PluginError::provider_typed(
                    ProviderErrorKind::RateLimit,
                    format!("rate limited: {snippet}"),
                ),
                _ => PluginError::provider(format!("HTTP {}: {snippet}", session.status)),
            }))
            .await,
        );
        return;
    }

    let mut events = session.chunks.eventsource();
    while let Some(event) = events.next().await {
        let event = match event {
            Ok(event) => event,
            Err(e) => {
                drop(
                    tx.send(Err(PluginError::provider(format!(
                        "read stream failed: {e}"
                    ))))
                    .await,
                );
                return;
            }
        };
        let payload = event.data.trim();
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
        if let Some(reason) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(Value::as_str)
        {
            let error = match reason {
                "length" => Some(PluginError::provider_typed(
                    ProviderErrorKind::Truncated,
                    "finish_reason=length: configured token limit reached",
                )),
                "content_filter" => Some(PluginError::provider_typed(
                    ProviderErrorKind::ContentFilter,
                    "finish_reason=content_filter: provider blocked the response",
                )),
                _ => None,
            };
            if let Some(error) = error {
                drop(tx.send(Err(error)).await);
                return;
            }
        }
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
        let mut spec = Self::llm_spec();
        let configured_window = PLUGIN_CONFIG
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|config| config.get("context_window"))
            .and_then(Value::as_u64)
            .and_then(|window| u32::try_from(window).ok())
            .filter(|window| *window > 0);
        if let Some(window) = configured_window {
            spec.context_window = Some(window);
        }
        vec![spec]
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

        let base_url = resolve_base_url(&config);
        let oa_messages = convert::to_openai_messages(&messages)?;
        let oa_tools = convert::to_openai_tools(&tools);
        let name_mapping = convert::tool_name_mapping(&tools);
        let body = build_chat_body(
            &model,
            max_tokens,
            &oa_messages,
            oa_tools,
            None,
            true,
            effective_thinking_disabled(&config, &model),
        );

        let session = stream_with_retry(&base_url, "chat/completions", &body).await?;

        let (tx, rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(stream_sse_response(session, name_mapping, tx));

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

        let base_url = resolve_base_url(&config);
        let oa_messages = convert::to_openai_messages(&messages)?;
        let body = build_chat_body(
            &model,
            max_tokens,
            &oa_messages,
            Vec::new(),
            json_schema,
            false,
            effective_thinking_disabled(&config, &model),
        );

        let response = post_with_retry(&base_url, "api_key", "chat/completions", &body).await?;
        let raw = String::from_utf8_lossy(&response.body);
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
                    return Err(PluginError::provider_typed(
                        ProviderErrorKind::Truncated,
                        "finish_reason=length: configured token limit reached",
                    ));
                }
                "content_filter" => {
                    return Err(PluginError::provider_typed(
                        ProviderErrorKind::ContentFilter,
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
    reason = "unit tests use expect for concise assertions"
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
    /// static (every `resolve_base_url` call reads it).
    static TEST_SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_base_url_precedence_and_default() {
        let _guard = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        let original = std::env::var("OPENAI_BASE_URL").ok();
        // SAFETY: Test-only env var mutation, serialized by `ENV_MUTEX`.
        unsafe {
            std::env::remove_var("OPENAI_BASE_URL");
        }
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
        // SAFETY: Test-only env restore, serialized by `ENV_MUTEX`.
        match original {
            Some(value) => unsafe {
                std::env::set_var("OPENAI_BASE_URL", value);
            },
            None => unsafe {
                std::env::remove_var("OPENAI_BASE_URL");
            },
        }
    }

    #[test]
    fn resolve_base_url_env_fallback_after_config() {
        let _guard = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        let original = std::env::var("OPENAI_BASE_URL").ok();
        *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
        // SAFETY: Test-only env var mutation, serialized by `ENV_MUTEX`.
        unsafe {
            std::env::set_var("OPENAI_BASE_URL", "https://env.example/v1");
        }
        assert_eq!(
            resolve_base_url(&json!({})),
            "https://env.example/v1",
            "OPENAI_BASE_URL must be used when no config sets a base_url"
        );
        assert_eq!(
            resolve_base_url(&json!({"base_url": "https://request.example/v1"})),
            "https://request.example/v1",
            "request config must win over OPENAI_BASE_URL"
        );
        *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = None;
        // SAFETY: Test-only env restore, serialized by `ENV_MUTEX`.
        match original {
            Some(value) => unsafe {
                std::env::set_var("OPENAI_BASE_URL", value);
            },
            None => unsafe {
                std::env::remove_var("OPENAI_BASE_URL");
            },
        }
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
            &[json!({"role": "user", "content": "Hi"})],
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
            &[json!({"role": "user", "content": "Hi"})],
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
            &[json!({"role": "user", "content": "Hi"})],
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
            &[json!({"role": "user", "content": "Hi"})],
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
            &[json!({"role": "user", "content": "Hi"})],
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
    fn explicit_thinking_disabled_overrides_heuristic() {
        assert!(effective_thinking_disabled(
            &json!({"thinking_disabled": true}),
            "gpt-4o-mini"
        ));
        assert!(!effective_thinking_disabled(
            &json!({"thinking_disabled": false}),
            "openrouter/mimo-7b"
        ));
    }

    #[test]
    fn absent_thinking_disabled_falls_back_to_heuristic() {
        assert!(effective_thinking_disabled(
            &json!({}),
            "openrouter/mimo-7b"
        ));
        assert!(!effective_thinking_disabled(&json!({}), "gpt-4o-mini"));
        assert!(!effective_thinking_disabled(
            &json!({"api_key": "sk-test"}),
            "gpt-4o-mini"
        ));
    }

    #[test]
    fn capabilities_advertise_openai_kind() {
        // `llm_capabilities` reads the process-wide `PLUGIN_CONFIG` static,
        // which config-mutating tests change under `TEST_SERIAL`.
        let _guard = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
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
    fn configured_context_window_overrides_static_advertisement() {
        let _guard = TEST_SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let plugin = OpenAiPlugin;
        plugin.set_config(&json!({"context_window": 1_000_000}));
        assert_eq!(plugin.llm_capabilities()[0].context_window, Some(1_000_000));
        plugin.set_config(&json!({}));
    }

    #[test]
    fn backoff_delay_respects_caps() {
        assert!(backoff_delay(0) <= MAX_DELAY);
        assert!(backoff_delay(10) <= MAX_DELAY);
    }
}
