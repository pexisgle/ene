use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::Deserialize;
use serde_json::{Value, json};

use ene_plugin_ipc::{
    IpcError, LlmGenerateRequest, LlmGeneration, LlmHandler, LlmMessage, LlmRole, LlmToolCall,
};

const DEFAULT_BASE: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct Anthropic {
    http: reqwest::Client,
}

impl Anthropic {
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
impl LlmHandler for Anthropic {
    async fn generate(&self, request: LlmGenerateRequest) -> Result<LlmGeneration, IpcError> {
        let key = if request.auth.api_key.is_empty() {
            env_api_key()
        } else {
            request.auth.api_key.clone()
        };
        if key.is_empty() {
            return Err(IpcError::Call("Anthropic API key is not set".into()));
        }
        let (system, messages) = split_system(&request.messages);
        let mut body = json!({
            "model": effective_model(&request.model),
            "max_tokens": request.max_tokens.unwrap_or(1024),
            "messages": messages,
        });
        if let Some(system) = system {
            body["system"] = json!(system);
        }
        if !request.tools.is_empty() {
            body["tools"] = json!(
                request
                    .tools
                    .iter()
                    .map(|tool| json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.parameters,
                    }))
                    .collect::<Vec<_>>()
            );
        }

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&key).map_err(|err| IpcError::Call(err.to_string()))?,
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let url = format!("{}/v1/messages", effective_base(&request.base_url));
        let response = self
            .http
            .post(url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        if !status.is_success() {
            return Err(IpcError::Call(format!("Anthropic HTTP {status}: {text}")));
        }
        parse_messages(&text)
    }
}

fn env_api_key() -> String {
    if let Ok(raw) = std::env::var("ENE_PROVIDER_CONFIG")
        && let Ok(value) = serde_json::from_str::<Value>(&raw)
        && let Some(key) = value.get("api_key").and_then(Value::as_str)
        && !key.is_empty()
    {
        return key.to_owned();
    }
    std::env::var("ANTHROPIC_API_KEY").unwrap_or_default()
}

fn effective_base(request: &str) -> String {
    if !request.is_empty() {
        return request.trim_end_matches('/').to_owned();
    }
    if let Ok(raw) = std::env::var("ENE_PROVIDER_CONFIG")
        && let Ok(value) = serde_json::from_str::<Value>(&raw)
        && let Some(url) = value.get("base_url").and_then(Value::as_str)
        && !url.is_empty()
    {
        return url.trim_end_matches('/').to_owned();
    }
    DEFAULT_BASE.to_owned()
}

fn effective_model(request: &str) -> String {
    if request.is_empty() || request == "echo" {
        "claude-sonnet-4-5".to_owned()
    } else {
        request.to_owned()
    }
}

fn split_system(messages: &[LlmMessage]) -> (Option<String>, Vec<Value>) {
    let mut system = String::new();
    let mut out = Vec::new();
    for message in messages {
        match message.role {
            LlmRole::System => {
                if !system.is_empty() {
                    system.push('\n');
                }
                system.push_str(&message.text);
            }
            LlmRole::User => {
                let content = if message.images.is_empty() {
                    json!(message.text)
                } else {
                    let mut parts = vec![json!({"type": "text", "text": message.text})];
                    for image in &message.images {
                        parts.push(json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": image.mime,
                                "data": image.base64,
                            }
                        }));
                    }
                    json!(parts)
                };
                out.push(json!({
                    "role": "user",
                    "content": content,
                }));
            }
            LlmRole::Assistant => {
                let mut value = json!({
                    "role": "assistant",
                    "content": message.text,
                });
                if !message.tool_calls.is_empty() {
                    value["content"] = json!(
                        message
                            .tool_calls
                            .iter()
                            .map(|call| json!({
                                "type": "tool_use",
                                "id": call.id,
                                "name": call.name,
                                "input": call.arguments,
                            }))
                            .collect::<Vec<_>>()
                    );
                }
                out.push(value);
            }
            LlmRole::Tool => out.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
                    "content": message.text,
                }]
            })),
        }
    }
    let system = if system.is_empty() {
        None
    } else {
        Some(system)
    };
    (system, out)
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    input: Value,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
}

fn parse_messages(body: &str) -> Result<LlmGeneration, IpcError> {
    let parsed: MessagesResponse =
        serde_json::from_str(body).map_err(|err| IpcError::Call(format!("decode: {err}")))?;
    let text = parsed
        .content
        .iter()
        .filter(|block| block.r#type == "text" || block.r#type.is_empty())
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("");
    let tool_calls = parsed
        .content
        .iter()
        .filter(|block| block.r#type == "tool_use")
        .map(|block| LlmToolCall {
            id: block.id.clone(),
            name: block.name.clone(),
            arguments: block.input.clone(),
        })
        .collect();
    Ok(LlmGeneration {
        text,
        inner: Vec::new(),
        tool_calls,
        finish_reason: parsed.stop_reason.unwrap_or_else(|| "stop".into()),
        model_id: parsed.model.unwrap_or_default(),
        input_tokens: parsed.usage.as_ref().map_or(0, |usage| usage.input_tokens),
        output_tokens: parsed.usage.as_ref().map_or(0, |usage| usage.output_tokens),
        thinking: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_system_moves_system_messages() {
        let (system, messages) = split_system(&[
            LlmMessage {
                role: LlmRole::System,
                text: "be brief".into(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                images: Vec::new(),
            },
            LlmMessage {
                role: LlmRole::User,
                text: "hi".into(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                images: Vec::new(),
            },
        ]);
        assert_eq!(system.as_deref(), Some("be brief"));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn parse_text_block() {
        let parsed = parse_messages(
            r#"{"content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","model":"claude-test","usage":{"input_tokens":2,"output_tokens":1}}"#,
        )
        .expect("parse");
        assert_eq!(parsed.text, "hello");
        assert_eq!(parsed.finish_reason, "end_turn");
        assert_eq!(parsed.model_id, "claude-test");
        assert_eq!(parsed.input_tokens, 2);
        assert!(parsed.inner.is_empty());
        assert!(parsed.tool_calls.is_empty());
    }

    #[test]
    fn parse_tool_use() {
        let parsed = parse_messages(
            r#"{"content":[{"type":"tool_use","id":"t1","name":"utility.hash","input":{"text":"hi"}}],"stop_reason":"tool_use"}"#,
        )
        .expect("parse");
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].name, "utility.hash");
    }
}
