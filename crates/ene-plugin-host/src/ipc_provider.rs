//! IPC-backed LLM provider bridging the plugin wire protocol to `ene_ai::LlmProvider`.
//!
//! [`IpcLlmProvider`] holds a shared connection to a plugin binary and
//! translates `LlmProvider` trait calls into `CreateChatStream` /
//! `ChatCompletion` IPC messages.
//!
//! ## Concurrency
//!
//! The connection `Mutex` is held only for the duration of individual IPC
//! operations (a single write or a single read), never for the lifetime of an
//! entire stream. This allows tool calls and other requests to proceed between
//! stream chunk reads instead of being serialized behind a long-running LLM
//! stream (#D2). The stream reader task's [`JoinHandle`] is tracked and
//! aborted when the stream is dropped, ensuring prompt cleanup on cancellation.
//!
//! ## Retry
//!
//! Transient failures (transport errors) on `chat_completion` and stream
//! establishment are retried according to the [`RetryPolicy`] supplied by the
//! factory, matching the OpenAI provider's behavior (#C2).

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use ene_ai::RetryPolicy;
use ene_ai::error::LlmProviderError;
use ene_ai::message::{LlmMessage, LlmResponseChunk, LlmToolCallChunk};
use ene_plugin_proto::PluginIpcResponse;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_stream::{Stream, wrappers::ReceiverStream};

use crate::error::PluginHostError;
use crate::ipc_plugin::IpcPluginConnection;

/// An `LlmProvider` that delegates to a plugin binary over IPC.
///
/// Created by [`IpcLlmProviderFactory`](crate::factory::IpcLlmProviderFactory)
/// during `PluginHostManager` startup.
pub struct IpcLlmProvider {
    kind: String,
    conn: Arc<Mutex<IpcPluginConnection>>,
    model: String,
    max_tokens: Option<u32>,
    provider_config: serde_json::Value,
    retry_policy: RetryPolicy,
}

impl IpcLlmProvider {
    /// Creates a new IPC-backed LLM provider.
    pub fn new(
        kind: String,
        conn: Arc<Mutex<IpcPluginConnection>>,
        model: String,
        max_tokens: Option<u32>,
        provider_config: serde_json::Value,
        retry_policy: RetryPolicy,
    ) -> Self {
        Self {
            kind,
            conn,
            model,
            max_tokens,
            provider_config,
            retry_policy,
        }
    }
}

/// Maps a [`PluginHostError`] into the [`LlmProviderError`] domain.
///
/// Transport failures become [`LlmProviderError::Network`] (retryable);
/// all other host errors become [`LlmProviderError::Provider`] (not
/// retried by the policy).
fn map_host_error(e: PluginHostError) -> LlmProviderError {
    match e {
        PluginHostError::TransportFailed { message } => LlmProviderError::Network(message),
        other => LlmProviderError::Provider(other.to_string()),
    }
}

/// A chat stream that aborts its background reader task when dropped.
///
/// Wraps the per-request [`ReceiverStream`] and the reader's [`JoinHandle`].
/// Dropping the stream (e.g. user cancellation) aborts the reader, releasing
/// the connection for other requests (#D2).
struct IpcChatStream {
    rx: ReceiverStream<Result<LlmResponseChunk, LlmProviderError>>,
    reader: Option<JoinHandle<()>>,
}

impl Drop for IpcChatStream {
    fn drop(&mut self) {
        if let Some(handle) = self.reader.take() {
            handle.abort();
        }
    }
}

impl Stream for IpcChatStream {
    type Item = Result<LlmResponseChunk, LlmProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Stream::poll_next(Pin::new(&mut self.rx), cx)
    }
}

