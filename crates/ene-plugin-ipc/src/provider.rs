//! Typed provider-subprotocol bodies (LLM / embed / TTS / STT).
//!
//! Audio is PCM (`f32` samples) on the frame, not base64. Long streams move
//! off-frame later; short utterances stay on the request body.

use serde::{Deserialize, Serialize};

/// Provider-envelope version advertised by the host.
pub const PROVIDER_VERSION: u32 = 1;
/// `provider.llm` version.
pub const PROVIDER_LLM_VERSION: u32 = 1;
/// `provider.embed` version.
pub const PROVIDER_EMBED_VERSION: u32 = 1;
/// `provider.tts` version.
pub const PROVIDER_TTS_VERSION: u32 = 1;
/// `provider.stt` version.
pub const PROVIDER_STT_VERSION: u32 = 1;
/// `provider.list_models` version.
pub const PROVIDER_MODELS_VERSION: u32 = 1;

/// Negotiated provider modalities. Absent faces are disabled, not fatal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderFaces {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vad: Option<u32>,
}

impl ProviderFaces {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.llm.is_none()
            && self.embed.is_none()
            && self.tts.is_none()
            && self.stt.is_none()
            && self.models.is_none()
            && self.vad.is_none()
    }
}

/// Message role on the provider seam (not session projection roles).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmRole {
    System,
    User,
    Assistant,
    Tool,
}

/// One chat message in host-canonical form. Plugins map this to vendor dialect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: LlmRole,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<LlmToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<LlmImage>,
}

impl LlmMessage {
    #[must_use]
    pub fn new(role: LlmRole, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            images: Vec::new(),
        }
    }
}

/// Image part on an LLM user message (PNG/JPEG bytes as base64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmImage {
    pub mime: String,
    pub base64: String,
}

/// Model-requested tool invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Tool schema offered to the model. `parameters` is JSON Schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Host-injected credentials. Plugins must not persist this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderAuth {
    #[serde(default)]
    pub api_key: String,
}

/// `llm.generate` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGenerateRequest {
    pub messages: Vec<LlmMessage>,
    #[serde(default)]
    pub tools: Vec<LlmToolSchema>,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub auth: ProviderAuth,
}

/// Inner-channel line produced alongside surface text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmInnerLine {
    pub aspect: String,
    pub text: String,
}

/// `llm.done` body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGeneration {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default)]
    pub inner: Vec<LlmInnerLine>,
    #[serde(default)]
    pub tool_calls: Vec<LlmToolCall>,
    pub finish_reason: String,
    pub model_id: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl Default for LlmGeneration {
    fn default() -> Self {
        Self {
            text: String::new(),
            thinking: None,
            inner: Vec::new(),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_owned(),
            model_id: String::new(),
            input_tokens: 0,
            output_tokens: 0,
        }
    }
}

/// Streaming token on an in-flight `llm.generate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmChunk {
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

/// `embed.encode` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbedRequest {
    pub texts: Vec<String>,
    pub model: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub auth: ProviderAuth,
}

/// `embed.encode` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbedResult {
    pub vectors: Vec<Vec<f32>>,
}

/// `tts.synthesize` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TtsRequest {
    pub text: String,
    #[serde(default)]
    pub voice: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub auth: ProviderAuth,
}

/// PCM at `sample_rate` Hz, mono `f32` in `-1.0..=1.0`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TtsAudio {
    pub pcm: Vec<f32>,
    pub sample_rate: u32,
}

/// `stt.transcribe` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SttRequest {
    pub pcm: Vec<f32>,
    pub sample_rate: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub auth: ProviderAuth,
}

/// `stt.transcribe` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SttResult {
    pub text: String,
}

/// `provider.list_models` request. `seam` is `seam.llm` / `seam.embed` / `seam.tts` / `seam.stt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListModelsRequest {
    pub seam: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub auth: ProviderAuth,
}

/// `provider.list_models` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ListModelsResult {
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
