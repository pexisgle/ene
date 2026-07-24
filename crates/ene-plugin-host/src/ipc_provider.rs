//! IPC-backed LLM provider bridging the plugin wire protocol to `ene_ai::LlmProvider`.
//!
//! [`IpcLlmProvider`] holds a shared connection to a plugin binary and
//! translates `LlmProvider` trait calls into `CreateChatStream` /
//! `ChatCompletion` IPC messages.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use ene_ai::error::LlmProviderError;
use ene_ai::message::{LlmMessage, LlmResponseChunk, LlmToolCallChunk};
use ene_plugin_proto::PluginIpcResponse;
use tokio::sync::Mutex;
use tokio_stream::{Stream, wrappers::ReceiverStream};

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
}

impl IpcLlmProvider {
    /// Creates a new IPC-backed LLM provider.
    pub fn new(
        kind: String,
        conn: Arc<Mutex<IpcPluginConnection>>,
        model: String,
        max_tokens: Option<u32>,
        provider_config: serde_json::Value,
    ) -> Self {
        Self {
            kind,
            conn,
            model,
            max_tokens,
            provider_config,
        }
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

        // Send the CreateChatStream request while holding the lock briefly.
        {
            let mut conn = self.conn.lock().await;
            conn.send_create_chat_stream(
                request_id.clone(),
                self.kind.clone(),
                self.provider_config.clone(),
                self.model.clone(),
                self.max_tokens,
                messages_json,
                tools_json,
            )
            .await
            .map_err(|e| LlmProviderError::Provider(e.to_string()))?;
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<LlmResponseChunk, LlmProviderError>>(32);

        // Spawn a reader task that re-acquires the lock and reads stream
        // responses until the terminal message. Safe because the connection
        // handles one stream at a time.
        let conn = Arc::clone(&self.conn);
        let rid = request_id.clone();
        tokio::spawn(async move {
            let mut conn = conn.lock().await;
            loop {
                match conn.read_response().await {
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
                    Ok(Some(PluginIpcResponse::Error { message })) => {
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

        Ok(Box::pin(ReceiverStream::new(rx)))
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

        let request_id = uuid::Uuid::new_v4().to_string();
        let mut conn = self.conn.lock().await;
        conn.chat_completion(
            request_id,
            self.kind.clone(),
            self.provider_config.clone(),
            self.model.clone(),
            self.max_tokens,
            messages_json,
            json_schema,
        )
        .await
        .map_err(|e| LlmProviderError::Provider(e.to_string()))
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
