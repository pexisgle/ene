use std::time::Duration;

use async_trait::async_trait;
use ene_plugin_ipc::{
    EmbedHandler, EmbedRequest, EmbedResult, IpcError, LlmGenerateRequest, LlmGeneration,
    LlmHandler, LlmMessage, LlmRole, LlmToolCall,
};
use serde_json::{Value, json};

pub struct Gguf {
    http: reqwest::Client,
}

impl Gguf {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_mins(2))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

#[async_trait]
impl LlmHandler for Gguf {
    async fn generate(&self, request: LlmGenerateRequest) -> Result<LlmGeneration, IpcError> {
        let url = format!("{}/chat/completions", sidecar_base()?);
        let mut body = json!({
            "model": effective_model(&request.model),
            "messages": map_messages(&request.messages),
        });
        if let Some(max) = request.max_tokens {
            body["max_tokens"] = json!(max);
        }
        if !request.tools.is_empty() {
            body["tools"] = json!(
                request
                    .tools
                    .iter()
                    .map(|tool| json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        }
                    }))
                    .collect::<Vec<_>>()
            );
        }
        let response = self.post_json(&url, body).await?;
        Ok(parse_chat(&response))
    }
}

#[async_trait]
impl EmbedHandler for Gguf {
    async fn encode(&self, request: EmbedRequest) -> Result<EmbedResult, IpcError> {
        let url = format!("{}/embeddings", sidecar_base()?);
        let body = json!({
            "model": effective_model(&request.model),
            "input": request.texts,
        });
        let response = self.post_json(&url, body).await?;
        let vectors = response
            .get("data")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        row.get("embedding")
                            .and_then(Value::as_array)
                            .map(|values| {
                                values
                                    .iter()
                                    .filter_map(Value::as_f64)
                                    .map(f64_to_f32)
                                    .collect::<Vec<_>>()
                            })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(EmbedResult { vectors })
    }
}

impl Gguf {
    async fn post_json(&self, url: &str, body: Value) -> Result<Value, IpcError> {
        let response = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(IpcError::Call(format_http_error(status, &body)));
        }
        response
            .json()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))
    }
}

fn sidecar_base() -> Result<String, IpcError> {
    crate::sidecar::managed_base()
        .map(str::to_owned)
        .ok_or_else(|| IpcError::Call("llama-server sidecar is not running".to_owned()))
}

fn format_http_error(status: reqwest::StatusCode, body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        let message = value
            .pointer("/error/message")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty());
        if let Some(message) = message {
            return format!("{status}: {message}");
        }
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        status.to_string()
    } else {
        format!("{status}: {trimmed}")
    }
}

fn effective_model(request: &str) -> String {
    if request.is_empty() || request == "echo" {
        "local-gguf".to_owned()
    } else {
        request.to_owned()
    }
}

fn map_messages(messages: &[LlmMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            let role = match message.role {
                LlmRole::System => "system",
                LlmRole::User => "user",
                LlmRole::Assistant => "assistant",
                LlmRole::Tool => "tool",
            };
            let mut value = json!({ "role": role, "content": message.text });
            if !message.images.is_empty() {
                let mut parts = vec![json!({"type": "text", "text": message.text})];
                for image in &message.images {
                    parts.push(json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{};base64,{}", image.mime, image.base64)
                        }
                    }));
                }
                value["content"] = json!(parts);
            }
            if let Some(name) = &message.tool_name {
                value["name"] = json!(name);
            }
            if let Some(id) = &message.tool_call_id {
                value["tool_call_id"] = json!(id);
            }
            if !message.tool_calls.is_empty() {
                value["tool_calls"] = json!(
                    message
                        .tool_calls
                        .iter()
                        .map(|call| json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments.to_string(),
                            }
                        }))
                        .collect::<Vec<_>>()
                );
            }
            value
        })
        .collect()
}

fn parse_chat(response: &Value) -> LlmGeneration {
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .cloned()
        .unwrap_or(Value::Null);
    let message = choice.get("message").cloned().unwrap_or(Value::Null);
    let text = message_text(&message);
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| {
                    let function = call.get("function")?;
                    Some(LlmToolCall {
                        id: call
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        name: function
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        arguments: function
                            .get("arguments")
                            .and_then(Value::as_str)
                            .and_then(|raw| serde_json::from_str(raw).ok())
                            .unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let usage = response.get("usage").cloned().unwrap_or(Value::Null);
    LlmGeneration {
        text,
        thinking: message
            .get("reasoning")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        inner: Vec::new(),
        tool_calls,
        finish_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or("stop")
            .to_owned(),
        model_id: response
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        input_tokens: usage
            .get("prompt_tokens")
            .and_then(Value::as_u64)
            .map_or(0, u64_to_u32),
        output_tokens: usage
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .map_or(0, u64_to_u32),
    }
}

fn message_text(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn f64_to_f32(value: f64) -> f32 {
    let clamped = value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX));
    #[expect(
        clippy::cast_possible_truncation,
        reason = "embedding components are f32 on the provider wire"
    )]
    {
        clamped as f32
    }
}

fn u64_to_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_user_message_role() {
        let mapped = map_messages(&[LlmMessage {
            role: LlmRole::User,
            text: "hi".to_owned(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            images: Vec::new(),
        }]);
        assert_eq!(mapped[0]["role"], "user");
        assert_eq!(mapped[0]["content"], "hi");
    }

    #[test]
    fn parses_chat_completion() {
        let generation = parse_chat(&json!({
            "model": "gemma-4-e2b",
            "choices": [{
                "finish_reason": "stop",
                "message": { "role": "assistant", "content": "hello there" }
            }],
            "usage": { "prompt_tokens": 3, "completion_tokens": 2 }
        }));
        assert_eq!(generation.text, "hello there");
        assert_eq!(generation.model_id, "gemma-4-e2b");
        assert_eq!(generation.input_tokens, 3);
        assert!(generation.inner.is_empty());
    }

    #[test]
    fn sidecar_base_errors_when_not_started() {
        let err = sidecar_base().expect_err("no sidecar");
        assert!(err.to_string().contains("sidecar"));
    }
}
