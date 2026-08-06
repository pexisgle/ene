//! OpenAI-compatible HTTP client for the managed `llama-server` sidecar.

use std::time::Duration;

use ene_plugin::{PluginCompletion, PluginError, PluginStream, PluginStreamChunk, TokenUsage};
use serde::Deserialize;
use serde_json::Value;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use crate::server::SidecarState;

/// Upper bound on one embedding request (short texts, batched).
const EMBED_TIMEOUT: Duration = Duration::from_secs(30);
/// Upper bound on a non-streaming completion; the plugin additionally wraps
/// decision completions in a tighter budget.
const COMPLETION_TIMEOUT: Duration = Duration::from_mins(5);

/// Chat client pinned to one sidecar instance.
#[derive(Clone)]
pub(crate) struct LlamaServerClient {
    http: reqwest::Client,
    base_url: String,
}

impl LlamaServerClient {
    pub(crate) fn new(state: &SidecarState) -> Result<Self, PluginError> {
        let bearer = format!("Bearer {}", state.api_key);
        let auth = reqwest::header::HeaderValue::from_str(&bearer)
            .map_err(|e| PluginError::provider(format!("invalid sidecar API key header: {e}")))?;
        let http = reqwest::Client::builder()
            .default_headers(reqwest::header::HeaderMap::from_iter([(
                reqwest::header::AUTHORIZATION,
                auth,
            )]))
            .build()
            .map_err(|e| PluginError::provider(format!("build HTTP client: {e}")))?;
        Ok(Self {
            http,
            base_url: state.base_url.clone(),
        })
    }

    /// One non-streaming chat completion.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the sidecar is unreachable or rejects
    /// the request; transport failures also reset the sidecar so the next
    /// request respawns it.
    pub(crate) async fn chat_completion(
        &self,
        model: &str,
        messages: Value,
        max_tokens: Option<u32>,
        json_schema: Option<Value>,
    ) -> Result<PluginCompletion, PluginError> {
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": false,
        });
        if let Some(max_tokens) = max_tokens {
            body["max_tokens"] = Value::from(max_tokens);
        }
        if let Some(schema) = json_schema {
            body["response_format"] = serde_json::json!({
                "type": "json_schema",
                "schema": schema,
            });
        }
        let response = match self
            .http
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&body)
            .timeout(COMPLETION_TIMEOUT)
            .send()
            .await
        {
            Ok(response) => response,
            Err(e) => {
                crate::server::reset_sidecar();
                return Err(PluginError::provider(format!(
                    "llama-server chat completion failed: {e}"
                )));
            }
        };
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            return Err(PluginError::provider(format!(
                "llama-server chat completion failed ({status}): {message}"
            )));
        }
        let completion: ChatCompletion = response
            .json()
            .await
            .map_err(|e| PluginError::provider(format!("invalid completion response: {e}")))?;
        let content = completion
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .unwrap_or_default();
        Ok(PluginCompletion {
            text: content,
            usage: completion.usage.map(|usage| token_usage(&usage)),
        })
    }

    /// Streaming chat completion over SSE.
    ///
    /// Dropping the returned stream (host cancellation) drops the HTTP
    /// response, which makes the sidecar stop generation.
    ///
    /// # Errors
    ///
    /// The stream yields a provider error when the sidecar is unreachable or
    /// rejects the request; transport failures also reset the sidecar so the
    /// next request respawns it.
    pub(crate) async fn chat_stream(
        &self,
        model: &str,
        messages: Value,
        max_tokens: Option<u32>,
        json_schema: Option<Value>,
    ) -> Result<PluginStream, PluginError> {
        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if let Some(max_tokens) = max_tokens {
            body["max_tokens"] = Value::from(max_tokens);
        }
        if let Some(schema) = json_schema {
            body["response_format"] = serde_json::json!({
                "type": "json_schema",
                "schema": schema,
            });
        }
        let response = match self
            .http
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(e) => {
                crate::server::reset_sidecar();
                return Err(PluginError::provider(format!(
                    "llama-server chat stream failed: {e}"
                )));
            }
        };
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            return Err(PluginError::provider(format!(
                "llama-server chat stream failed ({status}): {message}"
            )));
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<PluginStreamChunk, PluginError>>(16);
        tokio::spawn(drain_sse(response, tx));
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    /// Embeds every item in one request, in order.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the sidecar is unreachable or rejects
    /// the request; transport failures also reset the sidecar so the next
    /// request respawns it.
    pub(crate) async fn embed_batch(
        &self,
        model: &str,
        items: &[String],
    ) -> Result<Vec<Vec<f32>>, PluginError> {
        let response = match self
            .http
            .post(format!("{}/v1/embeddings", self.base_url))
            .json(&serde_json::json!({
                "model": model,
                "input": items,
            }))
            .timeout(EMBED_TIMEOUT)
            .send()
            .await
        {
            Ok(response) => response,
            Err(e) => {
                crate::server::reset_sidecar();
                return Err(PluginError::provider(format!(
                    "llama-server embeddings failed: {e}"
                )));
            }
        };
        let status = response.status();
        if !status.is_success() {
            let message = response.text().await.unwrap_or_default();
            return Err(PluginError::provider(format!(
                "llama-server embeddings failed ({status}): {message}"
            )));
        }
        let parsed: Embeddings = response
            .json()
            .await
            .map_err(|e| PluginError::provider(format!("invalid embeddings response: {e}")))?;
        if parsed.data.len() != items.len() {
            return Err(PluginError::provider(format!(
                "llama-server returned {} embeddings for {} items",
                parsed.data.len(),
                items.len()
            )));
        }
        Ok(parsed
            .data
            .into_iter()
            .map(|entry| entry.embedding)
            .collect())
    }

    /// Unloads a model from the sidecar. Idempotent: an unloaded model (or
    /// one the router does not know) is not an error.
    ///
    /// # Errors
    ///
    /// Returns a provider error only when the sidecar itself is unreachable
    /// or fails the request at the transport level.
    pub(crate) async fn unload(&self, model: &str) -> Result<(), PluginError> {
        let response = match self
            .http
            .post(format!("{}/models/unload", self.base_url))
            .json(&serde_json::json!({ "model": model }))
            .send()
            .await
        {
            Ok(response) => response,
            Err(e) => {
                crate::server::reset_sidecar();
                return Err(PluginError::provider(format!(
                    "llama-server unload failed: {e}"
                )));
            }
        };
        let status = response.status();
        if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        let message = response.text().await.unwrap_or_default();
        Err(PluginError::provider(format!(
            "llama-server unload failed ({status}): {message}"
        )))
    }
}