#[async_trait]
impl ene_ai::LlmProvider for IpcLlmProvider {
    fn name(&self) -> &str {
        &self.kind
    }

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ene_plugin_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        let messages_json: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null))
            .collect();
        let tools_json: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| serde_json::to_value(t).unwrap_or(serde_json::Value::Null))
            .collect();

        let request_id = uuid::Uuid::new_v4().to_string();

        // Establish the stream with retry on transient (transport) failures,
        // matching the OpenAI provider's `establish_sse_connection` (#C2).
        // The connection Mutex is held only for the duration of the write.
        self.retry_policy
            .run_with_retry_after(
                LlmProviderError::is_retryable,
                LlmProviderError::retry_after_secs,
                || {
                    let conn = Arc::clone(&self.conn);
                    let request_id = request_id.clone();
                    let kind = self.kind.clone();
                    let provider_config = self.provider_config.clone();
                    let model = self.model.clone();
                    let max_tokens = self.max_tokens;
                    let messages_json = messages_json.clone();
                    let tools_json = tools_json.clone();
                    async move {
                        let mut conn = conn.lock().await;
                        match conn
                            .send_create_chat_stream(
                                request_id,
                                kind,
                                provider_config,
                                model,
                                max_tokens,
                                messages_json,
                                tools_json,
                            )
                            .await
                        {
                            Ok(()) => Ok(()),
                            Err(e @ PluginHostError::TransportFailed { .. }) => {
                                // The stream is likely broken; reconnect so the
                                // next retry attempt starts from a clean
                                // connection (mirrors `do_request_with_timeout`).
                                if let Err(re) = conn.reconnect().await {
                                    tracing::warn!(
                                        component = "IpcLlmProvider",
                                        error = %re,
                                        "reconnect after stream transport failure failed"
                                    );
                                }
                                Err(map_host_error(e))
                            }
                            Err(e) => Err(map_host_error(e)),
                        }
                    }
                },
            )
            .await?;

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<LlmResponseChunk, LlmProviderError>>(32);

        // Spawn a reader task that acquires the connection lock only for the
        // duration of each individual read, releasing it between reads so
        // tool calls and other requests aren't serialized behind the entire
        // stream (#D2).
        let conn = Arc::clone(&self.conn);
        let rid = request_id.clone();
        let reader = tokio::spawn(async move {
            loop {
                let response = {
                    let mut conn = conn.lock().await;
                    conn.read_response().await
                };
                match response {
                    Ok(Some(PluginIpcResponse::StreamChunk {
                        request_id: chunk_rid,
                        text_delta,
                        tool_calls_delta,
                    })) if chunk_rid == rid => {
                        let tool_calls = if tool_calls_delta.is_empty() {
                            None
                        } else {
                            Some(
                                tool_calls_delta
                                    .iter()
                                    .enumerate()
                                    .map(|(i, v)| parse_tool_call_delta(v, i))
                                    .collect(),
                            )
                        };
                        let chunk = LlmResponseChunk {
                            text_delta: if text_delta.is_empty() {
                                None
                            } else {
                                Some(text_delta)
                            },
                            tool_calls_delta: tool_calls,
                        };
                        if tx.send(Ok(chunk)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Some(PluginIpcResponse::StreamEnd {
                        request_id: end_rid,
                    })) if end_rid == rid => {
                        break;
                    }
                    Ok(Some(PluginIpcResponse::StreamError {
                        request_id: err_rid,
                        message,
                    })) if err_rid == rid => {
                        let _ = tx.send(Err(LlmProviderError::Provider(message))).await;
                        break;
                    }
                    Ok(Some(PluginIpcResponse::Error { message, .. })) => {
                        let _ = tx.send(Err(LlmProviderError::Provider(message))).await;
                        break;
                    }
                    Ok(Some(_)) => {
                        // Unrelated response; skip.
                    }
                    Ok(None) => {
                        let _ = tx
                            .send(Err(LlmProviderError::Provider(
                                "connection closed during stream".to_string(),
                            )))
                            .await;
                        break;
                    }
                    Err(e) => {
                        let _ = tx
                            .send(Err(LlmProviderError::Provider(e.to_string())))
                            .await;
                        break;
                    }
                }
            }
        });

        Ok(Box::pin(IpcChatStream {
            rx: ReceiverStream::new(rx),
            reader: Some(reader),
        }))
    }

    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError> {
        let messages_json: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::to_value(m).unwrap_or(serde_json::Value::Null))
            .collect();

        // Retry transient (transport) failures with the same policy as the
        // OpenAI path (#C2). The connection's own `do_request` already
        // reconnects once on transport failure; this outer policy adds
        // backoff and additional attempts for persistent transient errors.
        self.retry_policy
            .run_with_retry_after(
                LlmProviderError::is_retryable,
                LlmProviderError::retry_after_secs,
                || {
                    let conn = Arc::clone(&self.conn);
                    let kind = self.kind.clone();
                    let provider_config = self.provider_config.clone();
                    let model = self.model.clone();
                    let max_tokens = self.max_tokens;
                    let messages_json = messages_json.clone();
                    let json_schema = json_schema.clone();
                    async move {
                        let request_id = uuid::Uuid::new_v4().to_string();
                        let mut conn = conn.lock().await;
                        conn.chat_completion(
                            request_id,
                            kind,
                            provider_config,
                            model,
                            max_tokens,
                            messages_json,
                            json_schema,
                        )
                        .await
                        .map_err(map_host_error)
                    }
                },
            )
            .await
    }
}

/// Parses a JSON tool-call delta into an [`LlmToolCallChunk`].
fn parse_tool_call_delta(value: &serde_json::Value, index: usize) -> LlmToolCallChunk {
    LlmToolCallChunk {
        index: value
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .map_or(index, |v| v as usize),
        id: value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        name: value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        arguments: value
            .get("arguments")
            .and_then(serde_json::Value::as_str)
            .map(String::from),
    }
}
