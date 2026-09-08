use std::time::Duration;

use async_trait::async_trait;
use ene_plugin_ipc::{
    EmbedHandler, EmbedRequest, EmbedResult, IpcError, ListModelsRequest, ListModelsResult,
    LlmGenerateRequest, LlmGeneration, LlmHandler, LlmMessage, LlmRole, LlmStreamSink, LlmToolCall,
    ModelsHandler, SttHandler, SttRequest, SttResult, TtsAudio, TtsHandler, TtsRequest,
};
use serde_json::{Value, json};

const DEFAULT_BASE: &str = "https://api.openai.com/v1";

pub struct OpenAiCompat {
    http: reqwest::Client,
}

impl OpenAiCompat {
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
impl LlmHandler for OpenAiCompat {
    async fn generate(&self, request: LlmGenerateRequest) -> Result<LlmGeneration, IpcError> {
        let url = format!("{}/chat/completions", effective_base(&request.base_url));
        let body = chat_body(&request);
        let response = self.post_json(&url, &request.auth.api_key, body).await?;
        Ok(parse_chat(&response))
    }

    async fn generate_stream(
        &self,
        request: LlmGenerateRequest,
        sink: &mut dyn LlmStreamSink,
    ) -> Result<LlmGeneration, IpcError> {
        match self.stream_chat(&request).await {
            Ok(response) => crate::stream::consume_chat_sse(response, sink).await,
            Err(err) => {
                tracing::debug!(error = %err, "chat stream unavailable; using one-shot");
                self.generate(request).await
            }
        }
    }
}

#[async_trait]
impl EmbedHandler for OpenAiCompat {
    async fn encode(&self, request: EmbedRequest) -> Result<EmbedResult, IpcError> {
        let url = format!("{}/embeddings", effective_base(&request.base_url));
        let body = json!({
            "model": effective_model(&request.model, "text-embedding-3-small"),
            "input": request.texts,
        });
        let response = self.post_json(&url, &request.auth.api_key, body).await?;
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

#[async_trait]
impl TtsHandler for OpenAiCompat {
    async fn synthesize(&self, request: TtsRequest) -> Result<TtsAudio, IpcError> {
        let url = format!("{}/audio/speech", effective_base(&request.base_url));
        let voice = if request.voice.is_empty() {
            "alloy"
        } else {
            request.voice.as_str()
        };
        let body = json!({
            "model": effective_model(&request.model, "gpt-4o-mini-tts"),
            "voice": voice,
            "input": request.text,
            "response_format": "pcm",
        });
        let bytes = self.post_bytes(&url, &request.auth.api_key, body).await?;
        Ok(TtsAudio {
            pcm: pcm16le_to_f32(&bytes),
            sample_rate: 24_000,
            bulk: None,
        })
    }
}

#[async_trait]
impl SttHandler for OpenAiCompat {
    async fn transcribe(&self, request: SttRequest) -> Result<SttResult, IpcError> {
        let url = format!("{}/audio/transcriptions", effective_base(&request.base_url));
        let wav = encode_wav(&request.pcm, request.sample_rate);
        let mut form = reqwest::multipart::Form::new()
            .text("model", effective_model(&request.model, "whisper-1"))
            .part(
                "file",
                reqwest::multipart::Part::bytes(wav)
                    .file_name("audio.wav")
                    .mime_str("audio/wav")
                    .map_err(|err| IpcError::Call(err.to_string()))?,
            );
        if let Some(language) = request.language.clone() {
            form = form.text("language", language);
        }
        let response = attach_vendor_headers(self.http.post(&url), &request.auth.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(IpcError::Call(format!("{status}: {body}")));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        Ok(SttResult {
            text: value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    }
}

#[async_trait]
impl ModelsHandler for OpenAiCompat {
    async fn list_models(&self, request: ListModelsRequest) -> Result<ListModelsResult, IpcError> {
        let url = format!("{}/models", effective_base(&request.base_url));
        let value = self.get_json(&url, &request.auth.api_key).await?;
        Ok(ListModelsResult {
            models: select_for_seam(parse_model_ids(&value), &request.seam),
            error: None,
        })
    }
}

impl OpenAiCompat {
    async fn stream_chat(
        &self,
        request: &LlmGenerateRequest,
    ) -> Result<reqwest::Response, IpcError> {
        let url = format!("{}/chat/completions", effective_base(&request.base_url));
        let mut body = chat_body(request);
        body["stream"] = json!(true);
        let response =
            attach_vendor_headers(self.http.post(&url).json(&body), &request.auth.api_key)
                .send()
                .await
                .map_err(|err| IpcError::Call(err.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(IpcError::Call(format_http_error(status, &body)));
        }
        Ok(response)
    }

    async fn post_json(&self, url: &str, api_key: &str, body: Value) -> Result<Value, IpcError> {
        let response = attach_vendor_headers(self.http.post(url).json(&body), api_key)
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

    async fn get_json(&self, url: &str, api_key: &str) -> Result<Value, IpcError> {
        let response = attach_vendor_headers(self.http.get(url), api_key)
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

    async fn post_bytes(&self, url: &str, api_key: &str, body: Value) -> Result<Vec<u8>, IpcError> {
        let response = attach_vendor_headers(self.http.post(url).json(&body), api_key)
            .send()
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(IpcError::Call(format_http_error(status, &body)));
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|err| IpcError::Call(err.to_string()))
    }
}

fn chat_body(request: &LlmGenerateRequest) -> Value {
    let mut body = json!({
        "model": effective_model(&request.model, "gpt-4.1-mini"),
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
    body
}

fn format_http_error(status: reqwest::StatusCode, body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        if let Some(message) = provider_error_message(&value) {
            return format!("{status}: {message}");
        }
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

fn provider_error_message(value: &Value) -> Option<String> {
    let top_level = value.pointer("/error/message")?.as_str()?.trim();
    if top_level.is_empty() || !top_level.eq_ignore_ascii_case("provider returned error") {
        return None;
    }
    let raw = value.pointer("/error/metadata/raw")?.as_str()?.trim();
    (!raw.is_empty()).then(|| raw.to_owned())
}

fn attach_vendor_headers(
    mut http: reqwest::RequestBuilder,
    api_key: &str,
) -> reqwest::RequestBuilder {
    if !api_key.is_empty() {
        http = http.bearer_auth(api_key);
    }
    http.header(reqwest::header::REFERER, "https://ene.local")
        .header("X-Title", "ene")
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

fn effective_model(request: &str, fallback: &str) -> String {
    if request.is_empty() || request == "echo" {
        fallback.to_owned()
    } else {
        request.to_owned()
    }
}

fn parse_model_ids(value: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    collect_ids(
        value
            .get("data")
            .and_then(Value::as_array)
            .map(Vec::as_slice),
        &mut ids,
    );
    if ids.is_empty() {
        collect_ids(
            value
                .get("models")
                .and_then(Value::as_array)
                .map(Vec::as_slice),
            &mut ids,
        );
    }
    ids
}

fn collect_ids(rows: Option<&[Value]>, ids: &mut Vec<String>) {
    let Some(rows) = rows else {
        return;
    };
    for row in rows {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| row.as_str())
            .unwrap_or("")
            .trim();
        if !id.is_empty() {
            ids.push(id.to_owned());
        }
    }
}

fn select_for_seam(ids: Vec<String>, seam: &str) -> Vec<String> {
    let filtered: Vec<String> = ids
        .iter()
        .filter(|id| model_matches_seam(id, seam))
        .cloned()
        .collect();
    let mut chosen = if filtered.is_empty() { ids } else { filtered };
    chosen.sort();
    chosen.dedup();
    chosen.truncate(500);
    chosen
}

fn model_matches_seam(id: &str, seam: &str) -> bool {
    let id = id.to_ascii_lowercase();
    match seam {
        "seam.embed" => id.contains("embed"),
        "seam.tts" => id.contains("tts"),
        "seam.stt" => id.contains("whisper") || id.contains("transcribe"),
        _ => {
            !(id.contains("embed")
                || id.contains("tts")
                || id.contains("whisper")
                || id.contains("transcribe")
                || id.contains("dall-e")
                || id.contains("dall_e")
                || id.contains("sora")
                || id.contains("moderation"))
        }
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

fn pcm16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| {
            let sample = i16::from_le_bytes(*chunk);
            f32::from(sample) / 32768.0
        })
        .collect()
}

fn encode_wav(pcm: &[f32], sample_rate: u32) -> Vec<u8> {
    let rate = sample_rate.max(1);
    let data: Vec<u8> = pcm
        .iter()
        .flat_map(|sample| {
            let clamped = sample.clamp(-1.0, 1.0);
            #[expect(
                clippy::cast_possible_truncation,
                reason = "PCM sample is clamped to i16 range"
            )]
            let int = (clamped * 32767.0) as i16;
            int.to_le_bytes()
        })
        .collect();
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    let data_len = u32::try_from(data.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&(36_u32.saturating_add(data_len)).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_openrouter_error_message() {
        let body = r#"{"error":{"message":"Invalid 'tools[0].function.name'","code":400}}"#;
        let formatted = format_http_error(reqwest::StatusCode::BAD_REQUEST, body);
        assert_eq!(
            formatted,
            "400 Bad Request: Invalid 'tools[0].function.name'"
        );
    }

    #[test]
    fn unwraps_openrouter_raw_provider_error() {
        let body = r#"{
            "error": {
                "message": "Provider returned error",
                "metadata": {
                    "raw": "Invalid parameter: messages with role 'tool' must follow tool_calls"
                }
            }
        }"#;
        let formatted = format_http_error(reqwest::StatusCode::BAD_REQUEST, body);
        assert_eq!(
            formatted,
            "400 Bad Request: Invalid parameter: messages with role 'tool' must follow tool_calls"
        );
    }

    #[test]
    fn keeps_non_generic_provider_error_message() {
        let body = r#"{
            "error": {
                "message": "Invalid request",
                "metadata": { "raw": "internal detail" }
            }
        }"#;
        let formatted = format_http_error(reqwest::StatusCode::BAD_REQUEST, body);
        assert_eq!(formatted, "400 Bad Request: Invalid request");
    }

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
            "model": "gpt-test",
            "choices": [{
                "finish_reason": "stop",
                "message": { "role": "assistant", "content": "hello there" }
            }],
            "usage": { "prompt_tokens": 3, "completion_tokens": 2 }
        }));
        assert_eq!(generation.text, "hello there");
        assert_eq!(generation.model_id, "gpt-test");
        assert_eq!(generation.input_tokens, 3);
        assert!(generation.inner.is_empty());
    }

    #[test]
    fn filters_openai_catalog_by_seam() {
        let ids = parse_model_ids(&json!({
            "data": [
                { "id": "gpt-4.1-mini" },
                { "id": "text-embedding-3-small" },
                { "id": "gpt-4o-mini-tts" },
                { "id": "whisper-1" }
            ]
        }));
        assert_eq!(
            select_for_seam(ids.clone(), "seam.llm"),
            vec!["gpt-4.1-mini"]
        );
        assert_eq!(
            select_for_seam(ids.clone(), "seam.embed"),
            vec!["text-embedding-3-small"]
        );
        assert_eq!(
            select_for_seam(ids.clone(), "seam.tts"),
            vec!["gpt-4o-mini-tts"]
        );
        assert_eq!(select_for_seam(ids, "seam.stt"), vec!["whisper-1"]);
    }

    #[test]
    fn wav_has_riff_header() {
        let wav = encode_wav(&[0.0, 0.5], 16_000);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
    }
}