/// Drains one SSE response into the plugin stream channel.
async fn drain_sse(
    response: reqwest::Response,
    tx: tokio::sync::mpsc::Sender<Result<PluginStreamChunk, PluginError>>,
) {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(item) = stream.next().await {
        let bytes = match item {
            Ok(bytes) => bytes,
            Err(e) => {
                crate::server::reset_sidecar();
                drop(
                    tx.send(Err(PluginError::provider(format!(
                        "llama-server stream interrupted: {e}"
                    ))))
                    .await,
                );
                return;
            }
        };
        buffer.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(line_end) = buffer.find('\n') {
            let line: String = buffer.drain(..=line_end).collect();
            let line = line.trim();
            if !line.starts_with("data:") {
                continue;
            }
            let payload = line["data:".len()..].trim();
            if payload.is_empty() || payload == "[DONE]" {
                if payload == "[DONE]" {
                    return;
                }
                continue;
            }
            let chunk: SseChunk = match serde_json::from_str(payload) {
                Ok(chunk) => chunk,
                Err(e) => {
                    drop(
                        tx.send(Err(PluginError::provider(format!(
                            "invalid llama-server stream chunk: {e}"
                        ))))
                        .await,
                    );
                    return;
                }
            };
            for choice in chunk.choices {
                if let Some(content) = choice.delta.and_then(|delta| delta.content)
                    && !content.is_empty()
                    && tx
                        .send(Ok(PluginStreamChunk {
                            text_delta: Some(content),
                            tool_calls_delta: None,
                            usage: None,
                        }))
                        .await
                        .is_err()
                {
                    return;
                }
            }
            if let Some(usage) = chunk.usage
                && tx
                    .send(Ok(PluginStreamChunk {
                        text_delta: None,
                        tool_calls_delta: None,
                        usage: Some(token_usage(&usage)),
                    }))
                    .await
                    .is_err()
            {
                return;
            }
        }
    }
}

#[derive(Deserialize)]
struct ChatCompletion {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<TokenUsageWire>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct Embeddings {
    data: Vec<EmbeddingEntry>,
}

#[derive(Deserialize)]
struct EmbeddingEntry {
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct SseChunk {
    #[serde(default)]
    choices: Vec<SseChoice>,
    #[serde(default)]
    usage: Option<TokenUsageWire>,
}

#[derive(Deserialize)]
struct SseChoice {
    delta: Option<SseDelta>,
}

#[derive(Deserialize)]
struct SseDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct TokenUsageWire {
    #[serde(rename = "prompt_tokens")]
    prompt_tokens: u32,
    #[serde(rename = "completion_tokens")]
    completion_tokens: u32,
    #[serde(rename = "total_tokens", default)]
    total: Option<u32>,
}

fn token_usage(usage: &TokenUsageWire) -> TokenUsage {
    let total = usage
        .total
        .unwrap_or_else(|| usage.prompt_tokens.saturating_add(usage.completion_tokens));
    TokenUsage::new(usage.prompt_tokens, usage.completion_tokens, total)
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect for concise assertions"
)]
mod tests {
    use super::*;

    #[test]
    fn usage_falls_back_to_prompt_plus_completion() {
        let usage = token_usage(&TokenUsageWire {
            prompt_tokens: 10,
            completion_tokens: 5,
            total: None,
        });
        assert_eq!(usage.prompt_tokens, Some(10));
        assert_eq!(usage.completion_tokens, Some(5));
        assert_eq!(usage.total_tokens, Some(15));
    }

    #[tokio::test]
    async fn sse_drain_emits_text_and_usage_chunks() {
        // A tiny in-process SSE server feeding two chunks: one text delta
        // and one usage-only chunk, then [DONE].
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            use tokio::io::AsyncWriteExt;
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            drop(socket.write_all(response.as_bytes()).await);
        });

        let response = reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("request");
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        drain_sse(response, tx).await;
        drop(server);

        let first = rx.recv().await.expect("first chunk").expect("ok");
        assert_eq!(first.text_delta.as_deref(), Some("hi"));
        assert!(first.usage.is_none());
        let second = rx.recv().await.expect("second chunk").expect("ok");
        assert!(second.text_delta.is_none());
        let usage = second.usage.expect("usage");
        assert_eq!(usage.total_tokens, Some(3));
        assert!(rx.recv().await.is_none());
    }
}
